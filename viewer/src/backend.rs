use anyhow::Result;
use std::{str::FromStr, sync::Arc};
use wasm_bindgen::JsCast;
use web_sys::FileSystemDirectoryHandle;

use crate::{
    data::{AppConfig, InstallLocation, SchemaLocation},
    excel::{boxed::BoxedExcelProvider, web::WebFileProvider},
    schema::{boxed::BoxedSchemaProvider, local::LocalProvider, web::WebProvider},
    utils::{js_error::JsError, web_store::WebStore, web_worker::WorkerMessenger},
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

                #[cfg(target_arch = "wasm32")]
                InstallLocation::WebSqpack(_, id) => {
                    BoxedExcelProvider::new_web_sqpack(
                        crate::excel::web_sqpack::WebSqpackFileProvider::new(
                            WebStore::open()
                                .await
                                .map_err(JsError::from_stderror)?
                                .get(id)
                                .await
                                .map_err(JsError::from_stderror)?
                                .ok_or_else(|| anyhow::anyhow!("Failed to get file handle"))?
                                .dyn_into::<FileSystemDirectoryHandle>()
                                .map_err(|_| {
                                    anyhow::anyhow!("Failed to cast to FileSystemDirectoryHandle")
                                })?,
                            WorkerMessenger::new().await?,
                        )?,
                    )
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
                #[cfg(target_arch = "wasm32")]
                SchemaLocation::WebLocal(_, _version) => {
                    todo!("WebLocal schema provider not implemented yet")
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
