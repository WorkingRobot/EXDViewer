use eframe::wasm_bindgen::{JsCast, JsValue};
use web_sys::js_sys;

mod codec;
mod directory;
mod file;
mod sqpack_worker;
mod stopwatch;
mod vfs;

pub use codec::PreservingCodec;
pub use sqpack_worker::{SqpackWorker, WorkerDirectory, WorkerRequest, WorkerResponse};

fn map_jserr(err: JsValue) -> std::io::Error {
    let ret = err
        .dyn_into::<js_sys::Error>()
        .map(|e| e.to_string())
        .unwrap_or_else(|v| js_sys::JsString::from(v));
    std::io::Error::new(std::io::ErrorKind::Other, String::from(ret))
}
