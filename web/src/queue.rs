use std::sync::{Arc, OnceLock};

use async_channel::Sender;
use bytes::Bytes;
use tokio::{runtime::Handle, select, sync::oneshot, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use xiv_core::file::version::GameVersion;

use crate::data::{GameData, RepositoryInfo, StoredFile, Target, VersionInfo};
use crate::paths::PathIndex;

#[derive(Debug, Clone)]
pub enum RequestData {
    GetFile(Target, Option<GameVersion>, String),
    GetFileByHash(
        Target,
        Option<GameVersion>,
        u8,
        u8,
        ironworks::sqpack::IndexHash,
    ),
    Exists(Target, Option<GameVersion>, Vec<String>),
    ListId,
    GlobalPaths,
    Presence(Target, Option<GameVersion>, u64),
    Repositories,
}

pub enum Response {
    GetFile(Result<Arc<StoredFile>, ironworks::Error>),
    GetFileByHash(Result<Arc<StoredFile>, ironworks::Error>),
    Exists(Result<Vec<bool>, ironworks::Error>),
    ListId(anyhow::Result<u64>),
    GlobalPaths(anyhow::Result<(u64, Bytes)>),
    Presence(anyhow::Result<Option<Bytes>>),
    Repositories(anyhow::Result<Vec<RepositoryInfo>>),
}

pub struct Request {
    pub data: RequestData,
    pub tx: oneshot::Sender<Response>,
}

#[derive(Debug, Clone)]
#[repr(transparent)]
pub struct MessageQueue(Arc<MessageQueueImpl>);

#[derive(Debug)]
struct MessageQueueImpl {
    data: Arc<GameData>,
    paths: Arc<PathIndex>,

    threads: OnceLock<Vec<JoinHandle<()>>>,
    cancel_token: CancellationToken,
    tx: Sender<Request>,
}

impl MessageQueue {
    pub fn new(data: Arc<GameData>, paths: Arc<PathIndex>, workers: usize) -> anyhow::Result<Self> {
        let (thread_tx, thread_rx) = async_channel::unbounded();
        let this = Self(Arc::new(MessageQueueImpl {
            data,
            paths,
            threads: OnceLock::new(),
            cancel_token: CancellationToken::new(),
            tx: thread_tx,
        }));

        let threads = (0..workers)
            .map(|_| {
                let cancellation_token = this.0.cancel_token.clone();
                let thread_rx = thread_rx.clone();
                let this = Arc::downgrade(&this.0);

                tokio::spawn(async move {
                    loop {
                        select! {
                            biased;
                            _ = cancellation_token.cancelled() => {
                                return;
                            }
                            result = thread_rx.recv() => {
                                let Ok(request) = result else {
                                    return;
                                };

                                let this = match this.upgrade() {
                                    Some(this) => this,
                                    None => return, // Queue has been dropped
                                };

                                let response = async {
                                    match request.data.clone() {
                                        RequestData::Repositories => {
                                            Response::Repositories(this.data.repositories().await)
                                        }
                                        RequestData::GetFile(target, version, path) => {
                                            let version = match version {
                                                Some(v) => Ok(v),
                                                None => {
                                                    this.data.versions_for(target).await.map(|v| v.latest).ok_or_else(|| ironworks::Error::NotFound(ironworks::ErrorValue::Other("No version info available".to_string())))
                                                }
                                            };
                                            let result = match version {
                                                Ok(version) => {
                                                    this.data.get(target, version, path).await
                                                }
                                                Err(e) => Err(e),
                                            };

                                            Response::GetFile(result)
                                        }
                                        RequestData::GetFileByHash(target, version, repository, category, hash) => {
                                            let version = match version {
                                                Some(v) => Ok(v),
                                                None => {
                                                    this.data.versions_for(target).await.map(|v| v.latest).ok_or_else(|| ironworks::Error::NotFound(ironworks::ErrorValue::Other("No version info available".to_string())))
                                                }
                                            };
                                            let result = match version {
                                                Ok(version) => {
                                                    this.data.get_by_hash(target, version, repository, category, hash).await
                                                }
                                                Err(e) => Err(e),
                                            };

                                            Response::GetFileByHash(result)
                                        }
                                        RequestData::Exists(target, version, files) => {
                                            let version = match version {
                                                Some(v) => Ok(v),
                                                None => {
                                                    this.data.versions_for(target).await.map(|v| v.latest).ok_or_else(|| ironworks::Error::NotFound(ironworks::ErrorValue::Other("No version info available".to_string())))
                                                }
                                            };
                                            let result = match version {
                                                Ok(version) => {
                                                    this.data.exists(target, version, files).await
                                                }
                                                Err(e) => Err(e),
                                            };

                                            Response::Exists(result)
                                        }
                                        RequestData::ListId => {
                                            Response::ListId(this.paths.list_id().await)
                                        }
                                        RequestData::GlobalPaths => {
                                            Response::GlobalPaths(this.paths.global().await)
                                        }
                                        RequestData::Presence(target, version, list_id) => {
                                            let version = match version {
                                                Some(v) => Some(v),
                                                None => this.data.versions_for(target).await.map(|v| v.latest),
                                            };
                                            let result = match version {
                                                Some(version) => this.paths.presence(&this.data, target, version, list_id).await,
                                                None => Err(anyhow::anyhow!("No version info available")),
                                            };

                                            Response::Presence(result)
                                        }
                                    }
                                };

                                let response = tokio::task::block_in_place(|| {
                                    Handle::current().block_on(response)
                                });

                                _ = request.tx.send(response);
                            }
                        }
                    }
                })
            })
            .collect::<Vec<_>>();

        this.0
            .threads
            .set(threads)
            .map_err(|_| anyhow::anyhow!("Failed to initialize message queue threads"))?;

        Ok(this)
    }

    /// Metadata lookups that only touch the slug registry, so they need no worker round-trip.
    pub async fn versions_for(&self, target: Target) -> Option<VersionInfo> {
        self.0.data.versions_for(target).await
    }

    pub async fn regions(&self) -> anyhow::Result<Vec<crate::data::Region>> {
        self.0.data.regions().await
    }

    pub async fn has_sqpack(&self, target: Target) -> bool {
        self.0.data.has_sqpack(target).await
    }

    pub async fn version_valid(&self, target: Target, version: &GameVersion) -> bool {
        self.0.data.version_valid(target, version).await
    }

    pub async fn repositories(&self) -> anyhow::Result<Vec<RepositoryInfo>> {
        let (tx, rx) = oneshot::channel();
        self.0
            .tx
            .send(Request {
                data: RequestData::Repositories,
                tx,
            })
            .await
            .expect("Failed to send request to message queue");

        match rx.await {
            Ok(Response::Repositories(result)) => result,
            _ => Err(anyhow::anyhow!("Failed to get repositories")),
        }
    }

    pub async fn exists(
        &self,
        target: Target,
        version: Option<GameVersion>,
        files: Vec<String>,
    ) -> Result<Vec<bool>, ironworks::Error> {
        let (tx, rx) = oneshot::channel();
        self.0
            .tx
            .send(Request {
                data: RequestData::Exists(target, version, files),
                tx,
            })
            .await
            .expect("Failed to send request to message queue");

        match rx.await {
            Ok(Response::Exists(result)) => result,
            _ => Err(ironworks::Error::Resource(Box::new(std::io::Error::other(
                "Failed to check existence",
            )))),
        }
    }

    pub async fn get_list_id(&self) -> anyhow::Result<u64> {
        let (tx, rx) = oneshot::channel();
        self.0
            .tx
            .send(Request {
                data: RequestData::ListId,
                tx,
            })
            .await
            .expect("Failed to send request to message queue");

        match rx.await {
            Ok(Response::ListId(result)) => result,
            _ => Err(anyhow::anyhow!("Failed to get the path list id")),
        }
    }

    pub async fn get_global_paths(&self) -> anyhow::Result<(u64, Bytes)> {
        let (tx, rx) = oneshot::channel();
        self.0
            .tx
            .send(Request {
                data: RequestData::GlobalPaths,
                tx,
            })
            .await
            .expect("Failed to send request to message queue");

        match rx.await {
            Ok(Response::GlobalPaths(result)) => result,
            _ => Err(anyhow::anyhow!("Failed to get the global path list")),
        }
    }

    pub async fn get_presence(
        &self,
        target: Target,
        version: Option<GameVersion>,
        list_id: u64,
    ) -> anyhow::Result<Option<Bytes>> {
        let (tx, rx) = oneshot::channel();
        self.0
            .tx
            .send(Request {
                data: RequestData::Presence(target, version, list_id),
                tx,
            })
            .await
            .expect("Failed to send request to message queue");

        match rx.await {
            Ok(Response::Presence(result)) => result,
            _ => Err(anyhow::anyhow!("Failed to get the presence map")),
        }
    }

    pub async fn get_file(
        &self,
        target: Target,
        version: Option<GameVersion>,
        path: String,
    ) -> Result<Arc<StoredFile>, ironworks::Error> {
        let (tx, rx) = oneshot::channel();
        self.0
            .tx
            .send(Request {
                data: RequestData::GetFile(target, version, path),
                tx,
            })
            .await
            .expect("Failed to send request to message queue");

        match rx.await {
            Ok(Response::GetFile(result)) => result,
            _ => Err(ironworks::Error::Resource(Box::new(std::io::Error::other(
                "Failed to get file",
            )))),
        }
    }

    pub async fn get_file_by_hash(
        &self,
        target: Target,
        version: Option<GameVersion>,
        repository: u8,
        category: u8,
        hash: ironworks::sqpack::IndexHash,
    ) -> Result<Arc<StoredFile>, ironworks::Error> {
        let (tx, rx) = oneshot::channel();
        self.0
            .tx
            .send(Request {
                data: RequestData::GetFileByHash(target, version, repository, category, hash),
                tx,
            })
            .await
            .expect("Failed to send request to message queue");

        match rx.await {
            Ok(Response::GetFileByHash(result)) => result,
            _ => Err(ironworks::Error::Resource(Box::new(std::io::Error::other(
                "Failed to get file by hash",
            )))),
        }
    }
}

impl Drop for MessageQueueImpl {
    fn drop(&mut self) {
        self.cancel_token.cancel();
    }
}
