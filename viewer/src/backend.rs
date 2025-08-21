use anyhow::Result;
use std::rc::Rc;

use crate::{
    excel::{boxed::BoxedExcelProvider, web::WebFileProvider},
    schema::{boxed::BoxedSchemaProvider, web::WebProvider},
    settings::{BackendConfig, InstallLocation, SchemaLocation},
};

#[derive(Clone)]
pub struct Backend(Rc<BackendImpl>);

struct BackendImpl {
    excel_provider: BoxedExcelProvider,
    schema_provider: BoxedSchemaProvider,
}

impl Backend {
    pub async fn new(config: BackendConfig) -> Result<Self> {
        let excel = async {
            anyhow::Result::<_>::Ok(match config.location {
                #[cfg(not(target_arch = "wasm32"))]
                InstallLocation::Sqpack(path) => {
                    BoxedExcelProvider::new_sqpack(crate::excel::sqpack::SqpackFileProvider::new(
                        &path,
                    ))
                    .await?
                }
                #[cfg(target_arch = "wasm32")]
                InstallLocation::Worker(path) => {
                    use crate::excel::worker::WorkerFileProvider;
                    let handle = WorkerFileProvider::folders()
                        .await?
                        .into_iter()
                        .find(|f| f.0.name() == path)
                        .ok_or_else(|| anyhow::anyhow!("WorkerFileProvider: Entry not found"))?;
                    WorkerFileProvider::verify_folder(handle.clone()).await?;
                    BoxedExcelProvider::new_worker(WorkerFileProvider::new(handle).await?).await?
                }

                InstallLocation::Web(base_url, version) => {
                    BoxedExcelProvider::new_web(WebFileProvider::new(&base_url, version)?).await?
                }
            })
        };
        let schema = async {
            anyhow::Result::<_>::Ok(match config.schema {
                #[cfg(not(target_arch = "wasm32"))]
                SchemaLocation::Local(path) => {
                    BoxedSchemaProvider::new_local(crate::schema::local::LocalProvider::new(&path))
                }
                #[cfg(target_arch = "wasm32")]
                SchemaLocation::Worker(path) => {
                    use crate::schema::worker::WorkerProvider;
                    let handle = WorkerProvider::folders()
                        .await?
                        .into_iter()
                        .find(|f| f.0.name() == path)
                        .ok_or_else(|| anyhow::anyhow!("WorkerProvider: Entry not found"))?;
                    WorkerProvider::verify_folder(handle.clone()).await?;
                    BoxedSchemaProvider::new_worker(WorkerProvider::new(handle).await?)
                }

                SchemaLocation::Web(base_url) => {
                    BoxedSchemaProvider::new_web(WebProvider::new(base_url))
                }
            })
        };
        let (excel, schema) = futures_util::try_join!(excel, schema)?;
        Ok(Self(Rc::new(BackendImpl {
            excel_provider: excel,
            schema_provider: schema,
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
        cell::{LazyCell, RefCell},
        sync::atomic::{AtomicBool, Ordering},
    };

    use gloo_worker::{Spawnable, WorkerBridge};
    use pinned::oneshot;

    use crate::worker::{PreservingCodec, SqpackWorker, WorkerRequest, WorkerResponse};

    static WORKER_FLAG: AtomicBool = AtomicBool::new(false);

    thread_local! {
        static WORKER: LazyCell<WorkerBridge<SqpackWorker>> = LazyCell::new(|| {
            if WORKER_FLAG.swap(true, Ordering::SeqCst) {
                panic!("Worker already initialized");
            }
            SqpackWorker::spawner()
                .encoding::<PreservingCodec>()
                .spawn("./worker.js")
        });
    }

    pub async fn transact(input: WorkerRequest) -> WorkerResponse {
        let (tx, rx) = oneshot::channel();
        let tx = RefCell::new(Some(tx));
        let bridge = WORKER.with(|w| {
            w.fork(Some(move |msg| {
                let ret = tx.take().map(|tx| tx.send(msg));
                match ret {
                    Some(Ok(())) => {}
                    Some(Err(_)) => {
                        log::error!("worker: failed to send message");
                    }
                    None => {
                        log::error!("worker: tx already taken");
                    }
                }
            }))
        });
        bridge.send(input);
        rx.await.unwrap()
    }
}
