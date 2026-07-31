//! One (stage, pass) group as a single HLSL source with `#if`s over the package's own shader keys.
//!
//! The merged value graph is rebuilt into one SM5 program, decompiled once, and its top-level `if`
//! blocks turned back into preprocessor regions.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use dxbc::chunks::ChunkData;
use dxbc::shex::InstructionKind;
use ironworks::file::shpk::ShaderPackage;

use crate::{Error, Merged, canon, factor, pick, synth};

/// Whether two things are ever emitted in the same variant.
fn meets(left: &[bool], right: &[bool]) -> bool {
    left.iter().zip(right).any(|(a, b)| *a && *b)
}

/// Children before parents, keeping to one guard for as long as the dependencies allow so that a
/// region covers a run of lines rather than one. Answers the values it could not place, which are
/// the ones a cycle runs through.
fn schedule(
    nodes: &HashMap<u64, canon::Node>,
    slots: &factor::Slots,
    speaks: &BTreeSet<u64>,
    seen_at: &HashMap<u64, &Vec<bool>>,
    kin: &HashMap<u64, Vec<u64>>,
) -> Result<Vec<u64>, Vec<u64>> {
    let mut pending: HashMap<u64, usize> = HashMap::new();
    let mut readers: HashMap<u64, Vec<u64>> = HashMap::new();
    for value in speaks {
        // A child may be one branch of a hole, so the branch this line wants is whichever one the
        // variants it runs in also hold. Ordering it against the rest of the class would be an
        // order between two branches that never meet.
        let here = seen_at[value];
        let mut children: Vec<u64> = nodes
            .get(value)
            .map(|node| {
                node.children
                    .iter()
                    .filter_map(|child| kin.get(&slots.of(*child)))
                    .flatten()
                    .copied()
                    .filter(|member| meets(here, seen_at[member]))
                    .collect()
            })
            .unwrap_or_default();
        children.sort_unstable();
        children.dedup();
        children.retain(|child| child != value);
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
    ready.sort_unstable();

    let mut order: Vec<u64> = Vec::new();
    let mut open: Option<&Vec<bool>> = None;
    while !ready.is_empty() {
        let pick = ready
            .iter()
            .position(|value| Some(seen_at[value]) == open)
            .unwrap_or_else(|| {
                ready
                    .iter()
                    .enumerate()
                    .max_by_key(|(_, value)| seen_at[*value].iter().filter(|held| **held).count())
                    .map(|(at, _)| at)
                    .expect("ready is not empty")
            });
        let value = ready.swap_remove(pick);
        open = Some(seen_at[&value]);
        pending.remove(&value);
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
    match pending.is_empty() {
        // A value left out was never written, and every reader of it would silently take register
        // nought instead.
        true => Ok(order),
        false => Err(pending.into_keys().collect()),
    }
}

pub fn merge(
    package: &ShaderPackage,
    raw: &[u8],
    stage: usize,
    want: u32,
    pooling: usize,
) -> Result<Merged, Error> {
    let mut out: Vec<String> = Vec::new();
    let held = pick::group(package, stage, want).ok_or(Error::NoSuchGroup)?;
    let columns = pick::columns(package);
    let spread = pick::spread(&held);

    let mut tuples: Vec<(Vec<u32>, usize)> = Vec::new();
    for (blob, picks) in &held.blobs {
        for tuple in picks {
            tuples.push((tuple.clone(), *blob));
        }
    }
    tuples.sort();
    let shapes: Vec<Vec<u32>> = tuples.iter().map(|(tuple, _)| tuple.clone()).collect();

    // Every condition the group can name, as `Key_Value`, which is what the merged shader's key
    // buffer declares and what the `#if`s test.
    let mut keys: Vec<String> = Vec::new();
    let mut key_at: HashMap<(usize, u32), usize> = HashMap::new();
    for (axis, values) in spread.iter().enumerate() {
        for value in values {
            let name = format!(
                "{}_{}",
                columns[held.axes[axis]],
                pick::shorten(&columns[held.axes[axis]], &pick::named(*value))
            );
            // A key nobody has recovered a name for is spelled as its hash, which is not something
            // `#define` will take.
            let name = match name.starts_with(|first: char| first.is_ascii_digit()) {
                true => format!("Key{name}"),
                false => name,
            };
            key_at.insert((axis, *value), keys.len());
            keys.push(name);
        }
    }

    let mut icb: Option<Vec<[f32; 4]>> = None;
    let mut layouts: HashMap<String, Vec<hlsl::layout::Member>> = HashMap::new();
    let mut nodes: HashMap<u64, canon::Node> = HashMap::new();
    let mut live: BTreeMap<usize, BTreeSet<u64>> = BTreeMap::new();
    let mut roots: BTreeMap<String, BTreeMap<u64, Vec<usize>>> = BTreeMap::new();
    let mut effects: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for (at, (_, blob)) in tuples.iter().enumerate() {
        let shader = &package.shaders()[*blob];
        let start = package.blobs_offset() + shader.blob_offset() as usize;
        let bytes = raw
            .get(start..start + shader.blob_size() as usize)
            .ok_or(Error::Truncated)?;
        let program = dxbc::scan_dxbc(bytes)
            .iter()
            .flat_map(|chunk| &chunk.chunks)
            .find_map(|chunk| match chunk.parse() {
                ChunkData::Shader(program) => Some(program),
                _ => None,
            })
            .ok_or(Error::NoProgram)?;
        // Every shader that binds a buffer describes it identically, so the first to mention one
        // settles what its fields are called.
        for chunk in dxbc::scan_dxbc(bytes).iter().flat_map(|held| &held.chunks) {
            let ChunkData::Rdef(reflection) = chunk.parse() else {
                continue;
            };
            for buffer in &reflection.constant_buffers {
                layouts
                    .entry(buffer.name.to_string())
                    .or_insert_with(|| hlsl::layout::members(buffer));
            }
        }
        // The immediate constant buffer belongs to the program rather than to a binding, so the
        // merged program can only carry one: two variants disagreeing about it have no single
        // source to be merged into.
        let held = table(&program);
        if !held.is_empty() {
            match &icb {
                Some(known) if *known != held => return Err(Error::Tables),
                _ => icb = Some(held),
            }
        }
        let graph = canon::build(&program, pick::naming(raw, package, *blob));
        // A loop is walked once with no back edge and nothing prints the construct, so the listing
        // for such a group would read as a program while not being one.
        if graph.loops > 0 {
            return Err(Error::Loops);
        }
        for (name, value) in &graph.roots {
            roots
                .entry(name.clone())
                .or_default()
                .entry(*value)
                .or_default()
                .push(at);
        }
        for (_, value) in &graph.effects {
            effects.entry(*value).or_default().push(at);
        }
        live.entry(*blob).or_insert_with(|| graph.live());
        nodes.extend(graph.nodes);
    }

    let mut presence: BTreeMap<u64, Vec<bool>> = BTreeMap::new();
    for (at, (_, blob)) in tuples.iter().enumerate() {
        for value in &live[blob] {
            presence
                .entry(*value)
                .or_insert_with(|| vec![false; tuples.len()])[at] = true;
        }
    }

    // Pool the branch-exclusive values that compute the same thing, so a tail that only duplicated
    // because one input differed is written once with that input `#if`d above it.
    let pooled = factor::factor(&nodes, &presence, pooling)?;

    // A pooled class is written once under everything its members reach; a hole's branches are each
    // written under their own keys, into the one register the tail reads.
    let speaks: BTreeSet<u64> = presence
        .keys()
        .copied()
        .filter(|value| pooled.speaks_for(*value))
        .collect();
    let seen_at: HashMap<u64, &Vec<bool>> = speaks
        .iter()
        .map(|value| {
            let slot = pooled.slots.of(*value);
            match pooled.shared.contains(&slot) {
                true => (*value, &pooled.presence[&slot]),
                false => (*value, &presence[value]),
            }
        })
        .collect();
    let mut kin: HashMap<u64, Vec<u64>> = HashMap::new();
    for value in &speaks {
        kin.entry(pooled.slots.of(*value)).or_default().push(*value);
    }

    // Writing a class once orders it against every branch of everything it reads, including branches
    // that never meet, and enough of those come out cyclic on a graph that has none. Un-sharing
    // whatever the cycle runs through does untangle it and is correct, but it writes every member
    // out separately: measured over the corpus that is 90,834 body lines against 57,101, so such a
    // group takes the plain layout instead.
    let order =
        schedule(&nodes, &pooled.slots, &speaks, &seen_at, &kin).map_err(|_| Error::Scheduling)?;

    let mut members: HashMap<u64, Vec<u64>> = HashMap::new();
    for value in presence.keys() {
        members
            .entry(pooled.slots.of(*value))
            .or_default()
            .push(*value);
    }

    // A guard as indices into the key table: a list of conjunctions.
    let guard = |seen: &Vec<bool>| -> synth::Guard {
        if seen.iter().all(|held| *held) {
            return Vec::new();
        }
        pick::cover(seen, &shapes, &spread)
            .iter()
            .map(|cube| {
                cube.iter()
                    .enumerate()
                    .filter(|(axis, held)| held.len() < spread[*axis].len())
                    .map(|(axis, held)| held.iter().map(|value| key_at[&(axis, *value)]).collect())
                    .collect()
            })
            .collect()
    };

    // Whatever a line reads has to have been written wherever that line runs. Pooling is what can
    // break this: the register belongs to a class, and a variant may reach the reader while every
    // member of the class that would fill the register is `#if`d out. `factor::prove` cannot see
    // this, because it checks what a class computes and not which variants emit it.
    for value in &order {
        let Some(node) = nodes.get(value) else {
            continue;
        };
        let here = seen_at[value];
        for child in &node.children {
            let class = pooled.slots.of(*child);
            let mut written = vec![false; here.len()];
            for member in kin.get(&class).into_iter().flatten() {
                for (slot, held) in written.iter_mut().zip(seen_at[member]) {
                    *slot |= held;
                }
            }
            let short: Vec<usize> = here
                .iter()
                .zip(&written)
                .enumerate()
                .filter(|(_, (wants, has))| **wants && !**has)
                .map(|(at, _)| at)
                .collect();
            if !short.is_empty() {
                return Err(Error::Unwritten);
            }
        }
    }

    // Runs of values under one guard become one region.
    let mut regions: Vec<synth::Region> = Vec::new();
    for value in &order {
        let cubes = guard(seen_at[value]);
        match regions.last_mut() {
            Some(last) if last.cubes == cubes => last.values.push(*value),
            _ => regions.push(synth::Region {
                cubes,
                values: vec![*value],
            }),
        }
    }

    // A root or an effect belongs to the variants that hold it, which may be fewer than the
    // variants holding its value.
    // (guards are collected below, then any without a region of their own get one)
    let mut root_guards: BTreeMap<String, (u64, synth::Guard)> = BTreeMap::new();
    for (name, held) in &roots {
        for (which, (value, at)) in held.iter().enumerate() {
            let mut seen = vec![false; tuples.len()];
            for one in at {
                seen[*one] = true;
            }
            root_guards.insert(format!("{name}#{which}"), (*value, guard(&seen)));
        }
    }
    let mut effect_guards: Vec<(u64, synth::Guard)> = Vec::new();
    for (value, at) in &effects {
        let mut seen = vec![false; tuples.len()];
        for one in at {
            seen[*one] = true;
        }
        effect_guards.push((*value, guard(&seen)));
    }
    // Outputs and effects are written in the last region under their guard, so every guard one of
    // them uses gets a region at the end whatever the values did. Anything a variant computed is
    // then already in its register by the time the variant's outputs are written.
    let mut trailing: BTreeSet<synth::Guard> = BTreeSet::new();
    for cubes in root_guards
        .values()
        .map(|(_, cubes)| cubes)
        .chain(effect_guards.iter().map(|(_, cubes)| cubes))
    {
        if trailing.insert(cubes.clone()) {
            regions.push(synth::Region {
                cubes: cubes.clone(),
                values: Vec::new(),
            });
        }
    }

    // An output or an effect reads a register like any other line, and a value no region wrote has
    // none: synth hands out register nought instead, which is a different shader that still
    // compiles. An effect is not a value in a register itself, so what it reads is its children.
    let reads = root_guards
        .values()
        .map(|(value, _)| std::slice::from_ref(value))
        .chain(
            effect_guards
                .iter()
                .map(|(value, _)| nodes.get(value).map_or(&[][..], |node| &node.children)),
        );
    for value in reads.flatten() {
        // A leaf is spelled where it is read unless a hole put it in a register, in which case it
        // is written like anything else and the same rule applies.
        let slot = pooled.slots.of(*value);
        if nodes.get(value).is_none_or(|node| node.children.is_empty()) && members[&slot].len() == 1
        {
            continue;
        }
        let written = regions.iter().any(|region| {
            region
                .values
                .iter()
                .any(|held| pooled.slots.of(*held) == slot)
        });
        if !written {
            return Err(Error::Unwritten);
        }
    }

    // Every member of a class reads the class register, not only the ones that emit a line: a
    // shared class emits its representative alone, and a reader still naming one of the members it
    // stood for would name a register nothing ever wrote. A class of one moves nothing, and telling
    // synth otherwise would put every leaf in a register.
    let mut pool: HashMap<u64, u64> = HashMap::new();
    for (slot, class) in &members {
        if class.len() > 1 {
            pool.extend(class.iter().map(|value| (*value, *slot)));
        }
    }

    let whole: BTreeSet<u64> = presence.keys().copied().collect();
    let built = synth::Synth::new(&nodes)
        .table(icb.unwrap_or_default())
        .pooled(pool)
        .build(
            &whole,
            &regions,
            &root_guards,
            &effect_guards,
            keys.clone(),
            match stage {
                0 => "vs",
                1 => "ps",
                2 => "hs",
                3 => "ds",
                _ => "gs",
            },
        );

    let names = naming(package, &built, &keys, &layouts);
    let read = hlsl::decompile(&built.program, &names);
    // The guard text comes from the cubes rather than from reading the condition back out of the
    // listing: the decompiler spells a bitwise `and` of key reads, which is the same condition but
    // not the same words.
    // Which keys belong to the same axis as each key, so a disjunction naming most of an axis can
    // be written as the negation of the few it leaves out.
    let mut axis_keys: Vec<Vec<usize>> = vec![Vec::new(); keys.len()];
    let mut first = 0;
    for values in &spread {
        let run: Vec<usize> = (first..first + values.len()).collect();
        for key in &run {
            axis_keys[*key] = run.clone();
        }
        first += values.len();
    }

    // Only the regions synth actually opened, since the text pass pairs them with blocks by
    // position and a region that emitted nothing has no block.
    let printed: Vec<Guarded> = built
        .kept
        .iter()
        .filter_map(|at| regions.get(*at))
        .map(|region| guarded(&region.cubes, &keys, &axis_keys))
        .collect();

    out.push(format!(
        "// {} {}",
        pick::tag(stage),
        pick::named(held.pass)
    ));
    out.push(format!(
        "// {} compiled shaders, {} key combinations, merged into one source",
        held.blobs.len(),
        tuples.len()
    ));
    out.push("//".to_owned());
    // `g_ShaderKeys` is not in here: it was only ever how the guards reached the decompiler, and
    // the conditions are the preprocessor's now.
    out.push("// slot table for repacking:".to_owned());
    for (slot, (name, size)) in built.table.constants.iter().enumerate() {
        out.push(format!("//   b{slot:<3} {name} ({size} registers)"));
    }
    for (slot, name) in built.table.textures.iter().enumerate() {
        out.push(format!("//   t{slot:<3} {name}"));
    }
    for (slot, name) in built.table.samplers.iter().enumerate() {
        out.push(format!("//   s{slot:<3} {name}"));
    }
    for (slot, (name, mask)) in built.table.inputs.iter().enumerate() {
        out.push(format!("//   v{slot:<3} {name} mask {mask:x}"));
    }
    for (slot, (name, mask)) in built.table.outputs.iter().enumerate() {
        out.push(format!("//   o{slot:<3} {name} mask {mask:x}"));
    }
    // Each variant as the defines that select it, so a gate can compile the source once per
    // combination the package actually ships.
    for (tuple, blob) in &tuples {
        let held: Vec<&str> = tuple
            .iter()
            .enumerate()
            .map(|(axis, value)| keys[key_at[&(axis, *value)]].as_str())
            .collect();
        out.push(format!("// variant {blob} {}", held.join(" ")));
    }
    out.push(String::new());
    let body = out.len();
    // The same header stands over either reading, so a fold and a slot table mean one thing.
    let mut asm = out.clone();
    let listing: Vec<String> = dxbc::shex::format_program(&built.program)
        .lines()
        .map(str::to_owned)
        .collect();
    asm.extend(guards(&listing, built.table.constants.len(), &printed)?);
    out.extend(strip(preprocess(&read.lines, &keys, &printed)?));
    Ok(Merged {
        regions: out.iter().filter(|line| line.starts_with("#if")).count(),
        variants: tuples.len(),
        blobs: held.blobs.len(),
        body,
        lines: out,
        asm,
    })
}

/// The immediate constant buffer a program carries, if it has one.
pub(crate) fn table(program: &dxbc::shex::Program) -> Vec<[f32; 4]> {
    program
        .instructions
        .iter()
        .find_map(|held| match &held.kind {
            InstructionKind::CustomData {
                subtype: dxbc::shex::CustomDataType::ImmediateConstantBuffer,
                values,
                ..
            } => Some(values.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// The names for the merged program, which are the union table rather than any one variant's.
/// The buffer a material fills, whose fields the reflection leaves as one bare array. The package's
/// own parameter table says which bytes each parameter takes, and the curated hash table names them,
/// so between the two the registers can be declared after all.
fn material(package: &ShaderPackage) -> Vec<hlsl::Field> {
    let mut fields = Vec::new();
    for param in package.material_params() {
        let Some(name) = shaders::names::resolve(param.id()) else {
            continue;
        };
        let first = usize::from(param.byte_offset()) / 4;
        let mut mask = 0u8;
        for step in 0..usize::from(param.byte_size()) / 4 {
            // A parameter wider than a register carries on into the next, which the field's own
            // size already says; only the one it starts in needs the components marking.
            if first + step < (first / 4 + 1) * 4 {
                mask |= 1 << ((first + step) % 4);
            }
        }
        fields.push(hlsl::Field::packed(
            name.to_owned(),
            param.byte_size(),
            (first / 4) as u32,
            mask,
        ));
    }
    fields
}

fn naming(
    package: &ShaderPackage,
    built: &synth::Built,
    keys: &[String],
    layouts: &HashMap<String, Vec<hlsl::layout::Member>>,
) -> hlsl::Names {
    let mut names = hlsl::Names::default();
    for (slot, (name, _)) in built.table.constants.iter().enumerate() {
        // What the bytecode's reflection says the buffer holds, so a read comes out as the field it
        // names rather than as a register of a nameless array.
        let fields = match layouts.get(name) {
            Some(members) if !members.is_empty() => members
                .iter()
                .map(|member| {
                    hlsl::Field::described(
                        member.name.clone(),
                        member.kind.clone(),
                        member.offset,
                        member.size,
                    )
                })
                .collect(),
            _ => match name.as_str() {
                "g_MaterialParameter" => material(package),
                _ => Vec::new(),
            },
        };
        names
            .constants
            .insert(slot as u16, hlsl::Buffer::new(name.clone(), fields));
    }
    // The key buffer names each condition, so a guard reads as the condition rather than as a slot.
    let fields = keys
        .iter()
        .enumerate()
        .map(|(at, name)| hlsl::Field::packed(name.clone(), 4, (at / 4) as u32, 1 << (at % 4)))
        .collect();
    names.constants.insert(
        built.table.constants.len() as u16,
        hlsl::Buffer::new("g_ShaderKeys".to_owned(), fields),
    );
    for (slot, name) in built.table.textures.iter().enumerate() {
        names.textures.insert(slot as u16, name.clone());
    }
    for (slot, name) in built.table.samplers.iter().enumerate() {
        names.samplers.insert(slot as u16, name.clone());
    }
    for (into, held) in [
        (&mut names.inputs, &built.table.inputs),
        (&mut names.outputs, &built.table.outputs),
    ] {
        for (slot, (name, mask)) in held.iter().enumerate() {
            let digits = name.len()
                - name
                    .trim_end_matches(|held: char| held.is_ascii_digit())
                    .len();
            let (base, index) = name.split_at(name.len() - digits);
            // The integer system values have to be declared as such or the pipeline rejects them.
            let kind = match base {
                "SV_INSTANCEID"
                | "SV_VERTEXID"
                | "SV_PRIMITIVEID"
                | "SV_SAMPLEINDEX"
                | "SV_GSINSTANCEID"
                | "SV_COVERAGE"
                | "SV_OUTPUTCONTROLPOINTID" => 1,
                _ => 0,
            };
            into.insert(
                slot as u32,
                hlsl::Semantic::new(base, index.parse().unwrap_or(0), kind, *mask),
            );
        }
    }
    names
}

/// Drop the key buffer's own declaration: the conditions live in the preprocessor now.
fn strip(lines: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        if !(lines[at].starts_with("struct ") || lines[at].starts_with("cbuffer ")) {
            out.push(lines[at].clone());
            at += 1;
            continue;
        }
        let mut block = Vec::new();
        while at < lines.len() {
            block.push(lines[at].clone());
            let done = lines[at].starts_with("};") || lines[at].starts_with('}');
            at += 1;
            if done {
                break;
            }
        }
        if !block.iter().any(|line| line.contains("g_ShaderKeys")) {
            out.extend(block);
        }
    }
    out
}

/// A guard as a preprocessor condition.
/// How a region's guard is written.
///
/// A plain conjunction comes back as its conditions, in key order, so that regions sharing a
/// leading condition can share the `#ifdef` that opens it. Anything with a disjunction in it has no
/// such prefix and is written whole.
struct Guarded {
    /// The conditions that open this region, outermost first, each as the directive that opens it
    /// and as the term it becomes inside a larger expression. A region beginning with the same ones
    /// as the region before writes only what it adds, which is what makes the blocks nest.
    levels: Vec<(String, String)>,
    /// The keys the guard's own cubes name. How it is written may name others — a negation names
    /// what it rules out — but this is what the block it lands on has to agree with.
    mentions: BTreeSet<String>,
}

/// One axis's constraint as preprocessor text, both as a directive of its own and as a term inside
/// a larger expression. Naming all but a few of an axis's values reads better as what it rules out.
fn condition(axis: &[usize], keys: &[String], axis_keys: &[Vec<usize>]) -> (String, String) {
    let spelled = |held: &[usize]| -> String {
        let parts: Vec<String> = held
            .iter()
            .map(|key| format!("defined({})", keys[*key]))
            .collect();
        match parts.as_slice() {
            [only] => only.clone(),
            _ => format!("({})", parts.join(" || ")),
        }
    };
    let whole = axis
        .first()
        .map(|key| axis_keys[*key].as_slice())
        .unwrap_or(&[]);
    let missing: Vec<usize> = whole
        .iter()
        .copied()
        .filter(|key| !axis.contains(key))
        .collect();
    if !missing.is_empty() && missing.len() < axis.len() {
        return match missing.as_slice() {
            [only] => (
                format!("#ifndef {}", keys[*only]),
                format!("!defined({})", keys[*only]),
            ),
            _ => {
                let text = format!("!{}", spelled(&missing));
                (format!("#if {text}"), text)
            }
        };
    }
    match axis {
        [only] => (
            format!("#ifdef {}", keys[*only]),
            format!("defined({})", keys[*only]),
        ),
        _ => {
            let text = spelled(axis);
            (format!("#if {text}"), text)
        }
    }
}

fn guarded(cubes: &synth::Guard, keys: &[String], axis_keys: &[Vec<usize>]) -> Guarded {
    let mentions: BTreeSet<String> = cubes
        .iter()
        .flatten()
        .flatten()
        .map(|key| keys[*key].clone())
        .collect();

    // Whatever every cube insists on is true of the whole guard, so it is hoisted into levels of
    // its own and only what the cubes disagree about is left to spell out.
    let shared: Vec<Vec<usize>> = match cubes.split_first() {
        None => Vec::new(),
        Some((head, rest)) => head
            .iter()
            .filter(|axis| rest.iter().all(|cube| cube.contains(axis)))
            .cloned()
            .collect(),
    };
    let mut levels: Vec<(String, String)> = Vec::new();
    let mut order: Vec<Vec<usize>> = shared.clone();
    order.sort_unstable();
    for axis in &order {
        levels.push(condition(axis, keys, axis_keys));
    }

    let terms: Vec<String> = cubes
        .iter()
        .map(|cube| {
            let rest: Vec<String> = cube
                .iter()
                .filter(|axis| !shared.contains(axis))
                .map(|axis| condition(axis, keys, axis_keys).1)
                .collect();
            match rest.as_slice() {
                [] => String::new(),
                [only] => only.clone(),
                _ => format!("({})", rest.join(" && ")),
            }
        })
        .filter(|term| !term.is_empty())
        .collect();
    if !terms.is_empty() {
        let text = match terms.as_slice() {
            [only] if only.starts_with('(') => only[1..only.len() - 1].to_owned(),
            [only] => only.clone(),
            _ => terms.join(" || "),
        };
        levels.push((format!("#if {text}"), format!("({text})")));
    }
    Guarded { levels, mentions }
}

/// How much of a condition an `#endif` repeats before it is doing more harm than the reminder is
/// worth.
const RECALL: usize = 60;

/// The assembly with its guards as preprocessor directives, the way the HLSL reading has them.
///
/// In the program a guard is a register built from key-buffer reads and an `if` on it. None of that
/// survives into a source where the condition is the preprocessor's, so the reads, the register that
/// combines them and the `if` itself all come out, and the block they wrapped is left where it is.
fn guards(lines: &[String], keys_at: usize, printed: &[Guarded]) -> Result<Vec<String>, Error> {
    let buffer = format!("cb{keys_at}[");
    let mut out: Vec<String> = Vec::new();
    let mut open: Vec<String> = Vec::new();
    let mut depth: Vec<bool> = Vec::new();
    let mut from_keys: BTreeSet<String> = BTreeSet::new();
    let mut taken = 0;

    for line in lines {
        let held = line.trim();
        let mut words = held.split_whitespace();
        let opcode = words.next().unwrap_or_default();
        let rest: Vec<String> = words
            .map(|word| word.trim_end_matches(',').to_owned())
            .collect();
        let register = |operand: &str| operand.split('.').next().unwrap_or(operand).to_owned();

        if opcode == "dcl_constantbuffer" && held.contains(&buffer) {
            continue;
        }
        if opcode == "if" {
            let on_keys = rest
                .first()
                .is_some_and(|held| from_keys.contains(&register(held)));
            depth.push(on_keys);
            if !on_keys {
                out.push(line.clone());
                continue;
            }
            let want = printed.get(taken).ok_or(Error::Regions)?;
            taken += 1;
            let same = open
                .iter()
                .zip(&want.levels)
                .take_while(|(open, want)| **open == want.0)
                .count();
            while open.len() > same {
                if let Some(held) = open.pop() {
                    out.push(closes(&held));
                }
            }
            for level in &want.levels[same..] {
                out.push(level.0.clone());
                open.push(level.0.clone());
            }
            continue;
        }
        if opcode == "endif" {
            match depth.pop() {
                // Closing is deferred so the next region can keep what it shares with this one.
                Some(true) => {}
                _ => out.push(line.clone()),
            }
            continue;
        }
        // A line whose every source is the key buffer or something already built from it exists
        // only to phrase a condition the preprocessor now carries.
        if let Some((dest, sources)) = rest.split_first()
            && !sources.is_empty()
            && sources
                .iter()
                .all(|held| held.starts_with(&buffer) || from_keys.contains(&register(held)))
        {
            from_keys.insert(register(dest));
            continue;
        }
        // Anything still inside a guarded block loses the indent that block gave it.
        out.push(match open.is_empty() {
            true => line.clone(),
            false => line.strip_prefix("  ").unwrap_or(line).to_owned(),
        });
    }
    while let Some(held) = open.pop() {
        out.push(closes(&held));
    }
    match taken == printed.len() {
        true => Ok(out),
        false => Err(Error::Regions),
    }
}

/// What an `#endif` closes, so a run of them can be read from the bottom. `defined()` is dropped:
/// the names are the point, and the comment is not compiled.
fn closes(directive: &str) -> String {
    if let Some(key) = directive.strip_prefix("#ifndef ") {
        return format!("#endif // !{key}");
    }
    let mut rest = directive
        .strip_prefix("#ifdef ")
        .or_else(|| directive.strip_prefix("#if "))
        .unwrap_or(directive);
    // Only the `defined(...)` wrappers come off; the parentheses that group the expression stay.
    let mut held = String::new();
    while let Some(at) = rest.find("defined(") {
        held.push_str(&rest[..at]);
        rest = &rest[at + "defined(".len()..];
        if let Some(close) = rest.find(')') {
            held.push_str(&rest[..close]);
            rest = &rest[close + 1..];
        }
    }
    held.push_str(rest);
    match held.char_indices().nth(RECALL) {
        None => format!("#endif // {held}"),
        Some((at, _)) => format!("#endif // {} ...", held[..at].trim_end()),
    }
}

/// Whether the text reads the key buffer by name.
fn spells(text: &str, keys: &[String]) -> bool {
    text.contains("g_ShaderKeys") || keys.iter().any(|key| text.contains(key.as_str()))
}

/// The register names a fragment mentions.
fn names(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|held: char| !(held.is_ascii_alphanumeric() || held == '_'))
        .filter(|held| {
            held.starts_with('r')
                && held.len() > 1
                && held[1..].chars().all(|held| held.is_ascii_digit())
        })
        .map(str::to_owned)
}

/// Turn the top-level `if (...)` blocks over the key buffer back into `#if`, in the order the
/// regions were laid down.
fn preprocess(
    lines: &[String],
    keys: &[String],
    printed: &[Guarded],
) -> Result<Vec<String>, Error> {
    let mut out: Vec<String> = Vec::new();
    let mut at = 0;
    let mut taken = 0;
    // Conditions currently open, outermost first. A region that begins with the same ones writes
    // only what it adds, which is what turns a column of `(A && B && C)` into blocks that nest.
    let mut open: Vec<String> = Vec::new();
    // Registers holding nothing but a key condition, which the decompiler builds before the `if`
    // that tests them rather than inside it.
    let mut from_keys: BTreeSet<String> = BTreeSet::new();
    while at < lines.len() {
        let line = &lines[at];
        let trimmed = line.trim();
        let held = trimmed.trim_end_matches(';');
        let guard = trimmed
            .strip_prefix("if (")
            .and_then(|rest| rest.strip_suffix(')'))
            .filter(|rest| names(rest).any(|held| from_keys.contains(&held)) || spells(rest, keys));
        let Some(condition) = guard else {
            // A line that computes nothing but a key condition exists only to phrase a guard the
            // preprocessor now carries, and it reads a buffer this source does not declare. The
            // decompiler puts a compound condition in a register first, so these are not always
            // folded into the `if` that uses them.
            if let Some((into, rhs)) = held.split_once(" = ")
                && (spells(rhs, keys)
                    || (names(rhs).next().is_some()
                        && names(rhs).all(|held| from_keys.contains(&held))))
            {
                for held in names(into) {
                    from_keys.insert(held);
                }
                at += 1;
                continue;
            }
            // Anything not under a guard belongs at the outermost level, so whatever is still open
            // has to close before it.
            while let Some(held) = open.pop() {
                out.push(closes(&held));
            }
            out.push(line.clone());
            at += 1;
            continue;
        };
        if std::env::var("ANYIF").is_ok() {
            eprintln!("block {} says {}", taken, condition);
        }
        let want = printed.get(taken).ok_or(Error::Regions)?;

        // The substitution is positional, so check the block really is the region it is being given:
        // the keys the decompiler spelled have to be the keys the region names. Both sides go
        // through the same test, since one key name can sit inside another (`..._Face` inside
        // `..._FaceEmissive`) and only matching tests cancel that out.
        let mentioned = |text: &str| -> BTreeSet<&str> {
            keys.iter()
                .map(String::as_str)
                .filter(|key| text.contains(*key))
                .collect()
        };
        let named: Vec<&str> = want.mentions.iter().map(String::as_str).collect();
        // Only where the condition still spells its keys. A compound one the decompiler put in a
        // register names none of them, so there is nothing to check it against — the pairing is
        // positional either way, and dxc plus the projection check cover what this cannot.
        if spells(condition, keys) && mentioned(condition) != mentioned(&named.join(" ")) {
            if std::env::var("WHY").is_ok() {
                eprintln!("block {taken} of {}", printed.len());
                eprintln!("  listing says: {condition}");
                eprintln!("  region says:  {}", named.join(" "));
            }
            return Err(Error::Regions);
        }
        taken += 1;
        at += 1;
        // Skip the brace the block opens with, and drop one level of indent until it closes.
        if lines.get(at).map(|held| held.trim()) == Some("{") {
            at += 1;
        }
        let mut body: Vec<String> = Vec::new();
        let mut depth = 1;
        while at < lines.len() && depth > 0 {
            let held = lines[at].trim();
            if held == "{" {
                depth += 1;
            }
            if held == "}" {
                depth -= 1;
                if depth == 0 {
                    at += 1;
                    break;
                }
            }
            body.push(
                lines[at]
                    .strip_prefix("    ")
                    .unwrap_or(&lines[at])
                    .to_owned(),
            );
            at += 1;
        }
        // Every value of the region folded into what reads it, so the region has nothing to say.
        if body.is_empty() {
            continue;
        }
        // Keep what this region shares with the one before and close the rest, so a condition is
        // written once for the run of regions under it.
        let same = open
            .iter()
            .zip(&want.levels)
            .take_while(|(open, want)| **open == want.0)
            .count();
        while open.len() > same {
            if let Some(held) = open.pop() {
                out.push(closes(&held));
            }
        }
        // A condition earns a block of its own where it groups something: either the region before
        // is already under it, or the region after will be. What nothing shares stays one `#if`,
        // rather than wrapping a single line in a column of directives it is the only thing under.
        let ahead = printed
            .get(taken)
            .map(|next| {
                want.levels
                    .iter()
                    .zip(&next.levels)
                    .take_while(|(held, next)| held.0 == next.0)
                    .count()
            })
            .unwrap_or(0);
        let nest = same.max(ahead).min(want.levels.len());
        for level in &want.levels[same..nest] {
            out.push(level.0.clone());
            open.push(level.0.clone());
        }
        match &want.levels[nest..] {
            [] => {}
            [only] => {
                out.push(only.0.clone());
                open.push(only.0.clone());
            }
            rest => {
                let text = rest
                    .iter()
                    .map(|level| level.1.as_str())
                    .collect::<Vec<_>>()
                    .join(" && ");
                out.push(format!("#if {text}"));
                open.push(format!("#if {text}"));
            }
        }
        out.extend(body);
    }
    while let Some(held) = open.pop() {
        out.push(closes(&held));
    }
    match taken == printed.len() {
        true => Ok(out),
        false => Err(Error::Regions),
    }
}
