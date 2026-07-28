use anyhow::Result;
use egui::{Rect, RichText, ScrollArea, Sense, Vec2, load::SizedTexture, pos2, vec2};
use ironworks::file::{File, gfd};
use std::io::Cursor;

use super::{PADDING, Preview, facts, grid, missing, textures};
use crate::assets::deps::{Dep, Deps};
use crate::backend::Backend;

/// Icon sheets are drawn at twice their stated size, which is the size the game uses them at in
/// anything but the smallest text.
const ICON_SCALE: f32 = 2.0;

/// The sheets the same icons are drawn on, one per controller the game supports.
const CONTROLLERS: [(&str, &str); 6] = [
    ("Xbox", "fontIcon_Xinput"),
    ("PS5", "fontIcon_Ps5"),
    ("PS4", "fontIcon_Ps4"),
    ("PS3", "fontIcon_Ps3"),
    ("Lys", "fontIcon_Lys"),
    ("Obe", "fonticon_obe"),
];

/// The icon sheet, decoded and ready to draw.
pub struct Rendered {
    icons: Vec<Icon>,
    /// Every controller's sheet, by the name the picker shows.
    sheets: Vec<(&'static str, String)>,
    /// The largest icon, which every cell of the grid is sized to.
    largest: Vec2,
    identity: Vec<(&'static str, String)>,
    /// Which controller's sheet is being drawn, kept per file the way the layout viewer keeps its
    /// selection.
    choice: egui::Id,
}

struct Icon {
    id: u16,
    source: Rect,
    size: Vec2,
    /// The icon actually drawn, where this one is only a name for another.
    redirect: u16,
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    let file = gfd::FontIcons::read(Cursor::new(bytes.to_vec()))?;

    // Every controller's sheet is 512x1024 and laid out alike, so the rectangles need no file to be
    // resolved against.
    let (width, height) = (512.0, 1024.0);
    let icons = file
        .icons()
        .iter()
        .map(|icon| {
            let drawn = file.icon(icon.id()).unwrap_or(icon);
            // The sheet holds a second copy of every icon at twice the size, which is what a cell
            // this size should be sampling rather than stretching the small one.
            let (left, top, wide, tall) = drawn.large();
            Icon {
                id: icon.id(),
                source: Rect::from_min_max(
                    pos2(f32::from(left) / width, f32::from(top) / height),
                    pos2(
                        f32::from(left + wide) / width,
                        f32::from(top + tall) / height,
                    ),
                ),
                size: vec2(f32::from(drawn.width()), f32::from(drawn.height())),
                redirect: icon.redirect(),
            }
        })
        .collect::<Vec<_>>();

    let largest = icons
        .iter()
        .fold(Vec2::ZERO, |largest, icon| largest.max(icon.size));

    let identity = vec![
        ("Icons", file.icons().len().to_string()),
        (
            "Aliases",
            file.icons()
                .iter()
                .filter(|icon| icon.redirect() != 0)
                .count()
                .to_string(),
        ),
        ("Sheet", "512 x 1024".to_owned()),
    ];

    let sheets = CONTROLLERS
        .iter()
        .map(|(name, sheet)| (*name, format!("common/font/{sheet}.tex")))
        .collect();

    Ok(Preview::Icons(Box::new(Rendered {
        icons,
        sheets,
        largest,
        identity,
        choice: egui::Id::new(("gfd controller", path)),
    })))
}

pub fn ui(ui: &mut egui::Ui, icons: &Rendered, deps: &mut Deps, backend: &Backend) {
    let mut chosen = ui
        .data(|data| data.get_temp::<usize>(icons.choice))
        .unwrap_or(0);
    ui.horizontal_wrapped(|ui| {
        for (index, (name, _)) in CONTROLLERS.iter().enumerate() {
            if ui.selectable_label(index == chosen, *name).clicked() {
                chosen = index;
            }
        }
    });
    ui.data_mut(|data| data.insert_temp(icons.choice, chosen));
    ui.separator();

    let path = format!("common/font/{}.tex", CONTROLLERS[chosen].1);
    let cell = icons.largest * ICON_SCALE + Vec2::splat(PADDING * 2.0);
    grid(ui, cell, icons.icons.len(), |ui, index, at| {
        let icon = &icons.icons[index];
        let rect = Rect::from_center_size(at.center(), icon.size * ICON_SCALE);
        match deps.atlas(ui.ctx(), backend, &path) {
            Dep::Ready(sheet) => {
                egui::Image::new(SizedTexture::new(sheet.texture(), rect.size()))
                    .uv(icon.source)
                    .paint_at(ui, rect);
            }
            Dep::Pending => {
                egui::Spinner::new().paint_at(ui, rect);
            }
            Dep::Failed => missing(ui, rect),
        }
        ui.interact(at, icons.choice.with(index), Sense::hover())
            .on_hover_ui(|ui| {
                ui.label(RichText::new(format!("Icon {}", icon.id)).monospace());
                if icon.redirect != 0 {
                    ui.label(RichText::new(format!("Drawn as {}", icon.redirect)).weak());
                }
            });
    });
}

impl Rendered {
    pub fn details_ui(&self, ui: &mut egui::Ui, follow: &mut Option<String>) {
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            facts(ui, "icons_identity", &self.identity);
            // The file names no sheet at all: every controller's holds the same rectangles.
            let sheets = self
                .sheets
                .iter()
                .map(|(name, path)| (Some(*name), path.as_str()));
            if let Some(path) = textures(ui, sheets) {
                *follow = Some(path);
            }
        });
    }
}
