use egui::{Button, KeyboardShortcut, Response, Ui, Widget, WidgetText};

/// A utility function to create a button with a keyboard shortcut.
pub fn button(ui: &mut Ui, text: impl Into<WidgetText>, shortcut: &KeyboardShortcut) -> Response {
    Button::new(text)
        .shortcut_text(ui.ctx().format_shortcut(shortcut))
        .ui(ui)
}

/// A utility function to consume a keyboard shortcut in the UI.
pub fn consume(ui: &mut egui::Ui, shortcut: &KeyboardShortcut) -> bool {
    ui.input_mut(|i| i.consume_shortcut(shortcut))
}
