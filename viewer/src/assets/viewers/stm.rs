//! `.stm` staining templates: the value every dyeable field of a color table row takes for each of
//! the game's stains.

use anyhow::Result;
use egui::{RichText, ScrollArea, Vec2, vec2};
use ironworks::file::{
    File,
    stm::{self, DyePack},
};
use std::io::Cursor;

use super::{Preview, facts, headers, section, swatch};
use crate::assets::deps::Deps;
use crate::backend::Backend;
use crate::sheet::draw_color;

/// Stains are numbered by their row in this sheet: slot 1 is Snow White, 4 Slate Gray, 7 Rose Pink.
/// The file carries more slots than the sheet has dyes, and the tail of them is unnamed.
const STAIN_SHEET: &str = "Stain";

/// The file that predates the field counts being stated, and the only one carrying the shorter row.
const LEGACY: u16 = 0x0101;

/// Space a color swatch is drawn in.
const SWATCH: Vec2 = vec2(48.0, 16.0);

/// A scalar column: its heading, and how a stain's value for it reads.
type Scalar = (&'static str, fn(&DyePack) -> String);

/// The scalar fields the current file states, in the order it holds them.
const SCALARS: [Scalar; 9] = [
    ("Scalar3", |dye| format!("{:.2}", dye.scalar3)),
    ("Metal", |dye| format!("{:.2}", dye.metalness)),
    ("Rough", |dye| format!("{:.2}", dye.roughness)),
    ("Sheen", |dye| format!("{:.2}", dye.sheen_rate)),
    ("Sheen tint", |dye| format!("{:.2}", dye.sheen_tint)),
    ("Sheen ap.", |dye| format!("{:.2}", dye.sheen_aperture)),
    ("Aniso", |dye| format!("{:.2}", dye.anisotropy)),
    ("Sphere", |dye| dye.sphere_index.to_string()),
    ("Sphere mask", |dye| format!("{:.2}", dye.sphere_mask)),
];

/// The pre-Dawntrail pair, which Penumbra.GameData names Shininess and SpecularMask and which
/// arrives in the first two fields of the row above.
const LEGACY_SCALARS: [Scalar; 2] = [("Shininess", SCALARS[0].1), ("Specular mask", SCALARS[1].1)];

/// A staining template file, decoded and ready to draw.
pub struct Rendered {
    identity: Vec<(&'static str, String)>,
    templates: stm::StainingTemplates,
    scalars: &'static [Scalar],
    /// Which template is on show, kept per file the way the font viewer keeps its blocks.
    picked: egui::Id,
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    let templates = stm::StainingTemplates::read(Cursor::new(bytes.to_vec()))?;

    let keys = templates
        .templates()
        .first()
        .zip(templates.templates().last());
    let identity = vec![
        ("Version", format!("{:#06x}", templates.version())),
        ("Templates", templates.templates().len().to_string()),
        (
            "Keys",
            keys.map_or_else(
                || "none".to_owned(),
                |(first, last)| format!("{} to {}", first.key(), last.key()),
            ),
        ),
        ("Stains", stm::Template::STAINS.to_string()),
    ];

    log::info!(
        "assets/stm: {path} version {:#06x}, {} templates",
        templates.version(),
        templates.templates().len()
    );

    Ok(Preview::Stm(Box::new(Rendered {
        identity,
        scalars: match templates.version() {
            LEGACY => &LEGACY_SCALARS,
            _ => &SCALARS,
        },
        templates,
        picked: egui::Id::new(("stm template", path)),
    })))
}

pub fn ui(ui: &mut egui::Ui, file: &Rendered, deps: &mut Deps, backend: &Backend) {
    let templates = file.templates.templates();
    let Some(last) = templates.len().checked_sub(1) else {
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new("This file carries no templates").weak());
        });
        return;
    };

    section(ui, "Templates");
    let mut picked = ui
        .data(|data| data.get_temp::<usize>(file.picked))
        .unwrap_or(0)
        .min(last);
    ui.horizontal_wrapped(|ui| {
        for (index, template) in templates.iter().enumerate() {
            if ui
                .selectable_label(index == picked, template.key().to_string())
                .clicked()
            {
                picked = index;
            }
        }
    });
    ui.data_mut(|data| data.insert_temp(file.picked, picked));

    ui.add_space(8.0);
    ui.separator();
    section(ui, "Stains");
    ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        let mut names = vec!["#", "Dye", "Diffuse", "Specular", "Emissive"];
        names.extend(file.scalars.iter().map(|(name, _)| *name));
        egui::Grid::new("stm_stains")
            .num_columns(names.len())
            .striped(true)
            .show(ui, |ui| {
                headers(ui, &names);
                for stain in 1..=stm::Template::STAINS as u8 {
                    let Some(dye) = templates[picked].dye(stain) else {
                        continue;
                    };
                    ui.label(RichText::new(stain.to_string()).monospace());
                    match deps.text(ui.ctx(), backend, STAIN_SHEET, u32::from(stain)) {
                        Some(name) => ui.label(name),
                        None => ui.label(RichText::new("—").weak()),
                    };
                    for color in [dye.diffuse, dye.specular, dye.emissive] {
                        ui.scope(|ui| {
                            ui.set_max_size(SWATCH);
                            draw_color(ui, swatch(color));
                        });
                    }
                    for (_, read) in file.scalars {
                        ui.label(RichText::new(read(&dye)).monospace());
                    }
                    ui.allocate_space(vec2(ui.available_width(), 0.0));
                    ui.end_row();
                }
            });
    });
}

impl Rendered {
    pub fn details_ui(&self, ui: &mut egui::Ui) {
        ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| facts(ui, "stm_identity", &self.identity));
    }
}
