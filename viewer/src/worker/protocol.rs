use serde::{Deserialize, Serialize};
use web_sys::FileSystemDirectoryHandle;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerDirectory(
    #[serde(with = "serde_wasm_bindgen::preserve")] pub FileSystemDirectoryHandle,
);

#[derive(Serialize, Deserialize)]
pub enum WorkerRequest {
    DataGet(),
    DataStore(WorkerDirectory),

    DataSetup(WorkerDirectory),
    DataRequestFile(String),
    /// `(repository, category, hash, split)`, for the files the index records without a path.
    DataRequestFileByHash((u8, u8, u64, bool)),
    /// URL of the global path list. The worker fetches it itself rather than being handed 20 MB
    /// through the message port, and the browser cache makes that a second hit, not a second
    /// download.
    DataPresence(String),
    DataRequestTexture(String),
    DataRequestExists(Vec<String>),

    SchemaGet(),
    SchemaStore(WorkerDirectory),

    SchemaSetup(WorkerDirectory),
    SchemaRequestGet(String),
    SchemaRequestStore((String, String)),

    VerifyFolder((WorkerDirectory, bool)),
}

#[derive(Serialize, Deserialize)]
pub enum WorkerResponse {
    DataGet(Result<Vec<WorkerDirectory>, String>),
    DataStore(Result<(), String>),

    DataSetup(Result<(), String>),
    DataRequestFile(Result<Vec<u8>, String>),
    DataRequestFileByHash(Result<Vec<u8>, String>),
    DataPresence(Result<Vec<u8>, String>),
    DataRequestTexture(Result<(u32, u32, Vec<u8>), String>),
    DataRequestExists(Result<Vec<bool>, String>),

    SchemaGet(Result<Vec<WorkerDirectory>, String>),
    SchemaStore(Result<(), String>),

    SchemaSetup(Result<(), String>),
    SchemaRequestGet(Result<String, String>),
    SchemaRequestStore(Result<(), String>),

    VerifyFolder(Result<(), String>),
}
