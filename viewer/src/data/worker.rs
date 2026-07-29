use crate::{
    backend::worker,
    worker::{WorkerDirectory, WorkerRequest, WorkerResponse},
};

use super::{DecodedTexture, FileProvider, get_icon_path, list_url, with_list_id};
use async_trait::async_trait;
use either::Either;
use image::RgbaImage;
use url::Url;

pub struct WorkerFileProvider(());

impl WorkerFileProvider {
    pub async fn new(handle: WorkerDirectory) -> anyhow::Result<Self> {
        match worker::transact(WorkerRequest::DataSetup(handle)).await {
            WorkerResponse::DataSetup(Ok(())) => Ok(Self(())),
            WorkerResponse::DataSetup(Err(e)) => Err(anyhow::anyhow!(
                "WorkerFileProvider: failed to setup folder: {e}"
            )),
            _ => Err(anyhow::anyhow!("WorkerFileProvider: invalid response")),
        }
    }

    pub async fn folders() -> anyhow::Result<Vec<WorkerDirectory>> {
        match worker::transact(WorkerRequest::DataGet()).await {
            WorkerResponse::DataGet(Ok(folders)) => Ok(folders),
            WorkerResponse::DataGet(Err(e)) => Err(anyhow::anyhow!(
                "WorkerFileProvider: failed to get folders: {e}"
            )),
            _ => Err(anyhow::anyhow!("WorkerFileProvider: invalid response")),
        }
    }

    pub async fn add_folder(handle: WorkerDirectory) -> anyhow::Result<()> {
        match worker::transact(WorkerRequest::DataStore(handle)).await {
            WorkerResponse::DataStore(Ok(())) => Ok(()),
            WorkerResponse::DataStore(Err(e)) => Err(anyhow::anyhow!(
                "WorkerFileProvider: failed to add folder: {e}"
            )),
            _ => Err(anyhow::anyhow!("WorkerFileProvider: invalid response")),
        }
    }

    pub async fn verify_folder(handle: WorkerDirectory) -> anyhow::Result<()> {
        match worker::transact(WorkerRequest::VerifyFolder((handle, false))).await {
            WorkerResponse::VerifyFolder(Ok(())) => Ok(()),
            WorkerResponse::VerifyFolder(Err(e)) => Err(anyhow::anyhow!(
                "WorkerFileProvider: failed to verify folder: {e}"
            )),
            _ => Err(anyhow::anyhow!("WorkerFileProvider: invalid response")),
        }
    }
}

#[async_trait(?Send)]
impl FileProvider for WorkerFileProvider {
    async fn read_stream(&self, path: &str) -> anyhow::Result<(Option<String>, Vec<u8>)> {
        log::info!("WorkerFileProvider: requesting file {path:?}");
        if let WorkerResponse::DataRequestFile(result) =
            worker::transact(WorkerRequest::DataRequestFile(path.to_string())).await
        {
            let (kind, bytes) =
                result.map_err(|e| ironworks::Error::NotFound(ironworks::ErrorValue::Other(e)))?;
            Ok((Some(kind), bytes))
        } else {
            Err(anyhow::anyhow!(
                "WorkerFileProvider: invalid response from worker"
            ))
        }
    }

    /// The list goes to the worker through the port rather than being fetched there as well.
    async fn path_index(&self, api_base: &str) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
        log::info!("WorkerFileProvider: building presence map");
        let paths = with_list_id(api_base, |id| {
            crate::utils::fetch_url(list_url(api_base, id))
        })
        .await?;
        if let WorkerResponse::DataPresence(result) =
            worker::transact(WorkerRequest::DataPresence(paths.clone())).await
        {
            let presence = result.map_err(|e| anyhow::anyhow!("WorkerFileProvider: {e}"))?;
            Ok((paths, presence))
        } else {
            Err(anyhow::anyhow!(
                "WorkerFileProvider: invalid response from worker"
            ))
        }
    }

    async fn read_stream_by_hash(
        &self,
        repository: u8,
        category: u8,
        hash: u64,
        split: bool,
    ) -> anyhow::Result<(Option<String>, Vec<u8>)> {
        log::info!("WorkerFileProvider: requesting file {repository}/{category}/{hash:X}");
        if let WorkerResponse::DataRequestFileByHash(result) = worker::transact(
            WorkerRequest::DataRequestFileByHash((repository, category, hash, split)),
        )
        .await
        {
            let (kind, bytes) =
                result.map_err(|e| ironworks::Error::NotFound(ironworks::ErrorValue::Other(e)))?;
            Ok((Some(kind), bytes))
        } else {
            Err(anyhow::anyhow!(
                "WorkerFileProvider: invalid response from worker"
            ))
        }
    }

    /// Read and decode in the one round trip, rather than fetching the bytes here only to send them
    /// straight back for decoding.
    async fn read_texture(
        &self,
        path: &str,
        max_dim: Option<u16>,
    ) -> anyhow::Result<DecodedTexture> {
        log::info!("WorkerFileProvider: requesting texture {path:?}");
        if let WorkerResponse::DataRequestTexture(result) = worker::transact(
            WorkerRequest::DataRequestTexture((path.to_owned(), max_dim)),
        )
        .await
        {
            DecodedTexture::from_worker(
                result.map_err(|e| {
                    anyhow::anyhow!("WorkerFileProvider: failed to get texture: {e}")
                })?,
            )
        } else {
            Err(anyhow::anyhow!(
                "WorkerFileProvider: invalid response from worker"
            ))
        }
    }

    async fn get_icon(&self, icon_id: u32, hires: bool) -> anyhow::Result<Either<Url, RgbaImage>> {
        log::info!("WorkerFileProvider: requesting icon {icon_id}, {hires}");
        let path = get_icon_path(icon_id, hires);
        Ok(Either::Right(self.read_texture(&path, None).await?.image))
    }

    async fn exists_many(&self, paths: &[String]) -> anyhow::Result<Vec<bool>> {
        log::info!("WorkerFileProvider: requesting existence of {paths:?}");
        if let WorkerResponse::DataRequestExists(result) =
            worker::transact(WorkerRequest::DataRequestExists(paths.to_vec())).await
        {
            result
                .map_err(|e| anyhow::anyhow!("WorkerFileProvider: failed to check existence: {e}"))
        } else {
            Err(anyhow::anyhow!(
                "WorkerFileProvider: invalid response from worker"
            ))
        }
    }
}
