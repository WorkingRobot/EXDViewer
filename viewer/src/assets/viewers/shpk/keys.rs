//! The keys a package switches between, and what its variants say about them.

use egui::RichText;
use ironworks::file::shpk::{self, Stage};

use super::super::shader::named;
use super::super::{hashed, heading, labelled, section};

/// A key as a node names it: where its value sits in a node's tuple, and what it is called.
pub struct KeyColumn {
    pub name: String,
    pub id: u32,
    pub default: u32,
    /// The values this key takes, shortened against its own name.
    pub values: Vec<(String, u32)>,
}

pub struct KeyRow {
    name: String,
    id: u32,
    value_id: u32,
    /// Every value the package's variants give this key. These are the conditions its source was
    /// compiled under, so the list is the switch the key really is.
    values: Vec<(String, u32)>,
}

/// A pass the package draws in, and the shaders it runs.
///
/// A pass says what the game is doing when it reaches a shader, and it is the coarsest thing that
/// tells one variant from another: a package's shaders divide cleanly along it.
pub struct PassRow {
    pub name: String,
    pub id: u32,
    pub shaders: Vec<usize>,
}

/// One combination of key values, and the shaders it picks. Built while decoding to work out what
/// each shader was compiled for, and not kept afterwards.
struct Variant {
    values: Vec<u32>,
    /// Pass id, and the flat shader index for each stage that runs in it.
    passes: Vec<(u32, Vec<usize>)>,
}

/// Everything the package's key tables and selector nodes amount to.
pub struct Keys {
    /// Keys under the heading each group is drawn with.
    groups: Vec<(&'static str, Vec<KeyRow>)>,
    /// The package's keys in the order a node lists their values.
    pub columns: Vec<KeyColumn>,
    pub passes: Vec<PassRow>,
    /// Per shader, how many combinations pick it and whether they cover every pairing of the keys
    /// they leave open. Where they do, listing them says nothing the conditions already shown do not.
    selected: Vec<(usize, bool)>,
    /// Per shader, the key values every variant selecting it agrees on. A shader is one compilation
    /// of the package's source, and these are the conditions it was compiled under.
    pub defines: Vec<Vec<(usize, u32)>>,
}

/// A value under its key with the key's own name taken off the front: `ApplyDitherClipOff` below
/// `ApplyDitherClip` is just `Off`, which is the part that tells one variant from another.
fn shorten(key: &str, value: &str) -> String {
    match value
        .strip_prefix(key)
        .map(|rest| rest.trim_start_matches('_'))
    {
        Some(rest) if !rest.is_empty() => rest.to_owned(),
        _ => value.strip_prefix("Val").unwrap_or(value).to_owned(),
    }
}

pub fn read(package: &shpk::ShaderPackage) -> Keys {
    // Which values each key actually takes, gathered from the variants rather than guessed at.
    let width = package.nodes().first().map_or(0, |node| node.keys().len());
    let mut taken: Vec<Vec<u32>> = vec![Vec::new(); width];
    for node in package.nodes() {
        for (held, value) in taken.iter_mut().zip(node.keys()) {
            if !held.contains(value) {
                held.push(*value);
            }
        }
    }

    let mut column = 0;
    let groups: Vec<(&'static str, Vec<KeyRow>)> = [
        ("System", package.system_keys()),
        ("Scene", package.scene_keys()),
        ("Material", package.material_keys()),
    ]
    .into_iter()
    .map(|(group, list)| {
        let rows = list
            .iter()
            .map(|key| {
                // The declared default is not always a value any variant picks, and a value every
                // variant picks is not always the default. Both belong in the list or one of them
                // has nowhere to appear.
                let mut held: Vec<u32> = vec![key.default_value()];
                held.extend(
                    taken
                        .get(column)
                        .into_iter()
                        .flatten()
                        .filter(|value| **value != key.default_value()),
                );
                let values = held
                    .into_iter()
                    .map(|value| (named(value), value))
                    .collect();
                column += 1;
                KeyRow {
                    name: named(key.id()),
                    id: key.id(),
                    value_id: key.default_value(),
                    values,
                }
            })
            .collect();
        (group, rows)
    })
    .collect();

    // A node lists a value for each key the package declares, then the two subview keys.
    let mut columns: Vec<KeyColumn> = package
        .system_keys()
        .iter()
        .chain(package.scene_keys())
        .chain(package.material_keys())
        .map(|key| KeyColumn {
            name: named(key.id()),
            id: key.id(),
            default: key.default_value(),
            values: Vec::new(),
        })
        .chain(
            package
                .subview_defaults()
                .into_iter()
                .enumerate()
                .map(|(index, default)| KeyColumn {
                    name: format!("Subview {}", index + 1),
                    id: 0,
                    default,
                    values: Vec::new(),
                }),
        )
        .collect();
    for column in &mut columns {
        let name = named(column.default);
        column
            .values
            .push((shorten(&column.name, &name), column.default));
    }
    for node in package.nodes() {
        for (column, value) in columns.iter_mut().zip(node.keys()) {
            if !column.values.iter().any(|(_, held)| held == value) {
                let name = named(*value);
                column.values.push((shorten(&column.name, &name), *value));
            }
        }
    }
    for column in &mut columns {
        column.values.sort_by(|left, right| left.0.cmp(&right.0));
    }

    // A pass names a shader by its place within its own stage, so the stages have to be laid end to
    // end the way the package stores them to reach the flat list drawn above.
    let mut offsets = [None; 5];
    for stage in [
        Stage::Vertex,
        Stage::Pixel,
        Stage::Hull,
        Stage::Domain,
        Stage::Geometry,
    ] {
        offsets[stage as usize] = package
            .shaders()
            .iter()
            .position(|shader| shader.stage() == stage);
    }

    let variants: Vec<Variant> = package
        .nodes()
        .iter()
        .map(|node| Variant {
            values: node.keys().to_vec(),
            passes: node
                .passes()
                .iter()
                .map(|pass| {
                    let stages = pass
                        .stages()
                        .into_iter()
                        .enumerate()
                        .filter(|(_, index)| *index != shpk::NONE)
                        .filter_map(|(stage, index)| Some(offsets[stage]? + index as usize))
                        .collect();
                    (pass.id(), stages)
                })
                .collect(),
        })
        .collect();

    // A shader is picked by every variant whose keys agree with it; where they all agree on one
    // value, that value is what the shader was compiled for.
    let mut agreed: Vec<Option<Vec<Option<u32>>>> = vec![None; package.shaders().len()];
    for variant in &variants {
        for shader in variant.passes.iter().flat_map(|(_, stages)| stages) {
            let Some(slot) = agreed.get_mut(*shader) else {
                continue;
            };
            match slot {
                None => *slot = Some(variant.values.iter().copied().map(Some).collect()),
                Some(seen) => {
                    for (held, value) in seen.iter_mut().zip(&variant.values) {
                        if *held != Some(*value) {
                            *held = None;
                        }
                    }
                }
            }
        }
    }
    let defines: Vec<Vec<(usize, u32)>> = agreed
        .into_iter()
        .map(|seen| {
            seen.unwrap_or_default()
                .into_iter()
                .enumerate()
                .filter_map(|(index, value)| Some((index, value?)))
                .collect()
        })
        .collect();

    let mut passes: Vec<PassRow> = Vec::new();
    for variant in &variants {
        for (id, stages) in &variant.passes {
            let row = match passes.iter_mut().find(|row| row.id == *id) {
                Some(row) => row,
                None => {
                    passes.push(PassRow {
                        name: named(*id),
                        id: *id,
                        shaders: Vec::new(),
                    });
                    passes.last_mut().expect("just pushed")
                }
            };
            for shader in stages {
                if !row.shaders.contains(shader) {
                    row.shaders.push(*shader);
                }
            }
        }
    }
    for row in &mut passes {
        row.shaders.sort_unstable();
    }

    // Which combinations reach each shader, and whether they are simply every pairing of the keys
    // they leave open. Almost always they are, and then the combinations themselves are no more than
    // the conditions restated.
    let mut picks: Vec<Vec<usize>> = vec![Vec::new(); package.shaders().len()];
    for (at, variant) in variants.iter().enumerate() {
        for shader in variant.passes.iter().flat_map(|(_, stages)| stages) {
            if let Some(slot) = picks.get_mut(*shader)
                && slot.last() != Some(&at)
            {
                slot.push(at);
            }
        }
    }
    let selected: Vec<(usize, bool)> = picks
        .iter()
        .map(|nodes| {
            let mut spread: Vec<Vec<u32>> = vec![Vec::new(); columns.len()];
            let mut tuples: Vec<&[u32]> = Vec::with_capacity(nodes.len());
            for at in nodes {
                let values = variants[*at].values.as_slice();
                tuples.push(values);
                for (held, value) in spread.iter_mut().zip(values) {
                    if !held.contains(value) {
                        held.push(*value);
                    }
                }
            }
            tuples.sort_unstable();
            tuples.dedup();
            let product = spread
                .iter()
                .try_fold(1u128, |total, held| total.checked_mul(held.len() as u128));
            (tuples.len(), product == Some(tuples.len() as u128))
        })
        .collect();

    Keys {
        groups,
        columns,
        passes,
        selected,
        defines,
    }
}

impl Keys {
    pub fn any(&self) -> bool {
        self.groups.iter().any(|(_, rows)| !rows.is_empty())
    }

    /// The value a shader was compiled with for one key, shortened, or nothing where its variants do
    /// not agree on one.
    pub fn condition(&self, shader: usize, column: usize) -> Option<&str> {
        let value = self
            .defines
            .get(shader)?
            .iter()
            .find(|(held, _)| *held == column)
            .map(|(_, value)| *value)?;
        let key = self.columns.get(column)?;
        key.values
            .iter()
            .find(|(_, held)| *held == value)
            .map(|(short, _)| short.as_str())
    }

    /// The keys worth putting on a list row: the ones whose value actually differs somewhere in the
    /// list as it currently stands, and how wide each needs to be. A key every listed shader agrees
    /// on separates none of them, and showing it would only push the ones that do off the end.
    pub fn discriminating(&self, listed: &[usize]) -> Vec<(usize, usize)> {
        (0..self.columns.len())
            .filter_map(|column| {
                let mut seen: Option<Option<&str>> = None;
                let mut width = 0;
                let mut varies = false;
                for shader in listed {
                    let here = self.condition(*shader, column);
                    width = width.max(here.map_or(1, str::len));
                    match seen {
                        None => seen = Some(here),
                        Some(first) if first != here => varies = true,
                        Some(_) => {}
                    }
                }
                varies.then_some((column, width))
            })
            .collect()
    }

    /// The key values a shader was compiled under, as its own variants agree on them.
    ///
    /// Every key is listed whether or not it decided anything, always in the same order, so that
    /// moving between two shaders leaves each key where it was and what differs is simply the rows
    /// that changed. A value the package would have taken anyway is dimmed; one a variant set is not.
    pub fn defines_ui(&self, ui: &mut egui::Ui, shader: usize) {
        let Some(defines) = self.defines.get(shader) else {
            return;
        };
        if defines.is_empty() {
            return;
        }
        section(ui, "Compiled for");
        if let Some((count, complete)) = self.selected.get(shader) {
            let note = match complete {
                true => format!("{count} combinations"),
                // The combinations reaching this shader are not every pairing of the keys they leave
                // open, so some pairings never happen and this cannot say which.
                false => format!("{count} combinations, not all pairings"),
            };
            ui.label(RichText::new(note).weak().small());
            ui.add_space(2.0);
        }
        egui::Grid::new("shpk_defines_grid")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                for (column, key) in self.columns.iter().enumerate() {
                    let held = defines
                        .iter()
                        .find(|(at, _)| *at == column)
                        .map(|(_, value)| *value);
                    hashed(ui, "Key", &key.name, key.id, held.is_none());
                    match held {
                        Some(value) => {
                            let full = named(value);
                            labelled(
                                ui,
                                &format!("{} value", key.name),
                                &full,
                                &shorten(&key.name, &full),
                                value,
                                value == key.default,
                            );
                        }
                        // The variants picking this shader disagree here, so the key decided nothing
                        // about it.
                        None => {
                            ui.label(RichText::new("either way").weak().small());
                        }
                    }
                    ui.end_row();
                }
            });
    }

    /// The package's keys, each under the values it switches between.
    pub fn ui(&self, ui: &mut egui::Ui) {
        for (group, rows) in &self.groups {
            if rows.is_empty() {
                continue;
            }
            heading(ui, group);
            for key in rows {
                hashed(ui, "Key", &key.name, key.id, false);
                // Underneath the key rather than beside it: a row of values is as wide as the key
                // has values, and in a column it would set a floor on how narrow the panel could be.
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    for (name, id) in &key.values {
                        // The default is dimmed here as it is wherever a value appears: it is what
                        // the key is worth unless a variant says otherwise, so the others are the
                        // ones worth reading.
                        labelled(
                            ui,
                            "Value",
                            name,
                            &shorten(&key.name, name),
                            *id,
                            *id == key.value_id,
                        );
                    }
                });
            }
            ui.add_space(4.0);
        }
    }
}

#[cfg(test)]
mod test {
    use super::shorten;

    /// The package names a value after the key it belongs to, so the key's own name carries nothing
    /// and comes off.
    #[test]
    fn a_value_drops_the_name_of_its_key() {
        assert_eq!(shorten("ApplyDitherClip", "ApplyDitherClipOff"), "Off");
        assert_eq!(shorten("TransformView", "TransformViewSkin"), "Skin");
        assert_eq!(shorten("GetMaterialValue", "GetMaterialValueFace"), "Face");
    }

    /// Some are joined with an underscore, which is no more use than the name was.
    #[test]
    fn a_separator_comes_off_with_it() {
        assert_eq!(
            shorten(
                "CalculateInstancingPosition",
                "CalculateInstancingPosition_On"
            ),
            "On"
        );
    }

    /// A value named after something else keeps what it has; dropping to nothing would be worse
    /// than a long label.
    #[test]
    fn a_value_named_otherwise_is_left_alone() {
        assert_eq!(shorten("Subview 2", "SUB_VIEW_MAIN"), "SUB_VIEW_MAIN");
        assert_eq!(
            shorten("ApplyDitherClip", "ApplyDitherClip"),
            "ApplyDitherClip"
        );
        assert_eq!(
            shorten("CategoryVertexColorMode", "0x1234ABCD"),
            "0x1234ABCD"
        );
    }
}
