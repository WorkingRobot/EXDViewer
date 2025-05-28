use egui::{Button, KeyboardShortcut, Response, Ui, Widget, WidgetText};

pub fn button(ui: &mut Ui, text: impl Into<WidgetText>, shortcut: &KeyboardShortcut) -> Response {
    Button::new(text)
        .shortcut_text(ui.ctx().format_shortcut(shortcut))
        .ui(ui)
}

pub fn consume(ui: &mut egui::Ui, shortcut: &KeyboardShortcut) -> bool {
    ui.input_mut(|i| i.consume_shortcut(shortcut))
}
