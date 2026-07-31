//! Whether a rebuilt program still means what the shader meant.
//!
//! Canonicalise a shader, rebuild a program from that graph alone, canonicalise that, and require
//! the two to agree on what the shader leaves behind. Value hashes are content-addressed, so
//! agreement is exact rather than a resemblance.
//!
//! Only the emitter is under test here. The reference is always the shader's own graph and never
//! the merged union: a union restricted to one variant is the variant's graph by construction, so
//! comparing the two would pass whatever the emitter did with it.

use std::collections::{BTreeMap, BTreeSet};

use dxbc::shex::Program;
use ironworks::file::shpk::ShaderPackage;

use crate::{Error, canon, pick, synth};

/// What a shader leaves behind: each output component, and each side effect under the condition it
/// fires under.
///
/// The effects are the part worth naming. An output that goes missing takes the shader's color with
/// it and is hard to overlook; a `discard` that keeps its own test but loses the branch it sat under
/// still compiles, still writes the right color, and kills every pixel.
#[derive(PartialEq, Eq, Debug)]
struct Leaves {
    outputs: BTreeMap<String, u64>,
    /// Each effect as its opcode and the set of conditions that all have to hold for it to fire.
    ///
    /// A set, because the two sides spell the conjunction differently and mean the same by it: the
    /// graph hangs the branch path and the instruction's own test off the effect as separate
    /// children, while a rebuilt program has to `and` them into one value first.
    effects: Vec<(String, BTreeSet<u64>)>,
}

/// The conditions an effect fires under, with the `and`s that join them flattened away.
fn conjuncts(graph: &canon::Graph, value: u64, into: &mut BTreeSet<u64>) {
    match graph.nodes.get(&value) {
        Some(node) if node.op == "and" || node.op == "path" => {
            for child in &node.children {
                conjuncts(graph, *child, into);
            }
        }
        _ => {
            into.insert(value);
        }
    }
}

fn leaves(graph: &canon::Graph) -> Leaves {
    Leaves {
        outputs: graph.roots.clone(),
        effects: graph
            .effects
            .iter()
            .map(|(tag, value)| {
                // An effect with nothing to test fires wherever it stands, and whether the machine
                // wrote it as a test for zero or for non-zero says nothing about it. The `ret` every
                // shader ends on is one of these.
                let bare = graph
                    .nodes
                    .get(value)
                    .is_none_or(|node| node.children.is_empty());
                match bare {
                    true => (
                        tag.trim_end_matches("_nz")
                            .trim_end_matches("_z")
                            .to_owned(),
                        BTreeSet::new(),
                    ),
                    false => {
                        let mut held = BTreeSet::new();
                        for child in &graph.nodes[value].children {
                            conjuncts(graph, *child, &mut held);
                        }
                        (tag.clone(), held)
                    }
                }
            })
            .collect(),
    }
}

/// The names the rebuilt program's slots go by, so its leaves hash as the original's did.
fn naming(built: &synth::Built) -> canon::Naming {
    let mut naming = canon::Naming::default();
    for (slot, (name, _)) in built.table.constants.iter().enumerate() {
        naming.constants.insert(slot as u32, name.clone());
    }
    for (slot, name) in built.table.textures.iter().enumerate() {
        naming.textures.insert(slot as u32, name.clone());
    }
    for (slot, name) in built.table.samplers.iter().enumerate() {
        naming.samplers.insert(slot as u32, name.clone());
    }
    for (into, held) in [
        (&mut naming.inputs, &built.table.inputs),
        (&mut naming.outputs, &built.table.outputs),
    ] {
        for (slot, (name, _)) in held.iter().enumerate() {
            into.insert(slot as u32, name.clone());
        }
    }
    naming
}

/// Children before parents, over everything the graph keeps.
fn ordered(graph: &canon::Graph, live: &BTreeSet<u64>) -> Vec<u64> {
    let mut pending: BTreeMap<u64, usize> = BTreeMap::new();
    let mut readers: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for value in live {
        let mut children = graph
            .nodes
            .get(value)
            .map(|node| node.children.clone())
            .unwrap_or_default();
        children.sort_unstable();
        children.dedup();
        children.retain(|child| live.contains(child) && child != value);
        pending.insert(*value, children.len());
        for child in children {
            readers.entry(child).or_default().push(*value);
        }
    }
    let mut ready: Vec<u64> = pending
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(value, _)| *value)
        .collect();
    let mut order = Vec::with_capacity(pending.len());
    while let Some(value) = ready.pop() {
        order.push(value);
        for reader in readers.get(&value).into_iter().flatten() {
            if let Some(slot) = pending.get_mut(reader) {
                *slot -= 1;
                if *slot == 0 {
                    ready.push(*reader);
                }
            }
        }
    }
    order
}

/// What the two readings disagreed about, for triage.
fn report(graph: &canon::Graph, again: &canon::Graph, was: &Leaves, now: &Leaves) {
    fn spell(graph: &canon::Graph, value: u64, depth: usize) -> String {
        match graph.nodes.get(&value) {
            None => format!("?{value:x}"),
            Some(node) if depth == 0 || node.children.is_empty() => node.op.clone(),
            Some(node) => format!(
                "{}({})",
                node.op,
                node.children
                    .iter()
                    .map(|child| spell(graph, *child, depth - 1))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
    for (name, value) in &was.outputs {
        let held = now.outputs.get(name);
        if held != Some(value) {
            eprintln!("    {name}");
            eprintln!("      was: {}", spell(graph, *value, 3));
            if let Some(held) = held {
                eprintln!("      now: {}", spell(again, *held, 3));
            }
        }
    }
    if was.effects != now.effects {
        eprintln!(
            "    effects differ: {} -> {}",
            was.effects.len(),
            now.effects.len()
        );
    }
}

/// Whether one shader survives being taken apart and put back together.
pub fn roundtrip(
    raw: &[u8],
    package: &ShaderPackage,
    at: usize,
    program: &Program,
) -> Result<bool, Error> {
    let graph = canon::build(program, pick::naming(raw, package, at));
    if graph.loops > 0 {
        return Err(Error::Loops);
    }
    let live = graph.live();

    // One variant, so nothing is guarded and every value belongs to the one region.
    let regions = vec![synth::Region {
        cubes: Vec::new(),
        values: ordered(&graph, &live),
    }];
    let roots: BTreeMap<String, (u64, synth::Guard)> = graph
        .roots
        .iter()
        .map(|(name, value)| (name.clone(), (*value, Vec::new())))
        .collect();
    let effects: Vec<(u64, synth::Guard)> = graph
        .effects
        .iter()
        .map(|(_, value)| (*value, Vec::new()))
        .collect();

    let stage = match &*program.shader_type.to_string() {
        "vs" => "vs",
        "ps" => "ps",
        "hs" => "hs",
        "ds" => "ds",
        _ => "gs",
    };
    let built =
        synth::Synth::new(&graph.nodes).build(&live, &regions, &roots, &effects, Vec::new(), stage);
    let again = canon::build(&built.program, naming(&built));
    let (was, now) = (leaves(&graph), leaves(&again));
    if was != now && std::env::var("WHY").is_ok() {
        report(&graph, &again, &was, &now);
    }
    Ok(was == now)
}
