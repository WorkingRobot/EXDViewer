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
    DataRequestTexture(String),

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
    DataRequestTexture(Result<(u32, u32, Vec<u8>), String>),

    SchemaGet(Result<Vec<WorkerDirectory>, String>),
    SchemaStore(Result<(), String>),

    SchemaSetup(Result<(), String>),
    SchemaRequestGet(Result<String, String>),
    SchemaRequestStore(Result<(), String>),

    VerifyFolder(Result<(), String>),
}
