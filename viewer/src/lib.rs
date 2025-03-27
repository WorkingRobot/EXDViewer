#![allow(dead_code)]
#![warn(
    clippy::all,
    rust_2018_idioms,
    rust_2021_compatibility,
    rust_2024_compatibility
)]

mod app;
mod backend;
mod data;
mod editable_schema;
mod excel;
mod future;
mod schema;
mod setup;
mod sheet_table;
mod syntax_highlighting;
mod value_cache;
mod web_stream;

pub use app::App;

pub const IS_WEB: bool = cfg!(target_arch = "wasm32");
pub const DEFAULT_API_URL: &str = "https://exd.camora.dev/api";
pub const DEFAULT_SCHEMA_URL: &str =
    "https://raw.githubusercontent.com/WorkingRobot/EXDSchema/refs/heads/latest";
