use egui::{Modal, Sense, UiBuilder};

use crate::{backend::Backend, settings::BackendConfig};

#[derive(Default)]
pub struct GoToWindow {
    requested_focused: bool,
    string_buffer: String,
    pub link: Option<String>,
}

impl GoToWindow {
    pub fn draw(&mut self, ctx: &egui::Context) -> Option<(Backend, BackendConfig)> {
        let show_inner = |ui: &mut egui::Ui| {
            ui.heading("Go To…");
            ui.separator();

            let singleline_widget = ui.text_edit_singleline(&mut self.string_buffer);

            // not sure the best way to say "we want to focus when we open!"
            if !self.requested_focused {
                singleline_widget.request_focus();
                self.requested_focused = true;
            }

            if singleline_widget.lost_focus() {
                if let Some((row_id, subrow_id)) = Self::parse_string(&self.string_buffer) {
                    self.link = Some(format!(
                        "#R{row_id}{}",
                        if let Some(subrow_id) = subrow_id {
                            format!(".{subrow_id}")
                        } else {
                            "".to_string()
                        }
                    ));
                }
            }
            None
        };

        Modal::default_area("goto-modal".into())
            .show(ctx, |ui| {
                ui.scope_builder(UiBuilder::new().sense(Sense::CLICK | Sense::DRAG), |ui| {
                    egui::containers::Frame::window(ui.style())
                        .show(ui, show_inner)
                        .inner
                })
                .inner
            })
            .inner
    }

    fn parse_string(string_buffer: &str) -> Option<(u32, Option<u16>)> {
        if string_buffer.contains(".") {
            // subrow case
            let (row_id_text, subrow_id_text) = string_buffer.split_once(".")?;

            Some((row_id_text.parse().ok()?, subrow_id_text.parse().ok()))
        } else {
            // normal row case
            Some((string_buffer.parse().ok()?, None))
        }
    }
}

#[cfg(test)]
mod test {
    use crate::goto::GoToWindow;

    #[test]
    fn string_parsing() {
        // Empty
        assert_eq!(GoToWindow::parse_string(""), None);

        // Row
        assert_eq!(GoToWindow::parse_string("5"), Some((5, None)));

        // Invalid Row
        assert_eq!(GoToWindow::parse_string("5a"), None);

        // Subrow
        assert_eq!(GoToWindow::parse_string("5.6"), Some((5, Some(6))));

        // Invalid Subrow
        assert_eq!(GoToWindow::parse_string("5.a"), Some((5, None)));
    }
}
