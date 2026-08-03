//! What a zone annotates its layers with: how much sky reaches an instance, the box a light is
//! clipped against, and the settings the zone takes underwater.
//!
//! `.svb` and `.lcb` key every entry by an instance one of the zone's layer groups placed. `.uwb`
//! shares their container and nothing else: its groups carry twenty values the format does not name.

use std::collections::HashSet;
use std::io::Cursor;

use anyhow::Result;
use egui::{RichText, ScrollArea};
use ironworks::file::{File, lcb, svb, uwb};

use super::super::{Preview, facts, line, link, section, table};
use super::axes;
use crate::utils::file_name;

/// Prepended to the columns below where a file holds more than one group, since a single group has
/// nothing to tell its rows apart from.
const GROUP: (&str, usize) = ("Group", 5);

const VISIBILITY: [(&str, usize); 3] = [("Instance", 10), ("Member", 9), ("Visibility", 10)];

const CLIP: [(&str, usize); 4] = [("Instance", 10), ("Member", 9), ("Min", 26), ("Max", 26)];

const UNDERWATER: [(&str, usize); 2] = [("Field", 5), ("Value", 12)];

/// The file the table was read from, which it keeps rather than copying every entry out.
enum Source {
    Sky(svb::SkyVisibility),
    Clip(lcb::ClipBoxes),
    Underwater(uwb::Underwater),
}

impl Source {
    /// How many rows each group contributes.
    fn lengths(&self) -> Vec<usize> {
        match self {
            Self::Sky(file) => file.groups().iter().map(|g| g.entries().len()).collect(),
            Self::Clip(file) => file.groups().iter().map(|g| g.entries().len()).collect(),
            Self::Underwater(file) => file.groups().iter().map(|g| g.unknown().len()).collect(),
        }
    }

    fn cells(&self, group: usize, index: usize) -> Vec<String> {
        match self {
            Self::Sky(file) => {
                let entry = file.groups()[group].entries()[index];
                vec![
                    entry.instance().to_string(),
                    member(entry.members()),
                    format!("{:.3}", entry.visibility()),
                ]
            }
            Self::Clip(file) => {
                let entry = file.groups()[group].entries()[index];
                vec![
                    entry.instance().to_string(),
                    member(entry.members()),
                    axes(entry.min()),
                    axes(entry.max()),
                ]
            }
            Self::Underwater(file) => vec![
                index.to_string(),
                format!("{:.3}", file.groups()[group].unknown()[index]),
            ],
        }
    }

    /// Instances the file keys entries by, each counted once.
    fn instances(&self) -> Option<usize> {
        let keys: HashSet<u32> = match self {
            Self::Sky(file) => file
                .groups()
                .iter()
                .flat_map(|group| group.entries())
                .map(|entry| entry.instance())
                .collect(),
            Self::Clip(file) => file
                .groups()
                .iter()
                .flat_map(|group| group.entries())
                .map(|entry| entry.instance())
                .collect(),
            Self::Underwater(_) => return None,
        };
        Some(keys.len())
    }
}

/// Reaches the part of an instance an entry applies to, an index per level of shared group it sits
/// under. The format fills them from the front, so the run before the first zero is the whole path.
fn member(members: [u8; 4]) -> String {
    members
        .iter()
        .take_while(|&&index| index != 0)
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

pub struct Rendered {
    identity: Vec<(&'static str, String)>,
    /// The zone's scene, which names the layer groups the instances were placed in.
    level: String,
    section: &'static str,
    columns: Vec<(&'static str, usize)>,
    source: Source,
    /// Group and index of every row.
    rows: Vec<(usize, usize)>,
}

fn rendered(
    path: &str,
    section: &'static str,
    columns: &[(&'static str, usize)],
    source: Source,
) -> Preview {
    let lengths = source.lengths();
    let rows = lengths
        .iter()
        .enumerate()
        .flat_map(|(group, count)| (0..*count).map(move |index| (group, index)))
        .collect::<Vec<_>>();
    let columns = match lengths.len() > 1 {
        true => std::iter::once(GROUP)
            .chain(columns.iter().copied())
            .collect(),
        false => columns.to_vec(),
    };

    let mut identity = vec![("Groups", lengths.len().to_string())];
    if let Some(instances) = source.instances() {
        identity.push(("Entries", rows.len().to_string()));
        identity.push(("Instances", instances.to_string()));
    }

    log::info!("assets/zone: {path} {} rows", rows.len());

    Preview::Zone(Box::new(Rendered {
        identity,
        level: format!(
            "{}.lvb",
            path.rsplit_once('.').map_or(path, |(stem, _)| stem)
        ),
        section,
        columns,
        source,
        rows,
    }))
}

pub fn sky_visibility(path: &str, bytes: &[u8]) -> Result<Preview> {
    let file = svb::SkyVisibility::read(Cursor::new(bytes.to_vec()))?;
    Ok(rendered(path, "Visibility", &VISIBILITY, Source::Sky(file)))
}

pub fn clip_boxes(path: &str, bytes: &[u8]) -> Result<Preview> {
    let file = lcb::ClipBoxes::read(Cursor::new(bytes.to_vec()))?;
    Ok(rendered(path, "Clip boxes", &CLIP, Source::Clip(file)))
}

pub fn underwater(path: &str, bytes: &[u8]) -> Result<Preview> {
    let file = uwb::Underwater::read(Cursor::new(bytes.to_vec()))?;
    Ok(rendered(
        path,
        "Values",
        &UNDERWATER,
        Source::Underwater(file),
    ))
}

pub fn ui(ui: &mut egui::Ui, file: &Rendered) -> Option<String> {
    let mut follow = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Level").weak());
        if link(ui, file_name(&file.level), &file.level) {
            follow = Some(file.level.clone());
        }
    });
    ui.add_space(4.0);

    section(ui, file.section);
    let grouped = file.columns.first() == Some(&GROUP);
    table(ui, &file.columns, file.rows.len(), |ui, index| {
        let (group, entry) = file.rows[index];
        let mut cells = file.source.cells(group, entry);
        if grouped {
            cells.insert(0, group.to_string());
        }
        ui.label(RichText::new(line(&file.columns, cells.iter().map(String::as_str))).monospace());
    });
    follow
}

impl Rendered {
    pub fn details_ui(&self, ui: &mut egui::Ui) {
        ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| facts(ui, "zone_identity", &self.identity));
    }
}
