use async_trait::async_trait;

use crate::{
    backend::worker,
    worker::{WorkerDirectory, WorkerRequest, WorkerResponse},
};

use super::provider::SchemaProvider;

pub struct WorkerProvider(());

impl WorkerProvider {
    pub async fn new(handle: WorkerDirectory) -> anyhow::Result<Self> {
        match worker::transact(WorkerRequest::SchemaSetup(handle)).await {
            WorkerResponse::SchemaSetup(Ok(())) => Ok(Self(())),
            WorkerResponse::SchemaSetup(Err(e)) => Err(anyhow::anyhow!(
                "WorkerProvider: failed to setup schema folder: {}",
                e
            )),
            _ => Err(anyhow::anyhow!("WorkerProvider: invalid schema response")),
        }
    }

    pub async fn folders() -> anyhow::Result<Vec<WorkerDirectory>> {
        match worker::transact(WorkerRequest::SchemaGet()).await {
            WorkerResponse::SchemaGet(Ok(folders)) => Ok(folders),
            WorkerResponse::SchemaGet(Err(e)) => Err(anyhow::anyhow!(
                "WorkerProvider: failed to get schema folders: {}",
                e
            )),
            _ => Err(anyhow::anyhow!("WorkerProvider: invalid schema response")),
        }
    }

    pub async fn add_folder(handle: WorkerDirectory) -> anyhow::Result<()> {
        match worker::transact(WorkerRequest::SchemaStore(handle)).await {
            WorkerResponse::SchemaStore(Ok(())) => Ok(()),
            WorkerResponse::SchemaStore(Err(e)) => Err(anyhow::anyhow!(
                "WorkerProvider: failed to add schema folder: {}",
                e
            )),
            _ => Err(anyhow::anyhow!("WorkerProvider: invalid schema response")),
        }
    }

    pub async fn verify_folder(handle: WorkerDirectory) -> anyhow::Result<()> {
        match worker::transact(WorkerRequest::VerifyFolder((handle, true))).await {
            WorkerResponse::VerifyFolder(Ok(())) => Ok(()),
            WorkerResponse::VerifyFolder(Err(e)) => Err(anyhow::anyhow!(
                "WorkerProvider: failed to verify schema folder: {}",
                e
            )),
            _ => Err(anyhow::anyhow!("WorkerProvider: invalid schema response")),
        }
    }
}

#[async_trait(?Send)]
impl SchemaProvider for WorkerProvider {
    async fn get_schema_text(&self, name: &str) -> anyhow::Result<String> {
        if let WorkerResponse::SchemaRequestGet(result) =
            worker::transact(WorkerRequest::SchemaRequestGet(format!("{name}.yml"))).await
        {
            result.map_err(|e| anyhow::anyhow!("WorkerProvider: failed to get schema: {}", e))
        } else {
            return Err(anyhow::anyhow!("WorkerProvider: invalid schema response"));
        }
    }

    fn can_save_schemas(&self) -> bool {
        true
    }

    fn save_schema_start_dir(&self) -> Option<std::path::PathBuf> {
        None
    }

    async fn save_schema(&self, name: &str, text: &str) -> anyhow::Result<()> {
        if let WorkerResponse::SchemaRequestStore(result) = worker::transact(
            WorkerRequest::SchemaRequestStore((format!("{name}.yml"), text.to_string())),
        )
        .await
        {
            result.map_err(|e| anyhow::anyhow!("WorkerProvider: failed to save schema: {}", e))
        } else {
            return Err(anyhow::anyhow!("WorkerProvider: invalid schema response"));
        }
    }
}
