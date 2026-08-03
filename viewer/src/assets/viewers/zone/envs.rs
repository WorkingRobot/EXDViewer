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

/// Values a keyframe's row shows before it runs out of room.
const PREVIEW: usize = 4;

fn color(colour: envs::Colour) -> Color32 {
    Color32::from_rgb(colour.red(), colour.green(), colour.blue())
}

fn described(colour: envs::Colour) -> String {
    format!(
        "{}, {}, {}, {} x {:.3}",
        colour.red(),
        colour.green(),
        colour.blue(),
        colour.alpha(),
        colour.intensity()
    )
}

/// One value of a keyframe, without the name it is filed under.
fn shown(value: &envs::Value) -> String {
    match value {
        envs::Value::Float(value) => format!("{value:.3}"),
        envs::Value::Unsigned(value) => value.to_string(),
        envs::Value::Flag(on) => match on {
            true => "yes".to_owned(),
            false => "no".to_owned(),
        },
        envs::Value::Colour(colour) => described(*colour),
        envs::Value::Path(path) => path.clone(),
    }
}

/// A field name as the format writes it, spaced out for a label.
fn titled(name: &str) -> String {
    let mut title = name.replace('_', " ");
    if let Some(first) = title.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    title
}

/// The scalars of a keyframe, which are the ones neither the color chips nor the asset links
/// already carry.
fn scalars(keyframe: &envs::Keyframe) -> impl Iterator<Item = (&'static str, &envs::Value)> {
    keyframe.fields().iter().filter_map(|(name, value)| {
        matches!(
            value,
            envs::Value::Float(_) | envs::Value::Unsigned(_) | envs::Value::Flag(_)
        )
        .then_some((*name, value))
    })
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
        .zip(envs::OPTIONS)
        .filter(|(on, _)| **on)
        .map(|(_, name)| name)
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
                    label: set.name().unwrap_or("Unnamed").to_owned(),
                    colors: Vec::new(),
                    asset: None,
                    detail: format!("{} keyframes", set.keyframes().len()),
                }
            }
            At::Keyframe(weather, set, index) => {
                let keyframe = self.keyframe(weather, set, index);
                let mut detail = scalars(keyframe)
                    .take(PREVIEW)
                    .map(|(_, value)| shown(value))
                    .collect::<Vec<_>>()
                    .join("  ");
                if scalars(keyframe).count() > PREVIEW {
                    detail.push('…');
                }
                let mut named = keyframe.paths().filter(|path| !path.is_empty());
                let first = named.next();
                Line {
                    label: clock(keyframe.time()),
                    colors: keyframe.colours().map(color).collect(),
                    asset: first.map(|path| (path, named.count())),
                    detail,
                }
            }
        }
    }

    /// Everything the selected row carries beside its colors and the files it names.
    fn fields(&self, at: At) -> Vec<(String, String)> {
        match at {
            At::Weather(index) => {
                let weather = self.weather(index);
                [
                    ("Weather", weather.id().to_string()),
                    ("Length", format!("{:.0}s", weather.length())),
                    ("Sets", weather.sets().len().to_string()),
                    ("Parameter", weather.parameter().to_string()),
                    ("Weight", format!("{:.3}", weather.weight())),
                ]
                .map(|(label, value)| (label.to_owned(), value))
                .into()
            }
            At::Set(weather, index) => {
                let set = self.set(weather, index);
                [
                    ("Animates", set.name().unwrap_or("Unnamed").to_owned()),
                    ("Kind", set.kind().to_string()),
                    ("Keyframes", set.keyframes().len().to_string()),
                ]
                .map(|(label, value)| (label.to_owned(), value))
                .into()
            }
            At::Keyframe(weather, set, index) => {
                let keyframe = self.keyframe(weather, set, index);
                std::iter::once(("Time".to_owned(), clock(keyframe.time())))
                    .chain(scalars(keyframe).map(|(name, value)| (titled(name), shown(value))))
                    .collect()
            }
        }
    }

    /// The files the selected row names, which for a weather are the two it may carry itself.
    fn assets(&self, at: At) -> Vec<&str> {
        match at {
            At::Weather(index) => self
                .weather(index)
                .paths()
                .iter()
                .map(String::as_str)
                .collect(),
            At::Set(..) => Vec::new(),
            At::Keyframe(weather, set, index) => {
                self.keyframe(weather, set, index).paths().collect()
            }
        }
        .into_iter()
        .filter(|path| !path.is_empty())
        .collect()
    }

    /// The colors the selected row reaches, under the names the format files them by.
    fn colors(&self, at: At) -> Vec<(&'static str, envs::Colour)> {
        let At::Keyframe(weather, set, index) = at else {
            return Vec::new();
        };
        self.keyframe(weather, set, index)
            .fields()
            .iter()
            .filter_map(|(name, value)| match value {
                envs::Value::Colour(colour) => Some((*name, *colour)),
                _ => None,
            })
            .collect()
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
                            ui.label(RichText::new(&label).weak());
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
                        .num_columns(3)
                        .striped(true)
                        .show(ui, |ui| {
                            for (name, colour) in colors {
                                chip(ui, color(colour));
                                ui.label(RichText::new(titled(name)).weak());
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
