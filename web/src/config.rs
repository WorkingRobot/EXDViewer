use serde::Deserialize;
use xiv_cache::builder::ServerBuilder;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AssetCache {
    pub version_capacity: u64,
    pub version_ttl_minutes: u64,
    pub file_capacity: u64,
    pub file_ttl_minutes: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server_addr: String,
    pub metrics_server_addr: Option<String>,
    pub log_filter: Option<String>,
    pub log_access_format: Option<String>,
    pub cache: ServerBuilder,
    pub assets: AssetCache,
    pub slug: String,
    pub file_readahead: usize,
    pub api_workers: usize,
}

impl Default for AssetCache {
    fn default() -> Self {
        Self {
            version_capacity: 4,
            version_ttl_minutes: 60,
            file_capacity: 50,
            file_ttl_minutes: 5,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_addr: "0.0.0.0:80".to_string(),
            metrics_server_addr: None,
            log_filter: Some(
                "debug,exdviewer_web=debug,tracing::span=warn,foyer_memory::raw=warn".to_string(),
            ),
            log_access_format: None,
            cache: ServerBuilder::default(),
            assets: AssetCache::default(),
            slug: "4e9a232b".parse().unwrap(),
            file_readahead: 0x800000, // 8 MiB
            api_workers: 1,
        }
    }
}
