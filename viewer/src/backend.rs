use anyhow::Result;
use std::{num::NonZeroUsize, rc::Rc};

use crate::{
    data::{FileProvider, web::WebFileProvider},
    excel::base::CachedProvider,
    schema::{boxed::BoxedSchemaProvider, web::WebProvider},
    settings::{BackendConfig, InstallLocation, SchemaLocation},
};

#[derive(Clone)]
pub struct Backend(Rc<BackendImpl>);

struct BackendImpl {
    files: Rc<dyn FileProvider>,
    excel_provider: CachedProvider,
    schema_provider: BoxedSchemaProvider,
}

impl Backend {
    pub async fn new(config: BackendConfig) -> Result<Self> {
        let excel = async {
            let (files, cache_size): (Rc<dyn FileProvider>, usize) = match config.location {
                #[cfg(not(target_arch = "wasm32"))]
                InstallLocation::Sqpack(path) => {
                    let files: Rc<dyn FileProvider> =
                        Rc::new(crate::data::sqpack::SqpackFileProvider::new(&path));
                    (files, 64)
                }
                #[cfg(target_arch = "wasm32")]
                InstallLocation::Worker(path) => {
                    use crate::data::worker::WorkerFileProvider;
                    let handle = WorkerFileProvider::folders()
                        .await?
                        .into_iter()
                        .find(|f| f.0.name() == path)
                        .ok_or_else(|| anyhow::anyhow!("WorkerFileProvider: Entry not found"))?;
                    WorkerFileProvider::verify_folder(handle.clone()).await?;
                    let files: Rc<dyn FileProvider> =
                        Rc::new(WorkerFileProvider::new(handle).await?);
                    (files, 64)
                }

                InstallLocation::Web(base_url, region, version) => {
                    let Some(slug) = region.slug() else {
                        anyhow::bail!("Region {} is not yet available", region.name());
                    };
                    let files: Rc<dyn FileProvider> =
                        Rc::new(WebFileProvider::new(&base_url, slug, version).await?);
                    (files, 256)
                }
            };
            let excel_provider =
                CachedProvider::new(files.clone(), NonZeroUsize::new(cache_size).unwrap()).await?;
            anyhow::Result::<_>::Ok((files, excel_provider))
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

                SchemaLocation::Github(location) => {
                    BoxedSchemaProvider::new_web(WebProvider::new_github(&location))
                }

                SchemaLocation::Web(base_url) => {
                    BoxedSchemaProvider::new_web(WebProvider::new(base_url))
                }
            })
        };
        let ((files, excel_provider), schema) = futures_util::try_join!(excel, schema)?;
        Ok(Self(Rc::new(BackendImpl {
            files,
            excel_provider,
            schema_provider: schema,
        })))
    }

    /// The shared raw-file provider. Read any game file with
    /// [`FileProviderExt::file`](crate::data::FileProviderExt::file), e.g.
    /// `backend.files().file::<Vec<u8>>(path)`.
    pub fn files(&self) -> &Rc<dyn FileProvider> {
        &self.0.files
    }

    pub fn excel(&self) -> &CachedProvider {
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
            assert!(!WORKER_FLAG.swap(true, Ordering::SeqCst), "Worker already initialized");
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
