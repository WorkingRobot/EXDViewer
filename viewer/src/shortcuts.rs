use egui::{Key, KeyboardShortcut, Modifiers};

pub const SCHEMA_REVERT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::R);
pub const SCHEMA_CLEAR: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::N);
pub const SCHEMA_SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::S);
pub const SCHEMA_SAVE_AS: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::CTRL.plus(Modifiers::SHIFT), Key::S);

pub const NAV_BACK: KeyboardShortcut = KeyboardShortcut::new(Modifiers::ALT, Key::ArrowLeft);
pub const NAV_FORWARD: KeyboardShortcut = KeyboardShortcut::new(Modifiers::ALT, Key::ArrowRight);

pub const GOTO_ROW: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::G);
pub const GOTO_SHEET: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::P);
