mod cell;
mod global_context;
mod schema_column;
mod sheet_column;
mod sheet_table;
mod table_context;

use base64::{Engine, prelude::BASE64_STANDARD};
pub use cell::CellResponse;
use egui::{Align, Color32, Direction, Label, Layout, Response, Sense};
pub use global_context::GlobalContext;
use ironworks::sestring::SeString;
pub use sheet_table::{FilterKey, SheetTable};
pub use table_context::TableContext;

use crate::settings::EVALUATE_STRINGS;

fn copyable_label(ui: &mut egui::Ui, text: impl ToString) -> Response {
    ui.with_layout(
        Layout::centered_and_justified(Direction::LeftToRight).with_main_align(Align::Min),
        |ui| {
            let resp = ui.add(Label::new(text.to_string()).sense(Sense::click()));
            resp.context_menu(|ui| {
                if ui.button("Copy").clicked() {
                    ui.ctx().copy_text(text.to_string());
                    ui.close();
                }
            });
            resp
        },
    )
    .inner
}

fn string_label(ui: &mut egui::Ui, text: SeString<'static>) -> Response {
    let value = if EVALUATE_STRINGS.get(ui.ctx()) {
        text.format()
    } else {
        text.macro_string()
    };
    let value = match value {
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

    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
        let resp = ui.add(Label::new(&value).sense(Sense::click()));
        resp.context_menu(|ui| {
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(value);
                ui.close();
            }
            if ui.button("Copy Raw (base64)").clicked() {
                ui.ctx().copy_text(BASE64_STANDARD.encode(text.as_bytes()));
                ui.close();
            }
            if ui.button("Copy Raw (hex)").clicked() {
                ui.ctx().copy_text(
                    text.as_bytes()
                        .iter()
                        .map(|b| format!("{b:02X}"))
                        .collect::<Vec<_>>()
                        .join(""),
                );
                ui.close();
            }
        });

        resp
    })
    .inner
}
