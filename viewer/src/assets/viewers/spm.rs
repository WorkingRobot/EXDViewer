//! `.spm` shader parameter maps: the grid a shader reads its lighting constants out of, indexed by
//! the profile a material names.
//!
//! The grid is drawn the other way up from how the file writes it. A parameter is the readable
//! unit, so the parameters run down the side and the profiles across the top.

use std::io::Cursor;

use anyhow::Result;
use egui::{RichText, ScrollArea};
use ironworks::file::{File, spm};

use super::{Preview, facts, line, section};

/// Room a profile's column takes, which fits the widest value any of them holds.
const COLUMN: usize = 9;

/// Room the parameter names take.
const NAME: usize = 36;

/// A hash the format writes and nothing spells out.
fn named(id: u32) -> String {
    spm::name(id).map_or_else(|| format!("{id:#010x}"), str::to_owned)
}

fn shown(value: spm::Value) -> String {
    match value {
        spm::Value::Float(value) => format!("{value:.3}"),
        spm::Value::Unsigned(value) => value.to_string(),
        spm::Value::Name(id) => named(id),
    }
}

pub struct Rendered {
    identity: Vec<(&'static str, String)>,
    columns: Vec<(&'static str, usize)>,
    /// The header, then one line per parameter, already padded to the columns.
    rows: Vec<String>,
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    let file = spm::ShaderParameters::read(Cursor::new(bytes.to_vec()))?;

    let mut columns = vec![("Parameter", NAME), ("Type", 8)];
    columns.extend(file.rows().iter().map(|_| ("", COLUMN)));

    let rows = file
        .columns()
        .iter()
        .enumerate()
        .map(|(column, parameter)| {
            let cells = [named(parameter.id()), format!("{:?}", parameter.kind())]
                .into_iter()
                .chain(
                    (0..file.rows().len())
                        .map(|row| file.value(row, column).map_or_else(String::new, shown)),
                )
                .collect::<Vec<_>>();
            line(&columns, cells.iter().map(String::as_str))
        })
        .collect::<Vec<_>>();

    let table = file
        .rows()
        .first()
        .map_or_else(|| "none".to_owned(), |row| named(row.table()));
    let identity = vec![
        ("Table", table),
        ("Parameters", file.columns().len().to_string()),
        ("Profiles", file.rows().len().to_string()),
    ];

    log::info!(
        "assets/spm: {path} {} parameters over {} profiles",
        file.columns().len(),
        file.rows().len()
    );

    Ok(Preview::Spm(Box::new(Rendered {
        identity,
        columns,
        rows,
    })))
}

pub fn ui(ui: &mut egui::Ui, file: &Rendered) {
    section(ui, "Parameters");
    // The grid runs wider than the panel once a file carries every profile, so it scrolls both
    // ways rather than being clipped.
    ScrollArea::both().auto_shrink(false).show(ui, |ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        let profiles = (0..file.columns.len() - 2).map(|index| index.to_string());
        let header = line(
            &file.columns,
            ["Parameter", "Type"]
                .into_iter()
                .map(str::to_owned)
                .chain(profiles)
                .collect::<Vec<_>>()
                .iter()
                .map(String::as_str),
        );
        ui.label(RichText::new(header).monospace().weak());
        for row in &file.rows {
            ui.label(RichText::new(row).monospace());
        }
    });
}

impl Rendered {
    pub fn details_ui(&self, ui: &mut egui::Ui) {
        ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| facts(ui, "spm_identity", &self.identity));
    }
}
