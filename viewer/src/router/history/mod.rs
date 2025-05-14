use super::path::Path;

pub mod memory;
#[cfg(target_arch = "wasm32")]
pub mod web;

#[cfg(not(target_arch = "wasm32"))]
pub type DefaultHistory = memory::MemoryHistory;

#[cfg(target_arch = "wasm32")]
pub type DefaultHistory = web::WebHistory;

pub trait History {
    fn new(ctx: egui::Context) -> Self;
    fn active_route(&self) -> Path;
    fn push(&mut self, location: Path) -> anyhow::Result<()>;
    fn replace(&mut self, location: Path) -> anyhow::Result<()>;
    fn back(&mut self) -> anyhow::Result<()>;
    fn forward(&mut self) -> anyhow::Result<()>;
}
