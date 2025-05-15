use crate::{
    backend::worker,
    worker::{WorkerDirectory, WorkerRequest, WorkerResponse},
};

use super::{base::FileProvider, get_icon_path};
use async_trait::async_trait;
use either::Either;
use image::RgbaImage;
use ironworks::file::File;
use std::io::Cursor;
use url::Url;
use web_sys::FileSystemDirectoryHandle;

pub struct WorkerFileProvider(());

impl WorkerFileProvider {
    pub async fn new(name: String) -> anyhow::Result<Self> {
        match worker::transact(WorkerRequest::GetStoredFolder(name)).await {
            WorkerResponse::GetStoredFolder(Ok(Some(f))) => {
                match worker::transact(WorkerRequest::SetupFolder(f)).await {
                    WorkerResponse::SetupFolder(Ok(())) => Ok(Self(())),
                    WorkerResponse::SetupFolder(Err(e)) => Err(anyhow::anyhow!(
                        "WorkerFileProvider: failed to setup folder: {}",
                        e
                    )),
                    _ => Err(anyhow::anyhow!("WorkerFileProvider: invalid response")),
                }
            }
            WorkerResponse::GetStoredFolder(Ok(None)) => {
                Err(anyhow::anyhow!("WorkerFileProvider: folder not found"))
            }
            WorkerResponse::GetStoredFolder(Err(e)) => Err(anyhow::anyhow!(
                "WorkerFileProvider: failed to setup folder: {}",
                e
            )),
            _ => Err(anyhow::anyhow!("WorkerFileProvider: invalid response")),
        }
    }

    pub async fn folders() -> anyhow::Result<Vec<String>> {
        match worker::transact(WorkerRequest::GetStoredNames()).await {
            WorkerResponse::GetStoredNames(Ok(folders)) => Ok(folders),
            WorkerResponse::GetStoredNames(Err(e)) => Err(anyhow::anyhow!(
                "WorkerFileProvider: failed to get folders: {}",
                e
            )),
            _ => Err(anyhow::anyhow!("WorkerFileProvider: invalid response")),
        }
    }

    pub async fn add_folder(handle: FileSystemDirectoryHandle) -> anyhow::Result<String> {
        match worker::transact(WorkerRequest::StoreFolder(WorkerDirectory(handle.clone()))).await {
            WorkerResponse::StoreFolder(Ok(())) => Ok(handle.name()),
            WorkerResponse::StoreFolder(Err(e)) => Err(anyhow::anyhow!(
                "WorkerFileProvider: failed to add folder: {}",
                e
            )),
            _ => Err(anyhow::anyhow!("WorkerFileProvider: invalid response")),
        }
    }
}

#[async_trait(?Send)]
impl FileProvider for WorkerFileProvider {
    async fn file<T: File>(&self, path: &str) -> Result<T, ironworks::Error> {
        if let WorkerResponse::File(result) =
            worker::transact(WorkerRequest::File(path.to_string())).await
        {
            let file =
                result.map_err(|e| ironworks::Error::NotFound(ironworks::ErrorValue::Other(e)))?;
            T::read(Cursor::new(file))
        } else {
            return Err(ironworks::Error::Invalid(
                ironworks::ErrorValue::Other("WorkerFileProvider".to_string()),
                "invalid response from worker".to_string(),
            ));
        }
    }

    async fn get_icon(
        &self,
        icon_id: u32,
        hires: bool,
    ) -> Result<Either<Url, RgbaImage>, anyhow::Error> {
        let path = get_icon_path(icon_id, hires);
        if let WorkerResponse::Texture(result) =
            worker::transact(WorkerRequest::Texture(path.to_string())).await
        {
            let file = result
                .map_err(|e| ironworks::Error::NotFound(ironworks::ErrorValue::Other(e)))
                .and_then(|(width, height, data)| {
                    RgbaImage::from_vec(width, height, data).ok_or_else(|| {
                        ironworks::Error::Invalid(
                            ironworks::ErrorValue::Other("WorkerFileProvider".to_string()),
                            "invalid image data".to_string(),
                        )
                    })
                })?;
            Ok(Either::Right(file))
        } else {
            return Err(ironworks::Error::Invalid(
                ironworks::ErrorValue::Other("WorkerFileProvider".to_string()),
                "invalid response from worker".to_string(),
            )
            .into());
        }
    }
}
