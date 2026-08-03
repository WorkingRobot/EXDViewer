//! `.cmp` character make parameters: every color character creation offers, and the range each of
//! its proportion sliders covers.
//!
//! Only `chara/xls/charaMake/human.cmp` ships. The file carries neither a magic nor a version, so
//! the blocks below sit at fixed offsets and the clans they belong to are the order the format
//! writes them in.

use std::io::Cursor;

use anyhow::Result;
use egui::{Color32, RichText, ScrollArea};
use ironworks::file::{File, cmp};

use super::{Preview, chip, facts, heading, line, section, table};

/// The clans the color blocks run through, two blocks each for a male and a female character.
const CLANS: [&str; 16] = [
    "Midlander",
    "Highlander",
    "Wildwood",
    "Duskwight",
    "Plainsfolk",
    "Dunesfolk",
    "Seeker of the Sun",
    "Keeper of the Moon",
    "Seawolf",
    "Hellsguard",
    "Raen",
    "Xaela",
    "Helion",
    "Lost",
    "Rava",
    "Veena",
];

const SCALES: [(&str, usize); 7] = [
    ("Clan", 20),
    ("Male height", 16),
    ("Male tail", 16),
    ("Female height", 16),
    ("Female tail", 16),
    ("Bust min", 26),
    ("Bust max", 26),
];

/// One run of colors the file offers under a name.
struct Palette {
    name: &'static str,
    colors: Vec<Color32>,
}

/// The range one clan's proportions can be adjusted over.
struct Scale {
    clan: &'static str,
    male_height: [f32; 2],
    male_tail: [f32; 2],
    female_height: [f32; 2],
    female_tail: [f32; 2],
    bust: [[f32; 3]; 2],
}

pub struct Rendered {
    identity: Vec<(&'static str, String)>,
    /// The blocks a picker chooses between, each holding its own palettes.
    blocks: Vec<(String, Vec<Palette>)>,
    scales: Vec<Scale>,
    /// Which block is on show, kept per file the way the staining viewer keeps its templates.
    picked: egui::Id,
}

fn color(color: cmp::Color) -> Color32 {
    Color32::from_rgb(color.red(), color.green(), color.blue())
}

fn palettes(colors: &cmp::ColorParameters) -> Vec<Palette> {
    let run = |name, colors: &[cmp::Color; 256]| Palette {
        name,
        colors: colors.iter().copied().map(color).collect(),
    };
    let halved = |name, pick: fn(&cmp::ColorParameters, usize) -> Option<cmp::Color>| Palette {
        name,
        colors: (0..256)
            .filter_map(|index| pick(colors, index))
            .map(color)
            .collect(),
    };
    vec![
        run("Eyes", colors.eyes()),
        run("Hair highlights", colors.hair_highlights()),
        halved("Lips", cmp::ColorParameters::lips),
        halved("Face paint", cmp::ColorParameters::face_paint),
        run("Features", colors.features()),
        run("Unused eyes A", colors.unused_eyes_a()),
        run("Unused eyes B", colors.unused_eyes_b()),
        run("Unused eyes C", colors.unused_eyes_c()),
        run("Unused features", colors.unused_features()),
    ]
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    let file = cmp::CharacterMakeParameters::read(Cursor::new(bytes.to_vec()))?;

    let mut blocks = vec![
        ("Colors".to_owned(), palettes(file.colors())),
        ("Interface".to_owned(), palettes(file.interface_colors())),
    ];
    for (index, clan) in file.races().iter().enumerate() {
        let (name, gender) = (CLANS[index / 2], ["male", "female"][index % 2]);
        blocks.push((
            format!("{name} {gender}"),
            vec![
                Palette {
                    name: "Skin",
                    colors: clan.skin().iter().copied().map(color).collect(),
                },
                Palette {
                    name: "Hair",
                    colors: clan.hair().iter().map(|it| color(it.main())).collect(),
                },
                Palette {
                    name: "Hair sheen",
                    colors: clan
                        .hair()
                        .iter()
                        .map(|it| color(it.unused_sheen()))
                        .collect(),
                },
                Palette {
                    name: "Skin (interface)",
                    colors: clan.skin_interface().iter().copied().map(color).collect(),
                },
                Palette {
                    name: "Hair (interface)",
                    colors: clan.hair_interface().iter().copied().map(color).collect(),
                },
            ],
        ));
    }

    // A race's group holds ten slots and fills only the two its clans use.
    let scales = file
        .scales()
        .iter()
        .flat_map(|group| &group[..2])
        .zip(CLANS)
        .map(|(scale, clan)| Scale {
            clan,
            male_height: [scale.male_min_height(), scale.male_max_height()],
            male_tail: [scale.male_min_tail(), scale.male_max_tail()],
            female_height: [scale.female_min_height(), scale.female_max_height()],
            female_tail: [scale.female_min_tail(), scale.female_max_tail()],
            bust: [scale.bust_min(), scale.bust_max()],
        })
        .collect::<Vec<_>>();

    let identity = vec![
        ("Clans", CLANS.len().to_string()),
        ("Color blocks", blocks.len().to_string()),
        (
            "Colors",
            blocks
                .iter()
                .flat_map(|(_, palettes)| palettes)
                .map(|palette| palette.colors.len())
                .sum::<usize>()
                .to_string(),
        ),
    ];

    log::info!("assets/cmp: {path} {} color blocks", blocks.len());

    Ok(Preview::Cmp(Box::new(Rendered {
        identity,
        blocks,
        scales,
        picked: egui::Id::new(("cmp block", path)),
    })))
}

pub fn ui(ui: &mut egui::Ui, file: &Rendered) {
    section(ui, "Colors");
    let mut picked = ui
        .data(|data| data.get_temp::<usize>(file.picked))
        .unwrap_or(0)
        .min(file.blocks.len().saturating_sub(1));
    ui.horizontal_wrapped(|ui| {
        for (index, (name, _)) in file.blocks.iter().enumerate() {
            if ui.selectable_label(index == picked, name).clicked() {
                picked = index;
            }
        }
    });
    ui.data_mut(|data| data.insert_temp(file.picked, picked));

    ui.add_space(8.0);
    ui.separator();
    ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        for palette in file.blocks[picked].1.iter() {
            heading(ui, palette.name);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (index, color) in palette.colors.iter().enumerate() {
                    chip(ui, *color).on_hover_text(format!("{index}"));
                }
            });
        }
    });
}

impl Rendered {
    pub fn details_ui(&self, ui: &mut egui::Ui) {
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            heading(ui, "Proportions");
            table(ui, &SCALES, self.scales.len(), |ui, index| {
                let scale = &self.scales[index];
                let range = |[low, high]: [f32; 2]| format!("{low:.2} to {high:.2}");
                let axes = |values: [f32; 3]| {
                    values
                        .iter()
                        .map(|value| format!("{value:>7.2}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                let cells = [
                    scale.clan.to_owned(),
                    range(scale.male_height),
                    range(scale.male_tail),
                    range(scale.female_height),
                    range(scale.female_tail),
                    axes(scale.bust[0]),
                    axes(scale.bust[1]),
                ];
                ui.label(
                    RichText::new(line(&SCALES, cells.iter().map(String::as_str))).monospace(),
                );
            });
            ui.add_space(8.0);
            ui.separator();
            facts(ui, "cmp_identity", &self.identity);
        });
    }
}
