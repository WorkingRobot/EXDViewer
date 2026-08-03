//! The environment sets a zone runs: one timeline per weather, split into sets that each animate
//! one thing across the day from their own keyframes.
//!
//! `.envb` lights and shades a space, `.obsb` drives the objects standing in it, and `.essb` says
//! what it sounds like. All three wrap the same section, and each numbers what a set animates in a
//! range of its own.

use std::collections::HashSet;
use std::io::Cursor;

use anyhow::Result;
use egui::{
    Color32, RichText, ScrollArea, Sense, Vec2, collapsing_header::paint_default_icon, vec2,
};
use ironworks::file::{File, envb, envs, essb, obsb};

use super::super::{Preview, facts, link, section};
use super::{chip, clock};
use crate::assets::deps::Deps;
use crate::backend::Backend;
use crate::utils::file_name;

/// The sheet a weather names a row of.
const WEATHER: &str = "Weather";

/// Space each level of the tree is set in by.
const INDENT: f32 = 12.0;

/// Room the expander takes, kept on rows without one so their labels still line up.
const TRIANGLE: f32 = 12.0;

/// The file the tree was read from, which it keeps rather than copying every keyframe out.
enum Source {
    Environment(envb::EnvironmentFile),
    ObjectBehavior(obsb::ObjectBehaviourFile),
    SoundEnvironment(essb::SoundEnvironmentFile),
}

impl Source {
    fn environments(&self) -> &envs::Environments {
        match self {
            Self::Environment(file) => file.environments(),
            Self::ObjectBehavior(file) => file.environments(),
            Self::SoundEnvironment(file) => file.environments(),
        }
    }
}

/// Where a row sits in the tree.
#[derive(Clone, Copy, PartialEq, Eq)]
enum At {
    Weather(usize),
    Set(usize, usize),
    Keyframe(usize, usize, usize),
}

impl At {
    fn depth(self) -> usize {
        match self {
            Self::Weather(..) => 0,
            Self::Set(..) => 1,
            Self::Keyframe(..) => 2,
        }
    }
}

/// One row as it is drawn.
struct Line<'a> {
    label: String,
    colors: Vec<Color32>,
    /// The first file the row names, and how many more it holds.
    asset: Option<(&'a str, usize)>,
    detail: String,
}

fn color(colour: &envs::Colour) -> Color32 {
    Color32::from_rgb(colour.red(), colour.green(), colour.blue())
}

fn described(colour: &envs::Colour) -> String {
    format!(
        "{}, {}, {}, {} x {:.3}",
        colour.red(),
        colour.green(),
        colour.blue(),
        colour.alpha(),
        colour.intensity()
    )
}

pub struct Rendered {
    identity: Vec<(&'static str, String)>,
    source: Source,
    rows: Vec<At>,
    /// Where the open rows and the selected one are kept, since drawing takes the file by
    /// reference.
    state: egui::Id,
}

fn rendered(path: &str, source: Source) -> Preview {
    let set = source.environments();
    let mut rows = Vec::new();
    let mut sets = 0;
    let mut keyframes = 0;
    for (index, weather) in set.weathers().iter().enumerate() {
        rows.push(At::Weather(index));
        sets += weather.sets().len();
        for (set_index, animated) in weather.sets().iter().enumerate() {
            rows.push(At::Set(index, set_index));
            keyframes += animated.keyframes().len();
            for keyframe in 0..animated.keyframes().len() {
                rows.push(At::Keyframe(index, set_index, keyframe));
            }
        }
    }

    let mut identity = vec![
        ("Version", set.version().to_string()),
        ("Weathers", set.weathers().len().to_string()),
        ("Sets", sets.to_string()),
        ("Keyframes", keyframes.to_string()),
    ];
    let options = set
        .options()
        .iter()
        .enumerate()
        .filter(|(_, on)| **on)
        .map(|(index, _)| index.to_string())
        .collect::<Vec<_>>();
    if !options.is_empty() {
        identity.push(("Options", options.join(", ")));
    }

    log::info!("assets/zone: {path} {} weathers", set.weathers().len());

    Preview::Environments(Box::new(Rendered {
        identity,
        source,
        rows,
        state: egui::Id::new(path).with("envs_tree"),
    }))
}

pub fn environment(path: &str, bytes: &[u8]) -> Result<Preview> {
    let file = envb::EnvironmentFile::read(Cursor::new(bytes.to_vec()))?;
    Ok(rendered(path, Source::Environment(file)))
}

pub fn object_behavior(path: &str, bytes: &[u8]) -> Result<Preview> {
    let file = obsb::ObjectBehaviourFile::read(Cursor::new(bytes.to_vec()))?;
    Ok(rendered(path, Source::ObjectBehavior(file)))
}

pub fn sound(path: &str, bytes: &[u8]) -> Result<Preview> {
    let file = essb::SoundEnvironmentFile::read(Cursor::new(bytes.to_vec()))?;
    Ok(rendered(path, Source::SoundEnvironment(file)))
}

pub fn ui(
    ui: &mut egui::Ui,
    file: &Rendered,
    deps: &mut Deps,
    backend: &Backend,
) -> Option<String> {
    let mut follow = None;
    let mut open = file.open(ui);
    let mut shown = Vec::new();
    let mut collapsed_at = None;
    for (index, at) in file.rows.iter().enumerate() {
        match collapsed_at {
            Some(depth) if at.depth() > depth => continue,
            _ => collapsed_at = None,
        }
        let parent = file.parent(*at);
        // Only a weather is open on arrival, since which set a keyframe belongs to is most of what
        // the file says.
        let expanded = parent && ((at.depth() == 0) != open.contains(&index));
        if parent && !expanded {
            collapsed_at = Some(at.depth());
        }
        shown.push((index, expanded));
    }

    section(ui, "Weathers");
    let picked = file.selected(ui);
    let mut selected = picked;
    let mut toggled = None;
    let height = ui
        .text_style_height(&egui::TextStyle::Monospace)
        .max(TRIANGLE)
        + 2.0 * ui.spacing().button_padding.y
        + ui.spacing().item_spacing.y;
    ScrollArea::vertical()
        .auto_shrink(false)
        .show_rows(ui, height, shown.len(), |ui, range| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            for &(index, expanded) in &shown[range] {
                let at = file.rows[index];
                let line = file.line(at);
                let named = match at {
                    At::Weather(weather) => deps
                        .text(ui.ctx(), backend, WEATHER, file.weather(weather).id())
                        .map(str::to_owned),
                    _ => None,
                };
                ui.horizontal(|ui| {
                    ui.add_space(at.depth() as f32 * INDENT);
                    match file.parent(at) {
                        false => ui.add_space(TRIANGLE),
                        true => {
                            let (_, response) =
                                ui.allocate_exact_size(Vec2::splat(TRIANGLE), Sense::click());
                            let openness = match expanded {
                                true => 1.0,
                                false => 0.0,
                            };
                            paint_default_icon(ui, openness, &response);
                            if response.clicked() {
                                toggled = Some(index);
                            }
                        }
                    }

                    if ui
                        .selectable_label(
                            picked == Some(index),
                            RichText::new(&line.label).monospace(),
                        )
                        .clicked()
                    {
                        selected = Some(index);
                    }
                    if let Some(name) = &named {
                        ui.label(RichText::new(name).monospace());
                    }
                    for color in &line.colors {
                        chip(ui, *color);
                    }
                    if let Some((asset, more)) = line.asset {
                        if link(ui, file_name(asset), asset) {
                            follow = Some(asset.to_owned());
                        }
                        if more > 0 {
                            ui.label(RichText::new(format!("+{more}")).monospace().weak());
                        }
                    }
                    if !line.detail.is_empty() {
                        ui.label(RichText::new(&line.detail).monospace().weak());
                    }
                });
            }
        });

    if selected != picked {
        ui.data_mut(|data| data.insert_temp(file.state.with("selected"), selected));
    }
    if let Some(index) = toggled {
        if !open.insert(index) {
            open.remove(&index);
        }
        ui.data_mut(|data| data.insert_temp(file.state, open));
    }
    follow
}

impl Rendered {
    fn open(&self, ui: &egui::Ui) -> HashSet<usize> {
        ui.data(|data| data.get_temp(self.state).unwrap_or_default())
    }

    fn selected(&self, ui: &egui::Ui) -> Option<usize> {
        ui.data(|data| data.get_temp(self.state.with("selected")).flatten())
    }

    fn weather(&self, index: usize) -> &envs::Weather {
        &self.source.environments().weathers()[index]
    }

    fn set(&self, weather: usize, index: usize) -> &envs::Set {
        &self.weather(weather).sets()[index]
    }

    fn keyframe(&self, weather: usize, set: usize, index: usize) -> &envs::Keyframe {
        &self.set(weather, set).keyframes()[index]
    }

    fn parent(&self, at: At) -> bool {
        match at {
            At::Weather(weather) => !self.weather(weather).sets().is_empty(),
            At::Set(weather, set) => !self.set(weather, set).keyframes().is_empty(),
            At::Keyframe(..) => false,
        }
    }

    fn line(&self, at: At) -> Line<'_> {
        match at {
            At::Weather(index) => {
                let weather = self.weather(index);
                Line {
                    label: weather.id().to_string(),
                    colors: Vec::new(),
                    asset: None,
                    detail: format!(
                        "{} sets over {:.0}s",
                        weather.sets().len(),
                        weather.length()
                    ),
                }
            }
            At::Set(weather, index) => {
                let set = self.set(weather, index);
                Line {
                    label: format!("kind {}", set.kind()),
                    colors: Vec::new(),
                    asset: None,
                    detail: format!("{} keyframes", set.keyframes().len()),
                }
            }
            At::Keyframe(weather, set, index) => {
                let keyframe = self.keyframe(weather, set, index);
                let detail = match keyframe.unknown().is_empty() {
                    true => String::new(),
                    false => format!("{} bytes unread", keyframe.unknown().len()),
                };
                Line {
                    label: clock(keyframe.time()),
                    colors: keyframe.colours().iter().map(color).collect(),
                    asset: keyframe
                        .paths()
                        .first()
                        .map(|path| (path.as_str(), keyframe.paths().len() - 1)),
                    detail,
                }
            }
        }
    }

    /// Everything the selected row carries beside its colors and the files it names.
    fn fields(&self, at: At) -> Vec<(&'static str, String)> {
        match at {
            At::Weather(index) => {
                let weather = self.weather(index);
                vec![
                    ("Weather", weather.id().to_string()),
                    ("Length", format!("{:.0}s", weather.length())),
                    ("Sets", weather.sets().len().to_string()),
                    ("Unknown A", weather.unknown_a().to_string()),
                    ("Unknown B", format!("{:.3}", weather.unknown_b())),
                ]
            }
            At::Set(weather, index) => {
                let set = self.set(weather, index);
                vec![
                    ("Kind", set.kind().to_string()),
                    ("Keyframes", set.keyframes().len().to_string()),
                ]
            }
            At::Keyframe(weather, set, index) => {
                let keyframe = self.keyframe(weather, set, index);
                let mut fields = vec![("Time", clock(keyframe.time()))];
                if !keyframe.unknown().is_empty() {
                    fields.push(("Unread", format!("{} bytes", keyframe.unknown().len())));
                }
                fields
            }
        }
    }

    /// The files the selected row names, which for a weather are the two it may carry itself.
    fn assets(&self, at: At) -> Vec<&str> {
        let paths: &[String] = match at {
            At::Weather(index) => self.weather(index).paths(),
            At::Set(..) => &[],
            At::Keyframe(weather, set, index) => self.keyframe(weather, set, index).paths(),
        };
        paths
            .iter()
            .filter(|path| !path.is_empty())
            .map(String::as_str)
            .collect()
    }

    fn colors(&self, at: At) -> &[envs::Colour] {
        match at {
            At::Keyframe(weather, set, index) => self.keyframe(weather, set, index).colours(),
            _ => &[],
        }
    }

    pub fn details_ui(&self, ui: &mut egui::Ui, follow: &mut Option<String>) {
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if let Some(index) = self.selected(ui)
                && let Some(&at) = self.rows.get(index)
            {
                ui.label(RichText::new(self.line(at).label).strong());
                ui.add_space(4.0);
                egui::Grid::new("envs_selected")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        for (label, value) in self.fields(at) {
                            ui.label(RichText::new(label).weak());
                            ui.label(RichText::new(value).monospace());
                            ui.allocate_space(vec2(ui.available_width(), 0.0));
                            ui.end_row();
                        }
                    });

                let colors = self.colors(at);
                if !colors.is_empty() {
                    ui.add_space(8.0);
                    ui.separator();
                    ui.label(RichText::new("Colors").weak());
                    ui.add_space(4.0);
                    egui::Grid::new("envs_colors")
                        .num_columns(2)
                        .striped(true)
                        .show(ui, |ui| {
                            for colour in colors {
                                chip(ui, color(colour));
                                ui.label(RichText::new(described(colour)).monospace());
                                ui.allocate_space(vec2(ui.available_width(), 0.0));
                                ui.end_row();
                            }
                        });
                }

                let assets = self.assets(at);
                if !assets.is_empty() {
                    ui.add_space(8.0);
                    ui.separator();
                    ui.label(RichText::new("Assets").weak());
                    ui.add_space(4.0);
                    for path in assets {
                        if link(ui, file_name(path), path) {
                            *follow = Some(path.to_owned());
                        }
                    }
                }

                ui.add_space(8.0);
                ui.separator();
            }

            facts(ui, "envs_identity", &self.identity);
        });
    }
}
