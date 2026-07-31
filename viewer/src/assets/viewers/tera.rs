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
    ("Center X, Z", 20),
    ("Model", 8),
];

/// The texture slots [`tera::Terrain::sampler_bias`] carries a bit for, lowest first.
const SLOTS: [&str; 3] = ["color", "normal", "specular"];

/// Height the plate map is given before the table takes what is left.
const MAP_HEIGHT: f32 = 260.0;

/// One plate of the zone.
struct Row {
    cell: (i16, i16),
    center: (f32, f32),
    model: String,
}

/// A terrain file, decoded and ready to draw.
pub struct Rendered {
    identity: Vec<(&'static str, String)>,
    rows: Vec<Row>,
    /// Cell bounds over every plate, as `(min, max)` inclusive, or `None` for a zone with none.
    bounds: Option<((i16, i16), (i16, i16))>,
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
            center: file.plate_position(*plate),
            model: format!("{directory}{}", tera::Terrain::plate_file(index)),
        })
        .collect::<Vec<_>>();

    let bias = file.sampler_bias();
    let slots = SLOTS
        .iter()
        .enumerate()
        .filter_map(|(bit, slot)| (bias & (1 << bit) != 0).then_some(*slot))
        .collect::<Vec<_>>();
    let identity = vec![
        ("Version", format!("{:#010x}", file.version())),
        ("Plates", rows.len().to_string()),
        ("Plate size", file.plate_size().to_string()),
        ("Clip distance", format!("{:.1}", file.clip_distance())),
        ("Edge bias", format!("{:.3}", file.edge_bias())),
        (
            "Alternate mip bias",
            match slots.is_empty() {
                true => "none".to_owned(),
                false => slots.join(", "),
            },
        ),
    ];

    let bounds = rows.iter().map(|row| row.cell).fold(None, |bounds, cell| {
        let ((min_x, min_y), (max_x, max_y)) = bounds.unwrap_or((cell, cell));
        Some((
            (min_x.min(cell.0), min_y.min(cell.1)),
            (max_x.max(cell.0), max_y.max(cell.1)),
        ))
    });

    log::info!("assets/tera: {path} {} plates", rows.len());

    Ok(Preview::Tera(Box::new(Rendered {
        identity,
        rows,
        bounds,
    })))
}

/// The plates drawn where they sit, which is the one thing the table cannot show: a zone is tiled
/// sparsely, and the holes are as much of its shape as the plates are.
///
/// Cell `y` runs down the screen because it is world Z, and the game's Z grows southward.
fn map(ui: &mut egui::Ui, file: &Rendered) -> Option<String> {
    let ((min_x, min_y), (max_x, max_y)) = file.bounds?;
    let mut follow = None;
    let across = f32::from(max_x - min_x) + 1.0;
    let down = f32::from(max_y - min_y) + 1.0;

    let room = egui::vec2(ui.available_width(), MAP_HEIGHT);
    let (response, painter) = ui.allocate_painter(room, egui::Sense::click());
    // Square cells, so a long thin zone reads as long and thin rather than being stretched to fit.
    let side = (room.x / across).min(room.y / down);
    let grid = egui::vec2(across * side, down * side);
    let origin = response.rect.center() - grid / 2.0;
    let at = |cell: (i16, i16)| {
        origin
            + egui::vec2(
                f32::from(cell.0 - min_x) * side,
                f32::from(cell.1 - min_y) * side,
            )
    };

    let pointer = response.hover_pos();
    let hovered = pointer.and_then(|pos| {
        file.rows.iter().position(|row| {
            egui::Rect::from_min_size(at(row.cell), egui::Vec2::splat(side)).contains(pos)
        })
    });

    let visuals = ui.visuals();
    let plate = visuals.widgets.inactive.bg_fill;
    let lit = visuals.widgets.hovered.bg_fill;
    let edge = visuals.widgets.noninteractive.bg_stroke;
    let label = visuals.text_color();
    // Nothing is drawn for a hole, so the plates have to carry a gap of their own to be told apart.
    let inset = (side * 0.06).clamp(0.5, 3.0);
    let font = egui::FontId::monospace((side * 0.3).clamp(6.0, 12.0));
    for (index, row) in file.rows.iter().enumerate() {
        let cell = egui::Rect::from_min_size(at(row.cell), egui::Vec2::splat(side)).shrink(inset);
        let on = hovered == Some(index);
        painter.rect(
            cell,
            2.0,
            if on { lit } else { plate },
            edge,
            egui::StrokeKind::Inside,
        );
        if side >= 22.0 {
            painter.text(
                cell.center(),
                egui::Align2::CENTER_CENTER,
                index,
                font.clone(),
                label,
            );
        }
    }

    if let Some(index) = hovered {
        let row = &file.rows[index];
        response.on_hover_text(format!(
            "Plate {index}\ncell {}, {}\ncenter {:.1}, {:.1}\n{}",
            row.cell.0,
            row.cell.1,
            row.center.0,
            row.center.1,
            crate::utils::file_name(&row.model),
        ));
        if ui.input(|i| i.pointer.primary_clicked()) {
            follow = Some(row.model.clone());
        }
    }
    follow
}

pub fn ui(ui: &mut egui::Ui, file: &Rendered) -> Option<String> {
    section(ui, "Layout");
    let mut follow = map(ui, file);
    ui.add_space(8.0);
    ui.separator();
    section(ui, "Plates");
    table(ui, &COLUMNS, file.rows.len(), |ui, index| {
        let row = &file.rows[index];
        let cells = [
            index.to_string(),
            format!("{}, {}", row.cell.0, row.cell.1),
            format!("{:.1}, {:.1}", row.center.0, row.center.1),
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
