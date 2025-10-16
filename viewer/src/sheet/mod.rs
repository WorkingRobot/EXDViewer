mod cell;
mod cell_iter;
mod filter;
mod global_context;
mod schema_column;
mod sheet_column;
mod sheet_table;
mod table_context;

use std::{fmt::Write, sync::Arc};

use base64::{Engine, prelude::BASE64_STANDARD};
pub use cell::{CellResponse, MatchOptions};
use egui::{
    Align, Color32, Direction, FontSelection, Galley, Label, Layout, Response, RichText, Sense,
    text::LayoutJob,
};
pub use filter::{ComplexFilter, FilterInput};
pub use global_context::GlobalContext;
use intmap::IntMap;
use ironworks::sestring::SeString;
pub use sheet_table::SheetTable;
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
                    ui.add(
                        Label::new(RichText::new("⚠").color(Color32::LIGHT_RED)).selectable(false),
                    )
                    .on_hover_text(e.to_string())
                })
                .inner;
            return resp;
        }
    };

    let (line_count, galley) = wrap_string_lines_galley(ui, text.clone());
    let resp = ui
        .with_layout(Layout::left_to_right(Align::Center), |ui| {
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
                    .show(ui, |ui| ui.label(galley))
                    .inner
            } else {
                ui.label(galley)
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
        .map_or(f32::INFINITY, |w| w.get().into());
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

    // let _sw = MULTILINE3_STOPWATCH.start();
    ui.fonts(|fonts| fonts.layout_job(layout))
}

fn wrap_string_lines_galley(ui: &egui::Ui, text: String) -> (usize, Arc<Galley>) {
    let galley = create_galley(ui, text, !TEXT_USE_SCROLL.get(ui.ctx()));
    (galley.rows.len(), galley)
}

static mut ESTIMATE_LUT: IntMap<u32, f32> = IntMap::new();

// SAFETY: Only accessed from the main thread
fn get_estimated_char_width(ui: &egui::Ui, ch: char) -> f32 {
    #[allow(static_mut_refs)]
    let lut = unsafe { &mut ESTIMATE_LUT };

    if let Some(width) = lut.get(ch.into()) {
        *width
    } else {
        let width = ui.fonts(|f| f.glyph_width(&FontSelection::default().resolve(ui.style()), ch));
        lut.insert(ch.into(), width);
        width
    }
}

/// Wraps the string to fit within a maximum width, returning line count.
fn wrap_string_lines_estimate(ui: &egui::Ui, text: &str) -> usize {
    // let _sw = MULTILINE4_STOPWATCH.start();

    if text.is_empty() {
        return 1;
    }

    let Some(max_width) = TEXT_WRAP_WIDTH.get(ui.ctx()).map(|f| f.get() as f32) else {
        return text.lines().count();
    };

    text.lines()
        .map(|line| {
            let mut line_count = 1;
            let mut current_width = 0.0;
            for char in line.chars() {
                let char_width = get_estimated_char_width(ui, char);
                current_width += char_width;
                if current_width > max_width {
                    line_count += 1;
                    current_width = char_width;
                }
            }
            line_count
        })
        .sum()
}

fn should_ignore_clicks(ui: &egui::Ui) -> bool {
    ui.input(|i| i.modifiers.matches_logically(egui::Modifiers::ALT))
}
