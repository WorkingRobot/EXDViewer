use std::sync::{Arc, OnceLock};

use async_channel::Sender;
use tokio::{
    runtime::Handle, select, sync::oneshot, task::JoinHandle
};
use tokio_util::sync::CancellationToken;
use xiv_core::file::version::GameVersion;

use crate::data::{GameData, VersionInfo};

#[derive(Debug, Clone)]
pub enum RequestData {
    Versions,
    GetFile(Option<GameVersion>, String),
}

pub enum Response {
    Versions(Option<VersionInfo>),
    GetFile(Result<Arc<Vec<u8>>, ironworks::Error>),
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

    threads: OnceLock<Vec<JoinHandle<()>>>,
    cancel_token: CancellationToken,
    tx: Sender<Request>,
}

impl MessageQueue {
    pub fn new(data: Arc<GameData>, workers: usize) -> anyhow::Result<Self> {
        let (thread_tx, thread_rx) = async_channel::unbounded();
        let this = Self(Arc::new(MessageQueueImpl {
            data,
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
                                        RequestData::Versions => {
                                            Response::Versions(this.data.versions().await)
                                        }
                                        RequestData::GetFile(version, path) => {
                                            let version = match version {
                                                Some(v) => Ok(v),
                                                None => {
                                                    this.data.versions().await.map(|v| v.latest).ok_or_else(|| ironworks::Error::NotFound(ironworks::ErrorValue::Other("No version info available".to_string())))
                                                }
                                            };
                                            let result = match version { 
                                                Ok(version) => {
                                                    this.data.get(version, path).await
                                                }
                                                Err(e) => Err(e),
                                            };

                                            Response::GetFile(result)
                                        }
                                    }
                                };

                                // let response = tokio::time::timeout(
                                //     std::time::Duration::from_secs(15),
                                //     response,
                                // );

                                let response = tokio::task::block_in_place(|| {
                                    Handle::current().block_on(response)
                                });

                                // let response = match response {
                                //     Ok(response) => response,
                                //     Err(_) => {
                                //         log::error!("Request timed out: {:?}", request.data);
                                //         Response::GetFile(Err(std::io::Error::other("Request timed out").into()))
                                //     }
                                // };

                                _ = request.tx.send(response);
                            }
                        }
                    }
                })
            })
            .collect::<Vec<_>>();


        this.0.threads
            .set(
                threads,
            )
            .map_err(|_| anyhow::anyhow!("Failed to initialize message queue threads"))?;

        Ok(this)
    }

    pub async fn versions(&self) -> Option<VersionInfo> {
        let (tx, rx) = oneshot::channel();
        self.0.tx.send(Request {
            data: RequestData::Versions,
            tx,
        }).await.expect("Failed to send request to message queue");

        match rx.await {
            Ok(Response::Versions(info)) => info,
            _ => None,
        }
    }

    pub async fn get_file(&self, version: Option<GameVersion>, path: String) -> Result<Arc<Vec<u8>>, ironworks::Error> {
        let (tx, rx) = oneshot::channel();
        self.0.tx.send(Request {
            data: RequestData::GetFile(version, path),
            tx,
        }).await.expect("Failed to send request to message queue");

        match rx.await {
            Ok(Response::GetFile(result)) => result,
            _ => Err(ironworks::Error::Resource(Box::new(std::io::Error::other(
                "Failed to get file",
            )))),
        }
    }
}

impl Drop for MessageQueueImpl {
    fn drop(&mut self) {
        self.cancel_token.cancel();
    }
}