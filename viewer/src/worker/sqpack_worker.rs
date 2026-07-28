use std::{cell::RefCell, convert::Infallible, rc::Rc};

use eframe::wasm_bindgen::JsCast;
use futures_util::lock::Mutex;
use gloo_worker::{HandlerId, Worker, WorkerScope};
use indexed_db::Database;
use ironworks::{
    Ironworks,
    sqpack::{SqPack, VInstall},
};
use wasm_bindgen_futures::spawn_local;
use web_sys::{FileSystemDirectoryHandle, js_sys::JsString};

use crate::{
    data::{build_local_presence, index_hash, stream},
    stopwatch::Stopwatch,
    utils::{fetch_url, tex_loader},
    worker::directory::{DynamicDirectory, get_file_str, set_file_str},
};

use super::{
    WorkerDirectory, WorkerRequest, WorkerResponse, WorkerTexture, directory::verify_permission,
    vfs::DirectoryVfs,
};

pub struct SqpackWorker {
    install_instance: Rc<RefCell<Option<InstallInstance>>>,
    schema_instance: Rc<Mutex<Option<DynamicDirectory>>>,
}

const STORE_DATA: &str = "folders";
const STORE_SCHEMA: &str = "schema_folders";

impl SqpackWorker {
    async fn get_db() -> Result<Database<String>, String> {
        let factory = indexed_db::Factory::get()
            .map_err(|e| format!("Failed to get IndexedDB factory: {e}"))?;
        factory
            .open("sqpack", 4, |evt| async move {
                let db = evt.database();
                let _ = db.delete_object_store(STORE_DATA);
                let _ = db.delete_object_store(STORE_SCHEMA);

                db.build_object_store(STORE_DATA).create()?;
                db.build_object_store(STORE_SCHEMA).create()?;

                Ok(())
            })
            .await
            .map_err(|e| format!("Failed to open IndexedDB database: {e}"))
    }

    async fn get_db_folders_impl(store: &'static str) -> Result<Vec<WorkerDirectory>, String> {
        let db = Self::get_db().await?;
        db.transaction(&[store])
            .run(move |t| async move {
                let data = t
                    .object_store(store)
                    .map_err(|e| format!("Failed to get object store: {e}"))?
                    .get_all(None)
                    .await
                    .map_err(|e| format!("Failed to get all values: {e}"))?;

                data.into_iter()
                    .map(|v| {
                        v.dyn_into::<FileSystemDirectoryHandle>()
                            .map(WorkerDirectory)
                            .map_err(|_| indexed_db::Error::InvalidKey)
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .await
            .map_err(|e| format!("Failed to get folders: {e}"))
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
}

fn decode_texture(bytes: &[u8], path: &str, max_dim: Option<u16>) -> Result<WorkerTexture, String> {
    tex_loader::decode_preview_sized(bytes, path, max_dim)
        .map(|(image, source)| {
            let image = image.to_rgba8();
            WorkerTexture {
                width: image.width(),
                height: image.height(),
                source,
                data: image.into_vec(),
            }
        })
        .map_err(|error| error.to_string())
}

impl Worker for SqpackWorker {
    type Message = Infallible;

    type Input = WorkerRequest;

    type Output = WorkerResponse;

    fn create(_scope: &WorkerScope<Self>) -> Self {
        Self {
            install_instance: Rc::new(None.into()),
            schema_instance: Rc::new(Mutex::new(None)),
        }
    }

    fn update(&mut self, _scope: &WorkerScope<Self>, _msg: Self::Message) {
        unimplemented!("Worker does not support messages");
    }

    fn received(&mut self, scope: &WorkerScope<Self>, msg: Self::Input, id: HandlerId) {
        match msg {
            WorkerRequest::DataGet() => {
                let _stop = Stopwatch::new("SqpackWorker::DataGet");
                let scope = scope.clone();
                spawn_local(async move {
                    let _stop = _stop;
                    let ret = SqpackWorker::get_db_folders_impl(STORE_DATA).await;
                    scope.respond(id, WorkerResponse::DataGet(ret));
                });
            }
            WorkerRequest::DataStore(handle) => {
                let _stop = Stopwatch::new("SqpackWorker::DataStore");
                let scope = scope.clone();
                spawn_local(async move {
                    let _stop = _stop;
                    let ret = Self::add_db_folder_impl(STORE_DATA, handle.0).await;
                    scope.respond(id, WorkerResponse::DataStore(ret));
                });
            }
            WorkerRequest::DataSetup(handle) => {
                let _stop = Stopwatch::new("SqpackWorker::DataSetup");
                let install_instance = self.install_instance.clone();

                let scope = scope.clone();
                spawn_local(async move {
                    let _stop = _stop;
                    let mut ret = InstallInstance::new(handle.0.clone())
                        .await
                        .map_err(|e| e.to_string());
                    if ret.is_ok() {
                        ret = Self::add_db_folder_impl(STORE_DATA, handle.0)
                            .await
                            .and(ret);
                    }
                    let ret = ret.map(|instance| {
                        install_instance.borrow_mut().replace(instance);
                    });
                    scope.respond(id, WorkerResponse::DataSetup(ret));
                });
            }
            WorkerRequest::DataRequestFile(path) => {
                let _stop = Stopwatch::new(format!("SqpackWorker::DataRequestFile({path:?})"));
                if let Some(inst) = self.install_instance.borrow().as_ref() {
                    let file = inst
                        .sqpack
                        .file(&path)
                        .and_then(stream)
                        .map_err(|e| e.to_string());
                    scope.respond(id, WorkerResponse::DataRequestFile(file));
                }
            }
            WorkerRequest::DataRequestFileByHash((repository, category, hash, split)) => {
                let _stop = Stopwatch::new(format!(
                    "SqpackWorker::DataRequestFileByHash({repository}/{category}/{hash:X})"
                ));
                if let Some(inst) = self.install_instance.borrow().as_ref() {
                    let file = inst
                        .sqpack
                        .file_by_hash(repository, category, index_hash(hash, split))
                        .and_then(stream)
                        .map_err(|e| e.to_string());
                    scope.respond(id, WorkerResponse::DataRequestFileByHash(file));
                }
            }
            WorkerRequest::DataPresence(url) => {
                let _stop = Stopwatch::new("SqpackWorker::DataPresence");
                let install_instance = self.install_instance.clone();
                let scope = scope.clone();
                spawn_local(async move {
                    let _stop = _stop;
                    let paths = match fetch_url(&url).await {
                        Ok(paths) => paths,
                        Err(error) => {
                            scope.respond(id, WorkerResponse::DataPresence(Err(error.to_string())));
                            return;
                        }
                    };
                    let map = match install_instance.borrow().as_ref() {
                        Some(inst) => build_local_presence(&inst.sqpack, &paths)
                            .map_err(|error| error.to_string()),
                        None => Err("no install is set up".to_string()),
                    };
                    scope.respond(id, WorkerResponse::DataPresence(map));
                });
            }
            WorkerRequest::DataRequestTexture((path, max_dim)) => {
                let _stop = Stopwatch::new(format!("SqpackWorker::DataRequestTexture({path:?})"));
                if let Some(inst) = self.install_instance.borrow().as_ref() {
                    let data = inst
                        .ironworks
                        .file::<Vec<u8>>(&path)
                        .map_err(|error| error.to_string())
                        .and_then(|bytes| decode_texture(&bytes, &path, max_dim));
                    scope.respond(id, WorkerResponse::DataRequestTexture(data));
                }
            }
            WorkerRequest::DecodeTexture {
                path,
                bytes,
                max_dim,
            } => {
                let _stop = Stopwatch::new(format!("SqpackWorker::DecodeTexture({path:?})"));
                let data = decode_texture(&bytes, &path, max_dim);
                scope.respond(id, WorkerResponse::DecodeTexture(data));
            }
            WorkerRequest::DataRequestExists(paths) => {
                let _stop = Stopwatch::new(format!("SqpackWorker::DataRequestExists({paths:?})"));
                if let Some(inst) = self.install_instance.borrow().as_ref() {
                    let mut result = Vec::with_capacity(paths.len());
                    for path in paths {
                        result.push(inst.ironworks.exists(&path).map_err(|e| e.to_string()));
                    }
                    let result: Result<Vec<bool>, String> = result.into_iter().collect();
                    scope.respond(id, WorkerResponse::DataRequestExists(result));
                }
            }
            WorkerRequest::SchemaGet() => {
                let _stop = Stopwatch::new("SqpackWorker::SchemaGet");
                let scope = scope.clone();
                spawn_local(async move {
                    let _stop = _stop;
                    let ret = SqpackWorker::get_db_folders_impl(STORE_SCHEMA).await;
                    scope.respond(id, WorkerResponse::SchemaGet(ret));
                });
            }
            WorkerRequest::SchemaStore(handle) => {
                let _stop = Stopwatch::new("SqpackWorker::SchemaStore");
                let scope = scope.clone();
                spawn_local(async move {
                    let _stop = _stop;
                    let ret = Self::add_db_folder_impl(STORE_SCHEMA, handle.0).await;
                    scope.respond(id, WorkerResponse::SchemaStore(ret));
                });
            }
            WorkerRequest::SchemaSetup(handle) => {
                let _stop = Stopwatch::new("SqpackWorker::SchemaSetup");
                let schema_instance = self.schema_instance.clone();

                let scope = scope.clone();
                spawn_local(async move {
                    let _stop = _stop;
                    let ret = DynamicDirectory::new(
                        handle.0.clone(),
                        web_sys::FileSystemPermissionMode::Readwrite,
                        false,
                    );
                    schema_instance.lock().await.replace(ret);
                    let result = Self::add_db_folder_impl(STORE_SCHEMA, handle.0).await;
                    scope.respond(id, WorkerResponse::SchemaSetup(result));
                });
            }
            WorkerRequest::SchemaRequestGet(name) => {
                let _stop = Stopwatch::new(format!("SqpackWorker::SchemaRequestGet({name:?})"));
                let schema_instance = self.schema_instance.clone();

                let scope = scope.clone();
                spawn_local(async move {
                    let _stop = _stop;
                    if let Some(inst) = schema_instance.lock().await.as_ref() {
                        match inst.get_file_handle(name).await.map_err(|e| e.to_string()) {
                            Ok(handle) => {
                                let ret = get_file_str(handle).await.map_err(|e| e.to_string());
                                scope.respond(id, WorkerResponse::SchemaRequestGet(ret));
                            }
                            Err(e) => {
                                scope.respond(id, WorkerResponse::SchemaRequestGet(Err(e)));
                            }
                        }
                    }
                });
            }
            WorkerRequest::SchemaRequestStore((name, data)) => {
                let _stop = Stopwatch::new(format!("SqpackWorker::SchemaRequestStore({name:?})"));
                let schema_instance = self.schema_instance.clone();
                let scope = scope.clone();
                spawn_local(async move {
                    if let Some(inst) = schema_instance.lock().await.as_ref() {
                        let _stop = _stop;
                        match inst.get_file_handle(name).await.map_err(|e| e.to_string()) {
                            Ok(handle) => {
                                let ret =
                                    set_file_str(handle, &data).await.map_err(|e| e.to_string());
                                scope.respond(id, WorkerResponse::SchemaRequestStore(ret));
                            }
                            Err(e) => {
                                scope.respond(id, WorkerResponse::SchemaRequestStore(Err(e)));
                            }
                        }
                    }
                });
            }
            WorkerRequest::VerifyFolder((handle, is_readwrite)) => {
                let _stop = Stopwatch::new("SqpackWorker::VerifyFolder");
                let scope = scope.clone();
                spawn_local(async move {
                    let _stop = _stop;
                    let ret = verify_permission(
                        if is_readwrite {
                            web_sys::FileSystemPermissionMode::Readwrite
                        } else {
                            web_sys::FileSystemPermissionMode::Read
                        },
                        &handle.0,
                    )
                    .await
                    .map_err(|e| e.to_string());
                    scope.respond(id, WorkerResponse::VerifyFolder(ret));
                });
            }
        }
    }
}

struct InstallInstance {
    ironworks: Ironworks<Rc<SqPack<VInstall<DirectoryVfs>>>>,
    /// The same resource the `Ironworks` holds. Hash lookups are a SqPack concept, so they need the
    /// concrete type; sharing it keeps one index cache rather than two.
    sqpack: Rc<SqPack<VInstall<DirectoryVfs>>>,
}

impl InstallInstance {
    async fn new(handle: FileSystemDirectoryHandle) -> std::io::Result<Self> {
        let resource = VInstall::at_sqpack(
            DirectoryVfs::new(handle)
                .await
                .map_err(std::io::Error::other)?,
        );
        let sqpack = Rc::new(SqPack::new(resource));
        Ok(Self {
            ironworks: Ironworks::new().with_resource(sqpack.clone()),
            sqpack,
        })
    }
}
