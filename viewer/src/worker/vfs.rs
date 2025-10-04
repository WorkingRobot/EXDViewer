use std::{io::BufReader, path::Path};

use ironworks::sqpack::Vfs;
use web_sys::{File, FileSystemDirectoryHandle, FileSystemPermissionMode};

use crate::utils::JsResult;

use super::{directory::Directory, file::SyncAccessFile};

pub struct DirectoryVfs(Directory);

impl DirectoryVfs {
    pub async fn new(handle: FileSystemDirectoryHandle) -> JsResult<Self> {
        Ok(Self(
            Directory::new(handle, FileSystemPermissionMode::Read, true).await?,
        ))
    }
}

impl Vfs for DirectoryVfs {
    type File = BufReader<SyncAccessFile>;

    fn exists(&self, path: impl AsRef<Path>) -> bool {
        // file
        self.0.file_exists(&path) ||
        // directory
        self.0.directory_exists(&path)
    }

    fn open(&self, path: impl AsRef<Path>) -> std::io::Result<Self::File> {
        let file_handle: File = self.0.get_file_handle(path)?;
        let file = SyncAccessFile::new(file_handle).map_err(std::io::Error::other)?;
        Ok(BufReader::with_capacity(0x80_0000, file))
    }
}
