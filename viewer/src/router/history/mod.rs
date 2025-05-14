use super::path::Path;

pub mod memory;
#[cfg(target_arch = "wasm32")]
pub mod web;

#[cfg(not(target_arch = "wasm32"))]
pub type DefaultHistory = memory::MemoryHistory;

#[cfg(target_arch = "wasm32")]
pub type DefaultHistory = web::WebHistory;

#[derive(Debug, Clone)]
pub struct HistoryEvent {
    pub location: Path,
    pub state: Option<u32>,
}

pub trait History {
    fn new(ctx: egui::Context) -> Self;
    fn tick(&mut self) -> Vec<HistoryEvent>;
    fn active_route(&self) -> (Path, Option<u32>);
    fn push(&mut self, location: Path, state: u32) -> anyhow::Result<()>;
    fn replace(&mut self, location: Path, state: u32) -> anyhow::Result<()>;
    fn back(&mut self) -> anyhow::Result<()>;
    fn forward(&mut self) -> anyhow::Result<()>;
}
