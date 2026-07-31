//! Content-addressed value graph for a decoded SM4/SM5 program.
//!
//! Every value is hashed by what it is computed from, so register allocation, instruction
//! scheduling and the compiler's common-subexpression choices all quotient out. Two shaders built
//! from one source under different defines agree on a value's hash wherever they compute the same
//! thing, which is what makes a structural comparison between variants possible at all.
//!
//! Leaves are structured and named rather than spelled as registers, because a register means
//! different things in different variants: adding one texture shifts every later slot, and an
//! input semantic does not keep its register either. A leaf therefore says `g_SamplerNormal` and
//! `TEXCOORD2.x`, never `t3` and `v2.x`. That is what lets the graph be rebuilt into one merged
//! program with a slot table of its own.

use dxbc::shex::{
    ComponentSelect, Instruction, InstructionKind, Opcode, Operand, OperandIndex, Program,
    RegisterType,
};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};

/// A value that comes from outside the shader's own arithmetic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Leaf {
    /// An input register component, by the semantic it carries.
    Input {
        semantic: String,
        comp: u8,
    },
    /// A constant buffer component. `dynamic` means the register index is the node's own child.
    Constant {
        buffer: String,
        register: u32,
        comp: u8,
        dynamic: bool,
    },
    /// The immediate constant buffer, which belongs to the program rather than to a binding.
    ImmConst {
        register: u32,
        comp: u8,
        dynamic: bool,
    },
    Immediate(u32),
    Resource(String),
    Sampler(String),
    Uav(String),
    /// A read with nothing written to it, which still has to hash to something.
    Undef(String),
    /// Anything else the walk meets, kept opaque.
    Other(String),
}

/// What a node is, beyond the string it hashes as.
#[derive(Clone, Debug)]
pub enum Kind {
    Leaf(Leaf),
    /// One destination lane of one instruction.
    Ins {
        opcode: Opcode,
        saturate: bool,
        /// The resource channel this lane takes, for a sample or load.
        channel: Option<u8>,
        /// How wide each source is read, for a dot product.
        reduce: Option<u8>,
    },
    /// Several lanes bundled so a whole-operand instruction can take them.
    Vec,
    Neg,
    Abs,
    /// The negation of a branch condition, which is logical rather than arithmetic.
    Not,
    /// A choice between the two arms of a branch.
    Phi,
    /// The conjunction of branch conditions in force.
    Path,
    /// A side effect, under its path condition.
    Effect {
        opcode: Opcode,
        test_nonzero: bool,
    },
}

#[derive(Clone, Debug)]
pub struct Node {
    pub op: String,
    pub children: Vec<u64>,
    pub kind: Kind,
}

/// What each bound register is called, so a value keeps its identity when the compiler moves it.
#[derive(Default)]
pub struct Naming {
    pub constants: HashMap<u32, String>,
    pub samplers: HashMap<u32, String>,
    pub textures: HashMap<u32, String>,
    pub uavs: HashMap<u32, String>,
    /// Input registers by the semantic they carry, e.g. `TEXCOORD2`.
    pub inputs: HashMap<u32, String>,
    pub outputs: HashMap<u32, String>,
}

#[derive(Default)]
pub struct Graph {
    pub nodes: HashMap<u64, Node>,
    /// What the shader leaves behind: each output component under its semantic, and each ordered
    /// side effect under the branch condition that guards it.
    pub roots: BTreeMap<String, u64>,
    pub effects: Vec<(String, u64)>,
    pub loops: usize,
}

/// A register slot: file, index, component.
type Slot = (u8, u32, u8);

const TEMP: u8 = 0;
const OUTPUT: u8 = 1;
const OTHER: u8 = 2;

struct Frame {
    before: HashMap<Slot, u64>,
    taken: Option<HashMap<Slot, u64>>,
    cond: u64,
}

pub struct Builder {
    graph: Graph,
    naming: Naming,
    env: HashMap<Slot, u64>,
    frames: Vec<Frame>,
    path: Vec<u64>,
    /// Output slots under the name they will be reported by.
    named_outputs: HashMap<Slot, String>,
}

fn hash(op: &str, children: &[u64]) -> u64 {
    let mut hasher = DefaultHasher::new();
    op.hash(&mut hasher);
    children.hash(&mut hasher);
    hasher.finish()
}

fn letter(comp: u8) -> char {
    ['x', 'y', 'z', 'w'][usize::from(comp).min(3)]
}

impl Leaf {
    fn text(&self) -> String {
        match self {
            Self::Input { semantic, comp } => format!("{semantic}.{}", letter(*comp)),
            Self::Constant {
                buffer,
                register,
                comp,
                dynamic,
            } => match dynamic {
                true => format!("{buffer}[{register}+rel].{}", letter(*comp)),
                false => format!("{buffer}[{register}].{}", letter(*comp)),
            },
            Self::ImmConst {
                register,
                comp,
                dynamic,
            } => match dynamic {
                true => format!("icb[{register}+rel].{}", letter(*comp)),
                false => format!("icb[{register}].{}", letter(*comp)),
            },
            Self::Immediate(bits) => format!("l({:e})", f32::from_bits(*bits)),
            Self::Resource(name) | Self::Uav(name) => name.clone(),
            // A texture and its sampler share a name in this format, so the two would be one value
            // and a sample would be handed the texture where the sampler goes.
            Self::Sampler(name) => format!("{name}#s"),
            Self::Undef(text) | Self::Other(text) => text.clone(),
        }
    }
}

impl Builder {
    pub fn new(naming: Naming) -> Self {
        Self {
            graph: Graph::default(),
            naming,
            env: HashMap::new(),
            frames: Vec::new(),
            path: Vec::new(),
            named_outputs: HashMap::new(),
        }
    }

    fn intern(&mut self, op: impl Into<String>, mut children: Vec<u64>, kind: Kind) -> u64 {
        let op = op.into();
        // The compiler writes a commutative operand pair in either order, so the order carries
        // nothing and two variants must not be told apart by it.
        let bare = op.split(['_', '#', '.']).next().unwrap_or(&op);
        match bare {
            "add" | "mul" | "min" | "max" | "and" | "or" | "xor" | "iadd" | "imul" | "umul"
            | "imin" | "imax" | "umin" | "umax" | "eq" | "ne" | "ieq" | "ine" | "dp2" | "dp3"
            | "dp4" => children.sort_unstable(),
            "mad" | "imad" => {
                if let Some(pair) = children.get_mut(..2) {
                    pair.sort_unstable();
                }
            }
            _ => {}
        }
        let id = hash(&op, &children);
        self.graph
            .nodes
            .entry(id)
            .or_insert(Node { op, children, kind });
        id
    }

    fn leaf(&mut self, held: Leaf) -> u64 {
        self.intern(held.text(), Vec::new(), Kind::Leaf(held))
    }

    fn level(&mut self, index: Option<&OperandIndex>) -> (u32, Option<u64>) {
        match index {
            Some(OperandIndex::Imm32(at)) => (*at, None),
            Some(OperandIndex::Imm64(at)) => (*at as u32, None),
            Some(OperandIndex::Relative(operand)) => {
                let value = self.source(operand, 0);
                (0, Some(value))
            }
            Some(OperandIndex::RelativePlusImm(at, operand)) => {
                let value = self.source(operand, 0);
                (*at, Some(value))
            }
            None => (0, None),
        }
    }

    /// Which component of a source operand feeds destination lane `lane`.
    ///
    /// A four-component operand with no swizzle is read positionally, which is what an inline
    /// immediate is: `l(0, -0.5, -0.5, 0)` written to `.yz` supplies -0.5 twice.
    fn component(operand: &Operand, lane: u8) -> u8 {
        match &operand.components {
            ComponentSelect::Scalar(at) => *at,
            ComponentSelect::Swizzle(order) => order[usize::from(lane).min(3)],
            ComponentSelect::Mask(_) => lane.min(3),
            _ => 0,
        }
    }

    fn source(&mut self, operand: &Operand, lane: u8) -> u64 {
        let comp = Self::component(operand, lane);
        let mut value = match operand.reg_type {
            RegisterType::Temp => {
                let (index, _) = self.level(operand.indices.first());
                match self.env.get(&(TEMP, index, comp)) {
                    Some(held) => *held,
                    None => self.leaf(Leaf::Undef(format!("undef r{index}.{}", letter(comp)))),
                }
            }
            RegisterType::Immediate32 => {
                let bits = operand
                    .immediate_values
                    .get(usize::from(comp))
                    .or_else(|| operand.immediate_values.first())
                    .copied()
                    .unwrap_or(0);
                self.leaf(Leaf::Immediate(bits))
            }
            RegisterType::ConstantBuffer => {
                let (slot, _) = self.level(operand.indices.first());
                let (register, relative) = self.level(operand.indices.get(1));
                let buffer = self
                    .naming
                    .constants
                    .get(&slot)
                    .cloned()
                    .unwrap_or_else(|| format!("cb{slot}"));
                let held = Leaf::Constant {
                    buffer,
                    register,
                    comp,
                    dynamic: relative.is_some(),
                };
                let text = held.text();
                self.intern(text, relative.into_iter().collect(), Kind::Leaf(held))
            }
            RegisterType::ImmConstBuffer => {
                let (register, relative) = self.level(operand.indices.first());
                let held = Leaf::ImmConst {
                    register,
                    comp,
                    dynamic: relative.is_some(),
                };
                let text = held.text();
                self.intern(text, relative.into_iter().collect(), Kind::Leaf(held))
            }
            RegisterType::Input | RegisterType::InputControlPoint => {
                let (index, _) = self.level(operand.indices.first());
                let semantic = self
                    .naming
                    .inputs
                    .get(&index)
                    .cloned()
                    .unwrap_or_else(|| format!("v{index}"));
                self.leaf(Leaf::Input { semantic, comp })
            }
            RegisterType::Resource => {
                let (at, _) = self.level(operand.indices.first());
                let name = self
                    .naming
                    .textures
                    .get(&at)
                    .cloned()
                    .unwrap_or_else(|| format!("t{at}"));
                self.leaf(Leaf::Resource(name))
            }
            RegisterType::Sampler => {
                let (at, _) = self.level(operand.indices.first());
                let name = self
                    .naming
                    .samplers
                    .get(&at)
                    .cloned()
                    .unwrap_or_else(|| format!("s{at}"));
                self.leaf(Leaf::Sampler(name))
            }
            RegisterType::Uav => {
                let (at, _) = self.level(operand.indices.first());
                let name = self
                    .naming
                    .uavs
                    .get(&at)
                    .cloned()
                    .unwrap_or_else(|| format!("u{at}"));
                self.leaf(Leaf::Uav(name))
            }
            other => {
                let (at, _) = self.level(operand.indices.first());
                self.leaf(Leaf::Other(format!("{}{at}.{}", tag(other), letter(comp))))
            }
        };
        if operand.abs
            && !matches!(
                self.graph.nodes.get(&value).map(|held| &held.kind),
                Some(Kind::Abs)
            )
        {
            value = self.intern("abs", vec![value], Kind::Abs);
        }
        if operand.negate {
            // Two negations are the value itself, and leaving them nested renders as `--x`, which
            // reads back as a pre-decrement.
            value = match self.graph.nodes.get(&value) {
                Some(node) if matches!(node.kind, Kind::Neg) => node.children[0],
                _ => self.intern("neg", vec![value], Kind::Neg),
            };
        }
        value
    }

    fn whole(&mut self, operand: &Operand, lanes: u8) -> u64 {
        let children: Vec<u64> = (0..lanes).map(|lane| self.source(operand, lane)).collect();
        self.intern("vec", children, Kind::Vec)
    }

    fn write(&mut self, operand: &Operand, comp: u8, value: u64) {
        let (index, _) = self.level(operand.indices.first());
        let slot = match operand.reg_type {
            RegisterType::Temp => (TEMP, index, comp),
            RegisterType::Output => {
                let slot = (OUTPUT, index, comp);
                if let Some(semantic) = self.naming.outputs.get(&index) {
                    self.named_outputs
                        .insert(slot, format!("{semantic}.{}", letter(comp)));
                }
                slot
            }
            RegisterType::OutputDepth => (OTHER, u32::MAX - 1, 0),
            RegisterType::OutputCoverageMask => (OTHER, u32::MAX - 2, 0),
            _ => (OTHER, index, comp),
        };
        self.env.insert(slot, value);
    }

    fn written(operand: &Operand) -> Vec<u8> {
        match &operand.components {
            ComponentSelect::Mask(mask) => (0..4).filter(|at| mask & (1 << at) != 0).collect(),
            ComponentSelect::Scalar(at) => vec![*at],
            _ => vec![0],
        }
    }

    fn guard(&mut self) -> Option<u64> {
        match self.path.len() {
            0 => None,
            _ => {
                let held = self.path.clone();
                Some(self.intern("path", held, Kind::Path))
            }
        }
    }

    pub fn run(mut self, program: &Program) -> Graph {
        for instruction in &program.instructions {
            self.step(instruction);
        }
        let finished: Vec<(Slot, u64)> = self
            .env
            .iter()
            .filter(|((file, _, _), _)| *file == OUTPUT || *file == OTHER)
            .map(|(slot, value)| (*slot, *value))
            .collect();
        for (slot, value) in finished {
            let name = match self.named_outputs.get(&slot) {
                Some(held) => held.clone(),
                None => match slot {
                    (OTHER, at, _) if at == u32::MAX - 1 => "oDepth".to_owned(),
                    (OTHER, at, _) if at == u32::MAX - 2 => "oMask".to_owned(),
                    (OUTPUT, at, comp) => format!("o{at}.{}", letter(comp)),
                    (_, at, comp) => format!("x{at}.{}", letter(comp)),
                },
            };
            self.graph.roots.insert(name, value);
        }
        self.graph
    }

    fn step(&mut self, instruction: &Instruction) {
        let InstructionKind::Generic { operands } = &instruction.kind else {
            return;
        };
        let name = instruction.opcode.name();
        let sat = if instruction.saturate { "_sat" } else { "" };

        match name {
            "if" => {
                let cond = operands
                    .first()
                    .map(|operand| self.source(operand, 0))
                    .unwrap_or(0);
                let cond = match instruction.test_nonzero {
                    true => cond,
                    false => self.intern("not", vec![cond], Kind::Not),
                };
                self.frames.push(Frame {
                    before: self.env.clone(),
                    taken: None,
                    cond,
                });
                self.path.push(cond);
                return;
            }
            "else" => {
                if let Some(frame) = self.frames.last_mut() {
                    frame.taken = Some(std::mem::replace(&mut self.env, frame.before.clone()));
                    let cond = frame.cond;
                    let flipped = self.intern("not", vec![cond], Kind::Not);
                    if let Some(last) = self.path.last_mut() {
                        *last = flipped;
                    }
                }
                return;
            }
            "endif" => {
                self.path.pop();
                let Some(frame) = self.frames.pop() else {
                    return;
                };
                let (taken, untaken) = match frame.taken {
                    Some(taken) => (taken, std::mem::take(&mut self.env)),
                    None => (std::mem::take(&mut self.env), frame.before.clone()),
                };
                let mut merged = frame.before.clone();
                let slots: BTreeSet<Slot> = taken.keys().chain(untaken.keys()).copied().collect();
                for slot in slots {
                    let left = taken.get(&slot).copied();
                    let right = untaken.get(&slot).copied();
                    let value = match (left, right) {
                        (Some(left), Some(right)) if left == right => left,
                        (left, right) => {
                            let missing = self.leaf(Leaf::Undef(format!(
                                "undef {}.{}",
                                slot.1,
                                letter(slot.2)
                            )));
                            let left = left.unwrap_or(missing);
                            let right = right.unwrap_or(missing);
                            self.intern("movc", vec![frame.cond, left, right], Kind::Phi)
                        }
                    };
                    merged.insert(slot, value);
                }
                self.env = merged;
                return;
            }
            "loop" => {
                self.graph.loops += 1;
                return;
            }
            "endloop" | "switch" | "endswitch" | "case" | "default" | "label" | "nop" => return,
            "discard" | "break" | "continue" | "ret" | "retc" | "breakc" | "continuec" => {
                let cond = operands
                    .first()
                    .map(|operand| self.source(operand, 0))
                    .unwrap_or(0);
                let guard = self.guard();
                let tag = match instruction.test_nonzero {
                    true => format!("{name}_nz"),
                    false => format!("{name}_z"),
                };
                let mut children: Vec<u64> = guard.into_iter().collect();
                if !operands.is_empty() {
                    children.push(cond);
                }
                let value = self.intern(
                    tag.clone(),
                    children,
                    Kind::Effect {
                        opcode: instruction.opcode,
                        test_nonzero: instruction.test_nonzero,
                    },
                );
                self.graph.effects.push((tag, value));
                return;
            }
            _ => {}
        }

        if name.starts_with("store") || name.starts_with("atomic") || name.starts_with("imm_atomic")
        {
            let guard = self.guard();
            let mut children: Vec<u64> = guard.into_iter().collect();
            for operand in operands.iter() {
                let value = self.whole(operand, 4);
                children.push(value);
            }
            let value = self.intern(
                name.to_owned(),
                children,
                Kind::Effect {
                    opcode: instruction.opcode,
                    test_nonzero: instruction.test_nonzero,
                },
            );
            self.graph.effects.push((name.to_owned(), value));
            return;
        }
        if name.starts_with("sync") || name.starts_with("dcl") || name.starts_with("emit") {
            return;
        }

        // A plain move computes nothing, so the value it makes is the value it was given. Keeping it
        // as a node of its own also stacks source modifiers, and `-` twice renders as `--x`.
        if name == "mov" && !instruction.saturate && operands.len() == 2 {
            let mut writes = Vec::new();
            for comp in Self::written(&operands[0]) {
                let value = self.source(&operands[1], comp);
                writes.push((comp, value));
            }
            for (comp, value) in writes {
                self.write(&operands[0], comp, value);
            }
            return;
        }

        let dests = match name {
            "sincos" | "imul" | "umul" | "udiv" | "swapc" => 2,
            _ => 1,
        };
        let reduce = match name {
            "dp2" => Some(2),
            "dp3" => Some(3),
            "dp4" => Some(4),
            _ => None,
        };
        let whole = name.starts_with("sample")
            || name.starts_with("ld")
            || name.starts_with("gather")
            || name.starts_with("resinfo")
            || name.starts_with("bufinfo")
            || name.starts_with("lod");

        let sources: Vec<&Operand> = operands.iter().skip(dests).collect();
        // Every source is read before any destination is written, so `mul r1.xyz, r1.x, v3.xyz`
        // multiplies by the old `r1.x` in all three lanes.
        let mut writes: Vec<(usize, u8, u64)> = Vec::new();
        for (which, dest) in operands.iter().take(dests).enumerate() {
            if matches!(dest.reg_type, RegisterType::Null) {
                continue;
            }
            for comp in Self::written(dest) {
                let op = match dests {
                    1 => format!("{name}{sat}"),
                    _ => format!("{name}{sat}#{which}"),
                };
                let value = match (reduce, whole) {
                    (Some(lanes), _) => {
                        let children: Vec<u64> = sources
                            .iter()
                            .map(|operand| self.whole(operand, lanes))
                            .collect();
                        self.intern(
                            op,
                            children,
                            Kind::Ins {
                                opcode: instruction.opcode,
                                saturate: instruction.saturate,

                                channel: None,
                                reduce: Some(lanes),
                            },
                        )
                    }
                    (None, true) => {
                        // The resource's own swizzle picks which channel a destination lane takes.
                        let channel = sources
                            .iter()
                            .find(|operand| {
                                matches!(
                                    operand.reg_type,
                                    RegisterType::Resource | RegisterType::Uav
                                )
                            })
                            .map_or(comp, |operand| Self::component(operand, comp));
                        let children: Vec<u64> = sources
                            .iter()
                            .map(|operand| match operand.reg_type {
                                RegisterType::Resource
                                | RegisterType::Sampler
                                | RegisterType::Uav => self.source(operand, 0),
                                _ => self.whole(operand, 4),
                            })
                            .collect();
                        self.intern(
                            format!("{op}.{channel}"),
                            children,
                            Kind::Ins {
                                opcode: instruction.opcode,
                                saturate: instruction.saturate,

                                channel: Some(channel),
                                reduce: None,
                            },
                        )
                    }
                    (None, false) => {
                        let children: Vec<u64> = sources
                            .iter()
                            .map(|operand| self.source(operand, comp))
                            .collect();
                        self.intern(
                            op,
                            children,
                            Kind::Ins {
                                opcode: instruction.opcode,
                                saturate: instruction.saturate,

                                channel: None,
                                reduce: None,
                            },
                        )
                    }
                };
                writes.push((which, comp, value));
            }
        }
        for (which, comp, value) in writes {
            self.write(&operands[which], comp, value);
        }
    }
}

fn tag(reg: RegisterType) -> &'static str {
    match reg {
        RegisterType::Input => "v",
        RegisterType::Output => "o",
        RegisterType::Resource => "t",
        RegisterType::Sampler => "s",
        RegisterType::Uav => "u",
        RegisterType::InputPrimitiveID => "vPrim",
        RegisterType::ThreadID => "vThreadID",
        RegisterType::ThreadGroupSharedMemory => "g",
        RegisterType::Null => "null",
        _ => "reg",
    }
}

impl Graph {
    pub fn live(&self) -> BTreeSet<u64> {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<u64> = self
            .roots
            .values()
            .copied()
            .chain(self.effects.iter().map(|(_, value)| *value))
            .collect();
        while let Some(value) = stack.pop() {
            if !seen.insert(value) {
                continue;
            }
            if let Some(node) = self.nodes.get(&value) {
                stack.extend(&node.children);
            }
        }
        seen
    }
}

pub fn build(program: &Program, naming: Naming) -> Graph {
    Builder::new(naming).run(program)
}
