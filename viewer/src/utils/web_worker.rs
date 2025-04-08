use std::sync::Arc;

use async_broadcast::Receiver;
use eframe::wasm_bindgen::{self, JsCast};
use wasm_bindgen::{JsValue, prelude::Closure};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    DirectoryPickerOptions, FileSystemDirectoryHandle, FileSystemPermissionMode, MessageEvent,
    ServiceWorker, ServiceWorkerRegistration,
    js_sys::{self, Promise, SharedArrayBuffer, Uint8Array},
};

use super::js_error::JsError;

pub enum WorkerRequest {
    Cleanup,
    SetDirectory(FileSystemDirectoryHandle),
    EntryExists(String),
    GetFileSize(String),
    ReadFileAll(String),
    ReadFileAt(String, u64, u32),
}

pub enum WorkerResponse {
    Cleanup,
    SetDirectory,
    EntryExists(bool),
    GetFileSize(u64),
    ReadFileAll(Vec<u8>),
    ReadFileAt(Vec<u8>),
}

#[derive(Clone)]
pub struct WorkerMessenger(Arc<WorkerMessengerImpl>);

#[derive(Clone)]
struct WorkerMessengerImpl {
    service_worker: ServiceWorker,
    receiver: Receiver<JsValue>,
}

impl WorkerMessenger {
    pub async fn new() -> Result<Self, JsError> {
        let container = web_sys::window()
            .expect("no global `window` exists")
            .navigator()
            .service_worker();
        let ret = JsFuture::from(container.ready()?).await?;
        let registration = ret.dyn_into::<ServiceWorkerRegistration>()?;
        let active_worker = registration.active().expect("no active service worker");
        let (sender, receiver) = async_broadcast::broadcast(1024);
        let sender = sender.clone();
        let closure =
            Closure::<dyn FnMut(_) -> Promise>::new(move |event: MessageEvent| -> Promise {
                wasm_bindgen_futures::future_to_promise({
                    let sender = sender.clone();
                    async move {
                        let result = sender.broadcast_direct(event.data()).await;
                        match result {
                            Ok(_) => Ok(JsValue::null()),
                            Err(e) => {
                                log::error!("Failed to send message from service worker: {e}");
                                Err(JsValue::from_str(&format!("{e}")))
                            }
                        }
                    }
                })
            });
        container.add_event_listener_with_callback("message", closure.as_ref().unchecked_ref())?;
        closure.forget();

        Ok(Self(Arc::new(WorkerMessengerImpl {
            service_worker: active_worker,
            receiver,
        })))
    }

    async fn send_message_internal(
        &self,
        r#type: &str,
        id: &str,
        data: JsValue,
    ) -> Result<js_sys::Map, JsError> {
        let message = js_sys::Map::new();
        message.set(&JsValue::from_str("type"), &JsValue::from_str(r#type));
        message.set(&JsValue::from_str("id"), &JsValue::from_str(id));
        message.set(&JsValue::from_str("data"), &data);

        let mut receiver = self.0.receiver.clone();
        self.0.service_worker.post_message(&message.into())?;
        loop {
            let result = receiver
                .recv()
                .await
                .map_err(|e| JsError::from_stderror(e))?;
            let result = result.dyn_into::<js_sys::Map>()?;
            if result.get(&JsValue::from_str("id")) == id {
                return Ok(result.get(&JsValue::from_str("data")).dyn_into()?);
            }
        }
    }

    async fn send_message(&self, request: WorkerRequest) -> Result<WorkerResponse, JsError> {
        let r#type = match &request {
            WorkerRequest::Cleanup => "cleanup",
            WorkerRequest::SetDirectory(..) => "set-directory",
            WorkerRequest::EntryExists(..) => "entry-exists",
            WorkerRequest::GetFileSize(..) => "get-file-size",
            WorkerRequest::ReadFileAll(..) => "read-file-all",
            WorkerRequest::ReadFileAt(..) => "read-file-at",
        };
        let mut id_bytes = 0u128.to_le_bytes();
        getrandom::getrandom(&mut id_bytes).map_err(JsError::from_stderror)?;
        let id = format!("{:x}", u128::from_le_bytes(id_bytes));
        let mut buffer = None;
        let data = match request {
            WorkerRequest::Cleanup => JsValue::null(),
            WorkerRequest::SetDirectory(ref handle) => handle.clone().into(),
            WorkerRequest::EntryExists(ref file_name) => JsValue::from_str(file_name),
            WorkerRequest::GetFileSize(ref file_name) => JsValue::from_str(file_name),
            WorkerRequest::ReadFileAll(ref file_name) => JsValue::from_str(file_name),
            WorkerRequest::ReadFileAt(ref file_name, offset, size) => {
                let map = js_sys::Map::new();
                map.set(&JsValue::from_str("path"), &JsValue::from_str(file_name));
                buffer = Some(SharedArrayBuffer::new(size));
                map.set(
                    &JsValue::from_str("offset"),
                    &JsValue::from_f64(offset as f64),
                );
                map.set(&JsValue::from_str("buffer"), &buffer.as_ref().unwrap());
                map.into()
            }
        };
        let result = self.send_message_internal(r#type, &id, data).await?;
        let err = result.get(&JsValue::from_str("error"));
        if !err.is_undefined() {
            return Err(err.dyn_into::<js_sys::Error>()?.into());
        }
        let success = result.get(&JsValue::from_str("success"));
        if !success
            .as_bool()
            .ok_or_else(|| JsError::from_stderror("invalid success value"))?
        {
            return Err(JsError::from_stderror("operation failed"));
        }
        let response = match &request {
            WorkerRequest::Cleanup => WorkerResponse::Cleanup,
            WorkerRequest::SetDirectory(..) => WorkerResponse::SetDirectory,
            WorkerRequest::EntryExists(..) => WorkerResponse::EntryExists(
                result
                    .get(&JsValue::from_str("exists"))
                    .as_bool()
                    .ok_or_else(|| JsError::from_stderror("invalid exists value"))?,
            ),
            WorkerRequest::GetFileSize(..) => WorkerResponse::GetFileSize(
                result
                    .get(&JsValue::from_str("size"))
                    .as_f64()
                    .ok_or_else(|| JsError::from_stderror("invalid size value"))?
                    as u64,
            ),
            WorkerRequest::ReadFileAll(..) => {
                let buffer = result
                    .get(&JsValue::from_str("data"))
                    .dyn_into::<js_sys::ArrayBuffer>()?;
                WorkerResponse::ReadFileAll(Uint8Array::new(&buffer).to_vec())
            }
            WorkerRequest::ReadFileAt(..) => {
                let buffer = buffer.unwrap();
                let bytes_read = result
                    .get(&JsValue::from_str("bytes_read"))
                    .as_f64()
                    .ok_or_else(|| JsError::from_stderror("invalid bytes_read value"))?
                    as u32;
                let data = Uint8Array::new(&buffer).subarray(0, bytes_read).to_vec();
                WorkerResponse::ReadFileAt(data)
            }
        };
        Ok(response)
    }

    pub async fn set_directory(&self, handle: FileSystemDirectoryHandle) -> Result<(), JsError> {
        let result = self
            .send_message(WorkerRequest::SetDirectory(handle))
            .await?;
        if let WorkerResponse::SetDirectory = result {
            Ok(())
        } else {
            Err(JsError::from_stderror("failed to set directory"))
        }
    }

    pub async fn entry_exists(&self, path: &str) -> Result<bool, JsError> {
        let result = self
            .send_message(WorkerRequest::EntryExists(path.to_string()))
            .await?;
        if let WorkerResponse::EntryExists(exists) = result {
            Ok(exists)
        } else {
            Err(JsError::from_stderror("failed to check entry existence"))
        }
    }

    pub async fn get_file_size(&self, path: &str) -> Result<u64, JsError> {
        let result = self
            .send_message(WorkerRequest::GetFileSize(path.to_string()))
            .await?;
        if let WorkerResponse::GetFileSize(size) = result {
            Ok(size)
        } else {
            Err(JsError::from_stderror("failed to get file size"))
        }
    }

    pub async fn read_file_all(&self, path: &str) -> Result<Vec<u8>, JsError> {
        let result = self
            .send_message(WorkerRequest::ReadFileAll(path.to_string()))
            .await?;
        if let WorkerResponse::ReadFileAll(data) = result {
            Ok(data)
        } else {
            Err(JsError::from_stderror("failed to read file"))
        }
    }

    pub async fn read_file_at(
        &self,
        path: &str,
        offset: u64,
        size: u32,
    ) -> Result<Vec<u8>, JsError> {
        let result = self
            .send_message(WorkerRequest::ReadFileAt(path.to_string(), offset, size))
            .await?;
        if let WorkerResponse::ReadFileAt(data) = result {
            Ok(data)
        } else {
            Err(JsError::from_stderror("failed to read file"))
        }
    }
}

// pub async fn service_worker_exists() -> Result<bool, JsError> {
//     let window = web_sys::window().expect("no global `window` exists");
//     let registration =
//         JsFuture::from(window.navigator().service_worker().get_registration()).await?;
//     return Ok(!registration.is_undefined());
// }

pub async fn pick_folder() -> Result<FileSystemDirectoryHandle, JsError> {
    let options = DirectoryPickerOptions::new();
    options.set_id("fs-folder-picker");
    options.set_mode(FileSystemPermissionMode::Read);

    let promise = web_sys::window()
        .expect("no global `window` exists")
        .show_directory_picker_with_options(&options)?;
    let future = JsFuture::from(promise);
    let ret = future.await?;
    let handle = ret.dyn_into::<FileSystemDirectoryHandle>()?;
    Ok(handle)
}
