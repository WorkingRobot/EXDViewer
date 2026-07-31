//! `.tera` terrain: which plates a zone is tiled from, and where each of them sits.

use anyhow::Result;
use egui::{RichText, ScrollArea};
use ironworks::file::{File, tera};
use std::io::Cursor;

use super::{Preview, facts, line, link, section, table};

/// The plate table's columns, each with the width its cells are padded to. The model is a link
/// rather than a padded cell, so it sits at the end.
const COLUMNS: [(&str, usize); 4] = [
    ("Plate", 5),
    ("Cell", 10),
    ("Centre X, Z", 20),
    ("Model", 8),
];

/// One plate of the zone.
struct Row {
    cell: (i16, i16),
    centre: (f32, f32),
    model: String,
}

/// A terrain file, decoded and ready to draw.
pub struct Rendered {
    identity: Vec<(&'static str, String)>,
    rows: Vec<Row>,
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    let file = tera::Terrain::read(Cursor::new(bytes.to_vec()))?;

    let directory = &path[..path.len() - crate::utils::file_name(path).len()];
    let rows = file
        .plates()
        .iter()
        .enumerate()
        .map(|(index, plate)| Row {
            cell: (plate.x(), plate.y()),
            centre: file.plate_position(*plate),
            model: format!("{directory}{}", tera::Terrain::plate_file(index)),
        })
        .collect::<Vec<_>>();

    let identity = vec![
        ("Version", format!("{:#010x}", file.version())),
        ("Plates", rows.len().to_string()),
        ("Plate size", file.plate_size().to_string()),
        ("Clip distance", format!("{:.1}", file.clip_distance())),
        ("Unknown A", format!("{:.3}", file.unknown_a())),
        ("Unknown B", format!("{:#010x}", file.unknown_b())),
    ];

    log::info!("assets/tera: {path} {} plates", rows.len());

    Ok(Preview::Tera(Box::new(Rendered { identity, rows })))
}

pub fn ui(ui: &mut egui::Ui, file: &Rendered) -> Option<String> {
    let mut follow = None;
    section(ui, "Plates");
    table(ui, &COLUMNS, file.rows.len(), |ui, index| {
        let row = &file.rows[index];
        let cells = [
            index.to_string(),
            format!("{}, {}", row.cell.0, row.cell.1),
            format!("{:.1}, {:.1}", row.centre.0, row.centre.1),
        ];
        ui.horizontal(|ui| {
            // The link is a widget of its own where the rest of the row is one padded string, so
            // the spacing between them has to go for it to land under its header.
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label(RichText::new(line(&COLUMNS, cells.iter().map(String::as_str))).monospace());
            if link(ui, crate::utils::file_name(&row.model), &row.model) {
                follow = Some(row.model.clone());
            }
        });
    });
    follow
}

impl Rendered {
    pub fn details_ui(&self, ui: &mut egui::Ui) {
        ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| facts(ui, "tera_identity", &self.identity));
    }
}
