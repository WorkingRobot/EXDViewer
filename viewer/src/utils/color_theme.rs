use egui::ThemePreference;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ColorTheme {
    Mocha,
    Macchiato,
    Frappe,
    Latte,

    OriginalLight,
    OriginalDark,

    System,
}

impl ColorTheme {
    pub fn themes() -> &'static [ColorTheme] {
        &[
            ColorTheme::System,
            ColorTheme::Mocha,
            ColorTheme::Macchiato,
            ColorTheme::Frappe,
            ColorTheme::Latte,
            ColorTheme::OriginalDark,
            ColorTheme::OriginalLight,
        ]
    }

    pub fn name(&self) -> &'static str {
        match self {
            ColorTheme::System => "💻 System",
            ColorTheme::Mocha => "🌿 Mocha",
            ColorTheme::Macchiato => "🌺 Macchiato",
            ColorTheme::Frappe => "🌱 Frappé",
            ColorTheme::Latte => "🌻 Latte",
            ColorTheme::OriginalDark => "🌙 Dark (classic)",
            ColorTheme::OriginalLight => "☀ Light (classic)",
        }
    }

    pub fn is_light(&self) -> bool {
        matches!(self, ColorTheme::OriginalLight | ColorTheme::Latte)
    }

    pub fn is_dark(&self) -> bool {
        matches!(
            self,
            ColorTheme::OriginalDark
                | ColorTheme::Mocha
                | ColorTheme::Macchiato
                | ColorTheme::Frappe
        )
    }

    fn theme_preference(&self) -> ThemePreference {
        if self.is_light() {
            ThemePreference::Light
        } else if self.is_dark() {
            ThemePreference::Dark
        } else {
            ThemePreference::System
        }
    }

    fn catppuccin_theme(&self) -> Option<catppuccin_egui::Theme> {
        Some(match self {
            ColorTheme::Mocha => catppuccin_egui::MOCHA,
            ColorTheme::Macchiato => catppuccin_egui::MACCHIATO,
            ColorTheme::Frappe => catppuccin_egui::FRAPPE,
            ColorTheme::Latte => catppuccin_egui::LATTE,
            _ => return None,
        })
    }

    pub fn apply(self, ctx: &egui::Context) {
        ctx.set_theme(self.theme_preference());
        if self == ColorTheme::System {
            Self::from(ctx.theme()).apply(ctx);
            return;
        }

        ctx.set_visuals(if self.is_dark() {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        if let Some(theme) = self.catppuccin_theme() {
            catppuccin_egui::set_theme(ctx, theme);
        }
    }
}

impl From<egui::Theme> for ColorTheme {
    fn from(theme: egui::Theme) -> Self {
        match theme {
            egui::Theme::Light => ColorTheme::Latte,
            egui::Theme::Dark => ColorTheme::Mocha,
        }
    }
}

impl From<ThemePreference> for ColorTheme {
    fn from(pref: ThemePreference) -> Self {
        match pref {
            ThemePreference::Light => ColorTheme::Latte,
            ThemePreference::Dark => ColorTheme::Mocha,
            ThemePreference::System => ColorTheme::System,
        }
    }
}
