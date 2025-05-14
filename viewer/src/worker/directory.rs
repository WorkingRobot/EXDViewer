use std::{
    collections::{HashMap, hash_map::Entry},
    path::{Path, PathBuf},
};

use eframe::wasm_bindgen::JsCast;
use futures_util::{FutureExt, StreamExt, future::LocalBoxFuture};
use wasm_bindgen_futures::{JsFuture, stream::JsStream};
use web_sys::{
    File, FileSystemDirectoryHandle, FileSystemFileHandle, FileSystemHandle, FileSystemHandleKind,
    FileSystemHandlePermissionDescriptor, FileSystemPermissionMode, FileSystemWritableFileStream,
    PermissionState,
};

use crate::utils::{JsErr, JsResult};

pub struct Directory<T> {
    handle: FileSystemDirectoryHandle,
    files: HashMap<PathBuf, T>,

    mode: FileSystemPermissionMode,
    mapper: Box<dyn Fn(FileSystemFileHandle) -> LocalBoxFuture<'static, JsResult<T>>>,
    recurse: bool,
}

impl<T> Directory<T> {
    pub async fn new(
        handle: FileSystemDirectoryHandle,
        mode: FileSystemPermissionMode,
        mapper: Box<dyn Fn(FileSystemFileHandle) -> LocalBoxFuture<'static, JsResult<T>>>,
        recurse: bool,
    ) -> JsResult<Self> {
        let mut ret = Self {
            handle,
            files: HashMap::new(),

            mode,
            mapper,
            recurse,
        };
        ret.fill_map(ret.handle.clone(), PathBuf::new()).await?;
        Ok(ret)
    }
    async fn verify_permission(&self, file: &FileSystemFileHandle) -> JsResult<()> {
        Self::verify_permission_mode(self.mode, file).await
    }

    async fn verify_permission_mode(
        mode: FileSystemPermissionMode,
        file: &FileSystemFileHandle,
    ) -> JsResult<()> {
        let perms = FileSystemHandlePermissionDescriptor::new();
        perms.set_mode(mode);
        let perm = JsFuture::from(file.query_permission_with_descriptor(&perms)).await?;
        let perm = PermissionState::from_js_value(&perm)
            .ok_or_else(|| JsErr::msg("permission is not a PermissionState"))?;
        if perm == PermissionState::Granted {
            return Ok(());
        }
        let perm = JsFuture::from(file.query_permission_with_descriptor(&perms)).await?;
        let perm = PermissionState::from_js_value(&perm)
            .ok_or_else(|| JsErr::msg("permission is not a PermissionState"))?;
        if perm == PermissionState::Granted {
            return Ok(());
        }
        Err(JsErr::msg("permission denied access to file"))
    }

    async fn fill_map(
        &mut self,
        directory: FileSystemDirectoryHandle,
        path: PathBuf,
    ) -> JsResult<()> {
        let mut entries = JsStream::from(directory.values());
        while let Some(entry) = entries.next().await {
            let entry = entry?
                .dyn_into::<FileSystemHandle>()
                .map_err(|_| JsErr::msg("entry is not a FileSystemHandle"))?;
            match entry.kind() {
                FileSystemHandleKind::File => {
                    let file_handle = entry
                        .dyn_into::<FileSystemFileHandle>()
                        .map_err(|_| JsErr::msg("entry is not a FileSystemFileHandle"))?;
                    let key = path.join(file_handle.name());
                    if let Entry::Vacant(e) = self.files.entry(key) {
                        Self::verify_permission_mode(self.mode, &file_handle).await?;
                        e.insert((*self.mapper)(file_handle).await?);
                    }
                }
                FileSystemHandleKind::Directory if self.recurse => {
                    let sub_dir = entry
                        .dyn_into::<FileSystemDirectoryHandle>()
                        .map_err(|_| JsErr::msg("entry is not a FileSystemDirectoryHandle"))?;
                    async {
                        self.fill_map(sub_dir.clone(), path.join(sub_dir.name()))
                            .await
                    }
                    .boxed_local()
                    .await?;
                }
                FileSystemHandleKind::Directory => {}
                _ => {
                    return Err(JsErr::msg("entry is not a FileSystemHandle"));
                }
            }
        }
        Ok(())
    }

    pub async fn refresh(&mut self) -> JsResult<()> {
        self.fill_map(self.handle.clone(), PathBuf::new()).await
    }

    pub fn file_exists(&self, path: impl AsRef<Path>) -> bool {
        self.files.contains_key(path.as_ref())
    }

    pub fn directory_exists(&self, path: impl AsRef<Path>) -> bool {
        self.files.keys().any(|k| {
            path.as_ref()
                .components()
                .zip(k.components())
                .all(|(a, b)| a == b)
        })
    }

    pub fn get_file_handle(&self, path: impl AsRef<Path>) -> std::io::Result<T>
    where
        T: Clone,
    {
        self.files
            .get(path.as_ref())
            .cloned()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"))
    }
}

pub async fn get_file_blob(handle: FileSystemFileHandle) -> JsResult<File> {
    JsFuture::from(handle.get_file())
        .await?
        .dyn_into::<File>()
        .map_err(|_| JsErr::msg("entry is not a File"))
}

pub async fn get_file_writer(
    handle: FileSystemFileHandle,
) -> JsResult<FileSystemWritableFileStream> {
    JsFuture::from(handle.create_writable())
        .await?
        .dyn_into::<FileSystemWritableFileStream>()
        .map_err(|_| JsErr::msg("entry is not a FileSystemWritableFileStream"))
}
