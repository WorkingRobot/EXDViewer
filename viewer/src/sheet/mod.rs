mod cell;
mod global_context;
mod schema_column;
mod sheet_column;
mod sheet_table;
mod table_context;

pub use global_context::GlobalContext;
pub use sheet_table::SheetTable;
pub use table_context::TableContext;

fn copyable_label(ui: &mut egui::Ui, text: impl ToString) {
    ui.with_layout(
        egui::Layout::centered_and_justified(egui::Direction::LeftToRight)
            .with_main_align(egui::Align::Min),
        |ui| {
            let resp = ui.add(egui::Label::new(text.to_string()).sense(egui::Sense::click()));
            resp.context_menu(|ui| {
                if ui.button("Copy").clicked() {
                    ui.ctx().copy_text(text.to_string());
                    ui.close_menu();
                }
            });
        },
    );
}
