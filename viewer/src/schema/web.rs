use async_trait::async_trait;
use ehttp::Request;
use serde::Deserialize;

use crate::utils::GameVersion;

use super::provider::SchemaProvider;

pub struct WebProvider {
    base_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GithubCommit {
    pub sha: String,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GithubBranch {
    pub name: String,
    pub commit: GithubCommit,
    pub protected: bool,
}

impl WebProvider {
    pub fn new(base_url: String) -> Self {
        WebProvider { base_url }
    }

    pub fn new_github(owner: &str, repo: &str, version: Option<GameVersion>) -> Self {
        WebProvider {
            base_url: format!(
                "https://raw.githubusercontent.com/{owner}/{repo}/refs/heads/{}",
                version.map_or("latest".to_string(), |v| format!("ver/{v}"))
            ),
        }
    }

    fn is_valid_github_name(name: &str) -> bool {
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    }

    pub async fn fetch_github_repository(
        owner: &str,
        repo: &str,
    ) -> anyhow::Result<Vec<GameVersion>> {
        if !Self::is_valid_github_name(owner) || !Self::is_valid_github_name(repo) {
            return Err(anyhow::anyhow!("Invalid GitHub repository format"));
        }
        let url = format!("https://api.github.com/repos/{owner}/{repo}/branches?per_page=100");
        let resp = ehttp::fetch_async(Request::get(url))
            .await
            .map_err(|msg| anyhow::anyhow!("{msg}"))?;

        if !resp.ok {
            anyhow::bail!(
                "Response not OK ({} {}): {}",
                resp.status,
                resp.status_text,
                String::from_utf8_lossy(&resp.bytes)
            );
        }

        let branches: Vec<GithubBranch> = serde_json::from_slice(&resp.bytes)?;

        let mut vers = Vec::new();
        let mut has_latest = false;
        for branch in branches {
            if branch.name == "latest" {
                has_latest = true;
            } else if let Some(version_string) = branch.name.strip_prefix("ver/") {
                vers.push(GameVersion::new(version_string)?);
            }
        }

        if !has_latest {
            anyhow::bail!("No 'latest' branch found in repository {owner}/{repo}");
        }

        vers.sort();
        vers.reverse();

        Ok(vers)
    }
}

#[async_trait(?Send)]
impl SchemaProvider for WebProvider {
    async fn get_schema_text(&self, name: &str) -> anyhow::Result<String> {
        let resp = ehttp::fetch_async(Request::get(format!("{}/{name}.yml", self.base_url)))
            .await
            .map_err(|msg| anyhow::anyhow!("Schema request failed: {msg}"))?;
        if !resp.ok {
            return Err(anyhow::anyhow!(
                "Schema request failed: {} ({})",
                resp.status_text,
                resp.status
            ));
        }
        Ok(resp
            .text()
            .ok_or_else(|| anyhow::anyhow!("Schema request failed: Could not decode data"))?
            .to_owned())
    }

    fn can_save_schemas(&self) -> bool {
        false
    }

    fn save_schema_start_dir(&self) -> Option<std::path::PathBuf> {
        None
    }

    async fn save_schema(&self, _name: &str, _text: &str) -> anyhow::Result<()> {
        unreachable!("Saving schemas is not supported by this provider");
    }
}
