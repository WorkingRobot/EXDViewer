use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
};

use eframe::wasm_bindgen::JsCast;
use futures_util::{FutureExt, StreamExt};
use ironworks::sqpack::Vfs;
use wasm_bindgen_futures::{JsFuture, stream::JsStream};
use web_sys::{
    File, FileSystemDirectoryHandle, FileSystemFileHandle, FileSystemHandle, FileSystemHandleKind,
    FileSystemHandlePermissionDescriptor, FileSystemPermissionMode, PermissionState,
};

use super::{file::SyncAccessFile, map_jserr};

pub struct DirectoryVfs {
    handle: FileSystemDirectoryHandle,
    files: HashMap<PathBuf, File>,
}

impl DirectoryVfs {
    pub async fn new(handle: FileSystemDirectoryHandle) -> std::io::Result<Self> {
        let mut ret = Self {
            handle,
            files: HashMap::new(),
        };
        Self::fill_map(&mut ret.files, &ret.handle, PathBuf::new()).await?;
        Ok(ret)
    }

    async fn verify_permission(file: &FileSystemFileHandle) -> std::io::Result<()> {
        let perms = FileSystemHandlePermissionDescriptor::new();
        perms.set_mode(FileSystemPermissionMode::Read);
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
        map: &mut HashMap<PathBuf, File>,
        directory: &FileSystemDirectoryHandle,
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
                    Self::verify_permission(&file_handle).await?;
                    let result = JsFuture::from(file_handle.get_file()).await;
                    let sync_handle = result.map_err(map_jserr)?;
                    let sync_handle = sync_handle.dyn_into::<File>().map_err(|_| {
                        std::io::Error::new(std::io::ErrorKind::InvalidInput, "entry is not a File")
                    })?;
                    map.insert(path.join(file_handle.name()), sync_handle);
                }
                FileSystemHandleKind::Directory => {
                    let sub_dir = entry.dyn_into::<FileSystemDirectoryHandle>().map_err(|_| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "entry is not a FileSystemDirectoryHandle",
                        )
                    })?;
                    async { Self::fill_map(map, &sub_dir, path.join(sub_dir.name())).await }
                        .boxed_local()
                        .await?;
                }
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
}

impl Vfs for DirectoryVfs {
    type File = SyncAccessFile;

    fn exists(&self, path: impl AsRef<Path>) -> bool {
        // file
        self.files.contains_key(path.as_ref()) ||
        // directory
        self.files.keys()
            .any(|k| path.as_ref().components().zip(k.components()).all(|(a, b)| a == b))
    }

    fn read_to_string(&self, path: impl AsRef<Path>) -> std::io::Result<String> {
        let mut buf = String::new();
        self.open(path)?.read_to_string(&mut buf)?;
        Ok(buf)
    }

    fn read(&self, path: impl AsRef<Path>) -> std::io::Result<Vec<u8>> {
        let mut buf = Vec::new();
        self.open(path)?.read_to_end(&mut buf)?;
        Ok(buf)
    }

    fn open(&self, path: impl AsRef<Path>) -> std::io::Result<Self::File> {
        let path = path.as_ref();
        let file_handle = self
            .files
            .get(path)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"))?;
        let file = SyncAccessFile::new(file_handle.clone())?;
        Ok(file)
    }
}
