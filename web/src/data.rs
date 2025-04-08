use std::{
    fmt::Display,
    path::PathBuf,
    sync::{Arc, LazyLock},
    time::Duration,
};

use ironworks::{
    Ironworks,
    sqpack::{Install, SqPack},
};
use itertools::Itertools;
use mini_moka::sync::{Cache, CacheBuilder};
use regex_lite::Regex;
use serde::Serialize;

static VERSION_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^H?\d{4}\.\d{2}\.\d{2}\.\d{4}\.\d{4}$").unwrap());

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[repr(transparent)]
pub struct GameVersion(String);

impl Display for GameVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TryFrom<String> for GameVersion {
    type Error = ();

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if !VERSION_REGEX.is_match(&value) {
            return Err(());
        }
        Ok(Self(value))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct VersionInfo {
    pub latest: GameVersion,
    pub versions: Vec<GameVersion>,
}

pub struct GameData {
    path: PathBuf,
    version_info_cache: Cache<(), VersionInfo>,
    version_cache: Cache<GameVersion, Arc<Ironworks<SqPack<Install>>>>,
    file_cache: Cache<(GameVersion, String), Arc<Vec<u8>>>,
}

impl GameData {
    pub fn new(
        path: PathBuf,
        version_capacity: u64,
        version_ttl_minutes: u64,
        file_capacity: u64,
        file_ttl_minutes: u64,
    ) -> Self {
        Self {
            path,
            version_info_cache: CacheBuilder::new(1)
                .time_to_live(Duration::from_secs(60 * 2))
                .build(),
            version_cache: CacheBuilder::new(version_capacity)
                .time_to_live(Duration::from_secs(60 * version_ttl_minutes))
                .build(),
            file_cache: CacheBuilder::new(file_capacity)
                .time_to_live(Duration::from_secs(60 * file_ttl_minutes))
                .build(),
        }
    }

    pub fn versions(&self) -> Option<VersionInfo> {
        if let Some(ret) = self.version_info_cache.get(&()) {
            return Some(ret);
        }

        let path = self.path.join("latest-ver.txt");
        let latest_version: GameVersion = std::fs::read_to_string(path).ok()?.try_into().ok()?;

        let versions: Vec<GameVersion> = self
            .path
            .read_dir()
            .expect("failed to read dir")
            .map(|e| e.expect("failed to read dir entry"))
            .map(|e| {
                (
                    e.file_name().into_string().expect("invalid entry name"),
                    e.file_type().expect("failed to get file type"),
                )
            })
            .filter(|(_, file_type)| file_type.is_dir())
            .filter_map(|(name, _)| name.try_into().ok())
            .collect_vec();

        let ret = VersionInfo {
            latest: latest_version,
            versions,
        };

        self.version_info_cache.insert((), ret.clone());
        Some(ret)
    }

    fn get_version(
        &self,
        version: GameVersion,
    ) -> Result<Arc<Ironworks<SqPack<Install>>>, ironworks::Error> {
        if let Some(ret) = self.version_cache.get(&version) {
            return Ok(ret);
        }

        let path = self.path.join(format!("{version}/sqpack"));

        // check if path exists
        if std::fs::read_dir(&path).is_err() {
            return Err(ironworks::Error::NotFound(ironworks::ErrorValue::Other(
                format!("version {version}"),
            )));
        }

        let resource = Install::at_sqpack(path);
        let resource = ironworks::sqpack::SqPack::new(resource);
        let ironworks = Arc::new(Ironworks::new().with_resource(resource));
        self.version_cache.insert(version, ironworks.clone());
        Ok(ironworks)
    }

    pub fn get(
        &self,
        version: GameVersion,
        file: String,
    ) -> Result<Arc<Vec<u8>>, ironworks::Error> {
        let key = (version, file);
        if let Some(ret) = self.file_cache.get(&key) {
            return Ok(ret);
        }
        let (version, file) = key;

        let data = Arc::new(self.get_version(version.clone())?.file::<Vec<u8>>(&file)?);
        self.file_cache.insert((version, file), data.clone());
        Ok(data)
    }
}
