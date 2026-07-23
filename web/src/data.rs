use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
    time::Duration,
};

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
    readahead_size: usize,
    ironworks_cache: Cache<(Slug, GameVersion), Arc<CacheIronworks>>,
    file_cache: Cache<(Slug, GameVersion, String), Arc<Vec<u8>>>,
}

impl GameData {
    pub async fn new(
        cache_config: ServerBuilder,
        asset_config: AssetCache,
        readahead_size: usize,
    ) -> anyhow::Result<Self> {
        let server = cache_config.build().await?;

        Ok(Self {
            cache: server,
            readahead_size,
            ironworks_cache: CacheBuilder::new(asset_config.version_capacity)
                .time_to_live(Duration::from_secs(60 * asset_config.version_ttl_minutes))
                .build(),
            file_cache: CacheBuilder::new(asset_config.file_capacity)
                .time_to_live(Duration::from_secs(60 * asset_config.file_ttl_minutes))
                .build(),
        })
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Region {
    Global,
    Korea,
    China,
}

impl Region {
    fn from_publisher(publisher: &str) -> Option<Self> {
        match publisher {
            "ffxivneo" => Some(Region::Global),
            "actoz" => Some(Region::Korea),
            "shanda" => Some(Region::China),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Repo {
    Boot,
    Game,
    Ex(u8),
}

impl Repo {
    fn from_node(node: &str) -> Option<Self> {
        match node {
            "boot" => Some(Repo::Boot),
            "game" => Some(Repo::Game),
            _ => node
                .strip_prefix("ex")
                .and_then(|n| n.parse::<u8>().ok())
                .filter(|n| (1..=5).contains(n))
                .map(Repo::Ex),
        }
    }

    fn is_game_side(self) -> bool {
        matches!(self, Repo::Game | Repo::Ex(_))
    }
}

/// The base game plus its expansions, in sqpack load order.
const GAME_SIDE: [Repo; 6] = [
    Repo::Game,
    Repo::Ex(1),
    Repo::Ex(2),
    Repo::Ex(3),
    Repo::Ex(4),
    Repo::Ex(5),
];

fn parse_repo(repository: &str) -> Option<(Region, Repo)> {
    let publisher = repository.split('/').next()?;
    let node = repository.rsplit('/').next()?;
    Some((Region::from_publisher(publisher)?, Repo::from_node(node)?))
}

/// Expand a base game slug into itself plus its region's expansions, each pinned to the newest
/// version at or before `version` (an expansion that didn't exist yet is dropped). A non-game
/// slug (boot, unknown publisher) contributes only itself, so per-repo browsing is unchanged.
async fn game_contributions(
    server: &Server,
    slug: Slug,
    version: GameVersion,
) -> anyhow::Result<Vec<(Slug, GameVersion)>> {
    let region = server
        .get_slug(slug)
        .await
        .ok()
        .and_then(|data| parse_repo(&data.repository))
        .filter(|(_, repo)| repo.is_game_side())
        .map(|(region, _)| region);
    let Some(region) = region else {
        return Ok(vec![(slug, version)]);
    };

    let Ok(slugs) = server.get_slug_list().await else {
        return Ok(vec![(slug, version)]);
    };
    let mut by_repo: HashMap<Repo, (Slug, Vec<GameVersion>)> = HashMap::new();
    for slug in slugs {
        let Ok(data) = server.get_slug(slug).await else {
            continue;
        };
        if let Some((r, repo)) = parse_repo(&data.repository)
            && r == region
            && repo.is_game_side()
        {
            by_repo.insert(repo, (slug, data.versions));
        }
    }

    let mut contributions = Vec::new();
    for repo in GAME_SIDE {
        let Some((slug, versions)) = by_repo.get(&repo) else {
            continue;
        };
        if let Some(pinned) = versions.iter().filter(|v| **v <= version).max() {
            contributions.push((*slug, pinned.clone()));
        }
    }
    if contributions.is_empty() {
        contributions.push((slug, version));
    }
    Ok(contributions)
}

/// A read-only sqpack Vfs spanning every repository of one game install: the base game and all
/// expansions merged into one file set, with each path routed back to the slug that owns it.
pub struct CacheVfs {
    server: Server,
    readahead_size: usize,
    files: HashMap<String, (Slug, GameVersion)>,
    folders: HashSet<String>,
}

impl CacheVfs {
    pub async fn new(
        server: Server,
        readahead_size: usize,
        slug: Slug,
        version: GameVersion,
    ) -> anyhow::Result<Self> {
        let mut files = HashMap::new();
        let mut folders = HashSet::new();
        for (slug, version) in game_contributions(&server, slug, version).await? {
            let clut = server.get_clut(slug, version.clone()).await?;
            for key in clut.files.keys() {
                files.insert(key.clone(), (slug, version.clone()));
            }
            folders.extend(clut.folders.iter().cloned());
        }
        Ok(Self {
            server,
            readahead_size,
            files,
            folders,
        })
    }
}

impl Vfs for CacheVfs {
    type File = SmartBufReader<BlockingReader<CacheFileStream>>;

    fn exists(&self, path: impl AsRef<Path>) -> bool {
        let path = Path::new("sqpack").join(path);
        let path_str = path.to_str().unwrap_or_default();
        self.files.contains_key(path_str)
            || self.folders.contains(path_str)
            || self.files.keys().chain(self.folders.iter()).any(|k| {
                Path::new(k).parent().map(|parent| parent == path).unwrap_or(false)
                    || Path::new(k).ancestors().any(|a| a == path)
            })
    }

    fn open(&self, path: impl AsRef<Path>) -> std::io::Result<Self::File> {
        let path = Path::new("sqpack").join(path);
        let path = path.to_str().unwrap_or_default();

        let Some((slug, version)) = self.files.get(path) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "file not found",
            ));
        };

        let file = tokio::task::block_in_place(|| {
            Handle::current().block_on(async move {
                CacheFile::new(self.server.clone(), *slug, version.clone(), path.to_string()).await
            })
        })?;

        Ok(SmartBufReader::unchecked_new(
            BlockingReader::new(file.into_reader()),
            self.readahead_size,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_repository_names() {
        assert_eq!(
            parse_repo("ffxivneo/win32/release/game"),
            Some((Region::Global, Repo::Game))
        );
        assert_eq!(
            parse_repo("ffxivneo/win32/release/ex5"),
            Some((Region::Global, Repo::Ex(5)))
        );
        assert_eq!(
            parse_repo("actoz/win32/release_ko/ex1"),
            Some((Region::Korea, Repo::Ex(1)))
        );
        assert_eq!(
            parse_repo("shanda/win32/release_chs/game"),
            Some((Region::China, Repo::Game))
        );
        assert_eq!(
            parse_repo("ffxivneo/win32/release/boot"),
            Some((Region::Global, Repo::Boot))
        );
        assert_eq!(parse_repo("nintendo/win32/release/game"), None);
        assert_eq!(parse_repo("ffxivneo/win32/release/ex6"), None);
    }

    #[test]
    fn only_game_and_expansions_are_game_side() {
        assert!(Repo::Game.is_game_side());
        assert!(Repo::Ex(3).is_game_side());
        assert!(!Repo::Boot.is_game_side());
    }
}
