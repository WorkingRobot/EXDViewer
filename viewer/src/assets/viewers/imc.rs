//! `.imc` image change: the material, decal, VFX and sound each variant of a model set is drawn
//! with.

use anyhow::Result;
use egui::{RichText, ScrollArea};
use ironworks::file::{File, imc};
use std::io::Cursor;

use super::{Preview, facts, line, section, table};

/// Where a part sits in its set. Equipment and accessories share the positions, and the file does
/// not say which of the two it holds, so both readings are named.
const PARTS: [&str; 5] = [
    "head/ears",
    "body/neck",
    "hands/wrists",
    "legs/ring R",
    "feet/ring L",
];

/// The entry table's columns, each with the width its cells are padded to.
const COLUMNS: [(&str, usize); 8] = [
    ("Variant", 7),
    ("Part", 12),
    ("Material", 8),
    ("Decal", 5),
    ("VFX", 3),
    ("Sound", 5),
    ("Anim", 4),
    ("Attributes", 10),
];

/// A file with one part names no slot: the part index is all it has, and it is always zero.
const SINGLE: [(&str, usize); 7] = [
    COLUMNS[0], COLUMNS[2], COLUMNS[3], COLUMNS[4], COLUMNS[5], COLUMNS[6], COLUMNS[7],
];

/// One part of one variant.
struct Row {
    variant: usize,
    part: u8,
    material: u8,
    decal: u8,
    vfx: u8,
    sound: u8,
    animation: u8,
    attributes: u16,
}

/// An image change file, decoded and ready to draw.
pub struct Rendered {
    identity: Vec<(&'static str, String)>,
    rows: Vec<Row>,
    /// Whether a part's position names a slot, which a file holding a single part has nothing to
    /// say for.
    named_parts: bool,
}

/// The attributes a variant enables, as the letters the model names them by.
fn attributes(mask: u16) -> String {
    ('a'..='j')
        .enumerate()
        .filter(|(bit, _)| mask & (1 << bit) != 0)
        .map(|(_, letter)| letter)
        .collect()
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    let file = imc::ImageChange::read(Cursor::new(bytes.to_vec()))?;

    let parts: Vec<u8> = (0..u16::BITS as u8)
        .filter(|bit| file.part_mask() & (1 << bit) != 0)
        .collect();
    let rows = file
        .entries()
        .chunks(parts.len().max(1))
        .enumerate()
        .flat_map(|(variant, variant_entries)| {
            variant_entries
                .iter()
                .zip(&parts)
                .map(move |(entry, &part)| Row {
                    variant,
                    part,
                    material: entry.material_id(),
                    decal: entry.decal_id(),
                    vfx: entry.vfx_id(),
                    sound: entry.sound_id(),
                    animation: entry.material_animation_id(),
                    attributes: entry.attribute_mask(),
                })
        })
        .collect::<Vec<_>>();

    // Variant 0 is the default, which the header does not count.
    let variants = u32::from(file.variant_count()) + 1;
    let identity = vec![
        ("Variants", variants.to_string()),
        ("Parts", parts.len().to_string()),
        ("Part mask", format!("{:#014b}", file.part_mask())),
        ("Entries", rows.len().to_string()),
    ];

    log::info!(
        "assets/imc: {path} {variants} variants, {} parts, {} entries",
        parts.len(),
        rows.len()
    );

    Ok(Preview::Imc(Box::new(Rendered {
        identity,
        rows,
        named_parts: parts.len() > 1,
    })))
}

pub fn ui(ui: &mut egui::Ui, file: &Rendered) {
    let columns: &[(&str, usize)] = match file.named_parts {
        true => &COLUMNS,
        false => &SINGLE,
    };
    section(ui, "Entries");
    table(ui, columns, file.rows.len(), |ui, index| {
        let row = &file.rows[index];
        let mut cells = vec![row.variant.to_string()];
        if file.named_parts {
            cells.push(
                PARTS
                    .get(usize::from(row.part))
                    .map_or_else(|| row.part.to_string(), ToString::to_string),
            );
        }
        cells.extend([
            row.material.to_string(),
            row.decal.to_string(),
            row.vfx.to_string(),
            row.sound.to_string(),
            row.animation.to_string(),
            attributes(row.attributes),
        ]);
        ui.label(RichText::new(line(columns, cells.iter().map(String::as_str))).monospace());
    });
}

impl Rendered {
    pub fn details_ui(&self, ui: &mut egui::Ui) {
        ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| facts(ui, "imc_identity", &self.identity));
    }
}
