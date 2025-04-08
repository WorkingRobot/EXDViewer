use crate::utils::{js_error::JsError, tex_loader, web_worker::WorkerMessenger};

use super::{base::FileProvider, get_icon_path};
use async_trait::async_trait;
use either::Either;
use futures_util::{AsyncRead, AsyncSeek};
use image::RgbaImage;
use ironworks::{
    Ironworks,
    file::File,
    sqpack::{SqPack, VirtualFilesystem, VirtualInstall},
};
use std::io::{Read, Seek};
use url::Url;
use web_sys::FileSystemDirectoryHandle;

pub struct WebSqpackFileProvider(Ironworks<SqPack<VirtualInstall<DirectoryFilesystem>>>);

impl WebSqpackFileProvider {
    pub async fn new(
        install_location: FileSystemDirectoryHandle,
        worker: WorkerMessenger,
    ) -> Result<Self, JsError> {
        let resource =
            VirtualInstall::at_sqpack(DirectoryFilesystem::new(install_location, worker).await?);
        let resource = ironworks::sqpack::SqPack::new(resource);
        let ironworks = Ironworks::new().with_resource(resource);
        Ok(Self(ironworks))
    }
}

#[async_trait(?Send)]
impl FileProvider for WebSqpackFileProvider {
    async fn file<T: File>(&self, path: &str) -> Result<T, ironworks::Error> {
        self.0.file(path)
    }

    fn get_icon(&self, icon_id: u32) -> Result<Either<Url, RgbaImage>, anyhow::Error> {
        let path = get_icon_path(icon_id, true);
        let data = tex_loader::read(&self.0, &path)?;
        Ok(Either::Right(data.into_rgba8()))
    }
}

struct DirectoryFilesystem {
    worker: WorkerMessenger,
}

impl DirectoryFilesystem {
    pub async fn new(
        handle: FileSystemDirectoryHandle,
        worker: WorkerMessenger,
    ) -> Result<Self, JsError> {
        worker.set_directory(handle).await?;
        Ok(Self { worker })
    }
}

impl VirtualFilesystem for DirectoryFilesystem {
    type File = FileHandle;

    async fn exists(&self, path: &str) -> bool {
        match self.worker.entry_exists(path).await {
            Ok(exists) => exists,
            Err(e) => {
                log::error!("Error checking existence of path {}: {}", path, e);
                false
            }
        }
    }

    async fn read_to_string(&self, path: &str) -> std::io::Result<String> {
        self.read(path)
            .await
            .map(|data| {
                String::from_utf8(data)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
            })
            .and_then(|result| result)
    }

    async fn read(&self, path: &str) -> std::io::Result<Vec<u8>> {
        let result = self.worker.read_file_all(path).await;
        match result {
            Ok(data) => Ok(data),
            Err(e) => Err(std::io::Error::new(std::io::ErrorKind::Other, e)),
        }
    }

    async fn open(&self, path: &str) -> std::io::Result<Self::File> {
        let result = self.worker.get_file_size(path).await;
        match result {
            Ok(size) => {
                let file_handle = FileHandle::new(path.to_string(), self.worker.clone(), size);
                Ok(file_handle)
            }
            Err(e) => Err(std::io::Error::new(std::io::ErrorKind::Other, e)),
        }
    }
}

struct FileHandle {
    path: String,
    worker: WorkerMessenger,
    offset: u64,
    size: u64,

    pending: Option<BoxFuture<'static, Result<Vec<u8>, JsError>>>,
}

impl FileHandle {
    pub fn new(path: String, worker: WorkerMessenger, size: u64) -> Self {
        Self {
            path,
            worker,
            offset: 0,
            size,
            pending: None,
        }
    }
}

impl AsyncRead for FileHandle {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let len: u32 = buf.len().try_into().map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "Buffer size too large")
        })?;

        if self.pending.is_none() {
            // Capture the current offset and desired size.
            let offset = self.offset;
            let size = buf.len() as u32;
            // Create the future using the internal async function.
            // Note: We must move a clone or reference of self.internal appropriately.
            // For simplicity, assume internal is cheaply cloneable or 'static.
            let fut = self.worker.read_file_at(&self.path, self.offset, len);
            // Box the future so we can store it.
            self.pending = Some(Box::pin(fut));
        }

        // Now poll the pending future.
        let fut = self.pending.as_mut().unwrap();
        match fut.as_mut().poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                // Clear the pending future for the next call.
                self.pending = None;
                match result {
                    Ok(data) => {
                        let n = data.len();
                        // Copy the data into the provided buffer.
                        // (If fewer bytes than buf.len() were read, that is fine.)
                        buf[..n].copy_from_slice(&data);
                        // Update our internal offset.
                        self.offset += n as u64;
                        Poll::Ready(Ok(n))
                    }
                    Err(e) => Poll::Ready(Err(e)),
                }
            }
        }
    }
}

impl AsyncSeek for FileHandle {
    fn poll_seek(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        pos: std::io::SeekFrom,
    ) -> std::task::Poll<std::io::Result<u64>> {
        let this = self.get_mut();
        let offset = match pos {
            std::io::SeekFrom::Start(offset) => {
                this.offset = offset;
                this.offset
            }
            std::io::SeekFrom::Current(offset) => {
                this.offset = this.offset.checked_add_signed(offset).ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "Offset overflow")
                })?;
                this.offset
            }
            std::io::SeekFrom::End(offset) => {
                this.offset = this.size.checked_add_signed(offset).ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "Offset overflow")
                })?;
                this.offset
            }
        };
        //cx.waker().wake_by_ref();
        std::task::Poll::Ready(Ok(offset))
    }
}
