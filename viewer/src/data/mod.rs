use std::io::{Cursor, Read, Seek};

use async_trait::async_trait;
use either::Either;
use image::RgbaImage;
use ironworks::excel::Language;
use ironworks::file::File;
use url::Url;

#[cfg(not(target_arch = "wasm32"))]
pub mod sqpack;
pub mod web;
#[cfg(target_arch = "wasm32")]
pub mod worker;

/// Reads raw game files by path from some backing store (a local sqpack install,
/// the web API, or an in-browser worker). Higher-level readers (excel, sound, …)
/// are layered on top of this.
#[async_trait(?Send)]
pub trait FileProvider {
    /// Read a file's raw bytes by path, alongside the name of the sqpack stream they were stored
    /// as. The kind is `None` where the store cannot report one, which is any API server older than
    /// the header it travels in.
    async fn read_stream(&self, path: &str) -> anyhow::Result<(Option<String>, Vec<u8>)>;

    /// Read a file the game records only as a hash.
    async fn read_stream_by_hash(
        &self,
        repository: u8,
        category: u8,
        hash: u64,
        split: bool,
    ) -> anyhow::Result<(Option<String>, Vec<u8>)>;

    /// Read a file's raw bytes by path.
    async fn read(&self, path: &str) -> anyhow::Result<Vec<u8>> {
        Ok(self.read_stream(path).await?.1)
    }

    /// The global path list and, alongside it, which of its entries this install actually ships as
    /// an encoded [`pathlist::Presence`].
    ///
    /// Both come back together because a local provider has to read the list in order to build the
    /// map, and returning it saves fetching the same 20 MB twice. The API serves a map built per
    /// version instead, so for it the two are separate requests; the pair has to agree, so it is
    /// the provider that decides how to fetch them, not the caller.
    async fn path_index(&self, api_base: &str) -> anyhow::Result<(Vec<u8>, Vec<u8>)>;

    async fn read_by_hash(
        &self,
        repository: u8,
        category: u8,
        hash: u64,
        split: bool,
    ) -> anyhow::Result<Vec<u8>> {
        Ok(self
            .read_stream_by_hash(repository, category, hash, split)
            .await?
            .1)
    }

    /// Read and decode a texture to RGBA, no larger than `max_dim` on its longest edge; `None`
    /// decodes at full size.
    async fn read_texture(
        &self,
        path: &str,
        max_dim: Option<u16>,
    ) -> anyhow::Result<DecodedTexture> {
        decode_texture(path, self.read(path).await?, max_dim).await
    }

    async fn get_icon(&self, path: &str) -> anyhow::Result<Either<Url, RgbaImage>>;

    async fn exists_many(&self, paths: &[String]) -> anyhow::Result<Vec<bool>>;
}

/// A texture decoded for display.
pub struct DecodedTexture {
    pub image: RgbaImage,
    /// The size of the texture the pixels were decoded from.
    pub source: [u16; 2],
}

#[cfg(target_arch = "wasm32")]
impl DecodedTexture {
    fn from_worker(texture: crate::worker::WorkerTexture) -> anyhow::Result<Self> {
        let image = RgbaImage::from_vec(texture.width, texture.height, texture.data)
            .ok_or_else(|| anyhow::anyhow!("decoded texture does not fill its own dimensions"))?;
        Ok(Self {
            image,
            source: texture.source,
        })
    }
}

#[cfg(target_arch = "wasm32")]
async fn decode_texture(
    path: &str,
    bytes: Vec<u8>,
    max_dim: Option<u16>,
) -> anyhow::Result<DecodedTexture> {
    use crate::worker::{WorkerRequest, WorkerResponse};

    let request = WorkerRequest::DecodeTexture {
        path: path.to_owned(),
        bytes,
        max_dim,
    };
    match crate::backend::worker::transact(request).await {
        WorkerResponse::DecodeTexture(result) => DecodedTexture::from_worker(
            result.map_err(|error| anyhow::anyhow!("failed to decode texture: {error}"))?,
        ),
        _ => Err(anyhow::anyhow!("invalid response from worker")),
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn decode_texture(
    path: &str,
    bytes: Vec<u8>,
    max_dim: Option<u16>,
) -> anyhow::Result<DecodedTexture> {
    let (image, source) = crate::utils::tex_loader::decode_preview_sized(&bytes, path, max_dim)?;
    Ok(DecodedTexture {
        image: image.to_rgba8(),
        source,
    })
}

/// Typed reads layered on [`FileProvider`]. Blanket-implemented for every
/// provider (including `dyn FileProvider`), so any file type can be read without
/// each backend knowing about it.
pub trait FileProviderExt: FileProvider {
    /// Read and parse a file into an ironworks [`File`] type. Pass `Vec<u8>` for
    /// raw bytes.
    fn file<T: File>(&self, path: &str) -> impl std::future::Future<Output = anyhow::Result<T>> {
        async move {
            let bytes = self.read(path).await?;
            Ok(T::read(Cursor::new(bytes))?)
        }
    }
}

impl<P: FileProvider + ?Sized> FileProviderExt for P {}

#[derive(serde::Deserialize)]
struct ListInfo {
    list: String,
}

async fn list_id(api_base: &str) -> anyhow::Result<u64> {
    let info: ListInfo =
        serde_json::from_slice(&crate::utils::fetch_url(format!("{api_base}/paths/")).await?)?;
    Ok(u64::from_str_radix(&info.list, 16)?)
}

pub fn list_url(api_base: &str, list_id: u64) -> String {
    format!("{api_base}/paths/{list_id:016x}/")
}

pub async fn with_list_id<T, F>(api_base: &str, take: impl Fn(u64) -> F) -> anyhow::Result<T>
where
    F: std::future::Future<Output = anyhow::Result<T>>,
{
    let id = list_id(api_base).await?;
    match take(id).await {
        Ok(taken) => Ok(taken),
        Err(_) => take(list_id(api_base).await?).await,
    }
}

/// Build a presence map for a local install by matching the global path list against what the
/// packages actually index. The API does this server-side per version; with no API there is nobody
/// else to ask, so the client does the same work over its own packages.
pub fn build_local_presence<R: ironworks::sqpack::Resource>(
    sqpack: &ironworks::sqpack::SqPack<R>,
    path_list: &[u8],
) -> anyhow::Result<Vec<u8>> {
    let paths = pathlist::PathList::decode(path_list)?;

    let mut installed = std::collections::HashSet::new();
    for entry in sqpack.entries()? {
        installed.insert((entry.repository, entry.category, entry.hash));
    }

    // A short walk would encode a map whose bits no longer line up with the list, so a failure to
    // read a directory's names has to surface rather than silently truncate.
    let mut failed = None;
    let map = pathlist::build_presence(
        paths.len(),
        &installed,
        |path| sqpack.locate(path).ok(),
        paths.list_id(),
        |visit| {
            for dir in 0..paths.dirs().len() {
                match paths.names(dir) {
                    Ok(names) => {
                        for name in &names {
                            visit(&paths.dirs()[dir], name);
                        }
                    }
                    Err(error) => {
                        failed = Some(error);
                        return;
                    }
                }
            }
        },
    );
    match failed {
        Some(error) => Err(error),
        None => Ok(map),
    }
}

/// A sqpack file read to the end, labelled with the kind of stream it was stored as.
pub fn stream<R: Read + Seek>(
    mut file: ironworks::sqpack::File<R>,
) -> Result<(String, Vec<u8>), ironworks::Error> {
    let kind = file.kind().name().to_owned();
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok((kind, bytes))
}

/// The `(hash, split)` pair [`FileProvider::read_by_hash`] carries, as ironworks models it. A split
/// hash comes from `.index` and covers directory and file name separately; a whole one comes from
/// `.index2` and is only ever 32 bits wide.
pub fn index_hash(hash: u64, split: bool) -> ironworks::sqpack::IndexHash {
    if split {
        ironworks::sqpack::IndexHash::Split(hash)
    } else {
        ironworks::sqpack::IndexHash::Whole(hash as u32)
    }
}

const HAS_LOW: u8 = 1;
const HAS_HIRES: u8 = 2;
const HAS_LOCALE_LOW: u8 = 4;
const HAS_LOCALE_HIRES: u8 = 8;

/// Which `ui/icon` files an install ships, as four bits per icon id: low-res and `_hr1`, each
/// unlocalized and under a locale folder.
pub struct IconIndex {
    /// Locale folders the install actually carries. A locale it does not ship resolves unlocalized
    /// rather than to a path nothing can read.
    locales: Vec<Language>,
    flags: Box<[u8]>,
}

impl IconIndex {
    pub fn build(paths: &pathlist::PathList, presence: &pathlist::Presence) -> Self {
        let mut locales: Vec<Language> = Vec::new();
        let mut flags: Vec<u8> = Vec::new();

        for (dir, path) in paths.dirs().iter().enumerate() {
            let Some(bucket) = path.strip_prefix("ui/icon/") else {
                continue;
            };
            // `hq` sits alongside the locales and is not one of them.
            let locale = match bucket.split_once('/') {
                None => None,
                Some((_, folder)) => {
                    match Language::iter().find(|language| icon_locale(*language) == Some(folder)) {
                        Some(language) => Some(language),
                        None => continue,
                    }
                }
            };

            let (Ok(offset), Ok(names)) = (paths.name_offset(dir), paths.names(dir)) else {
                continue;
            };
            let mut shipped = false;
            for (index, name) in names.iter().enumerate() {
                if !presence.contains(offset + index) {
                    continue;
                }
                let Some(stem) = name.strip_suffix(".tex") else {
                    continue;
                };
                let (stem, hires) = match stem.strip_suffix("_hr1") {
                    Some(stem) => (stem, true),
                    None => (stem, false),
                };
                let Ok(icon_id) = stem.parse::<u32>() else {
                    continue;
                };

                let bit = match (locale.is_some(), hires) {
                    (false, false) => HAS_LOW,
                    (false, true) => HAS_HIRES,
                    (true, false) => HAS_LOCALE_LOW,
                    (true, true) => HAS_LOCALE_HIRES,
                };
                let byte = icon_id as usize / 2;
                if byte >= flags.len() {
                    flags.resize(byte + 1, 0);
                }
                flags[byte] |= bit << (4 * (icon_id & 1));
                shipped = true;
            }

            if let Some(locale) = locale.filter(|_| shipped)
                && !locales.contains(&locale)
            {
                locales.push(locale);
            }
        }

        Self {
            locales,
            flags: flags.into(),
        }
    }

    fn flags(&self, icon_id: u32) -> u8 {
        let byte = self.flags.get(icon_id as usize / 2).copied().unwrap_or(0);
        (byte >> (4 * (icon_id & 1))) & 0xf
    }

    pub fn ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.flags
            .iter()
            .enumerate()
            .filter(|(_, byte)| **byte != 0)
            .flat_map(|(byte, packed)| {
                [(0, packed & 0xf), (1, packed >> 4)]
                    .into_iter()
                    .filter(|(_, flags)| *flags != 0)
                    .map(move |(half, _)| (byte as u32) * 2 + half)
            })
    }

    pub fn localized(&self, icon_id: u32) -> bool {
        self.flags(icon_id) & (HAS_LOCALE_LOW | HAS_LOCALE_HIRES) != 0
    }

    pub fn hires(&self, icon_id: u32) -> bool {
        self.flags(icon_id) & (HAS_HIRES | HAS_LOCALE_HIRES) != 0
    }

    fn path(&self, icon_id: u32, hires: bool, language: Language) -> String {
        let flags = self.flags(icon_id);
        let localized = flags & (HAS_LOCALE_LOW | HAS_LOCALE_HIRES) != 0;
        let locale = icon_locale(language)
            .filter(|_| localized && self.locales.contains(&language))
            // An icon that ships only under locales has no unlocalized file to fall back to.
            .or_else(|| match localized && flags & (HAS_LOW | HAS_HIRES) == 0 {
                true => self
                    .locales
                    .contains(&Language::English)
                    .then_some(Language::English)
                    .or_else(|| self.locales.first().copied())
                    .and_then(icon_locale),
                false => None,
            });
        let (low, high) = match locale {
            Some(_) => (HAS_LOCALE_LOW, HAS_LOCALE_HIRES),
            None => (HAS_LOW, HAS_HIRES),
        };
        let (wanted, other) = if hires { (high, low) } else { (low, high) };
        let hires = if flags & wanted == 0 && flags & other != 0 {
            !hires
        } else {
            hires
        };
        icon_path(icon_id, locale, hires)
    }
}

/// The locale folder `ui/icon` files sit under, spelled the way [`ironworks::excel::path::exd`]
/// spells its suffix.
fn icon_locale(language: Language) -> Option<&'static str> {
    use Language as L;
    match language {
        L::None => None,
        L::Japanese => Some("ja"),
        L::English => Some("en"),
        L::German => Some("de"),
        L::French => Some("fr"),
        L::ChineseSimplified => Some("chs"),
        L::ChineseTraditional => Some("cht"),
        L::Korean => Some("ko"),
        L::TaiwanChinese => Some("tc"),
    }
}

fn icon_path(icon_id: u32, locale: Option<&str>, hires: bool) -> String {
    format!(
        "ui/icon/{:03}000/{}{icon_id:06}{}.tex",
        icon_id / 1000,
        locale
            .map(|locale| format!("{locale}/"))
            .unwrap_or_default(),
        if hires { "_hr1" } else { "" }
    )
}

/// Where an icon actually lives for `language`. Without an index this is the blind format, which is
/// right for every icon that ships one unlocalized file at both resolutions.
pub fn get_icon_path(
    icons: Option<&IconIndex>,
    icon_id: u32,
    hires: bool,
    language: Language,
) -> String {
    match icons {
        Some(icons) => icons.path(icon_id, hires, language),
        None => icon_path(icon_id, None, hires),
    }
}
