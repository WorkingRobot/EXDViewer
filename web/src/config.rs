use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct DownloaderConfig {
    pub storage_dir: String,
    pub slug: String,
    pub file_regex: String,
    pub parallelism: u32,
    pub clut_path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub server_addr: String,
    pub metrics_server_addr: Option<String>,
    pub log_filter: Option<String>,
    pub log_access_format: Option<String>,
    pub downloader: DownloaderConfig,
}
