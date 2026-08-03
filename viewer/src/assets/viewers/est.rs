//! `.est` extra skeleton templates: the skeleton to use for a set on a given gender and race.

use anyhow::Result;
use egui::{RichText, ScrollArea};
use ironworks::file::{File, est};
use std::io::Cursor;

use super::{Preview, chara, facts, line, link, section, table};
use crate::utils::file_name;

/// The entry table's columns, each with the width its cells are padded to. The skeleton file is a
/// link rather than a padded cell, so it sits at the end.
const COLUMNS: [(&str, usize); 4] = [("Body", 28), ("Set", 5), ("Skeleton", 8), ("File", 8)];

/// The four templates that ship: what a set ID names in each, and the directory and letter its
/// skeletons are filed under.
const KINDS: [(&str, &str, &str, char); 4] = [
    ("extra_met.est", "equipment set", "met", 'm'),
    ("extra_top.est", "equipment set", "top", 't'),
    (
        "faceskeletontemplate.est",
        "character creation option",
        "face",
        'f',
    ),
    (
        "hairskeletontemplate.est",
        "character creation option",
        "hair",
        'h',
    ),
];

/// A skeleton template file, decoded and ready to draw.
pub struct Rendered {
    identity: Vec<(&'static str, String)>,
    /// Gender and race, set, and the skeleton they name.
    rows: Vec<(u16, u16, u16)>,
    /// Where this file's skeletons are filed, for the templates whose name says which they are.
    kind: Option<(&'static str, char)>,
}

/// The skeleton file an entry names, which the model code and the skeleton ID spell out in full.
fn skeleton(kind: (&str, char), gender_race: u16, id: u16) -> String {
    let (directory, letter) = kind;
    format!(
        "chara/human/c{gender_race:04}/skeleton/{directory}/{letter}{id:04}/skl_c{gender_race:04}{letter}{id:04}.sklb"
    )
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    let file = est::ExtraSkeletonTemplate::read(Cursor::new(bytes.to_vec()))?;
    let rows = file.entries().collect::<Vec<_>>();

    let name = file_name(path).to_lowercase();
    let known = KINDS.iter().find(|(file, ..)| *file == name);

    let mut identity = vec![("Entries", rows.len().to_string())];
    if let Some((_, names, ..)) = known {
        identity.push(("Set names", (*names).to_owned()));
    }

    log::info!("assets/est: {path} {} entries", rows.len());

    Ok(Preview::Est(Box::new(Rendered {
        identity,
        rows,
        kind: known.map(|(_, _, directory, letter)| (*directory, *letter)),
    })))
}

pub fn ui(ui: &mut egui::Ui, file: &Rendered) -> Option<String> {
    let mut follow = None;
    section(ui, "Entries");
    table(ui, &COLUMNS, file.rows.len(), |ui, index| {
        let (gender_race, set, id) = file.rows[index];
        let cells = [
            chara::described(gender_race),
            set.to_string(),
            id.to_string(),
        ];
        ui.horizontal(|ui| {
            // The link is a widget of its own where the rest of the row is one padded string, so
            // the spacing between them has to go for it to land under its header.
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label(RichText::new(line(&COLUMNS, cells.iter().map(String::as_str))).monospace());
            if let Some(kind) = file.kind {
                let path = skeleton(kind, gender_race, id);
                if link(ui, file_name(&path), &path) {
                    follow = Some(path);
                }
            }
        });
    });
    follow
}

impl Rendered {
    pub fn details_ui(&self, ui: &mut egui::Ui) {
        ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| facts(ui, "est_identity", &self.identity));
    }
}
