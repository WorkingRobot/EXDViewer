use anyhow::Result;
use std::{str::FromStr, sync::Arc};

use crate::{
    data::{AppConfig, InstallLocation, SchemaLocation},
    excel::{boxed::BoxedExcelProvider, web::WebFileProvider},
    schema::{boxed::BoxedSchemaProvider, local::LocalProvider, web::WebProvider},
};

#[derive(Clone)]
pub struct Backend(Arc<BackendImpl>);

struct BackendImpl {
    excel_provider: BoxedExcelProvider,
    schema_provider: BoxedSchemaProvider,
}

impl Backend {
    pub async fn new(config: AppConfig) -> Result<Self> {
        Ok(Self(Arc::new(BackendImpl {
            excel_provider: match config.location {
                #[cfg(not(target_arch = "wasm32"))]
                InstallLocation::Sqpack(path) => {
                    BoxedExcelProvider::new_sqpack(crate::excel::sqpack::SqpackFileProvider::new(
                        &path,
                    ))
                    .await?
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
        })))
    }

    pub fn excel(&self) -> &BoxedExcelProvider {
        &self.0.excel_provider
    }

    pub fn schema(&self) -> &BoxedSchemaProvider {
        &self.0.schema_provider
    }
}
