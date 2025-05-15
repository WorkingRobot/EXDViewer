use std::sync::LazyLock;

use egui::TextStyle;
use egui::text::LayoutJob;
use serde::{Deserialize, Serialize};
use syntect::{highlighting::ThemeSet, parsing::SyntaxSet};

use crate::settings::CODE_SYNTAX_THEME;

/// View some code with syntax highlighting and selection.
pub fn code_view_ui(
    ui: &mut egui::Ui,
    theme: &CodeTheme,
    code: &str,
    language: &str,
) -> egui::Response {
    let layout_job = highlight(ui.ctx(), ui.style(), theme, code, language);
    ui.add(egui::Label::new(layout_job).selectable(true))
}

/// Add syntax highlighting to a code string.
///
/// The results are memoized, so you can call this every frame without performance penalty.
pub fn highlight(
    ctx: &egui::Context,
    style: &egui::Style,
    theme: &CodeTheme,
    code: &str,
    language: &str,
) -> LayoutJob {
    // We take in both context and style so that in situations where ui is not available such as when
    // performing it at a separate thread (ctx, ctx.style()) can be used and when ui is available
    // (ui.ctx(), ui.style()) can be used

    #[allow(non_local_definitions)]
    impl egui::cache::ComputerMut<(&egui::FontId, &CodeTheme, &str, &str), LayoutJob> for Highlighter {
        fn compute(
            &mut self,
            (font_id, theme, code, lang): (&egui::FontId, &CodeTheme, &str, &str),
        ) -> LayoutJob {
            self.highlight(font_id.clone(), theme, code, lang)
        }
    }

    type HighlightCache = egui::cache::FrameCache<LayoutJob, Highlighter>;

    let font_id = style
        .override_font_id
        .clone()
        .unwrap_or_else(|| TextStyle::Monospace.resolve(style));

    ctx.memory_mut(|mem| {
        mem.caches
            .cache::<HighlightCache>()
            .get((&font_id, theme, code, language))
    })
}

fn monospace_font_size(style: &egui::Style) -> f32 {
    TextStyle::Monospace.resolve(style).size
}

/// A selected color theme.
#[derive(Clone, Hash, PartialEq, Deserialize, Serialize)]
pub struct CodeTheme {
    pub theme: String,
    pub font_id: egui::FontId,
}

impl CodeTheme {
    /// Load code theme from egui memory.
    pub fn from_memory(ctx: &egui::Context, style: &egui::Style) -> Self {
        CODE_SYNTAX_THEME.get_or_insert(ctx, || Self {
            theme: if style.visuals.dark_mode {
                "base16-mocha.dark"
            } else {
                "Solarized (light)"
            }
            .to_owned(),
            font_id: egui::FontId::monospace(monospace_font_size(style)),
        })
    }

    /// Store theme to egui memory.
    pub fn store_in_memory(self, ctx: &egui::Context) {
        CODE_SYNTAX_THEME.set(ctx, self);
    }
}

impl CodeTheme {
    /// A Vec of (id, name) of all available themes
    pub fn themes() -> Vec<(&'static str, &'static str)> {
        THEME_SET
            .themes
            .iter()
            .map(|(k, v)| (k.as_str(), v.name.as_deref().unwrap_or(k.as_str())))
            .collect()
    }
}

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

#[derive(Default)]
struct Highlighter {}

impl Highlighter {
    #[allow(clippy::unused_self, clippy::unnecessary_wraps)]
    fn highlight(
        &self,
        font_id: egui::FontId,
        theme: &CodeTheme,
        code: &str,
        lang: &str,
    ) -> LayoutJob {
        self.highlight_impl(theme, code, lang).unwrap_or_else(|| {
            // Fallback:
            LayoutJob::simple(
                code.into(),
                font_id,
                egui::Color32::LIGHT_GRAY,
                f32::INFINITY,
            )
        })
    }

    fn highlight_impl(&self, theme: &CodeTheme, text: &str, language: &str) -> Option<LayoutJob> {
        use syntect::easy::HighlightLines;
        use syntect::highlighting::FontStyle;
        use syntect::util::LinesWithEndings;

        let syntax = SYNTAX_SET
            .find_syntax_by_name(language)
            .or_else(|| SYNTAX_SET.find_syntax_by_extension(language))?;

        let mut h = HighlightLines::new(syntax, THEME_SET.themes.get(&theme.theme)?);

        use egui::text::{LayoutSection, TextFormat};

        let mut job = LayoutJob {
            text: text.into(),
            ..Default::default()
        };

        for line in LinesWithEndings::from(text) {
            for (style, range) in h.highlight_line(line, &SYNTAX_SET).ok()? {
                let fg = style.foreground;
                let text_color = egui::Color32::from_rgb(fg.r, fg.g, fg.b);
                let italics = style.font_style.contains(FontStyle::ITALIC);
                let underline = style.font_style.contains(FontStyle::ITALIC);
                let underline = if underline {
                    egui::Stroke::new(1.0, text_color)
                } else {
                    egui::Stroke::NONE
                };
                job.sections.push(LayoutSection {
                    leading_space: 0.0,
                    byte_range: as_byte_range(text, range),
                    format: TextFormat {
                        font_id: theme.font_id.clone(),
                        color: text_color,
                        italics,
                        underline,
                        ..Default::default()
                    },
                });
            }
        }

        Some(job)
    }
}

fn as_byte_range(whole: &str, range: &str) -> std::ops::Range<usize> {
    let whole_start = whole.as_ptr() as usize;
    let range_start = range.as_ptr() as usize;
    assert!(whole_start <= range_start);
    assert!(range_start + range.len() <= whole_start + whole.len());
    let offset = range_start - whole_start;
    offset..(offset + range.len())
}
