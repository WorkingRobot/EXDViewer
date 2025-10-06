mod cell;
mod global_context;
mod schema_column;
mod sheet_column;
mod sheet_table;
mod table_context;

use std::fmt::Write;

use base64::{Engine, prelude::BASE64_STANDARD};
pub use cell::CellResponse;
use egui::{Align, Color32, Direction, Label, Layout, Response, Sense};
pub use global_context::GlobalContext;
use ironworks::sestring::SeString;
pub use sheet_table::{FilterKey, SheetTable};
pub use table_context::TableContext;

use crate::settings::{EVALUATE_STRINGS, TEXT_MAX_LINES, TEXT_WRAP_WIDTH};

fn copyable_label(ui: &mut egui::Ui, text: &impl ToString) -> Response {
    ui.with_layout(
        Layout::centered_and_justified(Direction::LeftToRight).with_main_align(Align::Min),
        |ui| {
            let text = text.to_string();
            let resp = ui.add(Label::new(&text).sense(Sense::click()));
            resp.context_menu(|ui| {
                if ui.button("Copy").clicked() {
                    ui.ctx().copy_text(text);
                    ui.close();
                }
            });
            resp
        },
    )
    .inner
}

fn string_label_wrapped(ui: &mut egui::Ui, value: &SeString<'static>) -> Response {
    let text = if EVALUATE_STRINGS.get(ui.ctx()) {
        value.format()
    } else {
        value.macro_string()
    };

    let text = match text {
        Ok(v) => v,
        Err(e) => {
            log::error!("Failed to format string: {e:?}");
            let resp = ui
                .with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.colored_label(Color32::LIGHT_RED, "⚠")
                        .on_hover_text(e.to_string())
                })
                .inner;
            return resp;
        }
    };

    let line_count = wrap_string_lines(ui, &text);
    let resp = ui
        .with_layout(Layout::left_to_right(Align::Center), |ui| {
            let draw = |ui: &mut egui::Ui, text| {
                let mut label = egui::Label::new(text);
                if let Some(max_width) = TEXT_WRAP_WIDTH.get(ui.ctx()) {
                    ui.set_max_width(max_width.get().into());
                    label = label.wrap();
                }
                ui.add(label)
            };

            let max_lines = TEXT_MAX_LINES.get(ui.ctx());
            if let Some(max_lines) = max_lines
                && line_count > max_lines.get().into()
            {
                let max_height =
                    ui.text_style_height(&egui::TextStyle::Body) * f32::from(max_lines.get());
                ui.style_mut().spacing.item_spacing.y = 0.0;
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .max_height(max_height)
                    .min_scrolled_height(max_height)
                    .show(ui, |ui| draw(ui, &text))
                    .inner
            } else {
                draw(ui, &text)
            }
        })
        .inner;

    resp.context_menu(|ui| {
        if ui.button("Copy").clicked() {
            ui.ctx().copy_text(text);
            ui.close();
        }
        if ui.button("Copy Raw (base64)").clicked() {
            ui.ctx().copy_text(BASE64_STANDARD.encode(value.as_bytes()));
            ui.close();
        }
        if ui.button("Copy Raw (hex)").clicked() {
            ui.ctx().copy_text(
                value
                    .as_bytes()
                    .iter()
                    .fold(String::new(), |mut output, b| {
                        let _ = write!(output, "{b:02X}");
                        output
                    }),
            );
            ui.close();
        }
    });

    resp
}

/// Wraps the string to fit within a maximum width in pixels, returning line count.
fn wrap_string_lines(ui: &egui::Ui, text: &str) -> usize {
    let max_width = TEXT_WRAP_WIDTH.get(ui.ctx());
    let Some(max_width) = max_width else {
        return text.lines().count();
    };

    let font_id = egui::TextStyle::Body.resolve(ui.style());
    ui.fonts(|fonts| {
        let galley = fonts.layout(
            text.to_owned(),
            font_id.clone(),
            egui::Color32::PLACEHOLDER,
            max_width.get().into(),
        );
        galley.rows.len()
    })
}
