use async_trait::async_trait;
use web_sys::FileSystemDirectoryHandle;

use crate::{
    backend::worker,
    worker::{WorkerDirectory, WorkerRequest, WorkerResponse},
};

use super::provider::SchemaProvider;

pub struct WorkerProvider(());

impl WorkerProvider {
    pub async fn new(name: String) -> anyhow::Result<Self> {
        match worker::transact(WorkerRequest::GetStoredSchemaFolder(name)).await {
            WorkerResponse::GetStoredSchemaFolder(Ok(Some(f))) => {
                match worker::transact(WorkerRequest::SetupSchemaFolder(f)).await {
                    WorkerResponse::SetupSchemaFolder(Ok(())) => Ok(Self(())),
                    WorkerResponse::SetupSchemaFolder(Err(e)) => Err(anyhow::anyhow!(
                        "WorkerProvider: failed to setup schema folder: {}",
                        e
                    )),
                    _ => Err(anyhow::anyhow!("WorkerProvider: invalid schema response")),
                }
            }
            WorkerResponse::GetStoredSchemaFolder(Ok(None)) => {
                Err(anyhow::anyhow!("WorkerProvider: schema folder not found"))
            }
            WorkerResponse::GetStoredSchemaFolder(Err(e)) => Err(anyhow::anyhow!(
                "WorkerProvider: failed to setup schema folder: {}",
                e
            )),
            _ => Err(anyhow::anyhow!("WorkerProvider: invalid schema response")),
        }
    }

    pub async fn folders() -> anyhow::Result<Vec<String>> {
        match worker::transact(WorkerRequest::GetStoredSchemas()).await {
            WorkerResponse::GetStoredSchemas(Ok(folders)) => Ok(folders),
            WorkerResponse::GetStoredSchemas(Err(e)) => Err(anyhow::anyhow!(
                "WorkerProvider: failed to get schema folders: {}",
                e
            )),
            _ => Err(anyhow::anyhow!("WorkerProvider: invalid schema response")),
        }
    }

    pub async fn add_folder(handle: FileSystemDirectoryHandle) -> anyhow::Result<String> {
        match worker::transact(WorkerRequest::StoreSchemaFolder(WorkerDirectory(
            handle.clone(),
        )))
        .await
        {
            WorkerResponse::StoreSchemaFolder(Ok(())) => Ok(handle.name()),
            WorkerResponse::StoreSchemaFolder(Err(e)) => Err(anyhow::anyhow!(
                "WorkerProvider: failed to add schema folder: {}",
                e
            )),
            _ => Err(anyhow::anyhow!("WorkerProvider: invalid schema response")),
        }
    }
}

#[async_trait(?Send)]
impl SchemaProvider for WorkerProvider {
    async fn get_schema_text(&self, name: &str) -> anyhow::Result<String> {
        if let WorkerResponse::GetSchema(result) =
            worker::transact(WorkerRequest::GetSchema(format!("{name}.yml"))).await
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
        if let WorkerResponse::StoreSchema(result) = worker::transact(WorkerRequest::StoreSchema((
            format!("{name}.yml"),
            text.to_string(),
        )))
        .await
        {
            result.map_err(|e| anyhow::anyhow!("WorkerProvider: failed to save schema: {}", e))
        } else {
            return Err(anyhow::anyhow!("WorkerProvider: invalid schema response"));
        }
    }
}
