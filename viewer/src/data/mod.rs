use std::io::Cursor;
use std::sync::LazyLock;

use async_trait::async_trait;
use either::Either;
use image::RgbaImage;
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
    /// Read a file's raw bytes by path.
    async fn read(&self, path: &str) -> anyhow::Result<Vec<u8>>;

    /// The global path list and, alongside it, which of its entries this install actually ships as
    /// an encoded [`pathlist::Presence`].
    ///
    /// Both come back together because a local provider has to read the list in order to build the
    /// map, and returning it saves fetching the same 20 MB twice. The API serves a map built per
    /// version instead, so for it the two are separate requests.
    async fn path_index(&self, path_list_url: &str) -> anyhow::Result<(Vec<u8>, Vec<u8>)>;

    /// Read a file the game records only as a hash. Unnamed files have no path, so this is the only
    /// way to reach them.
    async fn read_by_hash(
        &self,
        repository: u8,
        category: u8,
        hash: u64,
        split: bool,
    ) -> anyhow::Result<Vec<u8>>;

    async fn get_icon(&self, icon_id: u32, hires: bool) -> anyhow::Result<Either<Url, RgbaImage>>;

    async fn exists_many(&self, paths: &[String]) -> anyhow::Result<Vec<bool>>;
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

pub fn get_icon_path(icon_id: u32, hires: bool) -> String {
    format!(
        "ui/icon/{:03}000/{:06}{}.tex",
        icon_id / 1000,
        icon_id,
        if hires { "_hr1" } else { "" }
    )
}

static XIVAPI_BASE_URL: LazyLock<Url> = LazyLock::new(|| {
    Url::parse("https://v2.xivapi.com/api/asset").expect("Failed to parse XIVAPI base URL")
});

fn get_xivapi_asset_url(path: &str, format: Option<&str>) -> Url {
    let mut url = XIVAPI_BASE_URL.clone();
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("path", path);
        if let Some(format) = format {
            pairs.append_pair("format", format);
        }
    }
    url
}
