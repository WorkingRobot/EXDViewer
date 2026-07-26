use crate::utils::{GameVersion, fetch_url};

use super::{FileProvider, get_icon_path, get_xivapi_asset_url};
use async_trait::async_trait;
use either::Either;
use image::RgbaImage;
use serde::Deserialize;
use url::Url;

pub struct WebFileProvider(Url);

#[derive(Debug, Clone, Deserialize)]
pub struct VersionInfo {
    pub latest: GameVersion,
    pub versions: Vec<GameVersion>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepositoryInfo {
    pub slug: String,
    pub name: String,
    pub latest: GameVersion,
}

#[derive(Debug, Clone, Deserialize)]
struct RepositoriesResponse {
    repositories: Vec<RepositoryInfo>,
}

#[derive(Debug, Clone, Deserialize)]
struct RegionsResponse {
    regions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ExistsResponse {
    exists: Vec<bool>,
}

impl WebFileProvider {
    /// `region` is the API key; the version is resolved once here and pinned for the life of the
    /// provider, so `latest` is never sent and every response is immutably cacheable.
    pub async fn new(
        base_url: &str,
        region: &str,
        version: Option<GameVersion>,
    ) -> anyhow::Result<Self> {
        let version_info = Self::get_versions(base_url, region).await?;

        let version = if let Some(v) = version {
            if !version_info.versions.contains(&v) {
                anyhow::bail!("Version {v} is not available");
            }
            v
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
            .map_err(|()| {
                ironworks::Error::Invalid(
                    ironworks::ErrorValue::Other("URL".to_string()),
                    "path parsing error".to_string(),
                )
            })?
            .push(region)
            .push(&version.to_string());

        Ok(Self(base_url))
    }

    /// Which regions the backend serves, so availability is not a table baked into this binary.
    pub async fn get_regions(base_url: &str) -> anyhow::Result<Vec<String>> {
        let mut url = Url::parse(base_url)?;
        url.path_segments_mut()
            .map_err(|()| {
                ironworks::Error::Invalid(
                    ironworks::ErrorValue::Other("URL".to_string()),
                    "path parsing error".to_string(),
                )
            })?
            .push("regions");
        let response: RegionsResponse = serde_json::from_slice(&fetch_url(url).await?)?;
        Ok(response.regions)
    }

    pub async fn get_versions(base_url: &str, region: &str) -> anyhow::Result<VersionInfo> {
        let mut url = Url::parse(base_url)?;

        url.path_segments_mut()
            .map_err(|()| {
                ironworks::Error::Invalid(
                    ironworks::ErrorValue::Other("URL".to_string()),
                    "path parsing error".to_string(),
                )
            })?
            .push(region)
            .push("versions");

        let resp = fetch_url(url).await?;

        let mut vers: VersionInfo = serde_json::from_slice(&resp)?;
        vers.versions.sort();
        vers.versions.reverse();
        Ok(vers)
    }

    pub async fn get_repositories(base_url: &str) -> anyhow::Result<Vec<RepositoryInfo>> {
        let mut url = Url::parse(base_url)?;

        url.path_segments_mut()
            .map_err(|()| {
                ironworks::Error::Invalid(
                    ironworks::ErrorValue::Other("URL".to_string()),
                    "path parsing error".to_string(),
                )
            })?
            .push("repositories");

        let resp = fetch_url(url).await?;

        let parsed: RepositoriesResponse = serde_json::from_slice(&resp)?;
        Ok(parsed.repositories)
    }
}

#[async_trait(?Send)]
impl FileProvider for WebFileProvider {
    async fn read(&self, path: &str) -> anyhow::Result<Vec<u8>> {
        let mut url = self.0.clone();

        url.path_segments_mut()
            .map_err(|()| {
                ironworks::Error::Invalid(
                    ironworks::ErrorValue::Other("URL".to_string()),
                    "path parsing error".to_string(),
                )
            })?
            .push("file")
            .extend(path.split('/'));

        Ok(fetch_url(url).await?)
    }

    async fn read_by_hash(
        &self,
        repository: u8,
        category: u8,
        hash: u64,
        split: bool,
    ) -> anyhow::Result<Vec<u8>> {
        let mut url = self.0.clone();
        let hash = if split {
            format!("{hash:016X}")
        } else {
            format!("{:08X}", hash as u32)
        };

        url.path_segments_mut()
            .map_err(|()| {
                ironworks::Error::Invalid(
                    ironworks::ErrorValue::Other("URL".to_string()),
                    "path parsing error".to_string(),
                )
            })?
            .push("hash")
            .push(&repository.to_string())
            .push(&category.to_string())
            .push(&hash);

        Ok(fetch_url(url).await?)
    }

    async fn get_icon(&self, icon_id: u32, hires: bool) -> anyhow::Result<Either<Url, RgbaImage>> {
        let path = get_icon_path(icon_id, hires);
        let url = get_xivapi_asset_url(&path, Some("png"));
        Ok(Either::Left(url))
    }

    async fn exists_many(&self, paths: &[String]) -> anyhow::Result<Vec<bool>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }

        let mut url = self.0.clone();
        url.path_segments_mut()
            .map_err(|()| {
                ironworks::Error::Invalid(
                    ironworks::ErrorValue::Other("URL".to_string()),
                    "path parsing error".to_string(),
                )
            })?
            .push("exists");
        url.query_pairs_mut().append_pair("files", &paths.join(","));

        let resp = fetch_url(url).await?;
        let parsed: ExistsResponse = serde_json::from_slice(&resp)?;
        Ok(parsed.exists)
    }
}
