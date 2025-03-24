use std::{
    fmt::Display,
    path::PathBuf,
    sync::{Arc, LazyLock},
    time::Duration,
};

use ironworks::{Ironworks, sqpack::Install};
use mini_moka::sync::{Cache, CacheBuilder};
use regex_lite::Regex;
use serde::Deserialize;

static VERSION_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^H?\d{4}\.\d{2}\.\d{2}\.\d{4}\.\d{4}$").unwrap());

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum GameVersion {
    #[default]
    Latest,
    Specific(String),
}

impl<'a> Deserialize<'a> for GameVersion {
    fn deserialize<D: serde::Deserializer<'a>>(deserializer: D) -> Result<GameVersion, D::Error> {
        String::deserialize(deserializer)?
            .try_into()
            .map_err(|_| serde::de::Error::custom("invalid game version"))
    }
}

impl TryFrom<String> for GameVersion {
    type Error = ();

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.eq_ignore_ascii_case("latest") {
            Ok(GameVersion::Latest)
        } else {
            if !VERSION_REGEX.is_match(&value) {
                return Err(());
            }
            Ok(GameVersion::Specific(value))
        }
    }
}

impl Display for GameVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GameVersion::Latest => write!(f, "latest"),
            GameVersion::Specific(version) => write!(f, "{}", version),
        }
    }
}

pub struct GameData {
    path: PathBuf,
    version_cache: Cache<GameVersion, Arc<Ironworks>>,
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
            version_cache: CacheBuilder::new(version_capacity)
                .time_to_live(Duration::from_secs(60 * version_ttl_minutes))
                .build(),
            file_cache: CacheBuilder::new(file_capacity)
                .time_to_live(Duration::from_secs(60 * file_ttl_minutes))
                .build(),
        }
    }

    fn get_version(&self, version: GameVersion) -> Result<Arc<Ironworks>, ironworks::Error> {
        if let Some(ret) = self.version_cache.get(&version) {
            return Ok(ret);
        }

        let path = self.path.join(format!("{version}/sqpack"));

        // check if path exists
        if let Err(_) = std::fs::read_dir(&path) {
            return Err(ironworks::Error::NotFound(ironworks::ErrorValue::Other(
                format!("version {version:?}"),
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
