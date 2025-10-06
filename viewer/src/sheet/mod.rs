mod cell;
mod global_context;
mod schema_column;
mod sheet_column;
mod sheet_table;
mod table_context;

use std::{fmt::Write, sync::Arc};

use base64::{Engine, prelude::BASE64_STANDARD};
pub use cell::CellResponse;
use egui::{
    Align, Color32, Direction, FontSelection, Galley, Label, Layout, Response, Sense,
    text::LayoutJob,
};
pub use global_context::GlobalContext;
use ironworks::sestring::SeString;
pub use sheet_table::{FilterKey, SheetTable};
pub use table_context::TableContext;

use crate::settings::{EVALUATE_STRINGS, TEXT_MAX_LINES, TEXT_USE_SCROLL, TEXT_WRAP_WIDTH};

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

    let line_count = wrap_string_lines(ui, text.clone());
    let resp = ui
        .with_layout(Layout::left_to_right(Align::Center), |ui| {
            let draw = |ui: &mut egui::Ui, text: &String| {
                let galley = create_galley(ui, text.clone(), !TEXT_USE_SCROLL.get(ui.ctx()));
                ui.label(galley)
            };

            if TEXT_USE_SCROLL.get(ui.ctx())
                && let Some(max_lines) = TEXT_MAX_LINES.get(ui.ctx())
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

fn create_galley(ui: &egui::Ui, text: String, try_elide: bool) -> Arc<Galley> {
    let max_width = TEXT_WRAP_WIDTH
        .get(ui.ctx())
        .map(|w| w.get().into())
        .unwrap_or(f32::INFINITY);
    let mut layout = LayoutJob::simple(
        text.clone(),
        FontSelection::default().resolve(ui.style()),
        Color32::PLACEHOLDER,
        max_width,
    );
    if try_elide && let Some(max_lines) = TEXT_MAX_LINES.get(ui.ctx()) {
        layout.wrap.max_rows = max_lines.get().into();
        if max_lines.get() == 1 {
            layout.wrap.break_anywhere = true;
        }
    }

    ui.fonts(|fonts| fonts.layout_job(layout))
}

/// Wraps the string to fit within a maximum width in pixels, returning line count.
fn wrap_string_lines(ui: &egui::Ui, text: String) -> usize {
    if TEXT_WRAP_WIDTH.get(ui.ctx()).is_none() {
        return text.lines().count();
    }

    create_galley(ui, text, false).rows.len()
}
