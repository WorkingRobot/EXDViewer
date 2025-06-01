mod cell;
mod global_context;
mod schema_column;
mod sheet_column;
mod sheet_table;
mod table_context;

pub use cell::CellResponse;
use egui::{Align, Direction, Label, Layout, Response, Sense};
pub use global_context::GlobalContext;
pub use sheet_table::{FilterKey, SheetTable};
pub use table_context::TableContext;

fn copyable_label(ui: &mut egui::Ui, text: impl ToString) -> Response {
    ui.with_layout(
        Layout::centered_and_justified(Direction::LeftToRight).with_main_align(Align::Min),
        |ui| {
            let resp = ui.add(Label::new(text.to_string()).sense(Sense::click()));
            resp.context_menu(|ui| {
                if ui.button("Copy").clicked() {
                    ui.ctx().copy_text(text.to_string());
                    ui.close_menu();
                }
            });
            resp
        },
    )
    .inner
}
