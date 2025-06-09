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

pub struct WorkerFileProvider(());

impl WorkerFileProvider {
    pub async fn new(handle: WorkerDirectory) -> anyhow::Result<Self> {
        match worker::transact(WorkerRequest::DataSetup(handle)).await {
            WorkerResponse::DataSetup(Ok(())) => Ok(Self(())),
            WorkerResponse::DataSetup(Err(e)) => Err(anyhow::anyhow!(
                "WorkerFileProvider: failed to setup folder: {}",
                e
            )),
            _ => Err(anyhow::anyhow!("WorkerFileProvider: invalid response")),
        }
    }

    pub async fn folders() -> anyhow::Result<Vec<WorkerDirectory>> {
        match worker::transact(WorkerRequest::DataGet()).await {
            WorkerResponse::DataGet(Ok(folders)) => Ok(folders),
            WorkerResponse::DataGet(Err(e)) => Err(anyhow::anyhow!(
                "WorkerFileProvider: failed to get folders: {}",
                e
            )),
            _ => Err(anyhow::anyhow!("WorkerFileProvider: invalid response")),
        }
    }

    pub async fn add_folder(handle: WorkerDirectory) -> anyhow::Result<()> {
        match worker::transact(WorkerRequest::DataStore(handle)).await {
            WorkerResponse::DataStore(Ok(())) => Ok(()),
            WorkerResponse::DataStore(Err(e)) => Err(anyhow::anyhow!(
                "WorkerFileProvider: failed to add folder: {}",
                e
            )),
            _ => Err(anyhow::anyhow!("WorkerFileProvider: invalid response")),
        }
    }

    pub async fn verify_folder(handle: WorkerDirectory) -> anyhow::Result<()> {
        match worker::transact(WorkerRequest::VerifyFolder((handle, false))).await {
            WorkerResponse::VerifyFolder(Ok(())) => Ok(()),
            WorkerResponse::VerifyFolder(Err(e)) => Err(anyhow::anyhow!(
                "WorkerFileProvider: failed to verify folder: {}",
                e
            )),
            _ => Err(anyhow::anyhow!("WorkerFileProvider: invalid response")),
        }
    }
}

#[async_trait(?Send)]
impl FileProvider for WorkerFileProvider {
    async fn file<T: File>(&self, path: &str) -> anyhow::Result<T> {
        log::info!("WorkerFileProvider: requesting file {:?}", path);
        if let WorkerResponse::DataRequestFile(result) =
            worker::transact(WorkerRequest::DataRequestFile(path.to_string())).await
        {
            let file =
                result.map_err(|e| ironworks::Error::NotFound(ironworks::ErrorValue::Other(e)))?;
            Ok(T::read(Cursor::new(file))?)
        } else {
            Err(anyhow::anyhow!(
                "WorkerFileProvider: invalid response from worker"
            ))
        }
    }

    async fn get_icon(&self, icon_id: u32, hires: bool) -> anyhow::Result<Either<Url, RgbaImage>> {
        log::info!("WorkerFileProvider: requesting icon {}, {}", icon_id, hires);
        let path = get_icon_path(icon_id, hires);
        if let WorkerResponse::DataRequestTexture(result) =
            worker::transact(WorkerRequest::DataRequestTexture(path.to_string())).await
        {
            let file = result
                .map_err(|e| anyhow::anyhow!("WorkerFileProvider: failed to get texture: {}", e))
                .and_then(|(width, height, data)| {
                    RgbaImage::from_vec(width, height, data).ok_or_else(|| {
                        anyhow::anyhow!("WorkerFileProvider: failed to create image from data")
                    })
                })?;
            Ok(Either::Right(file))
        } else {
            Err(anyhow::anyhow!(
                "WorkerFileProvider: invalid response from worker"
            ))
        }
    }
}
