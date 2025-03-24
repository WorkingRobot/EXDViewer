use std::{path::PathBuf, str::FromStr};

use async_trait::async_trait;

use super::provider::SchemaProvider;

pub struct LocalProvider {
    base_path: PathBuf,
}

impl LocalProvider {
    pub fn new(base_path: &str) -> Self {
        LocalProvider {
            base_path: PathBuf::from_str(base_path).unwrap(),
        }
    }
}

#[async_trait(?Send)]
impl SchemaProvider for LocalProvider {
    async fn get_schema_text(&self, name: &str) -> anyhow::Result<String> {
        Ok(std::fs::read_to_string(
            self.base_path.join(format!("{name}.yml")),
        )?)
    }

    fn can_save_schemas(&self) -> bool {
        true
    }

    fn save_schema_start_dir(&self) -> PathBuf {
        self.base_path.clone()
    }

    fn save_schema(&self, name: &str, text: &str) -> anyhow::Result<()> {
        std::fs::write(self.base_path.join(format!("{name}.yml")), text)?;
        Ok(())
    }
}
