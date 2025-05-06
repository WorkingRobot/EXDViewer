use std::path::PathBuf;

use async_trait::async_trait;

#[async_trait(?Send)]
pub trait SchemaProvider {
    async fn get_schema_text(&self, name: &str) -> anyhow::Result<String>;

    fn can_save_schemas(&self) -> bool;

    fn save_schema_start_dir(&self) -> Option<PathBuf>;

    async fn save_schema(&self, name: &str, text: &str) -> anyhow::Result<()>;
}
