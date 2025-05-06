use async_trait::async_trait;
use ehttp::Request;

use super::provider::SchemaProvider;

pub struct WebProvider {
    base_url: String,
}

impl WebProvider {
    pub fn new(base_url: String) -> Self {
        WebProvider { base_url }
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
