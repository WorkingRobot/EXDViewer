use std::{collections::HashSet, path::Path, sync::Arc, time::Duration};

use ironworks::{
    Ironworks,
    sqpack::{SqPack, VInstall, Vfs},
};
use mini_moka::sync::{Cache, CacheBuilder};
use serde::Serialize;
use tokio::runtime::Handle;
use xiv_cache::{
    builder::ServerBuilder,
    file::CacheFile,
    server::{Server, SlugData},
    stream::CacheFileStream,
};
use xiv_core::file::{slug::Slug, version::GameVersion};

use crate::{blocking_stream::BlockingReader, config::AssetCache, smart_bufreader::SmartBufReader};

#[derive(Debug, Clone, Serialize)]
pub struct VersionInfo {
    pub latest: GameVersion,
    pub versions: Vec<GameVersion>,
}

impl From<SlugData> for VersionInfo {
    fn from(value: SlugData) -> Self {
        Self {
            latest: value.latest_version,
            versions: value.versions,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RepositoryInfo {
    pub slug: Slug,
    pub name: String,
    pub latest: GameVersion,
}

impl RepositoryInfo {
    fn from_slug_data(slug: Slug, value: SlugData) -> Self {
        Self {
            slug,
            name: value.repository,
            latest: value.latest_version,
        }
    }
}

type CacheIronworks = Ironworks<SqPack<VInstall<CacheVfs>>>;

#[derive(Debug)]
pub struct GameData {
    cache: Server,
    default_slug: Slug,
    readahead_size: usize,
    ironworks_cache: Cache<(Slug, GameVersion), Arc<CacheIronworks>>,
    file_cache: Cache<(Slug, GameVersion, String), Arc<Vec<u8>>>,
}

impl GameData {
    pub async fn new(
        cache_config: ServerBuilder,
        asset_config: AssetCache,
        default_slug: Slug,
        readahead_size: usize,
    ) -> anyhow::Result<Self> {
        let server = cache_config.build().await?;

        Ok(Self {
            cache: server,
            default_slug,
            readahead_size,
            ironworks_cache: CacheBuilder::new(asset_config.version_capacity)
                .time_to_live(Duration::from_secs(60 * asset_config.version_ttl_minutes))
                .build(),
            file_cache: CacheBuilder::new(asset_config.file_capacity)
                .time_to_live(Duration::from_secs(60 * asset_config.file_ttl_minutes))
                .build(),
        })
    }

    pub fn resolve_slug(&self, slug: Option<Slug>) -> Slug {
        slug.unwrap_or(self.default_slug)
    }

    pub async fn versions(&self, slug: Slug) -> Option<VersionInfo> {
        self.cache.get_slug(slug).await.map(VersionInfo::from).ok()
    }

    pub async fn repositories(&self) -> anyhow::Result<Vec<RepositoryInfo>> {
        let slugs = self.cache.get_slug_list().await?;
        let mut repositories = Vec::with_capacity(slugs.len());
        for slug in slugs {
            if let Ok(slug_data) = self.cache.get_slug(slug).await {
                repositories.push(RepositoryInfo::from_slug_data(slug, slug_data));
            }
        }
        Ok(repositories)
    }

    async fn get_version(
        &self,
        slug: Slug,
        version: GameVersion,
    ) -> Result<Arc<CacheIronworks>, ironworks::Error> {
        let key = (slug, version);
        if let Some(ret) = self.ironworks_cache.get(&key) {
            return Ok(ret);
        }
        let (slug, version) = key;

        log::info!("Fetching ironworks for slug: {slug}, version: {version}");
        let vfs = CacheVfs::new(
            self.cache.clone(),
            self.readahead_size,
            slug,
            version.clone(),
        )
        .await
        .map_err(|e| ironworks::Error::Resource(Box::new(std::io::Error::other(e))))?;
        let resource = VInstall::at_sqpack(vfs);
        let resource = ironworks::sqpack::SqPack::new(resource);
        let ironworks = Arc::new(Ironworks::new().with_resource(resource));
        self.ironworks_cache
            .insert((slug, version), ironworks.clone());
        Ok(ironworks)
    }

    pub async fn get(
        &self,
        slug: Slug,
        version: GameVersion,
        file: String,
    ) -> Result<Arc<Vec<u8>>, ironworks::Error> {
        let key = (slug, version, file);
        if let Some(ret) = self.file_cache.get(&key) {
            return Ok(ret);
        }
        let (slug, version, file) = key;

        let ironworks = self.get_version(slug, version.clone()).await?;

        log::info!("Fetching file: {file} for slug: {slug}, version: {version}");
        let file_data = ironworks.file::<Vec<u8>>(&file)?;
        log::info!(
            "File fetched: {file} for slug: {slug}, version: {version}, size: {}",
            file_data.len()
        );

        let data = Arc::new(file_data);
        self.file_cache.insert((slug, version, file), data.clone());
        Ok(data)
    }

    pub async fn exists(
        &self,
        slug: Slug,
        version: GameVersion,
        files: Vec<String>,
    ) -> Result<Vec<bool>, ironworks::Error> {
        let ironworks = self.get_version(slug, version).await?;
        Ok(files
            .iter()
            .map(|file| ironworks.exists(file).unwrap_or(false))
            .collect())
    }

    pub async fn close(&self) -> anyhow::Result<()> {
        self.cache.close().await
    }
}

pub struct CacheVfs {
    server: Server,
    slug: Slug,
    version: GameVersion,
    readahead_size: usize,
    existing_files: HashSet<String>,
    existing_folders: HashSet<String>,
}

impl CacheVfs {
    pub async fn new(
        server: Server,
        readahead_size: usize,
        slug: Slug,
        version: GameVersion,
    ) -> anyhow::Result<Self> {
        let clut = server.get_clut(slug, version.clone()).await?;
        let existing_files = clut.files.keys().cloned().collect();
        let existing_folders = clut.folders.iter().cloned().collect();
        Ok(Self {
            server,
            slug,
            version,
            readahead_size,
            existing_files,
            existing_folders,
        })
    }
}

impl Vfs for CacheVfs {
    type File = SmartBufReader<BlockingReader<CacheFileStream>>;

    fn exists(&self, path: impl AsRef<Path>) -> bool {
        let path = Path::new("sqpack").join(path);
        let path_str = path.to_str().unwrap_or_default();
        // file
        self.existing_files
            .contains(path_str) ||
        // directory
        self.existing_folders
            .contains(path_str) ||
        {
            // Check if path is a parent directory of any file or folder
            self.existing_files.iter().chain(self.existing_folders.iter()).any(|k| {
                Path::new(k).parent()
                    .map(|parent| parent == path)
                    .unwrap_or(false) ||
                // Check all ancestor directories
                Path::new(k).ancestors().any(|a| a == path)
            })
        }
    }

    fn open(&self, path: impl AsRef<Path>) -> std::io::Result<Self::File> {
        let path = Path::new("sqpack").join(path);
        let path = path.to_str().unwrap_or_default();

        if !self.existing_files.contains(path) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "file not found",
            ));
        }

        let file = tokio::task::block_in_place(|| {
            Handle::current().block_on({
                async move {
                    CacheFile::new(
                        self.server.clone(),
                        self.slug,
                        self.version.clone(),
                        path.to_string(),
                    )
                    .await
                }
            })
        })?;

        Ok(SmartBufReader::unchecked_new(
            BlockingReader::new(file.into_reader()),
            self.readahead_size,
        ))
    }
}
