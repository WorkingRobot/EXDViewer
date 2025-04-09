use std::{cell::RefCell, convert::Infallible, rc::Rc};

use eframe::wasm_bindgen::JsCast;
use gloo_worker::{HandlerId, Worker, WorkerScope};
use indexed_db::Database;
use ironworks::{Ironworks, sqpack::VInstall};
use serde::{Deserialize, Serialize};
use wasm_bindgen_futures::spawn_local;
use web_sys::{FileSystemDirectoryHandle, js_sys::JsString};

use crate::utils::tex_loader;

use super::vfs::DirectoryVfs;

pub struct SqpackWorker {
    install_instance: Rc<RefCell<Option<InstallInstance>>>,
}

#[derive(Serialize, Deserialize)]
pub struct WorkerDirectory(
    #[serde(with = "serde_wasm_bindgen::preserve")] pub FileSystemDirectoryHandle,
);

#[derive(Serialize, Deserialize)]
pub enum WorkerRequest {
    GetStoredNames(),
    GetStoredFolder(String),
    StoreFolder(WorkerDirectory),

    SetupFolder(WorkerDirectory),
    File(String),
    Texture(String),
}

#[derive(Serialize, Deserialize)]
pub enum WorkerResponse {
    GetStoredNames(Result<Vec<String>, String>),
    GetStoredFolder(Result<Option<WorkerDirectory>, String>),
    StoreFolder(Result<(), String>),

    SetupFolder(Result<(), String>),
    File(Result<Vec<u8>, String>),
    Texture(Result<(u32, u32, Vec<u8>), String>),
}

impl SqpackWorker {
    async fn get_db() -> Result<Database<String>, String> {
        let factory = indexed_db::Factory::get()
            .map_err(|e| format!("Failed to get IndexedDB factory: {e}"))?;
        Ok(factory
            .open("sqpack", 3, |evt| async move {
                _ = evt.database().delete_object_store("folders");
                evt.database().build_object_store("folders").create()?;

                Ok(())
            })
            .await
            .map_err(|e| format!("Failed to open IndexedDB database: {e}"))?)
    }

    async fn get_db_folders() -> Result<Vec<String>, String> {
        let db = Self::get_db().await?;
        db.transaction(&["folders"])
            .run(|t| async move {
                let data = t
                    .object_store("folders")
                    .map_err(|e| format!("Failed to get object store: {e}"))?
                    .get_all_keys(None)
                    .await
                    .map_err(|e| format!("Failed to get all keys: {e}"))?;
                let data = data
                    .into_iter()
                    .filter_map(|v| v.as_string())
                    .collect::<Vec<_>>();
                Ok(data)
            })
            .await
            .map_err(|e| format!("Failed to get folders: {e}"))
    }

    async fn get_db_folder(name: String) -> Result<Option<FileSystemDirectoryHandle>, String> {
        let db = Self::get_db().await?;
        db.transaction(&["folders"])
            .run(|t| async move {
                let data = t
                    .object_store("folders")
                    .map_err(|e| format!("Failed to get object store: {e}"))?
                    .get(&JsString::from(name))
                    .await
                    .map_err(|e| format!("Failed to get folder: {e}"))?;
                Ok(if let Some(data) = data {
                    let data = data
                        .dyn_into::<FileSystemDirectoryHandle>()
                        .map_err(|_| format!("Failed to cast to FileSystemDirectoryHandle"))?;
                    Some(data)
                } else {
                    None
                })
            })
            .await
            .map_err(|e| format!("Failed to get folder: {e}"))
    }

    async fn add_db_folder(handle: FileSystemDirectoryHandle) -> Result<(), String> {
        let db = Self::get_db().await?;
        db.transaction(&["folders"])
            .rw()
            .run(|t| async move {
                t.object_store("folders")
                    .map_err(|e| format!("Failed to get object store: {e}"))?
                    .put_kv(&JsString::from(handle.name()), &handle)
                    .await
                    .map_err(|e| format!("Failed to put folder: {} {e}", handle.name()))?;
                Ok(())
            })
            .await
            .map_err(|e| format!("Failed to add folder: {e}"))
    }
}

impl Worker for SqpackWorker {
    type Message = Infallible;

    type Input = WorkerRequest;

    type Output = WorkerResponse;

    fn create(_scope: &WorkerScope<Self>) -> Self {
        Self {
            install_instance: Rc::new(None.into()),
        }
    }

    fn update(&mut self, _scope: &WorkerScope<Self>, _msg: Self::Message) {
        unimplemented!("Worker does not support messages");
    }

    fn received(&mut self, scope: &WorkerScope<Self>, msg: Self::Input, id: HandlerId) {
        match msg {
            WorkerRequest::GetStoredNames() => {
                let scope = scope.clone();
                spawn_local(async move {
                    let ret = match SqpackWorker::get_db_folders().await {
                        Ok(folders) => Ok(folders),
                        Err(e) => Err(e.to_string()),
                    };
                    scope.respond(id, WorkerResponse::GetStoredNames(ret));
                });
            }
            WorkerRequest::GetStoredFolder(name) => {
                let scope = scope.clone();
                spawn_local(async move {
                    let ret = match SqpackWorker::get_db_folder(name).await {
                        Ok(folder) => Ok(folder.map(WorkerDirectory)),
                        Err(e) => Err(e.to_string()),
                    };
                    scope.respond(id, WorkerResponse::GetStoredFolder(ret));
                });
            }
            WorkerRequest::StoreFolder(handle) => {
                let scope = scope.clone();
                spawn_local(async move {
                    let ret = Self::add_db_folder(handle.0).await;
                    scope.respond(id, WorkerResponse::StoreFolder(ret));
                });
            }

            WorkerRequest::SetupFolder(handle) => {
                let install_instance = self.install_instance.clone();

                let scope = scope.clone();
                spawn_local(async move {
                    let ret = InstallInstance::new(handle.0.clone()).await;
                    let ret = match ret {
                        Ok(ret) => {
                            if let Err(e) = Self::add_db_folder(handle.0).await {
                                Err(e)
                            } else {
                                Ok(ret)
                            }
                        }
                        Err(e) => Err(e.to_string()),
                    };
                    let ret = ret.map(|instance| {
                        install_instance.borrow_mut().replace(instance);
                    });
                    scope.respond(id, WorkerResponse::SetupFolder(ret));
                });
            }
            WorkerRequest::File(path) => {
                if let Some(inst) = self.install_instance.borrow().as_ref() {
                    let file = inst.0.file::<Vec<u8>>(&path).map_err(|e| e.to_string());
                    scope.respond(id, WorkerResponse::File(file));
                }
            }
            WorkerRequest::Texture(path) => {
                if let Some(inst) = self.install_instance.borrow().as_ref() {
                    let data = tex_loader::read(&inst.0, &path)
                        .map(|data| {
                            let data = data.to_rgba8();
                            (data.width(), data.height(), data.into_vec())
                        })
                        .map_err(|e| e.to_string());
                    scope.respond(id, WorkerResponse::Texture(data));
                }
            }
        }
    }
}

struct InstallInstance(pub Ironworks);

impl InstallInstance {
    async fn new(handle: FileSystemDirectoryHandle) -> std::io::Result<Self> {
        let resource = VInstall::at_sqpack(DirectoryVfs::new(handle).await?);
        let resource = ironworks::sqpack::SqPack::new(resource);
        Ok(Self(Ironworks::new().with_resource(resource)))
    }
}
