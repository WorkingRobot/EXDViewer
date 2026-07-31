//! `.eid` bind points: the points on a skeleton that weapons, effects and other objects hang off.

use anyhow::Result;
use egui::{RichText, ScrollArea};
use ironworks::file::{File, eid};
use std::io::Cursor;

use super::{Preview, facts, line, link, section, table};
use crate::utils::file_name;

/// The bind point table's columns, each with the width its cells are padded to.
const COLUMNS: [(&str, usize); 4] = [("Bone", 22), ("ID", 4), ("Position", 26), ("Rotation", 26)];

/// One bind point.
struct Row {
    bone: String,
    id: i32,
    position: [f32; 3],
    rotation: [f32; 3],
}

/// A bind point file, decoded and ready to draw.
pub struct Rendered {
    identity: Vec<(&'static str, String)>,
    rows: Vec<Row>,
    /// The skeleton these points hang off, which sits beside the file under the same name.
    skeleton: String,
}

fn axes(values: [f32; 3]) -> String {
    format!("{:.3}, {:.3}, {:.3}", values[0], values[1], values[2])
}

/// A layout tag, which is two ASCII digits held little-endian.
fn tag(version: i16) -> String {
    let bytes = version.to_le_bytes();
    match std::str::from_utf8(&bytes) {
        Ok(text) if text.bytes().all(|byte| byte.is_ascii_digit()) => text.to_owned(),
        _ => format!("{version:#06x}"),
    }
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    let file = eid::BindPoints::read(Cursor::new(bytes.to_vec()))?;

    let rows = file
        .bind_points()
        .iter()
        .map(|point| Row {
            bone: point.name().to_owned(),
            id: point.id(),
            position: point.position(),
            rotation: point.rotation(),
        })
        .collect::<Vec<_>>();

    let identity = vec![
        ("Version", tag(file.version1())),
        ("Version 2", tag(file.version2())),
        (
            "Rotation",
            match file.radians() {
                true => "radians".to_owned(),
                false => "degrees".to_owned(),
            },
        ),
        ("Points", rows.len().to_string()),
        ("Unknown", format!("{:#010x}", file.unknown())),
    ];

    let name = file_name(path);
    log::info!("assets/eid: {path} {} bind points", rows.len());

    Ok(Preview::Eid(Box::new(Rendered {
        identity,
        rows,
        skeleton: format!(
            "{}{}.sklb",
            &path[..path.len() - name.len()],
            name.trim_end_matches(".eid").replacen("eid_", "skl_", 1)
        ),
    })))
}

pub fn ui(ui: &mut egui::Ui, file: &Rendered) -> Option<String> {
    let mut follow = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Skeleton").weak());
        if link(ui, file_name(&file.skeleton), &file.skeleton) {
            follow = Some(file.skeleton.clone());
        }
    });
    ui.add_space(4.0);

    section(ui, "Bind points");
    table(ui, &COLUMNS, file.rows.len(), |ui, index| {
        let row = &file.rows[index];
        let cells = [
            row.bone.clone(),
            row.id.to_string(),
            axes(row.position),
            axes(row.rotation),
        ];
        ui.label(RichText::new(line(&COLUMNS, cells.iter().map(String::as_str))).monospace());
    });
    follow
}

impl Rendered {
    pub fn details_ui(&self, ui: &mut egui::Ui) {
        ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| facts(ui, "eid_identity", &self.identity));
    }
}
