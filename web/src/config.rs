use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DownloaderConfig {
    pub storage_dir: String,
    pub slug: String,
    pub file_regex: String,
    pub parallelism: u32,
    pub clut_path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server_addr: String,
    pub metrics_server_addr: Option<String>,
    pub log_filter: Option<String>,
    pub log_access_format: Option<String>,
    pub downloader: DownloaderConfig,
}

impl Default for DownloaderConfig {
    fn default() -> Self {
        Self {
            storage_dir: "downloads".to_string(),
            slug: "4e9a232b".to_string(),
            file_regex: r"^sqpack\/ffxiv\/0a0000\..+$".to_string(),
            parallelism: 4,
            clut_path: "https://raw.githubusercontent.com/WorkingRobot/ffxiv-downloader/refs/heads/main/cluts".to_string(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_addr: "0.0.0.0:80".to_string(),
            metrics_server_addr: None,
            log_filter: Some("debug,exdviewer_web=debug,tracing::span=warn".to_string()),
            log_access_format: None,
            downloader: DownloaderConfig::default(),
        }
    }
}
