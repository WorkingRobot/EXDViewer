use crate::router::path::Path;
use anyhow::{anyhow, bail};
use egui::{Id, util::IdTypeMap};

use super::History;

pub struct MemoryHistory {
    ctx: egui::Context,
}

impl MemoryHistory {
    fn history(d: &mut IdTypeMap) -> &mut Vec<Path> {
        d.get_persisted_mut_or_insert_with(Id::new("memory_history"), || vec!["/".into()])
    }

    fn position(d: &mut IdTypeMap) -> &mut usize {
        d.get_persisted_mut_or_insert_with(Id::new("memory_history_position"), || 0)
    }
}

impl History for MemoryHistory {
    fn new(ctx: egui::Context) -> Self {
        Self { ctx }
    }

    fn set_title(&mut self, title: String) {
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::Title(title));
    }

    fn base_url(&self) -> String {
        String::new()
    }

    fn active_route(&self) -> Path {
        self.ctx
            .data_mut(|d| {
                let position = {
                    let history_len = Self::history(d).len();
                    let position = Self::position(d);
                    if *position >= history_len {
                        log::warn!(
                            "Position {position} is out of bounds for history length {history_len}"
                        );
                        *position = history_len - 1;
                    }
                    *position
                };
                Self::history(d).get(position).cloned()
            })
            .unwrap()
    }

    fn push(&mut self, location: Path) -> anyhow::Result<()> {
        self.ctx.data_mut(|d| {
            let position = *Self::position(d);
            let history = Self::history(d);
            history.drain(position + 1..);
            history.push(location);
            *Self::position(d) += 1;
        });
        Ok(())
    }

    fn replace(&mut self, location: Path) -> anyhow::Result<()> {
        self.ctx.data_mut(|d| {
            let position = *Self::position(d);
            *Self::history(d)
                .get_mut(position)
                .ok_or_else(|| anyhow!("Invalid history position"))? = location;
            Ok(())
        })
    }

    fn back(&mut self) -> anyhow::Result<()> {
        self.ctx.data_mut(|d| {
            let position = Self::position(d);
            if *position == 0 {
                bail!("Cannot go before first entry");
            }
            *position -= 1;
            Ok(())
        })
    }

    fn forward(&mut self) -> anyhow::Result<()> {
        self.ctx.data_mut(|d| {
            let history_len = Self::history(d).len();
            let position = Self::position(d);
            if *position >= history_len - 1 {
                bail!("Cannot go past last entry");
            }
            *position += 1;
            Ok(())
        })
    }
}
