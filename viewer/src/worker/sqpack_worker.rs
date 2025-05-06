use std::{cell::RefCell, convert::Infallible, future::ready, io::Read, rc::Rc};

use eframe::wasm_bindgen::JsCast;
use gloo_worker::{HandlerId, Worker, WorkerScope};
use indexed_db::Database;
use ironworks::{
    Ironworks,
    sqpack::{SqPack, VInstall},
};
use serde::{Deserialize, Serialize};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{FileSystemDirectoryHandle, FileSystemFileHandle, js_sys::JsString};

use crate::utils::tex_loader;

use super::{
    directory::{Directory, get_file_blob, get_file_writer},
    file::SyncAccessFile,
    map_jserr,
    stopwatch::Stopwatch,
    vfs::DirectoryVfs,
};

pub struct SqpackWorker {
    install_instance: Rc<RefCell<Option<InstallInstance>>>,
    schema_instance: Rc<RefCell<Option<Directory<FileSystemFileHandle>>>>,
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

    GetStoredSchemas(),
    GetStoredSchemaFolder(String),
    StoreSchemaFolder(WorkerDirectory),

    SetupSchemaFolder(WorkerDirectory),
    GetSchema(String),
    StoreSchema((String, String)),
}

#[derive(Serialize, Deserialize)]
pub enum WorkerResponse {
    GetStoredNames(Result<Vec<String>, String>),
    GetStoredFolder(Result<Option<WorkerDirectory>, String>),
    StoreFolder(Result<(), String>),

    SetupFolder(Result<(), String>),
    File(Result<Vec<u8>, String>),
    Texture(Result<(u32, u32, Vec<u8>), String>),

    GetStoredSchemas(Result<Vec<String>, String>),
    GetStoredSchemaFolder(Result<Option<WorkerDirectory>, String>),
    StoreSchemaFolder(Result<(), String>),

    SetupSchemaFolder(Result<(), String>),
    GetSchema(Result<String, String>),
    StoreSchema(Result<(), String>),
}

impl SqpackWorker {
    async fn get_db() -> Result<Database<String>, String> {
        let factory = indexed_db::Factory::get()
            .map_err(|e| format!("Failed to get IndexedDB factory: {e}"))?;
        Ok(factory
            .open("sqpack", 4, |evt| async move {
                let db = evt.database();
                let _ = db.delete_object_store("folders");
                let _ = db.delete_object_store("schema_folders");

                db.build_object_store("folders").create()?;
                db.build_object_store("schema_folders").create()?;

                Ok(())
            })
            .await
            .map_err(|e| format!("Failed to open IndexedDB database: {e}"))?)
    }

    async fn get_db_folders_impl(store: &'static str) -> Result<Vec<String>, String> {
        let db = Self::get_db().await?;
        db.transaction(&[store])
            .run(move |t| async move {
                let data = t
                    .object_store(store)
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

    async fn get_db_folders() -> Result<Vec<String>, String> {
        Self::get_db_folders_impl("folders").await
    }

    async fn get_db_schema_folders() -> Result<Vec<String>, String> {
        Self::get_db_folders_impl("schema_folders").await
    }

    async fn get_db_folder_impl(
        store: &'static str,
        name: String,
    ) -> Result<Option<FileSystemDirectoryHandle>, String> {
        let db = Self::get_db().await?;
        db.transaction(&[store])
            .run(move |t| async move {
                let data = t
                    .object_store(store)
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

    async fn get_db_folder(name: String) -> Result<Option<FileSystemDirectoryHandle>, String> {
        Self::get_db_folder_impl("folders", name).await
    }

    async fn get_db_schema_folder(
        name: String,
    ) -> Result<Option<FileSystemDirectoryHandle>, String> {
        Self::get_db_folder_impl("schema_folders", name).await
    }

    async fn add_db_folder_impl(
        store: &'static str,
        handle: FileSystemDirectoryHandle,
    ) -> Result<(), String> {
        let db = Self::get_db().await?;
        db.transaction(&[store])
            .rw()
            .run(move |t| async move {
                t.object_store(store)
                    .map_err(|e| format!("Failed to get object store: {e}"))?
                    .put_kv(&JsString::from(handle.name()), &handle)
                    .await
                    .map_err(|e| format!("Failed to put folder: {} {e}", handle.name()))?;
                Ok(())
            })
            .await
            .map_err(|e| format!("Failed to add folder: {e}"))
    }

    async fn add_db_folder(handle: FileSystemDirectoryHandle) -> Result<(), String> {
        Self::add_db_folder_impl("folders", handle).await
    }

    async fn add_db_schema_folder(handle: FileSystemDirectoryHandle) -> Result<(), String> {
        Self::add_db_folder_impl("schema_folders", handle).await
    }
}

impl Worker for SqpackWorker {
    type Message = Infallible;

    type Input = WorkerRequest;

    type Output = WorkerResponse;

    fn create(_scope: &WorkerScope<Self>) -> Self {
        Self {
            install_instance: Rc::new(None.into()),
            schema_instance: Rc::new(None.into()),
        }
    }

    fn update(&mut self, _scope: &WorkerScope<Self>, _msg: Self::Message) {
        unimplemented!("Worker does not support messages");
    }

    fn received(&mut self, scope: &WorkerScope<Self>, msg: Self::Input, id: HandlerId) {
        match msg {
            WorkerRequest::GetStoredNames() => {
                let _stop = Stopwatch::new("SqpackWorker::GetStoredNames");
                let scope = scope.clone();
                spawn_local(async move {
                    let _stop = _stop;
                    let ret = match SqpackWorker::get_db_folders().await {
                        Ok(folders) => Ok(folders),
                        Err(e) => Err(e.to_string()),
                    };
                    scope.respond(id, WorkerResponse::GetStoredNames(ret));
                });
            }
            WorkerRequest::GetStoredFolder(name) => {
                let _stop = Stopwatch::new("SqpackWorker::GetStoredFolder");
                let scope = scope.clone();
                spawn_local(async move {
                    let _stop = _stop;
                    let ret = match SqpackWorker::get_db_folder(name).await {
                        Ok(folder) => Ok(folder.map(WorkerDirectory)),
                        Err(e) => Err(e.to_string()),
                    };
                    scope.respond(id, WorkerResponse::GetStoredFolder(ret));
                });
            }
            WorkerRequest::StoreFolder(handle) => {
                let _stop = Stopwatch::new("SqpackWorker::StoreFolder");
                let scope = scope.clone();
                spawn_local(async move {
                    let _stop = _stop;
                    let ret = Self::add_db_folder(handle.0).await;
                    scope.respond(id, WorkerResponse::StoreFolder(ret));
                });
            }
            WorkerRequest::SetupFolder(handle) => {
                let _stop = Stopwatch::new("SqpackWorker::SetupFolder");
                let install_instance = self.install_instance.clone();

                let scope = scope.clone();
                spawn_local(async move {
                    let _stop = _stop;
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
                let _stop = Stopwatch::new("SqpackWorker::File");
                if let Some(inst) = self.install_instance.borrow().as_ref() {
                    let file = inst.0.file::<Vec<u8>>(&path).map_err(|e| e.to_string());
                    scope.respond(id, WorkerResponse::File(file));
                }
            }
            WorkerRequest::Texture(path) => {
                let _stop = Stopwatch::new("SqpackWorker::Texture");
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
            WorkerRequest::GetStoredSchemas() => {
                let _stop = Stopwatch::new("SqpackWorker::GetStoredSchemas");
                let scope = scope.clone();
                spawn_local(async move {
                    let _stop = _stop;
                    let ret = match SqpackWorker::get_db_schema_folders().await {
                        Ok(folders) => Ok(folders),
                        Err(e) => Err(e.to_string()),
                    };
                    scope.respond(id, WorkerResponse::GetStoredSchemas(ret));
                });
            }
            WorkerRequest::GetStoredSchemaFolder(name) => {
                let _stop = Stopwatch::new("SqpackWorker::GetStoredSchemaFolder");
                let scope = scope.clone();
                spawn_local(async move {
                    let _stop = _stop;
                    let ret = match SqpackWorker::get_db_schema_folder(name).await {
                        Ok(folder) => Ok(folder.map(WorkerDirectory)),
                        Err(e) => Err(e.to_string()),
                    };
                    scope.respond(id, WorkerResponse::GetStoredSchemaFolder(ret));
                });
            }
            WorkerRequest::StoreSchemaFolder(handle) => {
                let _stop = Stopwatch::new("SqpackWorker::StoreSchemaFolder");
                let scope = scope.clone();
                spawn_local(async move {
                    let _stop = _stop;
                    let ret = Self::add_db_schema_folder(handle.0).await;
                    scope.respond(id, WorkerResponse::StoreSchemaFolder(ret));
                });
            }
            WorkerRequest::SetupSchemaFolder(handle) => {
                let _stop = Stopwatch::new("SqpackWorker::SetupSchemaFolder");
                let schema_instance = self.schema_instance.clone();

                let scope = scope.clone();
                spawn_local(async move {
                    let _stop = _stop;
                    let ret = Directory::new(
                        handle.0.clone(),
                        web_sys::FileSystemPermissionMode::Readwrite,
                        Box::new(|handle| Box::pin(ready(Ok(handle)))),
                        false,
                    )
                    .await;
                    let ret = match ret {
                        Ok(ret) => {
                            if let Err(e) = Self::add_db_schema_folder(handle.0).await {
                                Err(e)
                            } else {
                                Ok(ret)
                            }
                        }
                        Err(e) => Err(e.to_string()),
                    };
                    let ret = ret.map(|instance| {
                        schema_instance.borrow_mut().replace(instance);
                    });
                    scope.respond(id, WorkerResponse::SetupSchemaFolder(ret));
                });
            }
            WorkerRequest::GetSchema(name) => {
                let _stop = Stopwatch::new("SqpackWorker::GetSchema");
                if let Some(inst) = self.schema_instance.borrow().as_ref() {
                    match inst.get_file_handle(name) {
                        Ok(handle) => {
                            let scope = scope.clone();
                            spawn_local(async move {
                                let _stop = _stop;
                                let ret = get_file_blob(handle)
                                    .await
                                    .and_then(SyncAccessFile::new)
                                    .and_then(|mut f| {
                                        let mut s = String::new();
                                        f.read_to_string(&mut s)?;
                                        Ok(s)
                                    })
                                    .map_err(|e| e.to_string());
                                scope.respond(id, WorkerResponse::GetSchema(ret));
                            });
                        }
                        Err(e) => {
                            scope.respond(id, WorkerResponse::GetSchema(Err(e.to_string())));
                        }
                    };
                }
            }
            WorkerRequest::StoreSchema((name, data)) => {
                let _stop = Stopwatch::new("SqpackWorker::StoreSchema");
                if let Some(inst) = self.schema_instance.borrow().as_ref() {
                    match inst.get_file_handle(name) {
                        Ok(handle) => {
                            let scope = scope.clone();
                            spawn_local(async move {
                                let _stop = _stop;
                                let ret = get_file_writer(handle).await;
                                let ret = match ret {
                                    Ok(stream) => {
                                        let write_result =
                                            match stream.write_with_str(data.as_str()) {
                                                Ok(promise) => {
                                                    JsFuture::from(promise).await.map_err(map_jserr)
                                                }
                                                Err(e) => Err(map_jserr(e)),
                                            };

                                        let close_result = JsFuture::from(stream.close())
                                            .await
                                            .map_err(map_jserr)
                                            .map(|_| ());

                                        write_result.and(close_result)
                                    }
                                    Err(e) => Err(e),
                                }
                                .map_err(|e| e.to_string());
                                scope.respond(id, WorkerResponse::StoreSchema(ret));
                            });
                        }
                        Err(e) => {
                            scope.respond(id, WorkerResponse::StoreSchema(Err(e.to_string())));
                        }
                    };
                }
            }
        }
    }
}

struct InstallInstance(pub Ironworks<SqPack<VInstall<DirectoryVfs>>>);

impl InstallInstance {
    async fn new(handle: FileSystemDirectoryHandle) -> std::io::Result<Self> {
        let resource = VInstall::at_sqpack(DirectoryVfs::new(handle).await?);
        let resource = SqPack::new(resource);
        Ok(Self(Ironworks::new().with_resource(resource)))
    }
}
