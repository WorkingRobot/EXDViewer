use crate::utils::{GameVersion, fetch_url};

use super::{base::FileProvider, get_icon_path, get_xivapi_asset_url};
use async_trait::async_trait;
use either::Either;
use image::RgbaImage;
use ironworks::file::File;
use serde::Deserialize;
use std::io::Cursor;
use url::Url;

pub struct WebFileProvider(Url);

#[derive(Debug, Clone, Deserialize)]
pub struct VersionInfo {
    pub latest: GameVersion,
    pub versions: Vec<GameVersion>,
}

impl WebFileProvider {
    pub async fn new(base_url: &str, version: Option<GameVersion>) -> anyhow::Result<Self> {
        let version_info = Self::get_versions(base_url).await?;

        let version = if let Some(v) = version {
            if !version_info.versions.contains(&v) {
                anyhow::bail!("Version {v} is not available");
            } else {
                v
            }
        } else {
            log::info!(
                "No version specified, using latest: {}",
                version_info.latest
            );
            version_info.latest
        };

        let mut base_url = Url::parse(base_url)?;
        base_url
            .path_segments_mut()
            .map_err(|_| {
                ironworks::Error::Invalid(
                    ironworks::ErrorValue::Other("URL".to_string()),
                    "path parsing error".to_string(),
                )
            })?
            .push(&version.to_string());

        Ok(Self(base_url))
    }

    pub async fn get_versions(base_url: &str) -> anyhow::Result<VersionInfo> {
        let mut url = Url::parse(base_url)?;

        url.path_segments_mut()
            .map_err(|_| {
                ironworks::Error::Invalid(
                    ironworks::ErrorValue::Other("URL".to_string()),
                    "path parsing error".to_string(),
                )
            })?
            .push("versions");

        let resp = fetch_url(url).await?;

        let mut vers: VersionInfo = serde_json::from_slice(&resp)?;
        vers.versions.sort();
        vers.versions.reverse();
        Ok(vers)
    }
}

#[async_trait(?Send)]
impl FileProvider for WebFileProvider {
    async fn file<T: File>(&self, path: &str) -> anyhow::Result<T> {
        let mut url = self.0.clone();

        url.path_segments_mut()
            .map_err(|_| {
                ironworks::Error::Invalid(
                    ironworks::ErrorValue::Other("URL".to_string()),
                    "path parsing error".to_string(),
                )
            })?
            .extend(path.split('/'));

        let resp = fetch_url(url).await?;

        Ok(T::read(Cursor::new(resp))?)
    }

    async fn get_icon(&self, icon_id: u32, hires: bool) -> anyhow::Result<Either<Url, RgbaImage>> {
        let path = get_icon_path(icon_id, hires);
        let url = get_xivapi_asset_url(&path, Some("png"));
        Ok(Either::Left(url))
    }
}
