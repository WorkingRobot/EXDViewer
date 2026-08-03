//! `.amb` ambient light: the day-tracks the extension is used for, an environment location's own
//! light and the light every sky casts, told apart by what the file says it is.

use std::io::Cursor;

use anyhow::Result;
use egui::{RichText, ScrollArea};
use ironworks::file::{File, amb};

use super::super::{Preview, facts, line, link, section, swatch, table};
use super::{axes, chip, clock};
use crate::utils::file_name;

const TRACK: [(&str, usize); 3] = [("Track", 5), ("Time", 9), ("Light", 24)];

const SKY: [(&str, usize); 2] = [("Sky", 5), ("Samples", 8)];

/// The constant term of second order harmonics, which is what they average to.
const BAND: f32 = 0.282_094_8;

/// The texture a sky is drawn from, which its ID names three digits wide.
fn sky_texture(id: u16) -> String {
    format!("bgcommon/nature/sky/texture/sky_{id:03}.tex")
}

fn average(light: amb::Harmonics) -> [f32; 3] {
    [
        light.red()[0] * BAND,
        light.green()[0] * BAND,
        light.blue()[0] * BAND,
    ]
}

pub struct Rendered {
    identity: Vec<(&'static str, String)>,
    /// The texture sitting beside an environment location under its own name.
    texture: Option<String>,
    section: &'static str,
    columns: &'static [(&'static str, usize)],
    source: amb::Ambient,
    /// Track and keyframe of every row, for an environment location.
    rows: Vec<(usize, usize)>,
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    let source = amb::Ambient::read(Cursor::new(bytes.to_vec()))?;

    let (identity, texture, section, columns, rows) = match &source {
        amb::Ambient::EnvLocation(location) => {
            let rows = (0..amb::TRACK_COUNT)
                .flat_map(|track| {
                    let count = location.track(track).map_or(0, |frames| frames.len());
                    (0..count).map(move |keyframe| (track, keyframe))
                })
                .collect::<Vec<_>>();
            let used = (0..amb::TRACK_COUNT)
                .filter(|track| {
                    location
                        .track(*track)
                        .is_some_and(|frames| !frames.is_empty())
                })
                .count();
            let identity = vec![
                ("Version", location.version().to_string()),
                ("Tracks", format!("{used} of {}", amb::TRACK_COUNT)),
                ("Keyframes", rows.len().to_string()),
                (
                    "Sky visibility",
                    location
                        .sky_visibility()
                        .iter()
                        .map(|value| format!("{value:.3}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            ];
            let texture = format!(
                "{}.tex",
                path.rsplit_once('.').map_or(path, |(stem, _)| stem)
            );
            (identity, Some(texture), "Tracks", &TRACK[..], rows)
        }
        amb::Ambient::SkyLight(light) => {
            let samples = light
                .skies()
                .iter()
                .map(|sky| usize::from(sky.count()))
                .sum::<usize>();
            let identity = vec![
                ("Version", light.version().to_string()),
                ("Unknown", light.unknown().to_string()),
                ("Skies", light.skies().len().to_string()),
                ("Samples", samples.to_string()),
            ];
            (identity, None, "Skies", &SKY[..], Vec::new())
        }
    };

    log::info!(
        "assets/zone: {path} {}",
        match &source {
            amb::Ambient::EnvLocation(_) => format!("{} keyframes", rows.len()),
            amb::Ambient::SkyLight(light) => format!("{} skies", light.skies().len()),
        }
    );

    Ok(Preview::Ambient(Box::new(Rendered {
        identity,
        texture,
        section,
        columns,
        source,
        rows,
    })))
}

pub fn ui(ui: &mut egui::Ui, file: &Rendered) -> Option<String> {
    let mut follow = None;
    if let Some(texture) = &file.texture {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Texture").weak());
            if link(ui, file_name(texture), texture) {
                follow = Some(texture.clone());
            }
        });
        ui.add_space(4.0);
    }

    section(ui, file.section);
    match &file.source {
        amb::Ambient::EnvLocation(location) => {
            table(ui, file.columns, file.rows.len(), |ui, index| {
                let (track, at) = file.rows[index];
                let keyframe = location.track(track).expect("a track the rows name")[at];
                let light = average(keyframe.light());
                let cells = [track.to_string(), clock(keyframe.time()), axes(light)];
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(line(file.columns, cells.iter().map(String::as_str)))
                            .monospace(),
                    );
                    chip(ui, swatch(light));
                });
            });
        }
        amb::Ambient::SkyLight(light) => {
            let skies = light.skies();
            table(ui, file.columns, skies.len(), |ui, index| {
                let sky = skies[index];
                let path = sky_texture(sky.id());
                let cells = [sky.id().to_string(), sky.count().to_string()];
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label(
                        RichText::new(line(file.columns, cells.iter().map(String::as_str)))
                            .monospace(),
                    );
                    if link(ui, file_name(&path), &path) {
                        follow = Some(path);
                    }
                    ui.add_space(8.0);
                    // The samples run across the day, so they read as a strip rather than as
                    // separate values.
                    ui.spacing_mut().item_spacing.x = 1.0;
                    for sample in light.samples(sky.id()).unwrap_or_default() {
                        chip(ui, swatch(average(*sample)));
                    }
                });
            });
        }
    }
    follow
}

impl Rendered {
    pub fn details_ui(&self, ui: &mut egui::Ui) {
        ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| facts(ui, "amb_identity", &self.identity));
    }
}
