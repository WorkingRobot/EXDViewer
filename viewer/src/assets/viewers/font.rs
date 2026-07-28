use anyhow::Result;
use egui::{Rect, RichText, Sense, Vec2, load::SizedTexture, pos2, vec2};
use ironworks::file::{File, fdt};
use std::io::Cursor;

use super::{PADDING, Preview, facts, grid, missing, section};
use crate::assets::deps::{Dep, Deps};
use crate::backend::Backend;
use crate::utils::file_name;

/// Height a glyph cell takes in the grid, which the whole font is scaled to fit.
const CELL: f32 = 52.0;

/// What the sample line spells, chosen to exercise ascenders, descenders and digits.
const SAMPLE: &str = "Hamburgefonstiv 0123!";

/// Unicode ranges worth telling apart in a game font. A glyph outside all of them is `Other`, and
/// the ranges are in codepoint order so a font's blocks come out sorted.
const BLOCKS: [(&str, u32, u32); 12] = [
    ("Latin", 0x0000, 0x024F),
    ("Greek", 0x0370, 0x03FF),
    ("Cyrillic", 0x0400, 0x04FF),
    ("Punctuation", 0x2000, 0x206F),
    ("Symbols", 0x2070, 0x2BFF),
    ("CJK symbols", 0x3000, 0x303F),
    ("Kana", 0x3040, 0x30FF),
    ("Enclosed CJK", 0x3200, 0x33FF),
    ("CJK", 0x3400, 0x9FFF),
    ("Hangul", 0xAC00, 0xD7AF),
    ("Private use", 0xE000, 0xF8FF),
    ("Fullwidth", 0xFF00, 0xFFEF),
];

/// One glyph, resolved to the sheet it is cut from.
struct GlyphCell {
    character: char,
    /// Index into [`BLOCKS`], or its length for a character in none of them.
    block: usize,
    /// Which font texture and channel hold it.
    file: u16,
    channel: u16,
    source: Rect,
    size: Vec2,
    /// Where the glyph sits below the top of its line.
    offset_y: f32,
    advance: f32,
}

/// A font, decoded and ready to draw.
pub struct Rendered {
    /// Where the glyph sheets live, by the index a glyph names.
    sheets: Vec<String>,
    glyphs: Vec<GlyphCell>,
    /// The sample line, as indices into `glyphs`.
    sample: Vec<usize>,
    /// The blocks this font actually carries, and how many glyphs each holds.
    blocks: Vec<(usize, usize)>,
    line_height: f32,
    identity: Vec<(&'static str, String)>,
    /// Which blocks are on show, kept per file the way the icon sheet keeps its controller.
    shown: egui::Id,
}

/// Which texture a font's glyphs are baked into. The name says which family of sheets to look in,
/// and the glyph's own index says how far into it.
fn sheet_path(font: &str, file: u16) -> String {
    let stem = file_name(font).trim_end_matches(".fdt").to_lowercase();
    let family = match () {
        _ if stem.ends_with("_lobby") => "font_lobby",
        _ if stem.starts_with("chn") => "font_chn_",
        _ if stem.starts_with("krn") => "font_krn_",
        _ if stem.starts_with("tc") => "font_tc_",
        _ => "font",
    };
    format!("common/font/{family}{}.tex", file + 1)
}

/// Which [`BLOCKS`] entry a character falls in.
fn block_of(character: char) -> usize {
    let codepoint = u32::from(character);
    BLOCKS
        .iter()
        .position(|(_, first, last)| (*first..=*last).contains(&codepoint))
        .unwrap_or(BLOCKS.len())
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    let font = fdt::FontData::read(Cursor::new(bytes.to_vec()))?;
    let (width, height) = (
        f32::from(font.texture_width().max(1)),
        f32::from(font.texture_height().max(1)),
    );

    let glyphs = font
        .glyphs()
        .iter()
        .map(|glyph| GlyphCell {
            character: glyph.character(),
            block: block_of(glyph.character()),
            file: glyph.texture_file(),
            channel: glyph.texture_channel(),
            source: Rect::from_min_max(
                pos2(f32::from(glyph.x()) / width, f32::from(glyph.y()) / height),
                pos2(
                    (f32::from(glyph.x()) + f32::from(glyph.width())) / width,
                    (f32::from(glyph.y()) + f32::from(glyph.height())) / height,
                ),
            ),
            size: vec2(f32::from(glyph.width()), f32::from(glyph.height())),
            offset_y: f32::from(glyph.offset_y()),
            advance: glyph.advance_width() as f32,
        })
        .collect::<Vec<_>>();

    let sample = SAMPLE
        .chars()
        .filter_map(|character| glyphs.iter().position(|glyph| glyph.character == character))
        .collect();

    // Glyphs come in codepoint order, so a block is a run of them; the toggles offer only what
    // this font actually carries.
    let mut blocks: Vec<(usize, usize)> = Vec::new();
    for glyph in &glyphs {
        match blocks.iter_mut().find(|(block, _)| *block == glyph.block) {
            Some((_, count)) => *count += 1,
            None => blocks.push((glyph.block, 1)),
        }
    }

    let mut sheets = font
        .glyphs()
        .iter()
        .map(|glyph| glyph.texture_file())
        .collect::<Vec<_>>();
    sheets.sort_unstable();
    sheets.dedup();
    let used = sheets.len();
    let sheets = (0..=sheets.last().copied().unwrap_or(0))
        .map(|file| sheet_path(path, file))
        .collect();

    let identity = vec![
        ("Size", format!("{}px", font.size())),
        ("Line height", font.line_height().to_string()),
        ("Ascent", font.ascent().to_string()),
        ("Descent", font.descent().to_string()),
        ("Glyphs", font.glyphs().len().to_string()),
        ("Kerning pairs", font.kerning().len().to_string()),
        (
            "Sheet",
            format!("{} x {}", font.texture_width(), font.texture_height()),
        ),
        ("Sheets used", used.to_string()),
    ];

    Ok(Preview::Font(Box::new(Rendered {
        sheets,
        glyphs,
        sample,
        blocks,
        line_height: font.line_height().max(1) as f32,
        identity,
        shown: egui::Id::new(("fdt blocks", path)),
    })))
}

/// Draw one glyph into `rect`, which is its own size scaled to the grid. What the sheet it is cut
/// from is doing comes back, since a glyph waiting on one has nothing to show for itself.
fn glyph(
    ui: &mut egui::Ui,
    font: &Rendered,
    cell: &GlyphCell,
    rect: Rect,
    deps: &mut Deps,
    backend: &Backend,
) -> Dep<()> {
    let Some(path) = font.sheets.get(usize::from(cell.file)) else {
        return Dep::Failed;
    };
    match deps.glyph_sheet(ui.ctx(), backend, path, cell.channel) {
        Dep::Ready(sheet) => {
            egui::Image::new(SizedTexture::new(sheet, rect.size()))
                .uv(cell.source)
                .tint(ui.visuals().text_color())
                .paint_at(ui, rect);
            Dep::Ready(())
        }
        Dep::Pending => Dep::Pending,
        Dep::Failed => Dep::Failed,
    }
}

/// What Unicode calls the codepoint. The tooltip draws the character too, but the app's own fonts
/// cover far less than a game font does, so for a lot of them that is a tofu box.
fn named(cell: &GlyphCell) -> RichText {
    match glyphnames::of(cell.character) {
        Some(name) => RichText::new(name),
        // Private use holds the game's own gaiji, which Unicode names no more than it draws.
        None => RichText::new(
            BLOCKS
                .get(cell.block)
                .map_or("Unnamed", |(name, _, _)| *name),
        )
        .weak(),
    }
}

pub fn ui(ui: &mut egui::Ui, font: &Rendered, deps: &mut Deps, backend: &Backend) {
    // One scale for the whole font, so a grid of glyphs keeps their relative sizes, and never above
    // native: these are 4-bit alpha sheets, and magnifying one only blurs it. A font small enough
    // to fit outright gets a cell its own size rather than a large one it sits lost in.
    let scale = ((CELL - PADDING * 2.0) / font.line_height).min(1.0);
    let cell = Vec2::splat(font.line_height * scale + PADDING * 2.0);

    if !font.sample.is_empty() {
        section(ui, "Sample");
        let width: f32 = font
            .sample
            .iter()
            .map(|&index| font.glyphs[index].advance)
            .sum();
        let (rect, _) = ui.allocate_exact_size(
            vec2(width * scale, font.line_height * scale),
            Sense::hover(),
        );
        let mut pen = rect.min.x;
        let mut waiting = None;
        for &index in &font.sample {
            let glyph_cell = &font.glyphs[index];
            let at = Rect::from_min_size(
                pos2(pen, rect.min.y + glyph_cell.offset_y * scale),
                glyph_cell.size * scale,
            );
            match glyph(ui, font, glyph_cell, at, deps, backend) {
                Dep::Ready(()) => {}
                // One mark for the line rather than one per letter: they all come from the same
                // sheet, so they arrive together.
                Dep::Pending => waiting = waiting.or(Some(false)),
                Dep::Failed => waiting = Some(true),
            }
            pen += glyph_cell.advance * scale;
        }
        match waiting {
            Some(true) => missing(ui, rect),
            Some(false) => egui::Spinner::new().paint_at(ui, rect),
            None => {}
        }
        ui.separator();
    }

    section(ui, "Glyphs");
    let mut shown = ui
        .data(|data| data.get_temp::<u32>(font.shown))
        .unwrap_or(u32::MAX);
    if font.blocks.len() > 1 {
        ui.horizontal_wrapped(|ui| {
            for (block, count) in &font.blocks {
                let name = BLOCKS.get(*block).map_or("Other", |(name, _, _)| name);
                let bit = 1 << block;
                if ui
                    .selectable_label(shown & bit != 0, format!("{name} ({count})"))
                    .clicked()
                {
                    shown ^= bit;
                }
            }
        });
        ui.separator();
    }
    ui.data_mut(|data| data.insert_temp(font.shown, shown));

    let visible = font
        .glyphs
        .iter()
        .enumerate()
        .filter(|(_, glyph)| shown & (1 << glyph.block) != 0)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();

    grid(ui, cell, visible.len(), |ui, index, at| {
        let glyph_cell = &font.glyphs[visible[index]];
        let drawn = glyph_cell.size * scale;
        let status = glyph(
            ui,
            font,
            glyph_cell,
            Rect::from_min_size(
                pos2(
                    at.center().x - drawn.x / 2.0,
                    at.min.y + PADDING + glyph_cell.offset_y * scale,
                ),
                drawn,
            ),
            deps,
            backend,
        );
        match status {
            Dep::Ready(()) => {}
            Dep::Pending => egui::Spinner::new().paint_at(ui, at.shrink(PADDING)),
            Dep::Failed => missing(ui, at),
        }
        ui.interact(at, font.shown.with(index), Sense::hover())
            .on_hover_ui(|ui| {
                // A control character has nothing to draw in its own right; the name below says
                // what it is.
                let drawn = match glyph_cell.character.is_control() {
                    true => ' ',
                    false => glyph_cell.character,
                };
                ui.label(
                    RichText::new(format!("U+{:04X} {drawn}", u32::from(glyph_cell.character)))
                        .monospace(),
                );
                ui.label(named(glyph_cell));
                ui.label(
                    RichText::new(format!(
                        "{} x {} in sheet {} channel {}",
                        glyph_cell.size.x,
                        glyph_cell.size.y,
                        glyph_cell.file + 1,
                        glyph_cell.channel
                    ))
                    .weak(),
                );
            });
    });
}

impl Rendered {
    pub fn details_ui(&self, ui: &mut egui::Ui) {
        facts(ui, "font_identity", &self.identity);
    }
}
