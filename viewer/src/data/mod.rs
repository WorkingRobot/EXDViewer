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
