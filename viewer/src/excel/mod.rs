use std::sync::LazyLock;

use url::Url;

pub mod base;
pub mod boxed;
pub mod provider;
#[cfg(not(target_arch = "wasm32"))]
pub mod sqpack;
pub mod web;
#[cfg(target_arch = "wasm32")]
pub mod web_sqpack;

pub fn get_icon_path(icon_id: u32, hires: bool) -> String {
    format!(
        "ui/icon/{:03}000/{:06}{}.tex",
        icon_id / 1000,
        icon_id,
        if hires { "_hr1" } else { "" }
    )
}

const XIVAPI_BASE_URL: LazyLock<Url> = LazyLock::new(|| {
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
