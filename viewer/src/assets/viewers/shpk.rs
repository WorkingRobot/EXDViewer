//! `.shpk` shader packages: the compiled shaders a material names, and the resources, parameters
//! and keys they are driven by.

use anyhow::Result;
use dxbc::chunks::ChunkData;
use egui::{RichText, ScrollArea, Sense, vec2};
use ironworks::file::shpk::{self, Stage};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use hlsl::layout::{Member, members};
use shaders::names;

use super::{Preview, facts, section};
use crate::assets::Bytes;

/// Components in a vec4 register, which is the unit a constant buffer is addressed in.
const COMPONENTS: usize = 4;

/// An id as its name where one is known. Resources carry their own, so this is only for the ids the
/// file identifies by hash alone.
fn named(id: u32) -> String {
    names::resolve(id).map_or_else(|| format!("{id:#010X}"), str::to_owned)
}

struct ResourceRow {
    name: String,
    id: u32,
    kind: &'static str,
    slot: u16,
    size: String,
    /// What the shader declares inside a constant buffer, recovered from the bytecode's reflection.
    /// Empty for everything else, and for a buffer that is one bare array.
    members: Vec<Member>,
}

struct ParamRow {
    name: String,
    id: u32,
    offset: u16,
    size: u16,
}

/// One float of the parameter buffer, and which parameter owns it.
#[derive(Clone, Copy)]
struct Component {
    param: usize,
    /// False where the component continues a parameter that began in an earlier one.
    start: bool,
}

/// A key as a node names it: where its value sits in a node's tuple, and what it is called.
struct KeyColumn {
    name: String,
    id: u32,
    default: u32,
    /// The values this key takes, shortened against its own name.
    values: Vec<(String, u32)>,
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

/// A pass the package draws in, and the shaders it runs.
///
/// A pass says what the game is doing when it reaches a shader, and it is the coarsest thing that
/// tells one variant from another: a package's shaders divide cleanly along it.
struct PassRow {
    name: String,
    id: u32,
    shaders: Vec<usize>,
}

/// One combination of key values, and the shaders it picks. Built while decoding to work out what
/// each shader was compiled for, and not kept afterwards.
struct Variant {
    id: u32,
    values: Vec<u32>,
    /// Pass id, and the flat shader index for each stage that runs in it.
    passes: Vec<(u32, Vec<(&'static str, usize)>)>,
}

struct KeyRow {
    name: String,
    id: u32,
    value: String,
    value_id: u32,
    /// Every value the package's variants give this key. These are the conditions its source was
    /// compiled under, so the list is the switch the key really is.
    values: Vec<(String, u32)>,
}

/// One shader in the list. The bytecode itself stays in the file; this is only where to find it.
struct ShaderRow {
    stage: &'static str,
    blob: std::ops::Range<usize>,
    /// What the shader binds, which is what turns a bare `cb0` in the disassembly into a name. Slots
    /// are per-shader: the same buffer sits at different ones in different shaders of a package.
    bindings: Vec<Binding>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Register {
    Constant,
    Sampler,
    Texture,
}

#[derive(Clone, Copy)]
struct Binding {
    register: Register,
    slot: u16,
    id: u32,
}

/// A shader package, decoded and ready to draw.
pub struct Rendered {
    identity: Vec<(&'static str, String)>,
    /// Resources under the heading each group is drawn with.
    resources: Vec<(&'static str, Vec<ResourceRow>)>,
    params: Vec<ParamRow>,
    /// The parameter buffer as the registers a shader addresses it by.
    registers: Vec<[Option<Component>; COMPONENTS]>,
    /// The shader's defaults for that buffer, indexed by the same float.
    defaults: Vec<f32>,
    keys: Vec<(&'static str, Vec<KeyRow>)>,
    /// The package's keys in the order a node lists their values.
    columns: Vec<KeyColumn>,
    passes: Vec<PassRow>,
    /// Per shader, how many combinations pick it and whether they cover every pairing of the keys
    /// they leave open. Where they do, listing them says nothing the conditions above do not.
    selected: Vec<(usize, bool)>,
    /// Per shader, the key values every variant selecting it agrees on. A shader is one compilation
    /// of the package's source, and these are the conditions it was compiled under.
    defines: Vec<Vec<(usize, u32)>>,
    aliases: usize,
    /// Stage, how many shaders it holds, and how much bytecode they take.
    stages: Vec<(&'static str, usize, usize)>,
    shaders: Vec<ShaderRow>,
    /// Every resource name in the package, by id, for naming a register in a disassembly.
    names: HashMap<u32, String>,
    /// Constant buffer fields by buffer id, which the same disassembly resolves against.
    layouts: HashMap<u32, Vec<Member>>,
    /// The buffer a material fills, whose fields the reflection does not name; its registers are
    /// read off the grid above instead.
    material_buffer: u32,
    /// Which stage is filtered to and which shader is picked, kept per file the way the icon sheet
    /// keeps its controller.
    state: egui::Id,
}

/// Constant buffer layouts, by the resource id that names the buffer.
///
/// The layouts live in the compiled bytecode's reflection rather than in the package tables, and
/// every shader that binds a buffer describes it identically. So rather than sweeping thousands of
/// blobs, this walks the shader list once and takes only those that bind a buffer nothing before
/// them did: enough to cover every declared buffer, in around ten blobs even for the largest
/// package.
fn layouts(package: &shpk::ShaderPackage, bytes: &[u8]) -> HashMap<u32, Vec<Member>> {
    let wanted: HashSet<u32> = package.constants().iter().map(|c| c.id()).collect();
    let mut seen = HashSet::new();
    let mut found = HashMap::new();

    for shader in package.shaders() {
        if seen.len() == wanted.len() {
            break;
        }
        let adds = shader
            .resources()
            .iter()
            .any(|resource| wanted.contains(&resource.id()) && seen.insert(resource.id()));
        if !adds {
            continue;
        }

        let start = package.blobs_offset() + usize::try_from(shader.blob_offset()).unwrap_or(0);
        let end = start.saturating_add(usize::try_from(shader.blob_size()).unwrap_or(0));
        let Some(blob) = bytes.get(start..end) else {
            continue;
        };

        for container in dxbc::scan_dxbc(blob) {
            for chunk in &container.chunks {
                let ChunkData::Rdef(reflection) = chunk.parse() else {
                    continue;
                };
                for buffer in &reflection.constant_buffers {
                    // Buffers are keyed by the same crc the package's resource table uses.
                    let id = names::hash(buffer.name.as_bytes());
                    found.entry(id).or_insert_with(|| members(buffer));
                }
            }
        }
    }
    found
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    // Parsed off the caller's bytes rather than an owned copy
    let package = shpk::ShaderPackage::parse(bytes)?;
    let layouts = layouts(&package, bytes);

    let mut stages: Vec<(&'static str, usize, usize)> = Vec::new();
    let mut shaders = Vec::with_capacity(package.shaders().len());
    for shader in package.shaders() {
        let stage = match shader.stage() {
            Stage::Vertex => "Vertex",
            Stage::Pixel => "Pixel",
            Stage::Hull => "Hull",
            Stage::Domain => "Domain",
            Stage::Geometry => "Geometry",
        };
        let size = usize::try_from(shader.blob_size()).unwrap_or(0);
        let start = package.blobs_offset() + usize::try_from(shader.blob_offset()).unwrap_or(0);
        let bindings = [
            (Register::Constant, shader.constants()),
            (Register::Sampler, shader.samplers()),
            (Register::Texture, shader.textures()),
        ]
        .into_iter()
        .flat_map(|(register, list)| {
            list.iter().map(move |resource| Binding {
                register,
                slot: resource.slot(),
                id: resource.id(),
            })
        })
        .collect();
        shaders.push(ShaderRow {
            stage,
            blob: start..start.saturating_add(size),
            bindings,
        });
        match stages.iter_mut().find(|(name, _, _)| *name == stage) {
            Some((_, count, bytes)) => {
                *count += 1;
                *bytes += size;
            }
            None => stages.push((stage, 1, size)),
        }
    }

    let resources = [
        ("Constant buffers", package.constants(), "Constant buffer"),
        ("Samplers", package.samplers(), "Sampler"),
        ("Textures", package.textures(), "Texture"),
        ("Unordered access views", package.uavs(), "UAV"),
    ]
    .into_iter()
    .map(|(heading, list, kind)| {
        let rows = list
            .iter()
            .map(|resource| ResourceRow {
                // The package names its own resources, which is the one thing it knows that the
                // curated hash table does not.
                name: package
                    .name(resource)
                    .map_or_else(|| named(resource.id()), str::to_owned),
                id: resource.id(),
                kind,
                slot: resource.slot(),
                // A constant buffer states its size in registers; everything else binds one slot.
                size: match kind {
                    "Constant buffer" => format!(
                        "{} registers ({} B)",
                        resource.size(),
                        u32::from(resource.size()) * 16
                    ),
                    _ => resource.size().to_string(),
                },
                members: layouts.get(&resource.id()).cloned().unwrap_or_default(),
            })
            .collect();
        (heading, rows)
    })
    .collect();

    let params: Vec<ParamRow> = package
        .material_params()
        .iter()
        .map(|param| ParamRow {
            name: named(param.id()),
            id: param.id(),
            offset: param.byte_offset(),
            size: param.byte_size(),
        })
        .collect();

    let floats = usize::try_from(package.param_buffer_size()).unwrap_or(0) / 4;
    let mut registers = vec![[None; COMPONENTS]; floats.div_ceil(COMPONENTS)];
    for (index, param) in package.material_params().iter().enumerate() {
        let first = usize::from(param.byte_offset()) / 4;
        for step in 0..usize::from(param.byte_size()) / 4 {
            let at = first + step;
            if let Some(register) = registers.get_mut(at / COMPONENTS) {
                register[at % COMPONENTS] = Some(Component {
                    param: index,
                    start: step == 0,
                });
            }
        }
    }
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
    let keys: Vec<(&'static str, Vec<KeyRow>)> = [
        ("System", package.system_keys()),
        ("Scene", package.scene_keys()),
        ("Material", package.material_keys()),
    ]
    .into_iter()
    .map(|(heading, list)| {
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
                    value: named(key.default_value()),
                    value_id: key.default_value(),
                    values,
                }
            })
            .collect();
        (heading, rows)
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
    let stage_names = ["Vertex", "Pixel", "Hull", "Domain", "Geometry"];

    let variants: Vec<Variant> = package
        .nodes()
        .iter()
        .map(|node| Variant {
            id: node.id(),
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
                        .filter_map(|(stage, index)| {
                            Some((stage_names[stage], offsets[stage]? + index as usize))
                        })
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
        for (_, stages) in &variant.passes {
            for (_, shader) in stages {
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
            for (_, shader) in stages {
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
        for (_, stages) in &variant.passes {
            for (_, shader) in stages {
                if let Some(slot) = picks.get_mut(*shader)
                    && slot.last() != Some(&at)
                {
                    slot.push(at);
                }
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

    let (nodes, aliases) = (package.nodes().len(), package.aliases().len());
    let [subview_one, subview_two] = package.subview_defaults();
    let identity = vec![
        ("Version", format!("{:#06X}", package.version())),
        (
            "DirectX",
            match package.directx() {
                shpk::DirectX::Dx9 => "9".to_owned(),
                shpk::DirectX::Dx11 => "11".to_owned(),
                shpk::DirectX::Unknown(tag) => String::from_utf8_lossy(&tag).into_owned(),
            },
        ),
        ("Shaders", package.shaders().len().to_string()),
        ("Bytecode", Bytes(package.bytecode_size()).to_string()),
        (
            "Parameter buffer",
            format!(
                "{} registers ({} B)",
                registers.len(),
                package.param_buffer_size()
            ),
        ),
        ("Selector nodes", nodes.to_string()),
        ("Aliases", aliases.to_string()),
        ("Subview 1", named(subview_one)),
        ("Subview 2", named(subview_two)),
    ];

    Ok(Preview::Shpk(Box::new(Rendered {
        identity,
        resources,
        params,
        registers,
        defaults: package.param_defaults().to_vec(),
        keys,
        columns,
        passes,
        selected,
        defines,
        aliases,
        stages,
        shaders,
        names: package
            .shaders()
            .iter()
            .flat_map(shpk::Shader::resources)
            .chain(package.constants())
            .chain(package.samplers())
            .chain(package.textures())
            .chain(package.uavs())
            .filter_map(|resource| Some((resource.id(), package.name(resource)?.to_owned())))
            .collect(),
        layouts,
        material_buffer: names::hash(b"g_MaterialParameter"),
        state: egui::Id::new(("shpk shader", path)),
    })))
}

/// A clickable id, with the hover and copy menu every crc-named value in the browser gets.
fn hashed(ui: &mut egui::Ui, kind: &str, name: &str, id: u32, dim: bool) {
    labelled(ui, kind, name, name, id, dim);
}

/// The same, drawn under a shorter label. Hovering still gives the whole name and its hash, so
/// nothing is lost by not spelling out a key's own name in every one of its values.
fn labelled(ui: &mut egui::Ui, kind: &str, name: &str, shown: &str, id: u32, dim: bool) {
    let text = RichText::new(shown).monospace();
    let response = ui.add(
        egui::Label::new(match dim {
            true => text.weak(),
            false => text,
        })
        .sense(Sense::click()),
    );
    crate::assets::crc_context(&response, kind, name, id);
}

/// The weak header row above a striped table.
fn headers(ui: &mut egui::Ui, names: &[&str]) {
    for name in names {
        ui.label(RichText::new(*name).weak().small());
    }
    ui.allocate_space(vec2(ui.available_width(), 0.0));
    ui.end_row();
}

/// A group heading inside a section, for the tables that come in several kinds.
fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).weak());
    ui.add_space(4.0);
}

pub fn ui(ui: &mut egui::Ui, package: &Rendered, bytes: &[u8]) {
    // No scroll area around this: the list and the code each carry their own, and an outer one
    // leaves the code unable to tell how much of the panel is left for it.
    shaders_ui(ui, package, bytes);
}

/// Everything about the package that is not a shader. It sits beside the code rather than above it,
/// where it would push the thing being read off the screen.
fn metadata_ui(ui: &mut egui::Ui, package: &Rendered) {
    if !package.registers.is_empty() {
        section(ui, "Material parameters");
        // Four columns of long names overflow a narrow panel, and only this table does.
        ScrollArea::horizontal()
            .id_salt("shpk_params_scroll")
            .show(ui, |ui| {
                // Or every name wraps to the width of a narrow panel instead of the table simply
                // being wider than one.
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                params_ui(ui, package);
            });
        ui.add_space(8.0);
        ui.separator();
    }

    if package.resources.iter().any(|(_, rows)| !rows.is_empty()) {
        section(ui, "Resources");
        ScrollArea::horizontal()
            .id_salt("shpk_resources_scroll")
            .show(ui, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                for (group, rows) in &package.resources {
                    if rows.is_empty() {
                        continue;
                    }
                    heading(ui, group);
                    egui::Grid::new(*group)
                        .num_columns(3)
                        .striped(true)
                        .show(ui, |ui| {
                            headers(ui, &["Name", "Slot", "Size"]);
                            for resource in rows {
                                match resource.members.is_empty() {
                                    true => hashed(
                                        ui,
                                        resource.kind,
                                        &resource.name,
                                        resource.id,
                                        false,
                                    ),
                                    false => members_ui(ui, resource),
                                }
                                ui.label(RichText::new(resource.slot.to_string()).monospace());
                                ui.label(RichText::new(&resource.size).monospace());
                                ui.end_row();
                            }
                        });
                }
            });
        ui.add_space(8.0);
        ui.separator();
    }

    if package.keys.iter().any(|(_, rows)| !rows.is_empty()) {
        section(ui, "Keys");
        ScrollArea::horizontal()
            .id_salt("shpk_keys_scroll")
            .show(ui, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                keys_ui(ui, package);
            });
    }
}

/// The package's keys, each under the values it switches between.
fn keys_ui(ui: &mut egui::Ui, package: &Rendered) {
    for (group, rows) in &package.keys {
        if rows.is_empty() {
            continue;
        }
        heading(ui, group);
        for key in rows {
            hashed(ui, "Key", &key.name, key.id, false);
            // Underneath the key rather than beside it: a row of values is as wide as the key has
            // values, and in a column it would set a floor on how narrow the panel could be.
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                for (name, id) in &key.values {
                    // The default is dimmed here as it is wherever a value appears: it is what the
                    // key is worth unless a variant says otherwise, so the others are the ones
                    // worth reading.
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

/// The value a shader was compiled with for one key, shortened, or nothing where its variants do
/// not agree on one.
fn condition(package: &Rendered, shader: usize, column: usize) -> Option<&str> {
    let value = package
        .defines
        .get(shader)?
        .iter()
        .find(|(held, _)| *held == column)
        .map(|(_, value)| *value)?;
    let key = package.columns.get(column)?;
    key.values
        .iter()
        .find(|(_, held)| *held == value)
        .map(|(short, _)| short.as_str())
}

/// What a list row cannot hold: where the shader sits, what it binds, which passes reach it, and
/// every condition it was compiled under rather than only the ones that vary.
fn shader_tooltip(ui: &mut egui::Ui, package: &Rendered, index: usize) {
    let Some(shader) = package.shaders.get(index) else {
        return;
    };
    ui.label(
        RichText::new(format!("Shader #{index}  {}", shader.stage))
            .monospace()
            .strong(),
    );
    ui.label(
        RichText::new(format!(
            "{} at {:#X}",
            Bytes(shader.blob.len()),
            shader.blob.start
        ))
        .weak()
        .small(),
    );

    let reached: Vec<&str> = package
        .passes
        .iter()
        .filter(|pass| pass.shaders.binary_search(&index).is_ok())
        .map(|pass| pass.name.as_str())
        .collect();
    if !reached.is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new("Drawn in").weak().small());
        for pass in reached {
            ui.label(RichText::new(pass).monospace());
        }
    }

    if !shader.bindings.is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new("Binds").weak().small());
        egui::Grid::new(("shpk_tooltip_binds", index))
            .num_columns(2)
            .show(ui, |ui| {
                for binding in &shader.bindings {
                    let prefix = match binding.register {
                        Register::Constant => "cb",
                        Register::Sampler => "s",
                        Register::Texture => "t",
                    };
                    ui.label(
                        RichText::new(format!("{prefix}{}", binding.slot))
                            .monospace()
                            .weak(),
                    );
                    ui.label(
                        RichText::new(
                            package
                                .names
                                .get(&binding.id)
                                .cloned()
                                .unwrap_or_else(|| named(binding.id)),
                        )
                        .monospace(),
                    );
                    ui.end_row();
                }
            });
    }

    // Only what a variant set, since the grid under the list already carries the rest.
    let set: Vec<(&str, &str)> = package
        .defines
        .get(index)
        .into_iter()
        .flatten()
        .filter_map(|(column, value)| {
            let key = package.columns.get(*column)?;
            if key.default == *value {
                return None;
            }
            let (short, _) = key.values.iter().find(|(_, held)| held == value)?;
            Some((key.name.as_str(), short.as_str()))
        })
        .collect();
    if !set.is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new("Compiled for").weak().small());
        egui::Grid::new(("shpk_tooltip_defines", index))
            .num_columns(2)
            .show(ui, |ui| {
                for (key, value) in set {
                    ui.label(RichText::new(key).monospace().weak());
                    ui.label(RichText::new(value).monospace());
                    ui.end_row();
                }
            });
    }
}

/// The keys worth putting on a list row: the ones whose value actually differs somewhere in the list
/// as it currently stands. A key every listed shader agrees on separates none of them, and showing
/// it would only push the ones that do off the end.
fn discriminating(package: &Rendered, listed: &[usize]) -> Vec<(usize, usize)> {
    (0..package.columns.len())
        .filter_map(|column| {
            let mut seen: Option<Option<&str>> = None;
            let mut width = 0;
            let mut varies = false;
            for shader in listed {
                let here = condition(package, *shader, column);
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

/// Key and value pairs across one row of the condition filter. More than this and a long key name
/// pushes the row off a narrow panel.
const CONDITION_PAIRS: usize = 2;
/// Width every condition box takes, so that a column of them lines up.
const CONDITION_WIDTH: f32 = 110.0;

/// Rows of the shader list kept on screen at once, which bounds it against a package holding six
/// thousand shaders.
const LIST_ROWS: usize = 8;

/// Colours for the disassembly, following the Catppuccin accents the app themes with so they sit on
/// both its light and dark palettes.
struct Palette {
    opcode: egui::Color32,
    register: egui::Color32,
    swizzle: egui::Color32,
    literal: egui::Color32,
    comment: egui::Color32,
}

impl Palette {
    fn of(visuals: &egui::Visuals) -> Self {
        match visuals.dark_mode {
            true => Self {
                opcode: egui::Color32::from_rgb(0xCB, 0xA6, 0xF7),
                register: egui::Color32::from_rgb(0x89, 0xB4, 0xFA),
                swizzle: egui::Color32::from_rgb(0x94, 0xE2, 0xD5),
                literal: egui::Color32::from_rgb(0xFA, 0xB3, 0x87),
                comment: egui::Color32::from_rgb(0x7F, 0x84, 0x9C),
            },
            false => Self {
                opcode: egui::Color32::from_rgb(0x88, 0x39, 0xEF),
                register: egui::Color32::from_rgb(0x1E, 0x66, 0xF5),
                swizzle: egui::Color32::from_rgb(0x17, 0x92, 0x99),
                literal: egui::Color32::from_rgb(0xFE, 0x64, 0x0B),
                comment: egui::Color32::from_rgb(0x8C, 0x8F, 0xA1),
            },
        }
    }
}

/// Words that lead a line or stand for a type, which is what an opcode is to the assembly.
const KEYWORDS: [&str; 22] = [
    "if",
    "else",
    "while",
    "return",
    "discard",
    "break",
    "continue",
    "void",
    "bool",
    "int",
    "uint",
    "float",
    "float2",
    "float3",
    "float4",
    "cbuffer",
    "register",
    "packoffset",
    "true",
    "false",
    "SamplerState",
    "SamplerComparisonState",
];

/// One line, coloured by token.
///
/// Both grammars are small enough to walk directly: registers with an optional index and swizzle,
/// numeric literals, a trailing comment, and something naming the operation -- the leading opcode in
/// the assembly, a keyword or a called name in the HLSL. Anything unrecognised keeps the default
/// colour rather than being guessed at.
fn highlight(
    line: &str,
    assembly: bool,
    palette: &Palette,
    font: egui::FontId,
    fallback: egui::Color32,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let mut push = |text: &str, color: egui::Color32| {
        job.append(
            text,
            0.0,
            egui::TextFormat {
                font_id: font.clone(),
                color,
                ..Default::default()
            },
        );
    };

    let (code, comment) = match line.split_once("//") {
        Some((code, rest)) => (code, Some(rest)),
        None => (line, None),
    };

    let bytes = code.as_bytes();
    let mut at = 0;
    let mut first = true;
    while at < code.len() {
        let byte = bytes[at];
        if byte.is_ascii_whitespace() {
            let end = at
                + code[at..]
                    .find(|c: char| !c.is_whitespace())
                    .unwrap_or(code.len() - at);
            push(&code[at..end], fallback);
            at = end;
        } else if byte == b'.' && code[at + 1..].starts_with(|c: char| "xyzw".contains(c)) {
            let end = at
                + 1
                + code[at + 1..]
                    .find(|c: char| !"xyzw".contains(c))
                    .unwrap_or(code.len() - at - 1);
            push(&code[at..end], palette.swizzle);
            at = end;
        } else if byte.is_ascii_alphabetic() || byte == b'_' {
            let end = at
                + code[at..]
                    .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .unwrap_or(code.len() - at);
            let word = &code[at..end];
            let named = assembly && first;
            let colour = if named || code[end..].starts_with('(') || KEYWORDS.contains(&word) {
                palette.opcode
            } else if word.ends_with(|c: char| c.is_ascii_digit()) {
                palette.register
            } else {
                fallback
            };
            push(&code[at..end], colour);
            first = false;
            at = end;
        } else if byte.is_ascii_digit() {
            let end = at
                + code[at..]
                    .find(|c: char| !c.is_ascii_digit() && c != '.')
                    .unwrap_or(code.len() - at);
            push(&code[at..end], palette.literal);
            at = end;
        } else {
            let end = code[at..]
                .chars()
                .next()
                .map_or(at + 1, |c| at + c.len_utf8());
            push(&code[at..end], fallback);
            at = end;
        }
    }

    if let Some(comment) = comment {
        push("//", palette.comment);
        push(comment, palette.comment);
    }
    job
}

/// The shader list, and whichever one is picked, read either way.
///
/// Only the picked shader is read, and only its own blob is touched: the worst case in the shipped
/// packages is a shader of some eight milliseconds, against forty-five seconds to do all twenty-six
/// thousand at once.
fn shaders_ui(ui: &mut egui::Ui, package: &Rendered, bytes: &[u8]) {
    let (mut stage, mut pass, mut picked, mut source) = ui
        .data(|data| data.get_temp::<(usize, usize, usize, bool)>(package.state))
        .unwrap_or((0, 0, 0, true));

    ui.horizontal_wrapped(|ui| {
        if ui
            .selectable_label(stage == 0, format!("All ({})", package.shaders.len()))
            .clicked()
        {
            stage = 0;
        }
        for (index, (name, count, size)) in package.stages.iter().enumerate() {
            let label = format!("{name} ({count}, {})", Bytes(*size));
            if ui.selectable_label(stage == index + 1, label).clicked() {
                stage = index + 1;
            }
        }
    });
    let slot = package.state.with("conditions");
    let mut chosen: Vec<Option<u32>> = ui
        .data(|data| data.get_temp::<Vec<Option<u32>>>(slot))
        .filter(|held| held.len() == package.columns.len())
        .unwrap_or_else(|| vec![None; package.columns.len()]);
    if package.columns.iter().any(|key| key.values.len() > 1) {
        let set = chosen.iter().flatten().count();
        let title = match set {
            0 => "Conditions".to_owned(),
            set => format!("Conditions ({set} set)"),
        };
        egui::CollapsingHeader::new(title)
            .id_salt("shpk_conditions")
            .show(ui, |ui| {
                let filterable: Vec<(usize, &KeyColumn)> = package
                    .columns
                    .iter()
                    .enumerate()
                    .filter(|(_, key)| key.values.len() > 1)
                    .collect();
                egui::Grid::new("shpk_conditions_grid")
                    .num_columns(CONDITION_PAIRS * 2)
                    .show(ui, |ui| {
                        for (at, (index, key)) in filterable.iter().enumerate() {
                            let held = chosen[*index];
                            let selected = held
                                .and_then(|value| {
                                    key.values.iter().find(|(_, held)| *held == value)
                                })
                                .map_or("any", |(short, _)| short.as_str());
                            // The grid puts every box at its column's edge; the name only needs to
                            // stand as tall as one so the two share a line.
                            ui.allocate_ui_with_layout(
                                vec2(0.0, ui.spacing().interact_size.y),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| hashed(ui, "Key", &key.name, key.id, held.is_none()),
                            );
                            egui::ComboBox::from_id_salt(("shpk_condition", key.id, index))
                                .width(CONDITION_WIDTH)
                                .selected_text(RichText::new(selected).monospace())
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut chosen[*index], None, "any");
                                    for (short, value) in &key.values {
                                        ui.selectable_value(
                                            &mut chosen[*index],
                                            Some(*value),
                                            short,
                                        );
                                    }
                                });
                            if at % CONDITION_PAIRS == CONDITION_PAIRS - 1 {
                                ui.end_row();
                            }
                        }
                        if !filterable.len().is_multiple_of(CONDITION_PAIRS) {
                            ui.end_row();
                        }
                    });
                if set > 0 && ui.small_button("Clear").clicked() {
                    chosen = vec![None; package.columns.len()];
                }
            });
    }
    ui.data_mut(|data| data.insert_temp(slot, chosen.clone()));

    if !package.passes.is_empty() {
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            if ui.selectable_label(pass == 0, "Any pass").clicked() {
                pass = 0;
            }
            for (index, row) in package.passes.iter().enumerate() {
                let label = format!("{} ({})", row.name, row.shaders.len());
                if ui.selectable_label(pass == index + 1, label).clicked() {
                    pass = index + 1;
                }
            }
        });
    }
    ui.add_space(4.0);

    // Zero is the unfiltered chip, so the stages are offset by one.
    let shown = stage
        .checked_sub(1)
        .and_then(|index| package.stages.get(index))
        .map(|(name, _, _)| *name);
    // Zero is the unfiltered chip here too.
    let drawn = pass
        .checked_sub(1)
        .and_then(|index| package.passes.get(index))
        .map(|row| row.shaders.as_slice());
    let listed: Vec<usize> = package
        .shaders
        .iter()
        .enumerate()
        .filter(|(_, shader)| shown.is_none_or(|name| shader.stage == name))
        .filter(|(index, _)| drawn.is_none_or(|shaders| shaders.binary_search(index).is_ok()))
        .filter(|(index, _)| {
            let defines = package.defines.get(*index);
            chosen.iter().enumerate().all(|(column, want)| match want {
                None => true,
                Some(value) => defines.is_some_and(|held| held.contains(&(column, *value))),
            })
        })
        .map(|(index, _)| index)
        .collect();
    // Narrowing to a stage the picked shader is not in would otherwise leave the disassembly below
    // showing something the list no longer offers.
    if listed.binary_search(&picked).is_err() {
        picked = listed.first().copied().unwrap_or(0);
    }

    let varying = discriminating(package, &listed);
    let height = ui.text_style_height(&egui::TextStyle::Monospace) + ui.spacing().item_spacing.y;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ScrollArea::both()
            .id_salt("shpk_shader_list")
            .max_height(height * LIST_ROWS as f32)
            .auto_shrink([false, true])
            .show_rows(ui, height, listed.len(), |ui, rows| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                for &index in &listed[rows] {
                    let shader = &package.shaders[index];
                    // Laid out in fixed columns so that moving down the list, the same key stays
                    // under the same place and a difference shows itself.
                    let mut flags = String::new();
                    for (column, width) in &varying {
                        let value = condition(package, index, *column).unwrap_or("-");
                        flags.push_str(&format!("{value:<width$} ", width = width));
                    }
                    let label = format!(
                        "#{index:<5} {:<7}{:>8} {:>3} bound   {}",
                        shader.stage,
                        Bytes(shader.blob.len()).to_string(),
                        shader.bindings.len(),
                        flags.trim_end()
                    );
                    let row = ui
                        .selectable_label(picked == index, RichText::new(label).monospace())
                        .on_hover_ui(|ui| shader_tooltip(ui, package, index));
                    if row.clicked() {
                        picked = index;
                    }
                }
            });
    });
    let Some(shader) = package.shaders.get(picked) else {
        ui.data_mut(|data| data.insert_temp(package.state, (stage, pass, picked, source)));
        return;
    };

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        heading(ui, &format!("Shader #{picked}"));
        if ui.selectable_label(source, "HLSL").clicked() {
            source = true;
        }
        if ui.selectable_label(!source, "Assembly").clicked() {
            source = false;
        }
    });
    ui.data_mut(|data| data.insert_temp(package.state, (stage, pass, picked, source)));

    // Held against the pick rather than redone each frame: the text runs to thirteen hundred lines,
    // and nothing about it changes until another shader or another reading is chosen.
    let slot = package.state.with("reading");
    let cached = ui.data(|data| data.get_temp::<((usize, bool), Arc<(Vec<String>, usize)>)>(slot));
    let lines = match cached {
        Some((at, lines)) if at == (picked, source) => Some(lines),
        _ => {
            let fresh = bytes
                .get(shader.blob.clone())
                .and_then(|blob| Some((program(blob)?, blob)))
                .map(|(program, blob)| {
                    Arc::new(match source {
                        true => {
                            let read = hlsl::decompile(&program, &naming(package, shader, blob));
                            (read.lines, read.body)
                        }
                        // The assembly names nothing itself, so what a line touches goes in a
                        // comment beside it. It declares as it goes, so there is nothing to fold.
                        false => (
                            annotate(package, shader, &dxbc::shex::format_program(&program)),
                            0,
                        ),
                    })
                });
            if let Some(lines) = &fresh {
                ui.data_mut(|data| data.insert_temp(slot, ((picked, source), Arc::clone(lines))));
            }
            fresh
        }
    };

    let Some(held) = lines else {
        ui.label(RichText::new("No shader program in this blob.").weak());
        return;
    };
    let (lines, body) = (&held.0, held.1);

    let folded = package.state.with("declarations");
    let mut hide = ui
        .data(|data| data.get_temp::<bool>(folded))
        .unwrap_or(false);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{} lines", lines.len()))
                .weak()
                .small(),
        );
        if ui.small_button("Copy").clicked() {
            // Always the whole thing: what is hidden is what makes the rest compile.
            ui.ctx().copy_text(lines.join("\n"));
        }
        if body > 0 {
            ui.checkbox(&mut hide, RichText::new("Hide declarations").small());
        }
        if source {
            ui.label(
                RichText::new("compiles, but not guaranteed to be perfect")
                    .weak()
                    .small(),
            );
        }
    });
    ui.data_mut(|data| data.insert_temp(folded, hide));
    let from = match hide {
        true => body,
        false => 0,
    };
    let lines = &lines[from..];

    let palette = Palette::of(ui.visuals());
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let fallback = ui.visuals().text_color();
    let height = ui.text_style_height(&egui::TextStyle::Monospace);
    // Only the lines on screen are laid out, which is what keeps a shader of thirteen hundred of
    // them from building a job for every one each frame.
    ScrollArea::both()
        .id_salt("shpk_disassembly")
        // What is left of the panel, which is the point of moving everything else out of it.
        .max_height(ui.available_height().max(height * 12.0))
        .auto_shrink([false, true])
        .show_rows(ui, height, lines.len(), |ui, rows| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            for (offset, line) in lines[rows.clone()].iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    // Selection runs across labels, so a gutter that takes part in it lands in
                    // whatever is dragged out. Only the code answers to the mouse.
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("{:>5}  ", from + rows.start + offset))
                                .monospace()
                                .weak(),
                        )
                        .selectable(false),
                    );
                    ui.add(
                        egui::Label::new(highlight(
                            line,
                            !source,
                            &palette,
                            font.clone(),
                            fallback,
                        ))
                        .selectable(true),
                    );
                });
            }
        });
}

/// The shader program in a blob. A vertex shader's blob opens with a short header before the
/// container, so the container is found rather than assumed at zero.
fn program(blob: &[u8]) -> Option<dxbc::shex::Program> {
    dxbc::scan_dxbc(blob)
        .iter()
        .flat_map(|container| &container.chunks)
        .find_map(|chunk| match chunk.parse() {
            ChunkData::Shader(program) => Some(program),
            _ => None,
        })
}

/// The material parameter buffer as named fields.
///
/// The reflection leaves this one buffer as a bare array, so its contents come from the package's
/// own parameter table instead. Several parameters share a register, so each carries the components
/// it occupies in the one it starts in.
fn parameter_fields(package: &Rendered) -> Vec<hlsl::Field> {
    let mut fields = Vec::new();
    for (index, register) in package.registers.iter().enumerate() {
        let mut here: Vec<(usize, u8)> = Vec::new();
        for (component, cell) in register.iter().enumerate() {
            let Some(cell) = cell else { continue };
            match here.iter_mut().find(|(param, _)| *param == cell.param) {
                Some((_, mask)) => *mask |= 1 << component,
                None => here.push((cell.param, 1 << component)),
            }
        }
        // A parameter the table cannot name would read as a hash where a field should be, so it is
        // left out and its register speaks for itself.
        fields.extend(here.into_iter().filter_map(|(param, mask)| {
            let owner = &package.params[param];
            Some(hlsl::Field::packed(
                names::resolve(owner.id)?.to_owned(),
                owner.size,
                index as u32,
                mask,
            ))
        }));
    }
    fields
}

/// What this shader's registers are called, so the reading names them rather than their slots.
fn naming(package: &Rendered, shader: &ShaderRow, blob: &[u8]) -> hlsl::Names {
    let mut names = hlsl::Names::default();
    for binding in &shader.bindings {
        let Some(name) = package.names.get(&binding.id) else {
            continue;
        };
        match binding.register {
            Register::Texture => {
                names.textures.insert(binding.slot, name.clone());
            }
            Register::Sampler => {
                names.samplers.insert(binding.slot, name.clone());
            }
            Register::Constant => {
                let fields = match binding.id == package.material_buffer {
                    true => parameter_fields(package),
                    false => package
                        .layouts
                        .get(&binding.id)
                        .map(|members| {
                            members
                                .iter()
                                .map(|member| {
                                    hlsl::Field::described(
                                        member.name.clone(),
                                        member.kind.clone(),
                                        member.offset,
                                        member.size,
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                };
                names
                    .constants
                    .insert(binding.slot, hlsl::Buffer::new(name.clone(), fields));
            }
        }
    }

    // The signatures name the interpolators, which the package tables say nothing about.
    for chunk in dxbc::scan_dxbc(blob)
        .iter()
        .flat_map(|container| &container.chunks)
    {
        let (into, signature) = match chunk.parse() {
            ChunkData::InputSignature(signature) => (&mut names.inputs, signature),
            ChunkData::OutputSignature(signature) => (&mut names.outputs, signature),
            _ => continue,
        };
        for element in &signature.elements {
            into.entry(element.register).or_insert_with(|| {
                hlsl::Semantic::new(
                    &element.semantic_name,
                    element.semantic_index,
                    element.component_type,
                    element.mask,
                )
            });
        }
    }
    names
}

/// A register reference in a line of disassembly: `cb0[6]`, `t3`, `s1`.
fn reference(line: &str, at: usize) -> Option<(Register, u16, Option<u16>, usize)> {
    let rest = &line[at..];
    let (register, tag) = [
        (Register::Constant, "cb"),
        (Register::Texture, "t"),
        (Register::Sampler, "s"),
    ]
    .into_iter()
    .find(|(_, tag)| rest.starts_with(tag))?;

    let digits = rest[tag.len()..]
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len() - tag.len());
    if digits == 0 {
        return None;
    }
    let slot = rest[tag.len()..tag.len() + digits].parse().ok()?;
    let mut end = tag.len() + digits;

    // A constant buffer reference carries the register it reads, which is what names the field.
    let index = match rest[end..].strip_prefix('[') {
        Some(inner) => {
            let close = inner.find(']')?;
            end += close + 2;
            inner[..close].parse().ok()
        }
        None => None,
    };
    Some((register, slot, index, at + end))
}

/// What a shader's own bindings say a register reference is.
///
/// `cb0[6]` is the buffer this shader put at slot zero, read at its sixth vec4. For most buffers the
/// bytecode's reflection names the field sitting there; for the material buffer, which the
/// reflection leaves as one bare array, the parameters occupying that register are named instead.
fn explain(
    package: &Rendered,
    shader: &ShaderRow,
    at: (Register, u16, Option<u16>),
    swizzle: &str,
) -> Option<String> {
    let (register, slot, index) = at;
    let binding = shader
        .bindings
        .iter()
        .find(|binding| binding.register == register && binding.slot == slot)?;
    let name = package.names.get(&binding.id)?;

    let Some(index) = index else {
        return Some(name.clone());
    };

    if binding.id == package.material_buffer {
        // Several parameters share a register, so the swizzle decides which of them a line actually
        // reads. Without one, every component is in play.
        let components = package.registers.get(usize::from(index))?;
        let read: Vec<usize> = match swizzle.is_empty() {
            true => (0..COMPONENTS).collect(),
            false => swizzle
                .chars()
                .filter_map(|component| "xyzw".find(component))
                .collect(),
        };
        let mut owners: Vec<&str> = Vec::new();
        for component in read {
            let Some(cell) = components.get(component).copied().flatten() else {
                continue;
            };
            let owner = package.params[cell.param].name.as_str();
            if !owners.contains(&owner) {
                owners.push(owner);
            }
        }
        return match owners.is_empty() {
            true => Some(format!("{name}[{index}]")),
            false => Some(format!("{name}: {}", owners.join(", "))),
        };
    }

    // Fields are laid out in order, so the one covering a register is the last starting at or
    // before it.
    let field = package
        .layouts
        .get(&binding.id)?
        .iter()
        .rfind(|member| member.offset / 16 <= u32::from(index))?;
    Some(format!("{name}.{}", field.name))
}

/// The disassembly with a trailing comment naming what each line touches.
fn annotate(package: &Rendered, shader: &ShaderRow, text: &str) -> Vec<String> {
    text.lines()
        .map(|line| {
            let mut seen: Vec<String> = Vec::new();
            let mut at = 0;
            // On a declaration the bracket is the buffer's size, not a register it reads, so only
            // the buffer itself is named.
            let declaring = line.trim_start().starts_with("dcl_");
            while at < line.len() {
                if !line.is_char_boundary(at) {
                    at += 1;
                    continue;
                }
                // Only at a token start, so the `s` of `mors` is not read as sampler zero.
                let boundary = at == 0 || !line.as_bytes()[at - 1].is_ascii_alphanumeric();
                match boundary.then(|| reference(line, at)).flatten() {
                    Some((register, slot, index, end)) => {
                        let index = match declaring {
                            true => None,
                            false => index,
                        };
                        let swizzle = line[end..]
                            .strip_prefix('.')
                            .map(|rest| {
                                let end = rest
                                    .find(|c: char| !"xyzw".contains(c))
                                    .unwrap_or(rest.len());
                                &rest[..end]
                            })
                            .unwrap_or_default();
                        if let Some(text) =
                            explain(package, shader, (register, slot, index), swizzle)
                            && !seen.contains(&text)
                        {
                            seen.push(text);
                        }
                        at = end;
                    }
                    None => at += 1,
                }
            }
            match seen.is_empty() {
                true => line.to_owned(),
                false => format!("{line}  // {}", seen.join(", ")),
            }
        })
        .collect()
}

/// The key values a shader was compiled under, as its own variants agree on them.
///
/// Every key is listed whether or not it decided anything, always in the same order, so that moving
/// between two shaders leaves each key where it was and what differs is simply the rows that
/// changed. A value the package would have taken anyway is dimmed; one a variant set is not.
fn defines_ui(ui: &mut egui::Ui, package: &Rendered, shader: usize) {
    let Some(defines) = package.defines.get(shader) else {
        return;
    };
    if defines.is_empty() {
        return;
    }
    section(ui, "Compiled for");
    if let Some((count, complete)) = package.selected.get(shader) {
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
            for (column, key) in package.columns.iter().enumerate() {
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

/// A constant buffer whose fields the bytecode named, as a collapsing row over its layout. The
/// package tables carry only the buffer, so this is the one thing on screen that comes from the
/// compiled shaders rather than from the file's own headers.
fn members_ui(ui: &mut egui::Ui, resource: &ResourceRow) {
    let header = egui::CollapsingHeader::new(RichText::new(&resource.name).monospace())
        .id_salt(resource.id)
        .show(ui, |ui| {
            egui::Grid::new(("shpk_members", resource.id))
                .num_columns(4)
                .striped(true)
                .show(ui, |ui| {
                    headers(ui, &["Field", "Offset", "Size", "Type"]);
                    for member in &resource.members {
                        ui.label(RichText::new(&member.name).monospace());
                        ui.label(RichText::new(format!("+{}", member.offset)).monospace());
                        ui.label(RichText::new(format!("{} B", member.size)).monospace());
                        ui.label(RichText::new(&member.kind).weak());
                        ui.end_row();
                    }
                });
        });
    crate::assets::crc_context(
        &header.header_response,
        resource.kind,
        &resource.name,
        resource.id,
    );
}

/// The parameter buffer as the shader addresses it: a row per register, a column per component.
/// A component continuing a parameter that began earlier is dimmed, so a vec3 reads as one block
/// rather than three repetitions of a name.
fn params_ui(ui: &mut egui::Ui, package: &Rendered) {
    egui::Grid::new("shpk_params")
        .num_columns(COMPONENTS + 1)
        .striped(true)
        .show(ui, |ui| {
            headers(ui, &["", "x", "y", "z", "w"]);
            for (index, register) in package.registers.iter().enumerate() {
                ui.label(RichText::new(format!("c{index}")).monospace().weak());
                for (component, cell) in register.iter().enumerate() {
                    let Some(cell) = cell else {
                        ui.label(RichText::new("·").weak());
                        continue;
                    };
                    let param = &package.params[cell.param];
                    ui.vertical(|ui| {
                        hashed(
                            ui,
                            &format!("Parameter at +{} B, {} B", param.offset, param.size),
                            &param.name,
                            param.id,
                            !cell.start,
                        );
                        if let Some(value) = package.defaults.get(index * COMPONENTS + component) {
                            ui.label(RichText::new(value.to_string()).weak().small());
                        }
                    });
                }
                ui.end_row();
            }
        });
}

impl Rendered {
    pub fn details_ui(&self, ui: &mut egui::Ui) {
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            // Whichever shader the list has picked, so that clicking between two leaves this in
            // place and what differs is the rows that changed.
            if let Some((_, _, picked, _)) =
                ui.data(|data| data.get_temp::<(usize, usize, usize, bool)>(self.state))
            {
                defines_ui(ui, self, picked);
                ui.add_space(8.0);
                ui.separator();
            }
            facts(ui, "shpk_identity", &self.identity);
            ui.add_space(8.0);
            ui.separator();
            metadata_ui(ui, self);
        });
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
