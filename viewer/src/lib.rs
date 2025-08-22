#![allow(dead_code)]
#![warn(
    clippy::all,
    rust_2018_idioms,
    rust_2021_compatibility,
    rust_2024_compatibility
)]

mod app;
mod backend;
mod editable_schema;
mod excel;
mod goto;
mod router;
mod schema;
mod settings;
mod setup;
mod sheet;
pub mod stopwatch;
mod utils;
#[cfg(target_arch = "wasm32")]
pub mod worker;

pub use app::App;

pub const IS_WEB: bool = cfg!(target_arch = "wasm32");
pub const DEFAULT_API_URL: &str = "https://exd.camora.dev/api";
pub const DEFAULT_SCHEMA_URL: &str =
    "https://raw.githubusercontent.com/xivdev/EXDSchema/refs/heads/latest";
pub const DEFAULT_GITHUB_REPO: (&str, &str) = ("xivdev", "EXDSchema");
