//! `.atch` attach points: where a character's weapons and tools sit on their skeleton, in each of
//! the states the file carries.

use anyhow::Result;
use egui::{RichText, ScrollArea};
use ironworks::file::{File, atch};
use std::io::Cursor;

use super::{Preview, facts, line, section, table};

/// The placement table's columns, each with the width its cells are padded to.
const COLUMNS: [(&str, usize); 7] = [
    ("Point", 5),
    ("Acc", 3),
    ("State", 5),
    ("Bone", 16),
    ("Scale", 6),
    ("Offset", 24),
    ("Rotation", 24),
];

/// A file declaring no states has only its points to show, which
/// `chara/xls/attachoffset/d1011.atch` is.
const POINTS: [(&str, usize); 2] = [COLUMNS[0], COLUMNS[1]];

/// Where one point sits in one state.
struct Placement {
    state: usize,
    bone: String,
    scale: f32,
    offset: [f32; 3],
    rotation: [f32; 3],
}

/// One row of the table: a point, and the placement it takes in one state.
struct Row {
    point: String,
    accessory: bool,
    placement: Option<Placement>,
}

/// An attach point file, decoded and ready to draw.
pub struct Rendered {
    identity: Vec<(&'static str, String)>,
    rows: Vec<Row>,
    columns: &'static [(&'static str, usize)],
}

fn axes(values: [f32; 3]) -> String {
    format!("{:.3}, {:.3}, {:.3}", values[0], values[1], values[2])
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    let file = atch::AttachPoints::read(Cursor::new(bytes.to_vec()))?;

    let rows = file
        .tags()
        .iter()
        .enumerate()
        .flat_map(|(point, tag)| {
            let name = tag
                .as_str()
                .map_or_else(|| format!("#{point}"), str::to_owned);
            let accessory = file.accessory(point);
            let held: Vec<Option<Placement>> = match file.state_count() {
                // A point with no states still names something, so it keeps a row of its own.
                0 => vec![None],
                _ => file
                    .states(point)
                    .unwrap_or_default()
                    .iter()
                    .enumerate()
                    .map(|(state, held)| {
                        Some(Placement {
                            state,
                            bone: held.bone().to_owned(),
                            scale: held.scale(),
                            offset: held.offset(),
                            rotation: held.rotation(),
                        })
                    })
                    .collect(),
            };
            held.into_iter().map(move |placement| Row {
                point: name.clone(),
                accessory,
                placement,
            })
        })
        .collect::<Vec<_>>();

    let identity = vec![
        ("Points", file.tags().len().to_string()),
        ("States", file.state_count().to_string()),
        (
            "Accessories",
            (0..file.tags().len())
                .filter(|point| file.accessory(*point))
                .count()
                .to_string(),
        ),
    ];

    log::info!(
        "assets/atch: {path} {} points, {} states",
        file.tags().len(),
        file.state_count()
    );

    Ok(Preview::Atch(Box::new(Rendered {
        identity,
        columns: match file.state_count() {
            0 => &POINTS,
            _ => &COLUMNS,
        },
        rows,
    })))
}

pub fn ui(ui: &mut egui::Ui, file: &Rendered) {
    section(ui, "Placements");
    table(ui, file.columns, file.rows.len(), |ui, index| {
        let row = &file.rows[index];
        let mut cells = vec![
            row.point.clone(),
            match row.accessory {
                true => "yes".to_owned(),
                false => String::new(),
            },
        ];
        if let Some(placement) = &row.placement {
            cells.extend([
                placement.state.to_string(),
                placement.bone.clone(),
                format!("{:.3}", placement.scale),
                axes(placement.offset),
                axes(placement.rotation),
            ]);
        }
        ui.label(RichText::new(line(file.columns, cells.iter().map(String::as_str))).monospace());
    });
}

impl Rendered {
    pub fn details_ui(&self, ui: &mut egui::Ui) {
        ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| facts(ui, "atch_identity", &self.identity));
    }
}
