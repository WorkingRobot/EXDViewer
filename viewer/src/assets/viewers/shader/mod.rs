//! What the two shader viewers share.
//!
//! A `.shpk` holds many compiled shaders behind one set of resource tables and a `.shcd` holds a
//! single one, but what a register is called, what a constant buffer holds, and how a reading is
//! drawn are the same question in either. Only the tables around them differ.

pub mod code;

use std::collections::HashMap;
use std::ops::Range;

use dxbc::chunks::ChunkData;
use egui::RichText;
use hlsl::layout::{Member, members};
use ironworks::file::shpk::Resource;
use shaders::names;

use super::{hashed, headers, heading};

pub fn named(id: u32) -> String {
    names::resolve(id).map_or_else(|| format!("{id:#010X}"), str::to_owned)
}

/// Which bank a resource binds to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Register {
    Constant,
    Sampler,
    Texture,
}

#[derive(Clone, Copy)]
pub struct Binding {
    pub register: Register,
    pub slot: u16,
    pub id: u32,
}

/// One shader of a file. The bytecode itself stays in the file; this is only where to find it.
pub struct Shader {
    pub stage: &'static str,
    pub blob: Range<usize>,
    /// What the shader binds, which is what turns a bare `cb0` in the disassembly into a name. Slots
    /// are per-shader: the same buffer sits at different ones in different shaders of a package.
    pub bindings: Vec<Binding>,
}

/// What a shader binds, taken from the bands its resource table divides into.
pub fn bindings(
    constants: &[Resource],
    samplers: &[Resource],
    textures: &[Resource],
) -> Vec<Binding> {
    [
        (Register::Constant, constants),
        (Register::Sampler, samplers),
        (Register::Texture, textures),
    ]
    .into_iter()
    .flat_map(|(register, list)| {
        list.iter().map(move |resource| Binding {
            register,
            slot: resource.slot(),
            id: resource.id(),
        })
    })
    .collect()
}

/// A resource a file binds, as a table row.
pub struct ResourceRow {
    pub name: String,
    pub id: u32,
    pub kind: &'static str,
    pub slot: u16,
    /// How much of its bank it takes, for the one band whose record says.
    pub size: Option<String>,
    /// What the shader declares inside a constant buffer, recovered from the bytecode's reflection.
    /// Empty for everything else, and for a buffer that is one bare array.
    pub members: Vec<Member>,
}

/// The bands a shader file lists its resources in: the heading each is drawn under, and what one of
/// them is called.
const BANDS: [(&str, &str); 4] = [
    ("Constant buffers", "Constant buffer"),
    ("Samplers", "Sampler"),
    ("Textures", "Texture"),
    ("Unordered access views", "UAV"),
];

/// The resource tables as rows, under the heading each band is drawn with.
pub fn resources<'a>(
    bands: [&[Resource]; 4],
    name: impl Fn(&Resource) -> Option<&'a str>,
    layouts: &HashMap<u32, Vec<Member>>,
) -> Vec<(&'static str, Vec<ResourceRow>)> {
    BANDS
        .into_iter()
        .zip(bands)
        .map(|((group, kind), list)| {
            let rows = list
                .iter()
                .map(|resource| ResourceRow {
                    // The file names its own resources, which is the one thing it knows that the
                    // curated hash table does not.
                    name: name(resource).map_or_else(|| named(resource.id()), str::to_owned),
                    id: resource.id(),
                    kind,
                    slot: resource.slot(),
                    // A constant buffer states its size in registers. On the other bands the field
                    // is not a size at all but an index within the band, so the column comes off.
                    size: matches!(kind, "Constant buffer").then(|| {
                        format!(
                            "{} registers ({} B)",
                            resource.size(),
                            u32::from(resource.size()) * 16
                        )
                    }),
                    members: layouts.get(&resource.id()).cloned().unwrap_or_default(),
                })
                .collect();
            (group, rows)
        })
        .collect()
}

/// The resources a file binds, a table per band.
pub fn resources_ui(ui: &mut egui::Ui, groups: &[(&'static str, Vec<ResourceRow>)]) {
    for (group, rows) in groups {
        if rows.is_empty() {
            continue;
        }
        heading(ui, group);
        let sized = rows.iter().any(|row| row.size.is_some());
        egui::Grid::new(*group)
            .num_columns(2 + usize::from(sized))
            .striped(true)
            .show(ui, |ui| {
                match sized {
                    true => headers(ui, &["Name", "Slot", "Size"]),
                    false => headers(ui, &["Name", "Slot"]),
                }
                for resource in rows {
                    match resource.members.is_empty() {
                        true => hashed(ui, resource.kind, &resource.name, resource.id, false),
                        false => members_ui(ui, resource),
                    }
                    ui.label(RichText::new(resource.slot.to_string()).monospace());
                    if let Some(size) = &resource.size {
                        ui.label(RichText::new(size).monospace());
                    }
                    ui.end_row();
                }
            });
    }
}

/// A constant buffer whose fields the bytecode named, as a collapsing row over its layout.
fn members_ui(ui: &mut egui::Ui, resource: &ResourceRow) {
    let header = egui::CollapsingHeader::new(RichText::new(&resource.name).monospace())
        .id_salt(resource.id)
        .show(ui, |ui| {
            egui::Grid::new(("shader_members", resource.id))
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

/// The constant buffer layouts a blob's reflection describes, added to what is already known. They
/// are keyed by the same crc the file's own resource tables identify a buffer by.
pub fn buffers(blob: &[u8], into: &mut HashMap<u32, Vec<Member>>) {
    for container in dxbc::scan_dxbc(blob) {
        for chunk in &container.chunks {
            let ChunkData::Rdef(reflection) = chunk.parse() else {
                continue;
            };
            for buffer in &reflection.constant_buffers {
                into.entry(names::hash(buffer.name.as_bytes()))
                    .or_insert_with(|| members(buffer));
            }
        }
    }
}

/// The shader program in a blob. A vertex shader's blob opens with a short header before the
/// container, so the container is found rather than assumed at zero.
pub fn program(blob: &[u8]) -> Option<dxbc::shex::Program> {
    dxbc::scan_dxbc(blob)
        .iter()
        .flat_map(|container| &container.chunks)
        .find_map(|chunk| match chunk.parse() {
            ChunkData::Shader(program) => Some(program),
            _ => None,
        })
}

/// What a reading resolves a register against: what the file calls its resources, and what its
/// constant buffers hold.
pub struct Naming {
    /// Every resource name in the file, by id.
    pub resources: HashMap<u32, String>,
    /// Constant buffer fields by buffer id, out of the bytecode's reflection.
    pub layouts: HashMap<u32, Vec<Member>>,
    /// The one buffer the reflection leaves undescribed, where the file has its own account of it.
    pub packed: Option<Packed>,
}

/// A constant buffer the reflection gives as one bare array, described from the file's own tables
/// instead.
pub struct Packed {
    /// Which buffer it is.
    pub buffer: u32,
    /// What occupies each of its registers.
    pub owners: Vec<Vec<Owner>>,
}

/// Something occupying part of one register of a [`Packed`] buffer, several of which share it.
pub struct Owner {
    pub name: String,
    /// Which components of the register are its own.
    pub mask: u8,
    /// How many bytes it takes, where it can be declared at all. A name the hash table cannot
    /// recover is no identifier, so what reads it is still named but nothing is declared for it.
    pub declared: Option<u16>,
}
