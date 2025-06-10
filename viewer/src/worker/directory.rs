use std::{
    collections::{HashMap, hash_map::Entry},
    path::{Path, PathBuf},
};

use eframe::wasm_bindgen::JsCast;
use futures_util::{FutureExt, StreamExt};
use itertools::Itertools;
use wasm_bindgen_futures::{JsFuture, stream::JsStream};
use web_sys::{
    File, FileSystemDirectoryHandle, FileSystemFileHandle, FileSystemHandle, FileSystemHandleKind,
    FileSystemHandlePermissionDescriptor, FileSystemPermissionMode, FileSystemWritableFileStream,
    PermissionState,
};

use crate::utils::{JsErr, JsResult};

pub struct Directory {
    backend: DynamicDirectory,
    files: HashMap<PathBuf, File>,
}

impl Directory {
    pub async fn new(
        handle: FileSystemDirectoryHandle,
        mode: FileSystemPermissionMode,
        recurse: bool,
    ) -> JsResult<Self> {
        let backend = DynamicDirectory::new(handle, mode, recurse);
        let files = backend.get_file_blob_map().await?;
        Ok(Self { backend, files })
    }

    pub async fn refresh(&mut self) -> JsResult<()> {
        self.files = self.backend.get_file_blob_map().await?;
        Ok(())
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

    pub fn get_file_handle(&self, path: impl AsRef<Path>) -> std::io::Result<File> {
        self.files
            .get(path.as_ref())
            .cloned()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"))
    }
}

pub struct DynamicDirectory {
    handle: FileSystemDirectoryHandle,
    mode: FileSystemPermissionMode,
    recurse: bool,
}

impl DynamicDirectory {
    pub fn new(
        handle: FileSystemDirectoryHandle,
        mode: FileSystemPermissionMode,
        recurse: bool,
    ) -> Self {
        Self {
            handle,
            mode,
            recurse,
        }
    }

    async fn fill_map<T, F: Future<Output = JsResult<T>>>(
        &self,
        files: &mut HashMap<PathBuf, T>,
        mapper: impl Copy + Fn(FileSystemFileHandle) -> F,
        directory: FileSystemDirectoryHandle,
        path: PathBuf,
    ) -> JsResult<()> {
        verify_permission(self.mode, &directory).await?;
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
                    if let Entry::Vacant(e) = files.entry(key) {
                        verify_permission(self.mode, &file_handle).await?;
                        e.insert(mapper(file_handle).await?);
                    }
                }
                FileSystemHandleKind::Directory if self.recurse => {
                    let sub_dir = entry
                        .dyn_into::<FileSystemDirectoryHandle>()
                        .map_err(|_| JsErr::msg("entry is not a FileSystemDirectoryHandle"))?;
                    async {
                        let sub_dir_path = path.join(sub_dir.name());
                        self.fill_map(files, mapper, sub_dir, sub_dir_path).await
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

    pub async fn get_file_map(&self) -> JsResult<HashMap<PathBuf, FileSystemFileHandle>> {
        let mut files = HashMap::new();
        self.fill_map(
            &mut files,
            async |f| Ok(f),
            self.handle.clone(),
            PathBuf::new(),
        )
        .await?;
        Ok(files)
    }

    pub async fn get_file_blob_map(&self) -> JsResult<HashMap<PathBuf, File>> {
        let mut blobs = HashMap::new();
        self.fill_map(
            &mut blobs,
            get_file_blob,
            self.handle.clone(),
            PathBuf::new(),
        )
        .await?;
        Ok(blobs)
    }

    pub async fn get_file_handle(&self, path: impl AsRef<Path>) -> JsResult<FileSystemFileHandle> {
        let path = path.as_ref();
        let mut current_dir = self.handle.clone();
        let components = path.components().collect_vec();

        for component in &components[..components.len().saturating_sub(1)] {
            match component {
                std::path::Component::Normal(name) => {
                    let entry =
                        JsFuture::from(current_dir.get_directory_handle(&name.to_string_lossy()))
                            .await?;
                    current_dir = entry
                        .dyn_into::<FileSystemDirectoryHandle>()
                        .map_err(|_| JsErr::msg("entry is not a FileSystemDirectoryHandle"))?;
                }
                _ => return Err(JsErr::msg("invalid path component")),
            }
        }

        if let Some(std::path::Component::Normal(filename)) = components.last() {
            let entry =
                JsFuture::from(current_dir.get_file_handle(&filename.to_string_lossy())).await?;
            entry
                .dyn_into::<FileSystemFileHandle>()
                .map_err(|_| JsErr::msg("entry is not a FileSystemFileHandle"))
        } else {
            Err(JsErr::msg("invalid file path"))
        }
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

pub async fn get_file_str(handle: FileSystemFileHandle) -> JsResult<String> {
    let file = get_file_blob(handle).await?;
    JsFuture::from(file.text())
        .await?
        .as_string()
        .ok_or_else(|| JsErr::msg("file text is not a string"))
}

pub async fn set_file_str(handle: FileSystemFileHandle, content: &str) -> JsResult<()> {
    let writer = get_file_writer(handle).await?;

    let write_result = match writer.write_with_str(content) {
        Ok(promise) => JsFuture::from(promise).await.map_err(JsErr::from),
        Err(e) => Err(JsErr::from(e)),
    };
    let close_result = JsFuture::from(writer.close())
        .await
        .map_err(JsErr::from)
        .map(|_| ());

    write_result.and(close_result)
}

pub async fn verify_permission(
    mode: FileSystemPermissionMode,
    handle: &FileSystemHandle,
) -> JsResult<()> {
    let perms = FileSystemHandlePermissionDescriptor::new();
    perms.set_mode(mode);
    let perm = JsFuture::from(handle.query_permission_with_descriptor(&perms)).await?;
    let perm = PermissionState::from_js_value(&perm)
        .ok_or_else(|| JsErr::msg("permission is not a PermissionState"))?;
    if perm == PermissionState::Granted {
        return Ok(());
    }
    let perm = JsFuture::from(handle.request_permission_with_descriptor(&perms)).await?;
    let perm = PermissionState::from_js_value(&perm)
        .ok_or_else(|| JsErr::msg("permission is not a PermissionState"))?;
    if perm == PermissionState::Granted {
        return Ok(());
    }
    Err(JsErr::msg(format!(
        "permission denied access to file (request for {} was {perm:?})",
        handle.name()
    )))
}
