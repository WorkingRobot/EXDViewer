use anyhow::Result;
use std::str::FromStr;

use crate::{
    data::{AppConfig, InstallLocation, SchemaLocation},
    excel::{boxed::BoxedExcelProvider, sqpack::SqpackFileProvider, web::WebFileProvider},
    schema::{boxed::BoxedSchemaProvider, local::LocalProvider, web::WebProvider},
};

pub struct Backend {
    excel_provider: BoxedExcelProvider,
    schema_provider: BoxedSchemaProvider,
}

impl Backend {
    pub async fn new(config: AppConfig) -> Result<Self> {
        Ok(Self {
            excel_provider: match config.location {
                #[cfg(not(target_arch = "wasm32"))]
                InstallLocation::Sqpack(path) => {
                    BoxedExcelProvider::new_sqpack(SqpackFileProvider::new(&path)).await?
                }
                InstallLocation::Web(base_url) => {
                    BoxedExcelProvider::new_web(WebFileProvider::from_str(&base_url)?).await?
                }
            },
            schema_provider: match config.schema {
                #[cfg(not(target_arch = "wasm32"))]
                SchemaLocation::Local(path) => {
                    BoxedSchemaProvider::new_local(LocalProvider::new(&path))
                }
                SchemaLocation::Web(base_url) => {
                    BoxedSchemaProvider::new_web(WebProvider::new(base_url))
                }
            },
        })
    }

    pub fn excel(&self) -> &BoxedExcelProvider {
        &self.excel_provider
    }

    pub fn schema(&self) -> &BoxedSchemaProvider {
        &self.schema_provider
    }
}
