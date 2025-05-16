use crate::router::path::Path;
use anyhow::bail;

use super::History;

pub struct MemoryHistory {
    ctx: egui::Context,
    history: Vec<Path>,
    position: usize,
}

impl History for MemoryHistory {
    fn new(ctx: egui::Context) -> Self {
        Self {
            ctx,
            history: vec!["/".into()],
            position: 0,
        }
    }

    fn set_title(&mut self, title: String) {
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::Title(title));
    }

    fn base_url(&self) -> String {
        "".to_string()
    }

    fn active_route(&self) -> Path {
        self.history.get(self.position).unwrap().clone()
    }

    fn push(&mut self, location: Path) -> anyhow::Result<()> {
        self.history.drain(self.position + 1..);
        self.history.push(location);
        self.position += 1;
        Ok(())
    }

    fn replace(&mut self, location: Path) -> anyhow::Result<()> {
        *self.history.get_mut(self.position).unwrap() = location;
        Ok(())
    }

    fn back(&mut self) -> anyhow::Result<()> {
        if self.position == 0 {
            bail!("Cannot go before first entry");
        }
        self.position -= 1;
        Ok(())
    }

    fn forward(&mut self) -> anyhow::Result<()> {
        if self.position == self.history.len() - 1 {
            bail!("Cannot go past last entry");
        }
        self.position += 1;
        Ok(())
    }
}
