use anyhow::Result;
use std::{str::FromStr, sync::Arc};

use crate::{
    data::{AppConfig, InstallLocation, SchemaLocation},
    excel::{boxed::BoxedExcelProvider, web::WebFileProvider},
    schema::{boxed::BoxedSchemaProvider, web::WebProvider},
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
                InstallLocation::Worker(path) => {
                    BoxedExcelProvider::new_worker(
                        crate::excel::worker::WorkerFileProvider::new(path).await?,
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
                    BoxedSchemaProvider::new_local(crate::schema::local::LocalProvider::new(&path))
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

#[cfg(target_arch = "wasm32")]
pub mod worker {
    use std::{
        cell::LazyCell,
        sync::atomic::{AtomicBool, Ordering},
    };

    use gloo_worker::{Spawnable, WorkerBridge};

    use crate::worker::{PreservingCodec, SqpackWorker};

    static WORKER_FLAG: AtomicBool = AtomicBool::new(false);

    thread_local! {
        pub static WORKER: LazyCell<WorkerBridge<SqpackWorker>> = LazyCell::new(|| {
            if WORKER_FLAG.swap(true, Ordering::SeqCst) {
                panic!("Worker already initialized");
            }
            SqpackWorker::spawner()
                .encoding::<PreservingCodec>()
                .spawn("./worker.js")
        });
    }
}
