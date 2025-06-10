use std::{
    io::{BufReader, Read},
    path::Path,
};

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
        let file_handle: File = self.0.get_file_handle(path)?;
        let file = SyncAccessFile::new(file_handle)
            .map_err(|jserr| std::io::Error::new(std::io::ErrorKind::Other, jserr))?;
        Ok(BufReader::with_capacity(0x800000, file))
    }
}
