//! Rebuilding a merged value graph into one SM5 program.
//!
//! The decompiler reads a `dxbc::Program`, so the way to get real HLSL out of a merge is to make
//! the merge a program again: one instruction per value, every guard region wrapped in `if`/`endif`
//! over a synthetic key buffer, and one slot table covering every resource any variant binds. That
//! is decompiled once, so there is a single set of folding decisions and nothing to reconcile.
//!
//! Guards come out as `if (g_ShaderKeys.KEY_VALUE)` blocks, which a text pass turns into `#if`.
//! Emitting the preprocessor directly would mean teaching the emitter about a scope the block tree
//! already gives for free.

use crate::canon::{Kind, Leaf, Node};
use dxbc::shex::{
    ComponentSelect, Immediates, Indices, Instruction, InstructionKind, Opcode, Operand,
    OperandIndex, Operands, Program, RegisterType, ReturnType,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The slot every name in the merged shader is bound to, which is what a packager needs to write
/// the per-blob resource tables back.
#[derive(Default)]
pub struct Table {
    pub constants: Vec<(String, u32)>,
    pub textures: Vec<String>,
    pub samplers: Vec<String>,
    pub inputs: Vec<(String, u8)>,
    pub outputs: Vec<(String, u8)>,
    /// Textures read by a structured or raw load, which declare as buffers rather than as images.
    pub buffers: BTreeSet<String>,
    /// The key conditions, in the order they are declared in the key buffer.
    pub keys: Vec<String>,
}

impl Table {
    fn slot_of(held: &[(String, u32)], name: &str) -> Option<u32> {
        held.iter()
            .position(|(held, _)| held == name)
            .map(|at| at as u32)
    }
}

fn operand(reg_type: RegisterType, components: ComponentSelect, indices: Vec<u32>) -> Operand {
    Operand {
        reg_type,
        components,
        negate: false,
        abs: false,
        indices: indices.into_iter().map(OperandIndex::Imm32).collect(),
        immediate_values: Immediates::new(),
    }
}

fn temp(index: u32, components: ComponentSelect) -> Operand {
    operand(RegisterType::Temp, components, vec![index])
}

fn literal(bits: [u32; 4]) -> Operand {
    Operand {
        reg_type: RegisterType::Immediate32,
        components: ComponentSelect::Swizzle([0, 1, 2, 3]),
        negate: false,
        abs: false,
        indices: Indices::new(),
        immediate_values: bits.into_iter().collect(),
    }
}

fn generic(
    opcode: Opcode,
    saturate: bool,
    test_nonzero: bool,
    operands: Vec<Operand>,
) -> Instruction {
    Instruction {
        opcode,
        saturate,
        test_nonzero,
        precise_mask: 0,
        resinfo_return_type: None,
        sync_flags: 0,
        tex_offsets: None,
        resource_dim: None,
        resource_return_type: None,
        kind: InstructionKind::Generic {
            operands: operands.into_iter().collect::<Operands>(),
        },
    }
}

/// A guard: a disjunction of cubes, each an conjunction over axes, each a disjunction of the key
/// conditions that axis admits. Empty is unconditional.
pub type Guard = Vec<Vec<Vec<usize>>>;

/// A region of the merged listing: the values it holds and the key condition that admits them.
pub struct Region {
    pub cubes: Guard,
    pub values: Vec<u64>,
}

pub struct Built {
    pub program: Program,
    pub table: Table,
    /// Which regions actually put an instruction in the program, by index.
    ///
    /// A region can turn out to hold nothing that emits — its values were leaves read where they are
    /// used, or pooling gave them all to another class. Guarding nothing is a block the decompiler is
    /// entitled to drop, and the text pass pairs blocks with regions by position, so what was not
    /// emitted must not be counted.
    pub kept: Vec<usize>,
}

pub struct Synth<'a> {
    nodes: &'a HashMap<u64, Node>,
    table: Table,
    sizes: BTreeMap<String, u32>,
    /// Which temp holds each value, and how many components of it are live.
    at: HashMap<u64, u32>,
    /// Values factoring pooled into one register, as member to the class standing for it. Branches
    /// of a hole write the same temp, which is what lets the tail below them be written once.
    slot: HashMap<u64, u64>,
    next: u32,
    out: Vec<Instruction>,
    pixel: bool,
    /// The immediate constant buffer, which belongs to the program rather than to a binding and so
    /// has to be carried over rather than recovered from the graph.
    icb: Vec<[f32; 4]>,
    /// Whether the shader writes depth, which is declared rather than bound.
    depth: bool,
}

impl<'a> Synth<'a> {
    pub fn new(nodes: &'a HashMap<u64, Node>) -> Self {
        Self {
            nodes,
            table: Table::default(),
            sizes: BTreeMap::new(),
            at: HashMap::new(),
            slot: HashMap::new(),
            next: 0,
            out: Vec::new(),
            pixel: true,
            icb: Vec::new(),
            depth: false,
        }
    }

    pub fn table(mut self, icb: Vec<[f32; 4]>) -> Self {
        self.icb = icb;
        self
    }

    pub fn pooled(mut self, slot: HashMap<u64, u64>) -> Self {
        self.slot = slot;
        self
    }

    /// The register a value is read from and written to, which its class decides.
    fn key(&self, value: u64) -> u64 {
        self.slot.get(&value).copied().unwrap_or(value)
    }

    /// Walk every live value once to collect what the merged shader has to bind.
    fn survey(&mut self, live: &BTreeSet<u64>, roots: &BTreeMap<String, u64>) {
        let mut textures = BTreeSet::new();
        let mut samplers = BTreeSet::new();
        let mut buffers = BTreeSet::new();
        // A structured or raw load is a buffer read, and declaring the resource as an image makes
        // its `Load` return a vector where the shader wants a word.
        for value in live {
            let Some(node) = self.nodes.get(value) else {
                continue;
            };
            let Kind::Ins { opcode, .. } = &node.kind else {
                continue;
            };
            if !opcode.name().starts_with("ld_") {
                continue;
            }
            for child in &node.children {
                if let Some(held) = self.nodes.get(child)
                    && let Kind::Leaf(Leaf::Resource(name)) = &held.kind
                {
                    buffers.insert(name.clone());
                }
            }
        }
        let mut inputs: BTreeMap<String, u8> = BTreeMap::new();
        for value in live {
            let Some(node) = self.nodes.get(value) else {
                continue;
            };
            let Kind::Leaf(leaf) = &node.kind else {
                continue;
            };
            match leaf {
                Leaf::Constant {
                    buffer,
                    register,
                    comp,
                    ..
                } => {
                    let _ = comp;
                    let slot = self.sizes.entry(buffer.clone()).or_default();
                    *slot = (*slot).max(register + 1);
                }
                Leaf::Resource(name) => {
                    textures.insert(name.clone());
                }
                Leaf::Sampler(name) => {
                    samplers.insert(name.clone());
                }
                Leaf::Input { semantic, comp } => {
                    let mask = inputs.entry(semantic.clone()).or_default();
                    *mask |= 1 << comp;
                    // The pipeline gives a position all four components whatever the shader reads.
                    if semantic.starts_with("SV_POSITION") {
                        *mask = 0xF;
                    }
                }
                _ => {}
            }
        }
        // A dynamically indexed buffer is read past whatever register the walk saw, so give it room.
        for value in live {
            if let Some(node) = self.nodes.get(value)
                && let Kind::Leaf(Leaf::Constant {
                    buffer,
                    dynamic: true,
                    ..
                }) = &node.kind
            {
                let slot = self.sizes.entry(buffer.clone()).or_default();
                *slot = (*slot).max(4096);
            }
        }
        self.table.constants = self
            .sizes
            .iter()
            .map(|(name, size)| (name.clone(), *size))
            .collect();
        self.table.buffers = buffers;
        self.table.textures = textures.into_iter().collect();
        self.table.samplers = samplers.into_iter().collect();
        self.table.inputs = inputs.into_iter().collect();

        let mut outputs: BTreeMap<String, u8> = BTreeMap::new();
        for name in roots.keys() {
            // One component can hold a different value in different variants, so a name carries a
            // disambiguator: `SV_Target0.x#2`. Each is written under its own guard.
            let base = name
                .rsplit_once('#')
                .map_or(name.as_str(), |(base, _)| base);
            let Some((semantic, comp)) = base.rsplit_once('.') else {
                continue;
            };
            let bit = match comp {
                "x" => 1,
                "y" => 2,
                "z" => 4,
                _ => 8,
            };
            *outputs.entry(semantic.to_owned()).or_default() |= bit;
        }
        self.table.outputs = outputs.into_iter().collect();
    }

    /// The operand a value reads as, which for a leaf is the thing itself and for anything else is
    /// the temp it was put in.
    fn read(&self, value: u64, lane: u8) -> Operand {
        let Some(node) = self.nodes.get(&value) else {
            return literal([0; 4]);
        };
        // A leaf is normally spelled where it is used, but one that is a branch of a hole was put in
        // the class register so the shared tail below could read it from one place.
        if let Some(held) = self.at.get(&self.key(value)) {
            return match node.kind {
                Kind::Vec => temp(*held, ComponentSelect::Swizzle([0, 1, 2, 3])),
                _ => temp(*held, ComponentSelect::Swizzle([lane; 4])),
            };
        }
        self.spell(value, lane)
    }

    /// The value written out as itself, whatever register it may also have been given.
    fn spell(&self, value: u64, lane: u8) -> Operand {
        let Some(node) = self.nodes.get(&value) else {
            return literal([0; 4]);
        };
        match &node.kind {
            Kind::Leaf(leaf) => match leaf {
                Leaf::Immediate(bits) => literal([*bits; 4]),
                Leaf::Input { semantic, comp } => {
                    let slot = self
                        .table
                        .inputs
                        .iter()
                        .position(|(held, _)| held == semantic)
                        .unwrap_or(0) as u32;
                    operand(
                        RegisterType::Input,
                        ComponentSelect::Swizzle([*comp; 4]),
                        vec![slot],
                    )
                }
                Leaf::Constant {
                    buffer,
                    register,
                    comp,
                    dynamic,
                } => {
                    let slot = Table::slot_of(&self.table.constants, buffer).unwrap_or(0);
                    match dynamic {
                        // A dynamic index is the node's own child, already in a temp.
                        true => {
                            let index = node.children.first().copied().unwrap_or(0);
                            let held = self.at.get(&index).copied().unwrap_or(0);
                            Operand {
                                reg_type: RegisterType::ConstantBuffer,
                                components: ComponentSelect::Swizzle([*comp; 4]),
                                negate: false,
                                abs: false,
                                indices: vec![
                                    OperandIndex::Imm32(slot),
                                    OperandIndex::RelativePlusImm(
                                        *register,
                                        Box::new(temp(held, ComponentSelect::Swizzle([0; 4]))),
                                    ),
                                ]
                                .into_iter()
                                .collect(),
                                immediate_values: Immediates::new(),
                            }
                        }
                        false => operand(
                            RegisterType::ConstantBuffer,
                            ComponentSelect::Swizzle([*comp; 4]),
                            vec![slot, *register],
                        ),
                    }
                }
                Leaf::ImmConst {
                    register,
                    comp,
                    dynamic,
                } => match dynamic {
                    true => {
                        let index = node.children.first().copied().unwrap_or(0);
                        let held = self.at.get(&self.key(index)).copied().unwrap_or(0);
                        Operand {
                            reg_type: RegisterType::ImmConstBuffer,
                            components: ComponentSelect::Swizzle([*comp; 4]),
                            negate: false,
                            abs: false,
                            indices: vec![OperandIndex::RelativePlusImm(
                                *register,
                                Box::new(temp(held, ComponentSelect::Swizzle([0; 4]))),
                            )]
                            .into_iter()
                            .collect(),
                            immediate_values: Default::default(),
                        }
                    }
                    false => operand(
                        RegisterType::ImmConstBuffer,
                        ComponentSelect::Swizzle([*comp; 4]),
                        vec![*register],
                    ),
                },
                Leaf::Resource(name) => {
                    let slot = self
                        .table
                        .textures
                        .iter()
                        .position(|held| held == name)
                        .unwrap_or(0) as u32;
                    operand(
                        RegisterType::Resource,
                        ComponentSelect::Swizzle([0, 1, 2, 3]),
                        vec![slot],
                    )
                }
                Leaf::Sampler(name) => {
                    let slot = self
                        .table
                        .samplers
                        .iter()
                        .position(|held| held == name)
                        .unwrap_or(0) as u32;
                    operand(
                        RegisterType::Sampler,
                        ComponentSelect::Swizzle([0; 4]),
                        vec![slot],
                    )
                }
                _ => literal([0; 4]),
            },
            // Everything else was put in a temp; a bundle spreads over its lanes, anything else
            // sits in x.
            Kind::Vec => temp(
                self.at.get(&value).copied().unwrap_or(0),
                ComponentSelect::Swizzle([0, 1, 2, 3]),
            ),
            _ => temp(
                self.at.get(&value).copied().unwrap_or(0),
                ComponentSelect::Swizzle([lane; 4]),
            ),
        }
    }

    fn claim(&mut self, value: u64) -> u32 {
        let key = self.key(value);
        // Every branch of a hole writes the register its class already took.
        if let Some(held) = self.at.get(&key) {
            return *held;
        }
        let held = self.next;
        self.next += 1;
        self.at.insert(key, held);
        held
    }

    /// Put one value into a temp of its own.
    fn value(&mut self, value: u64) {
        let Some(node) = self.nodes.get(&value).cloned() else {
            return;
        };
        match &node.kind {
            // A leaf is read where it is used, unless it is one branch of a hole, which has to go
            // into the class register for the shared tail to find it.
            Kind::Leaf(_) if !self.slot.contains_key(&value) => {}
            Kind::Leaf(_) => {
                let source = self.spell(value, 0);
                let into = self.claim(value);
                self.out.push(generic(
                    Opcode::Mov,
                    false,
                    true,
                    vec![temp(into, ComponentSelect::Mask(1)), source],
                ));
            }
            Kind::Vec => {
                let into = self.claim(value);
                for (lane, child) in node.children.iter().enumerate().take(4) {
                    let source = self.read(*child, 0);
                    self.out.push(generic(
                        Opcode::Mov,
                        false,
                        true,
                        vec![temp(into, ComponentSelect::Mask(1 << lane)), source],
                    ));
                }
            }
            Kind::Not => {
                let into = self.claim(value);
                let source = self.read(node.children[0], 0);
                self.out.push(generic(
                    Opcode::Not,
                    false,
                    true,
                    vec![temp(into, ComponentSelect::Mask(1)), source],
                ));
            }
            Kind::Neg | Kind::Abs => {
                let into = self.claim(value);
                let mut source = self.read(node.children[0], 0);
                match node.kind {
                    Kind::Neg => source.negate = true,
                    _ => source.abs = true,
                }
                self.out.push(generic(
                    Opcode::Mov,
                    false,
                    true,
                    vec![temp(into, ComponentSelect::Mask(1)), source],
                ));
            }
            Kind::Phi => {
                let into = self.claim(value);
                let cond = self.read(node.children[0], 0);
                let left = self.read(node.children[1], 0);
                let right = self.read(node.children[2], 0);
                self.out.push(generic(
                    Opcode::Movc,
                    false,
                    true,
                    vec![temp(into, ComponentSelect::Mask(1)), cond, left, right],
                ));
            }
            // The branch conditions in force where the value sits, which an effect under them has
            // to be gated on. `if_nz` and a comparison both leave a lane of all-ones or none, so
            // the conjunction is a bitwise `and`.
            Kind::Path => {
                let mut all: Option<u32> = None;
                for child in &node.children {
                    let held = match self.at.get(&self.key(*child)) {
                        Some(held) => *held,
                        None => {
                            let into = self.next;
                            self.next += 1;
                            let source = self.read(*child, 0);
                            self.out.push(generic(
                                Opcode::Mov,
                                false,
                                true,
                                vec![temp(into, ComponentSelect::Mask(1)), source],
                            ));
                            into
                        }
                    };
                    all = Some(self.join(all, held, Opcode::And));
                }
                if let Some(all) = all {
                    self.at.insert(self.key(value), all);
                }
            }
            Kind::Effect { .. } => {}
            Kind::Ins {
                opcode,
                saturate,
                channel,
                reduce,
                ..
            } => {
                let into = self.claim(value);
                let mut sources: Vec<Operand> = node
                    .children
                    .iter()
                    .map(|child| self.read(*child, 0))
                    .collect();
                // A dot product reads its operands whole; a sample takes the channel its own
                // swizzle names, so put that channel in the lane being written.
                if let Some(lanes) = reduce {
                    for source in &mut sources {
                        if matches!(source.reg_type, RegisterType::Temp) {
                            source.components = ComponentSelect::Swizzle([0, 1, 2, 3]);
                        }
                        let _ = lanes;
                    }
                }
                if let Some(channel) = channel {
                    for source in &mut sources {
                        if matches!(source.reg_type, RegisterType::Resource) {
                            source.components = ComponentSelect::Swizzle([*channel; 4]);
                        }
                    }
                }
                let mut operands = vec![temp(into, ComponentSelect::Mask(1))];
                // An opcode with two destinations still writes both; the one not wanted goes to null.
                if matches!(
                    opcode,
                    Opcode::Sincos | Opcode::IMul | Opcode::UMul | Opcode::UDiv | Opcode::Swapc
                ) {
                    let null = operand(RegisterType::Null, ComponentSelect::Mask(0), vec![]);
                    match node.op.contains("#0") {
                        true => operands.push(null),
                        false => operands.insert(0, null),
                    }
                }
                operands.extend(sources);
                self.out.push(generic(*opcode, *saturate, true, operands));
            }
        }
    }

    /// The whole merged program.
    pub fn build(
        mut self,
        live: &BTreeSet<u64>,
        regions: &[Region],
        roots: &BTreeMap<String, (u64, Guard)>,
        effects: &[(u64, Guard)],
        keys: Vec<String>,
        stage: &'static str,
    ) -> Built {
        let plain: BTreeMap<String, u64> = roots
            .iter()
            .map(|(name, (value, _))| (name.clone(), *value))
            .collect();
        self.table.keys = keys;
        self.pixel = stage == "ps";
        self.survey(live, &plain);

        let mut kept: Vec<usize> = Vec::new();
        for (at, region) in regions.iter().enumerate() {
            // An output or an effect goes in the last region under its guard, not in each of them:
            // written in the first one it would be written again in every later one, and the values
            // it reads may not have been computed yet.
            let last = !regions[at + 1..]
                .iter()
                .any(|held| held.cubes == region.cubes);
            let before = self.out.len();
            let close = self.open(&region.cubes);
            let opened = self.out.len();
            for value in &region.values {
                self.value(*value);
            }
            for (value, cubes) in effects {
                if *cubes != region.cubes || !last {
                    continue;
                }
                let Some(node) = self.nodes.get(value).cloned() else {
                    continue;
                };
                let Kind::Effect {
                    opcode,
                    test_nonzero,
                } = node.kind
                else {
                    continue;
                };
                // The region's guard says which variants reach this effect, not which pixels: the
                // branches it sat under are a separate, run-time thing and have to be kept.
                let (path, test): (Vec<u64>, Vec<u64>) = node.children.iter().partition(|child| {
                    matches!(
                        self.nodes.get(child).map(|held| &held.kind),
                        Some(Kind::Path)
                    )
                });
                let mut gate: Option<u32> = None;
                for held in &path {
                    if let Some(at) = self.at.get(&self.key(*held)) {
                        gate = Some(self.join(gate, *at, Opcode::And));
                    }
                }
                // `discard_z` fires where its test is nought, so joining it to a path makes it a
                // test for non-zero like the rest.
                let mut wanted = test.first().map(|child| {
                    let into = self.next;
                    self.next += 1;
                    let source = self.read(*child, 0);
                    self.out.push(generic(
                        match test_nonzero {
                            true => Opcode::Mov,
                            false => Opcode::Not,
                        },
                        false,
                        true,
                        vec![temp(into, ComponentSelect::Mask(1)), source],
                    ));
                    into
                });
                if let Some(gate) = gate {
                    wanted = Some(self.join(Some(gate), wanted.unwrap_or(gate), Opcode::And));
                }
                if let Some(wanted) = wanted {
                    self.out.push(generic(
                        opcode,
                        false,
                        true,
                        vec![temp(wanted, ComponentSelect::Swizzle([0; 4]))],
                    ));
                }
            }
            for (name, (value, cubes)) in roots {
                if cubes != &region.cubes || !last {
                    continue;
                }
                let base = name
                    .rsplit_once('#')
                    .map_or(name.as_str(), |(base, _)| base);
                // Depth is a register of its own rather than a component of a render target, so it
                // carries no semantic and no lane and would otherwise fall out here.
                if base == "oDepth" {
                    let source = self.read(*value, 0);
                    self.depth = true;
                    self.out.push(generic(
                        Opcode::Mov,
                        false,
                        true,
                        vec![
                            operand(RegisterType::OutputDepth, ComponentSelect::Mask(1), vec![]),
                            source,
                        ],
                    ));
                    continue;
                }
                let Some((semantic, comp)) = base.rsplit_once('.') else {
                    continue;
                };
                let slot = self
                    .table
                    .outputs
                    .iter()
                    .position(|(held, _)| held == semantic)
                    .unwrap_or(0) as u32;
                let lane = match comp {
                    "x" => 0,
                    "y" => 1,
                    "z" => 2,
                    _ => 3,
                };
                let source = self.read(*value, 0);
                self.out.push(generic(
                    Opcode::Mov,
                    false,
                    true,
                    vec![
                        operand(
                            RegisterType::Output,
                            ComponentSelect::Mask(1 << lane),
                            vec![slot],
                        ),
                        source,
                    ],
                ));
            }
            match self.out.len() > opened {
                true => {
                    if close {
                        self.out.push(generic(Opcode::EndIf, false, true, vec![]));
                    }
                    if !region.cubes.is_empty() {
                        kept.push(at);
                    }
                }
                // Nothing to guard, so take the guard back out with it.
                false => self.out.truncate(before),
            }
        }
        self.out.push(generic(Opcode::Ret, false, true, vec![]));

        let mut instructions = self.declarations();
        instructions.append(&mut self.out);
        Built {
            program: Program {
                shader_type: stage,
                major_version: 5,
                minor_version: 0,
                instructions,
                warnings: Vec::new(),
                fourcc: *b"SHEX",
            },
            table: self.table,
            kept,
        }
    }

    /// Open a region, returning whether it needs closing. The key buffer is the last one declared,
    /// one component per condition, so a guard is a read of it.
    /// Combine two condition registers, or pass the second through where there is no first.
    fn join(&mut self, left: Option<u32>, right: u32, opcode: Opcode) -> u32 {
        let Some(before) = left else { return right };
        let joined = self.next;
        self.next += 1;
        self.out.push(generic(
            opcode,
            false,
            true,
            vec![
                temp(joined, ComponentSelect::Mask(1)),
                temp(before, ComponentSelect::Swizzle([0; 4])),
                temp(right, ComponentSelect::Swizzle([0; 4])),
            ],
        ));
        joined
    }

    fn open(&mut self, cubes: &Guard) -> bool {
        if cubes.is_empty() {
            return false;
        }
        let keys = self.table.constants.len() as u32;
        let mut whole: Option<u32> = None;
        for cube in cubes {
            let mut all: Option<u32> = None;
            for axis in cube {
                // Several values of one key are alternatives, so they join with `or`; the axes
                // themselves have to hold together, so they join with `and`.
                let mut any: Option<u32> = None;
                for key in axis {
                    let held = self.next;
                    self.next += 1;
                    let source = operand(
                        RegisterType::ConstantBuffer,
                        ComponentSelect::Swizzle([(*key % 4) as u8; 4]),
                        vec![keys, (*key / 4) as u32],
                    );
                    self.out.push(generic(
                        Opcode::Mov,
                        false,
                        true,
                        vec![temp(held, ComponentSelect::Mask(1)), source],
                    ));
                    any = Some(self.join(any, held, Opcode::Or));
                }
                if let Some(any) = any {
                    all = Some(self.join(all, any, Opcode::And));
                }
            }
            if let Some(all) = all {
                whole = Some(self.join(whole, all, Opcode::Or));
            }
        }
        let Some(whole) = whole else { return false };
        self.out.push(generic(
            Opcode::If,
            false,
            true,
            vec![temp(whole, ComponentSelect::Swizzle([0; 4]))],
        ));
        true
    }

    fn declarations(&self) -> Vec<Instruction> {
        let mut held = vec![Instruction {
            opcode: Opcode::DclGlobalFlags,
            saturate: false,
            test_nonzero: false,
            precise_mask: 0,
            resinfo_return_type: None,
            sync_flags: 0,
            tex_offsets: None,
            resource_dim: None,
            resource_return_type: None,
            kind: InstructionKind::DclGlobalFlags {
                flags: ["refactoringAllowed"].into_iter().collect(),
            },
        }];
        let declare = |kind: InstructionKind, opcode: Opcode| Instruction {
            opcode,
            saturate: false,
            test_nonzero: false,
            precise_mask: 0,
            resinfo_return_type: None,
            sync_flags: 0,
            tex_offsets: None,
            resource_dim: None,
            resource_return_type: None,
            kind,
        };
        for (slot, (_, size)) in self.table.constants.iter().enumerate() {
            held.push(declare(
                InstructionKind::DclConstantBuffer {
                    access: "immediateIndexed",
                    operands: [operand(
                        RegisterType::ConstantBuffer,
                        ComponentSelect::Swizzle([0, 1, 2, 3]),
                        vec![slot as u32, *size],
                    )]
                    .into_iter()
                    .collect(),
                },
                Opcode::DclConstantBuffer,
            ));
        }
        // The key buffer sits after them, one component per condition.
        let keys = self.table.constants.len() as u32;
        held.push(declare(
            InstructionKind::DclConstantBuffer {
                access: "immediateIndexed",
                operands: [operand(
                    RegisterType::ConstantBuffer,
                    ComponentSelect::Swizzle([0, 1, 2, 3]),
                    vec![keys, self.table.keys.len().div_ceil(4).max(1) as u32],
                )]
                .into_iter()
                .collect(),
            },
            Opcode::DclConstantBuffer,
        ));
        for slot in 0..self.table.samplers.len() {
            held.push(declare(
                InstructionKind::DclSampler {
                    mode: "mode_default",
                    operands: [operand(
                        RegisterType::Sampler,
                        ComponentSelect::Swizzle([0, 1, 2, 3]),
                        vec![slot as u32],
                    )]
                    .into_iter()
                    .collect(),
                },
                Opcode::DclSampler,
            ));
        }
        for (slot, name) in self.table.textures.iter().enumerate() {
            let operands = [operand(
                RegisterType::Resource,
                ComponentSelect::Swizzle([0, 1, 2, 3]),
                vec![slot as u32],
            )]
            .into_iter()
            .collect();
            held.push(match self.table.buffers.contains(name) {
                true => declare(
                    InstructionKind::DclResourceRaw { operands },
                    Opcode::DclResourceRaw,
                ),
                false => declare(
                    InstructionKind::DclResource {
                        dimension: "texture2d",
                        sample_count: 0,
                        return_type: [ReturnType::Float; 4],
                        operands,
                    },
                    Opcode::DclResource,
                ),
            });
        }
        for (slot, (_, mask)) in self.table.inputs.iter().enumerate() {
            held.push(declare(
                InstructionKind::DclInput {
                    interpolation: self.pixel.then_some("linear"),
                    system_value: None,
                    operands: [operand(
                        RegisterType::Input,
                        ComponentSelect::Mask(*mask),
                        vec![slot as u32],
                    )]
                    .into_iter()
                    .collect(),
                },
                match self.pixel {
                    true => Opcode::DclInputPs,
                    false => Opcode::DclInput,
                },
            ));
        }
        for (slot, (_, mask)) in self.table.outputs.iter().enumerate() {
            held.push(declare(
                InstructionKind::DclOutput {
                    system_value: None,
                    operands: [operand(
                        RegisterType::Output,
                        ComponentSelect::Mask(*mask),
                        vec![slot as u32],
                    )]
                    .into_iter()
                    .collect(),
                },
                Opcode::DclOutput,
            ));
        }
        if self.depth {
            held.push(declare(
                InstructionKind::DclOutput {
                    system_value: None,
                    operands: [operand(
                        RegisterType::OutputDepth,
                        ComponentSelect::Mask(1),
                        vec![],
                    )]
                    .into_iter()
                    .collect(),
                },
                Opcode::DclOutput,
            ));
        }
        held.push(declare(
            InstructionKind::DclTemps { count: self.next },
            Opcode::DclTemps,
        ));
        if !self.icb.is_empty() {
            held.push(declare(
                InstructionKind::CustomData {
                    subtype: dxbc::shex::CustomDataType::ImmediateConstantBuffer,
                    values: self.icb.clone(),
                    raw_dword_count: self.icb.len() * 4 + 2,
                },
                Opcode::CustomData,
            ));
        }
        held
    }
}
