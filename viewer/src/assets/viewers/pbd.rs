//! `.pbd` pre-bone deformers: the transform that carries a skeleton's bones from one race and
//! gender to another.
//!
//! Each deformer names a character code and sits in a tree; walking from one code up to another
//! gives the deformers to apply in turn.

use std::io::Cursor;

use anyhow::Result;
use egui::{RichText, ScrollArea};
use ironworks::file::{File, pbd};

use super::{Preview, facts, heading, line, section, table};

const DEFORMERS: [(&str, usize); 4] = [("Code", 7), ("Parent", 7), ("Bones", 7), ("Scale", 8)];

const BONES: [(&str, usize); 4] = [
    ("Bone", 28),
    ("Translation", 26),
    ("Rotation", 34),
    ("Scale", 26),
];

/// One deformer, flattened out of the tree the file stores it in.
struct Entry {
    id: u16,
    parent: Option<u16>,
    scale: f32,
    bones: Vec<(String, pbd::BoneMatrix)>,
}

pub struct Rendered {
    identity: Vec<(&'static str, String)>,
    deformers: Vec<Entry>,
    /// Which deformer's bones are on show, kept per file the way the staining viewer keeps its
    /// templates.
    picked: egui::Id,
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    let file = pbd::PreBoneDeformer::read(Cursor::new(bytes.to_vec()))?;

    let deformers = file
        .deformers()
        .map(|deformer| Entry {
            id: deformer.id(),
            parent: deformer
                .node()
                .parent()
                .map(|parent| parent.deformer().id()),
            scale: deformer.scale(),
            bones: deformer.bones().unwrap_or_default().to_vec(),
        })
        .collect::<Vec<_>>();

    let bones = deformers
        .iter()
        .map(|deformer| deformer.bones.len())
        .sum::<usize>();
    let identity = vec![
        ("Deformers", deformers.len().to_string()),
        (
            "With bones",
            deformers
                .iter()
                .filter(|deformer| !deformer.bones.is_empty())
                .count()
                .to_string(),
        ),
        ("Bones", bones.to_string()),
    ];

    log::info!("assets/pbd: {path} {} deformers", deformers.len());

    Ok(Preview::Pbd(Box::new(Rendered {
        identity,
        deformers,
        picked: egui::Id::new(("pbd deformer", path)),
    })))
}

pub fn ui(ui: &mut egui::Ui, file: &Rendered) {
    section(ui, "Deformers");
    let mut picked = ui
        .data(|data| data.get_temp::<usize>(file.picked))
        .unwrap_or(0)
        .min(file.deformers.len().saturating_sub(1));

    table(ui, &DEFORMERS, file.deformers.len(), |ui, index| {
        let deformer = &file.deformers[index];
        let cells = [
            format!("c{:04}", deformer.id),
            deformer
                .parent
                .map_or_else(String::new, |id| format!("c{id:04}")),
            deformer.bones.len().to_string(),
            format!("{:.3}", deformer.scale),
        ];
        // A selectable label pads itself taller than the row the table lays out, so the picked
        // one is drawn strong rather than boxed.
        let text = RichText::new(line(&DEFORMERS, cells.iter().map(String::as_str))).monospace();
        let response = ui.add(
            egui::Label::new(match index == picked {
                true => text.strong(),
                false => text,
            })
            .sense(egui::Sense::click()),
        );
        if response.clicked() {
            picked = index;
        }
    });
    ui.data_mut(|data| data.insert_temp(file.picked, picked));
}

impl Rendered {
    pub fn details_ui(&self, ui: &mut egui::Ui) {
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let picked = ui
                .data(|data| data.get_temp::<usize>(self.picked))
                .unwrap_or(0);
            if let Some(deformer) = self.deformers.get(picked)
                && !deformer.bones.is_empty()
            {
                heading(ui, &format!("c{:04} bones", deformer.id));
                table(ui, &BONES, deformer.bones.len(), |ui, index| {
                    let (name, matrix) = &deformer.bones[index];
                    let row = |values: [f32; 4], count: usize| {
                        values[..count]
                            .iter()
                            .map(|value| format!("{value:>8.3}"))
                            .collect::<Vec<_>>()
                            .join(" ")
                    };
                    let cells = [
                        name.clone(),
                        row(matrix[0], 3),
                        row(matrix[1], 4),
                        row(matrix[2], 3),
                    ];
                    ui.label(
                        RichText::new(line(&BONES, cells.iter().map(String::as_str))).monospace(),
                    );
                });
                ui.add_space(8.0);
                ui.separator();
            }
            facts(ui, "pbd_identity", &self.identity);
        });
    }
}
