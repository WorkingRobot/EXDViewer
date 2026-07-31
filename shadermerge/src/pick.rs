//! Picking a (stage, pass) group out of a package, and working out which keys select within it.

use crate::canon::Naming;
use dxbc::chunks::ChunkData;
use ironworks::file::shpk::{self, ShaderPackage, Stage};
use shaders::names;
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub fn named(id: u32) -> String {
    names::resolve(id).map_or_else(|| format!("{id:#010x}"), str::to_owned)
}

pub fn shorten(key: &str, value: &str) -> String {
    match value
        .strip_prefix(key)
        .map(|rest| rest.trim_start_matches('_'))
    {
        Some(rest) if !rest.is_empty() => rest.to_owned(),
        _ => value.strip_prefix("Val").unwrap_or(value).to_owned(),
    }
}

pub fn stage_of(text: &str) -> usize {
    match text {
        "vs" => 0,
        "ps" => 1,
        "hs" => 2,
        "ds" => 3,
        _ => 4,
    }
}

pub fn tag(stage: usize) -> &'static str {
    ["vs", "ps", "hs", "ds", "gs"][stage]
}

pub fn offsets(package: &ShaderPackage) -> [Option<usize>; 5] {
    let mut held = [None; 5];
    for stage in [
        Stage::Vertex,
        Stage::Pixel,
        Stage::Hull,
        Stage::Domain,
        Stage::Geometry,
    ] {
        held[stage as usize] = package
            .shaders()
            .iter()
            .position(|shader| shader.stage() == stage);
    }
    held
}

pub fn columns(package: &ShaderPackage) -> Vec<String> {
    let mut held: Vec<String> = package
        .system_keys()
        .iter()
        .chain(package.scene_keys())
        .chain(package.material_keys())
        .map(|key| named(key.id()))
        .collect();
    held.push("Subview1".into());
    held.push("Subview2".into());
    held
}

fn determines(rows: &[(Vec<u32>, usize)], keep: &[usize]) -> bool {
    let mut seen: HashMap<Vec<u32>, usize> = HashMap::new();
    for (tuple, blob) in rows {
        let key: Vec<u32> = keep.iter().map(|at| tuple[*at]).collect();
        match seen.get(&key) {
            Some(held) if held != blob => return false,
            _ => {
                seen.insert(key, *blob);
            }
        }
    }
    true
}

/// One (stage, pass) group: which blobs it runs, and the key values that pick each.
pub struct Group {
    pub pass: u32,
    /// The key columns the blob choice actually depends on.
    pub axes: Vec<usize>,
    /// Blob index, under every axis tuple that reaches it.
    pub blobs: BTreeMap<usize, BTreeSet<Vec<u32>>>,
}

/// One group, by the pass's own id rather than by its name: two names agree only as far as the two
/// hash tables that produced them, and a substring test over names would fold
/// `PASS_COMPOSITE_SEMITRANSPARENCY` together with `PASS_COMPOSITE_SEMITRANSPARENCY_UNDER_WATER`,
/// which run different shaders, so one key tuple would name two blobs at once.
pub fn group(package: &ShaderPackage, stage: usize, want: u32) -> Option<Group> {
    let offsets = offsets(package);
    let mut rows: Vec<(Vec<u32>, usize)> = Vec::new();
    let mut pass = 0;
    for node in package.nodes() {
        for held in node.passes() {
            if held.id() != want {
                continue;
            }
            let index = held.stages()[stage];
            if index == shpk::NONE {
                continue;
            }
            let base = offsets[stage]?;
            pass = held.id();
            rows.push((node.keys().to_vec(), base + index as usize));
        }
    }
    if rows.is_empty() {
        return None;
    }

    let width = rows[0].0.len();
    let mut axes: Vec<usize> = (0..width).collect();
    for at in (0..width).rev() {
        let without: Vec<usize> = axes.iter().copied().filter(|held| *held != at).collect();
        if determines(&rows, &without) {
            axes = without;
        }
    }

    let mut blobs: BTreeMap<usize, BTreeSet<Vec<u32>>> = BTreeMap::new();
    for (tuple, blob) in &rows {
        blobs
            .entry(*blob)
            .or_default()
            .insert(axes.iter().map(|at| tuple[*at]).collect());
    }
    Some(Group { pass, axes, blobs })
}

/// The distinct values each axis takes, in the order the group lists them.
pub fn spread(held: &Group) -> Vec<Vec<u32>> {
    let mut spread: Vec<BTreeSet<u32>> = vec![BTreeSet::new(); held.axes.len()];
    for tuples in held.blobs.values() {
        for tuple in tuples {
            for (into, value) in spread.iter_mut().zip(tuple) {
                into.insert(*value);
            }
        }
    }
    spread
        .into_iter()
        .map(|values| values.into_iter().collect())
        .collect()
}

/// What a shader calls each register it binds. The package lists this per shader, which is the only
/// place the mapping exists: the bytecode knows slots, not names.
pub fn naming(raw: &[u8], package: &ShaderPackage, blob: usize) -> Naming {
    let shader = &package.shaders()[blob];
    let mut held = Naming::default();
    // Semantics, not registers: 202 of the corpus's 837 signature elements sit at a different
    // register in a different variant of the same group.
    let at = package.blobs_offset() + shader.blob_offset() as usize;
    if let Some(bytes) = raw.get(at..at + shader.blob_size() as usize) {
        for chunk in dxbc::scan_dxbc(bytes).iter().flat_map(|held| &held.chunks) {
            let (into, signature) = match chunk.parse() {
                ChunkData::InputSignature(signature) => (&mut held.inputs, signature),
                ChunkData::OutputSignature(signature) => (&mut held.outputs, signature),
                _ => continue,
            };
            for element in &signature.elements {
                // HLSL semantics are case-insensitive, and the corpus spells the same one both
                // ways in different variants of one group: `SV_TARGET0` beside `SV_Target0`.
                // Keying on the spelling would split one value in two and then declare both.
                into.entry(element.register).or_insert(format!(
                    "{}{}",
                    element.semantic_name.to_uppercase(),
                    element.semantic_index
                ));
            }
        }
    }
    for (into, band) in [
        (&mut held.constants, shader.constants()),
        (&mut held.samplers, shader.samplers()),
        (&mut held.textures, shader.textures()),
        (&mut held.uavs, shader.uavs()),
    ] {
        for resource in band {
            let name = package
                .name(resource)
                .map_or_else(|| format!("{:#010x}", resource.id()), str::to_owned);
            into.insert(u32::from(resource.slot()), name);
        }
    }
    held
}

/// The axis tuples that compute a value, covered by maximal conjunctions, greedily.
///
/// One cube is a plain `#ifdef`; a handful is a readable `||`; a count approaching the tuples
/// themselves means the guard is a list of variants rather than a condition. A cube holds, per
/// axis, the values it admits.
pub fn cover(seen: &[bool], tuples: &[Vec<u32>], spread: &[Vec<u32>]) -> Vec<Vec<Vec<u32>>> {
    let mut left = seen.to_vec();
    let mut cubes = Vec::new();
    while let Some(at) = left.iter().position(|held| *held) {
        let mut cube: Vec<Vec<u32>> = tuples[at].iter().map(|value| vec![*value]).collect();
        for (axis, values) in spread.iter().enumerate() {
            for value in values {
                if cube[axis].contains(value) {
                    continue;
                }
                cube[axis].push(*value);
                let fits = tuples
                    .iter()
                    .enumerate()
                    .filter(|(_, tuple)| admits(&cube, tuple))
                    .all(|(at, _)| seen[at]);
                if !fits {
                    cube[axis].pop();
                }
            }
        }
        for (at, tuple) in tuples.iter().enumerate() {
            if admits(&cube, tuple) {
                left[at] = false;
            }
        }
        cubes.push(cube);
    }
    cubes
}

/// Whether a cube admits a tuple.
pub fn admits(cube: &[Vec<u32>], tuple: &[u32]) -> bool {
    tuple
        .iter()
        .zip(cube)
        .all(|(value, held)| held.contains(value))
}
