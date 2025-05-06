use std::{io::Read, path::Path};

use ironworks::sqpack::Vfs;
use web_sys::{File, FileSystemDirectoryHandle, FileSystemPermissionMode};

use super::{
    directory::{Directory, get_file_blob},
    file::SyncAccessFile,
};

pub struct DirectoryVfs(Directory<File>);

impl DirectoryVfs {
    pub async fn new(handle: FileSystemDirectoryHandle) -> std::io::Result<Self> {
        Ok(Self(
            Directory::new(
                handle,
                FileSystemPermissionMode::Read,
                Box::new(|handle| Box::pin(get_file_blob(handle))),
                true,
            )
            .await?,
        ))
    }
}

impl Vfs for DirectoryVfs {
    type File = SyncAccessFile;

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
        let file = SyncAccessFile::new(file_handle)?;
        Ok(file)
    }
}
