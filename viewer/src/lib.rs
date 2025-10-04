#![allow(dead_code)]
#![allow(
    clippy::used_underscore_binding,
    clippy::cast_possible_truncation,
    clippy::cast_lossless,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    clippy::if_not_else,
    clippy::match_same_arms,
    clippy::unused_self,
    clippy::needless_pass_by_value,
    clippy::trivially_copy_pass_by_ref,
    clippy::redundant_closure_for_method_calls,
    clippy::default_trait_access
)]
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
mod shortcuts;
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
