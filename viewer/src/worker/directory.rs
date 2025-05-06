use std::{
    collections::HashMap,
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

use super::map_jserr;

pub struct Directory<T> {
    handle: FileSystemDirectoryHandle,
    files: HashMap<PathBuf, T>,

    mode: FileSystemPermissionMode,
    mapper: Box<dyn Fn(FileSystemFileHandle) -> LocalBoxFuture<'static, std::io::Result<T>>>,
    recurse: bool,
}

impl<T> Directory<T> {
    pub async fn new(
        handle: FileSystemDirectoryHandle,
        mode: FileSystemPermissionMode,
        mapper: Box<dyn Fn(FileSystemFileHandle) -> LocalBoxFuture<'static, std::io::Result<T>>>,
        recurse: bool,
    ) -> std::io::Result<Self> {
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

    async fn verify_permission(&self, file: &FileSystemFileHandle) -> std::io::Result<()> {
        let perms = FileSystemHandlePermissionDescriptor::new();
        perms.set_mode(self.mode);
        let perm = JsFuture::from(file.query_permission_with_descriptor(&perms))
            .await
            .map_err(map_jserr)?;
        let perm = PermissionState::from_js_value(&perm).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "permission is not a PermissionState",
            )
        })?;
        if perm == PermissionState::Granted {
            return Ok(());
        }
        let perm = JsFuture::from(file.query_permission_with_descriptor(&perms))
            .await
            .map_err(map_jserr)?;
        let perm = PermissionState::from_js_value(&perm).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "permission is not a PermissionState",
            )
        })?;
        if perm == PermissionState::Granted {
            return Ok(());
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "permission denied access to file",
        ))
    }

    async fn fill_map(
        &mut self,
        directory: FileSystemDirectoryHandle,
        path: PathBuf,
    ) -> std::io::Result<()> {
        let mut entries = JsStream::from(directory.values());
        while let Some(entry) = entries.next().await {
            let entry = entry.map_err(map_jserr)?;
            let entry = entry.dyn_into::<FileSystemHandle>().map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "entry is not a FileSystemHandle",
                )
            })?;
            match entry.kind() {
                FileSystemHandleKind::File => {
                    let file_handle = entry.dyn_into::<FileSystemFileHandle>().map_err(|_| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "entry is not a FileSystemFileHandle",
                        )
                    })?;
                    let key = path.join(file_handle.name());
                    if !self.files.contains_key(&key) {
                        self.verify_permission(&file_handle).await?;
                        self.files.insert(key, (*self.mapper)(file_handle).await?);
                    }
                }
                FileSystemHandleKind::Directory if self.recurse => {
                    let sub_dir = entry.dyn_into::<FileSystemDirectoryHandle>().map_err(|_| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "entry is not a FileSystemDirectoryHandle",
                        )
                    })?;
                    async {
                        self.fill_map(sub_dir.clone(), path.join(sub_dir.name()))
                            .await
                    }
                    .boxed_local()
                    .await?;
                }
                FileSystemHandleKind::Directory => {}
                _ => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "entry is not a FileSystemHandle",
                    ));
                }
            }
        }
        Ok(())
    }

    pub async fn refresh(&mut self) -> std::io::Result<()> {
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

pub async fn get_file_blob(handle: FileSystemFileHandle) -> std::io::Result<File> {
    let result = JsFuture::from(handle.get_file()).await;
    let result = result.map_err(map_jserr)?;
    let result = result.dyn_into::<File>().map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "entry is not a File")
    })?;
    Ok(result)
}

pub async fn get_file_writer(
    handle: FileSystemFileHandle,
) -> std::io::Result<FileSystemWritableFileStream> {
    let result = JsFuture::from(handle.create_writable()).await;
    let result = result.map_err(map_jserr)?;
    let result = result
        .dyn_into::<FileSystemWritableFileStream>()
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "entry is not a FileSystemWritableFileStream",
            )
        })?;
    Ok(result)
}
