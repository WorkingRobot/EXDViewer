use serde::{Deserialize, Serialize};
use web_sys::FileSystemDirectoryHandle;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerDirectory(
    #[serde(with = "serde_wasm_bindgen::preserve")] pub FileSystemDirectoryHandle,
);

#[derive(Serialize, Deserialize)]
pub struct WorkerFile {
    pub kind: String,
    #[serde(with = "bytes")]
    pub bytes: Vec<u8>,
}

impl From<(String, Vec<u8>)> for WorkerFile {
    fn from((kind, bytes): (String, Vec<u8>)) -> Self {
        Self { kind, bytes }
    }
}

#[derive(Serialize, Deserialize)]
pub struct WorkerBytes(#[serde(with = "bytes")] pub Vec<u8>);

/// A decoded texture crossing the message port: RGBA pixels at whatever mip covered the requested
/// size, and the size of the texture they came from. Anything indexing into a texture is expressed
/// in that space rather than the decoded image's, so a caller cropping the result needs both.
#[derive(Serialize, Deserialize)]
pub struct WorkerTexture {
    pub width: u32,
    pub height: u32,
    pub source: [u16; 2],
    #[serde(with = "bytes")]
    pub data: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
pub enum WorkerRequest {
    DataGet(),
    DataStore(WorkerDirectory),

    DataSetup(WorkerDirectory),
    DataRequestFile(String),
    /// `(repository, category, hash, split)`, for the files the index records without a path.
    DataRequestFileByHash((u8, u8, u64, bool)),
    /// The global path list.
    DataPresence(#[serde(with = "bytes")] Vec<u8>),
    /// `(path, longest edge to decode at)`. `None` decodes at full size.
    DataRequestTexture((String, Option<u16>)),
    /// A `.tex` a provider fetched itself, to decode without reading it again.
    DecodeTexture {
        path: String,
        #[serde(with = "bytes")]
        bytes: Vec<u8>,
        max_dim: Option<u16>,
    },
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
    DataRequestFile(Result<WorkerFile, String>),
    DataRequestFileByHash(Result<WorkerFile, String>),
    DataPresence(Result<WorkerBytes, String>),
    DataRequestTexture(Result<WorkerTexture, String>),
    DecodeTexture(Result<WorkerTexture, String>),
    DataRequestExists(Result<Vec<bool>, String>),

    SchemaGet(Result<Vec<WorkerDirectory>, String>),
    SchemaStore(Result<(), String>),

    SchemaSetup(Result<(), String>),
    SchemaRequestGet(Result<String, String>),
    SchemaRequestStore(Result<(), String>),

    VerifyFolder(Result<(), String>),
}

/// Bytes as a `Uint8Array`. Serde treats a `Vec<u8>` as any other sequence, which would box every
/// byte into its own JS number on the way through the message port.
mod bytes {
    use serde::{Deserializer, Serializer, de};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(bytes)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        deserializer.deserialize_byte_buf(Bytes)
    }

    struct Bytes;

    impl<'de> de::Visitor<'de> for Bytes {
        type Value = Vec<u8>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("bytes")
        }

        fn visit_byte_buf<E>(self, bytes: Vec<u8>) -> Result<Self::Value, E> {
            Ok(bytes)
        }
    }
}
