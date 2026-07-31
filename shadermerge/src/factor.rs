// Ouroboros keeps one body and puts small `#ifdef`s inside it. The merger writes one body per
// branch instead, because a value hashes by everything it was computed from: change a single input
// and every value downstream re-hashes, so the whole tail duplicates even where it is identical.
// Recovering that tail is anti-unification. Find values from different branches computing the same
// thing modulo one input, give that input a register both branches write, and emit the tail once.
//
// Two values may share a register exactly when no variant holds both, which branch-exclusive values
// satisfy by construction. That disjointness is the only safety condition: a reader of one is live
// only where that one is live, so it reads the register and finds what it wrote.
use crate::Error;

/// A candidate partner: how many inputs differ, which value it is, and the pairs that differ.
type Fit = (usize, u64, Vec<(u64, u64)>);
use crate::canon::Node;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Which values share a register, as a union-find over value hashes.
#[derive(Default)]
pub struct Slots {
    up: HashMap<u64, u64>,
}

impl Slots {
    pub fn of(&self, value: u64) -> u64 {
        let mut root = value;
        while let Some(next) = self.up.get(&root) {
            root = *next;
        }
        root
    }

    fn join(&mut self, into: u64, from: u64) {
        let (into, from) = (self.of(into), self.of(from));
        if into != from {
            self.up.insert(from, into);
        }
    }
}

pub struct Factoring {
    pub slots: Slots,
    /// Which variants reach each class once its members are pooled.
    pub presence: HashMap<u64, Vec<bool>>,
    /// Classes whose members all compute the same thing, so the representative stands for them.
    /// A class not in here is a hole: every member is emitted, under its own guard, into the one
    /// register the shared tail reads.
    pub shared: HashSet<u64>,
}

impl Factoring {
    /// Whether this value is the one its class emits, for a class that emits only one.
    pub fn speaks_for(&self, value: u64) -> bool {
        let slot = self.slots.of(value);
        slot == value || !self.shared.contains(&slot)
    }
}

/// How many leading children of an operator commute, matching what `canon::intern` sorted.
fn commuting(op: &str) -> usize {
    match op.split(['_', '#', '.']).next().unwrap_or(op) {
        "add" | "mul" | "min" | "max" | "and" | "or" | "xor" | "iadd" | "imul" | "umul"
        | "imin" | "imax" | "umin" | "umax" | "eq" | "ne" | "ieq" | "ine" | "dp2" | "dp3"
        | "dp4" => usize::MAX,
        "mad" | "imad" => 2,
        _ => 0,
    }
}

fn disjoint(left: &[bool], right: &[bool]) -> bool {
    !left.iter().zip(right).any(|(a, b)| *a && *b)
}

fn merge_into(into: &mut [bool], from: &[bool]) {
    for (slot, held) in into.iter_mut().zip(from) {
        *slot |= held;
    }
}

/// The children as the registers they will be read from, with the commuting ones in a fixed order
/// so two branches that sorted them by hash still line up.
fn shape(slots: &Slots, node: &Node) -> Vec<u64> {
    let mut held: Vec<u64> = node.children.iter().map(|child| slots.of(*child)).collect();
    let commutes = commuting(&node.op).min(held.len());
    held[..commutes].sort_unstable();
    held
}

/// Children of two nodes paired up, answering the positions that disagree. Commuting children are
/// matched greedily on what they already share, so only what is genuinely different is left over.
fn divergence(slots: &Slots, left: &Node, right: &Node) -> Option<Vec<(u64, u64)>> {
    if left.op != right.op || left.children.len() != right.children.len() {
        return None;
    }
    let commutes = commuting(&left.op).min(left.children.len());
    let mut holes = Vec::new();
    for (a, b) in left.children[commutes..]
        .iter()
        .zip(&right.children[commutes..])
    {
        if slots.of(*a) != slots.of(*b) {
            holes.push((*a, *b));
        }
    }

    let mut spare: Vec<u64> = right.children[..commutes].to_vec();
    let mut left_over = Vec::new();
    for child in &left.children[..commutes] {
        match spare
            .iter()
            .position(|held| slots.of(*held) == slots.of(*child))
        {
            Some(at) => {
                spare.swap_remove(at);
            }
            None => left_over.push(*child),
        }
    }
    holes.extend(left_over.into_iter().zip(spare));
    Some(holes)
}

/// Children before parents, over the values that are live somewhere.
fn ordered(nodes: &HashMap<u64, Node>, presence: &BTreeMap<u64, Vec<bool>>) -> Vec<u64> {
    let mut pending: HashMap<u64, usize> = HashMap::new();
    let mut readers: HashMap<u64, Vec<u64>> = HashMap::new();
    for value in presence.keys() {
        let mut children = nodes
            .get(value)
            .map(|node| node.children.clone())
            .unwrap_or_default();
        children.sort_unstable();
        children.dedup();
        children.retain(|child| presence.contains_key(child));
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

    let mut order = Vec::with_capacity(pending.len());
    while let Some(value) = ready.pop() {
        order.push(value);
        for reader in readers.get(&value).into_iter().flatten() {
            let slot = pending.get_mut(reader).expect("reader is live");
            *slot -= 1;
            if *slot == 0 {
                ready.push(*reader);
            }
        }
    }
    order
}

/// Pool the branch-exclusive values that compute the same thing.
///
/// `holes` caps how many inputs of a node may differ and still be pooled. One is what Ouroboros's
/// hand-written sources use; nought reduces this to plain congruence and finds almost nothing,
/// since a differing input is the whole reason the tail duplicated.
pub fn factor(
    nodes: &HashMap<u64, Node>,
    presence: &BTreeMap<u64, Vec<bool>>,
    holes: usize,
) -> Result<Factoring, Error> {
    let variants = presence.values().next().map_or(0, Vec::len);
    let mut held = Factoring {
        slots: Slots::default(),
        presence: presence
            .iter()
            .map(|(v, seen)| (*v, seen.clone()))
            .collect(),
        shared: HashSet::new(),
    };

    // A value every variant computes is already written once and can never move register.
    let core: HashSet<u64> = presence
        .iter()
        .filter(|(_, seen)| seen.iter().filter(|held| **held).count() == variants)
        .map(|(value, _)| *value)
        .collect();

    // Pooling one node changes the registers every node downstream of it reads, so a pass can only
    // see the matches its predecessors made. Repeat until nothing more joins; each pass strictly
    // reduces the number of classes, so this ends.
    let order = ordered(nodes, presence);
    let mut moved = true;
    while moved {
        moved = false;
        let mut congruent: HashMap<(String, Vec<u64>), u64> = HashMap::new();
        let mut by_op: HashMap<(String, usize), Vec<u64>> = HashMap::new();

        for value in order.iter().copied() {
            if core.contains(&value) {
                continue;
            }
            let Some(node) = nodes.get(&value) else {
                continue;
            };

            // Already the same computation on the same registers: pool with no hole at all.
            let key = (node.op.clone(), shape(&held.slots, node));
            if let Some(other) = congruent.get(&key).copied() {
                let (into, from) = (held.slots.of(other), held.slots.of(value));
                if into != from && disjoint(&held.presence[&into], &held.presence[&from]) {
                    let seen = held.presence.remove(&from).expect("class has presence");
                    merge_into(
                        held.presence.get_mut(&into).expect("class has presence"),
                        &seen,
                    );
                    held.slots.join(into, from);
                    moved = true;
                    continue;
                }
            }
            congruent.entry(key).or_insert(value);

            if holes == 0 {
                by_op
                    .entry((node.op.clone(), node.children.len()))
                    .or_default()
                    .push(value);
                continue;
            }

            // Otherwise look for the same computation on inputs that differ in at most `holes`
            // places, and pool those inputs too so the tail can be written once.
            let bucket = by_op
                .entry((node.op.clone(), node.children.len()))
                .or_default();
            // Nearest first: a partner that differs in one place leaves fewer registers behind than
            // one that differs in three, and taking whichever came first in the bucket lets a poor
            // partner use up a node a better one wanted.
            let mut fits: Vec<Fit> = Vec::new();
            for other in bucket.iter().copied() {
                let (into, from) = (held.slots.of(other), held.slots.of(value));
                if into == from || !disjoint(&held.presence[&into], &held.presence[&from]) {
                    continue;
                }
                let Some(diff) = divergence(&held.slots, &nodes[&other], node) else {
                    continue;
                };
                if diff.len() > holes {
                    continue;
                }
                // A hole's own branches have to be poolable on the same terms, or the register the
                // tail reads would hold two things at once in some variant.
                let usable = diff.iter().all(|(a, b)| {
                    let (a, b) = (held.slots.of(*a), held.slots.of(*b));
                    !core.contains(&a)
                        && !core.contains(&b)
                        && held.presence.contains_key(&a)
                        && held.presence.contains_key(&b)
                });
                if !usable {
                    continue;
                }
                let near = diff.len();
                fits.push((near, other, diff));
                if near == 0 {
                    break;
                }
            }
            fits.sort_by_key(|(near, ..)| *near);

            let mut joined = None;
            for (_, other, diff) in fits {
                let (into, from) = (held.slots.of(other), held.slots.of(value));
                if into == from || !disjoint(&held.presence[&into], &held.presence[&from]) {
                    continue;
                }

                // Pooling the holes grows their classes, which can make a later one of them overlap
                // after all. Play the whole candidate out first and take it only if every join it
                // implies stays disjoint, so a rejected one leaves nothing half-done.
                let mut plan: Vec<(u64, u64)> = diff.clone();
                plan.push((other, value));
                let mut ahead: HashMap<u64, u64> = HashMap::new();
                let mut pooled: HashMap<u64, Vec<bool>> = HashMap::new();
                let mut sound = true;
                for (a, b) in &plan {
                    let mut left = held.slots.of(*a);
                    while let Some(next) = ahead.get(&left) {
                        left = *next;
                    }
                    let mut right = held.slots.of(*b);
                    while let Some(next) = ahead.get(&right) {
                        right = *next;
                    }
                    if left == right {
                        continue;
                    }
                    let seen = pooled.get(&right).unwrap_or(&held.presence[&right]).clone();
                    let mut into = pooled.get(&left).unwrap_or(&held.presence[&left]).clone();
                    if !disjoint(&into, &seen) {
                        sound = false;
                        break;
                    }
                    merge_into(&mut into, &seen);
                    pooled.insert(left, into);
                    ahead.insert(right, left);
                }
                if !sound {
                    continue;
                }
                // A class that took a union can itself be merged away later in the same candidate,
                // so settle every join first and only then hang each union on whatever root it
                // ended up under. Removing and inserting as the joins went would lose the union
                // whenever the map handed back its entries in the wrong order.
                for (from, into) in &ahead {
                    held.slots.join(*into, *from);
                }
                let mut settled: HashMap<u64, Vec<bool>> = HashMap::new();
                for (slot, seen) in &pooled {
                    let root = held.slots.of(*slot);
                    let into = settled
                        .entry(root)
                        .or_insert_with(|| vec![false; seen.len()]);
                    merge_into(into, seen);
                }
                for slot in ahead.keys().chain(pooled.keys()) {
                    held.presence.remove(slot);
                }
                held.presence.extend(settled);
                joined = Some(into);
                moved = true;
                break;
            }
            if joined.is_none() {
                bucket.push(value);
            }
        }
    }

    // A class stands for its members only where they really do compute the same thing. Deciding it
    // here rather than while pooling keeps it true of whatever the pooling ended up doing.
    let mut members: HashMap<u64, Vec<u64>> = HashMap::new();
    for value in presence.keys() {
        members
            .entry(held.slots.of(*value))
            .or_default()
            .push(*value);
    }
    for (slot, class) in &members {
        let Some(speaker) = nodes.get(slot) else {
            continue;
        };
        let want = shape(&held.slots, speaker);
        let alike = class.iter().all(|value| match nodes.get(value) {
            Some(node) => node.op == speaker.op && shape(&held.slots, node) == want,
            None => false,
        });
        if alike {
            held.shared.insert(*slot);
        }
    }

    prove(nodes, presence, &held)?;
    Ok(held)
}

/// Every pooled value must still compute what it computed alone.
///
/// For a class that emits one line, each member has to read the same registers in the same places
/// as the member standing for it, so that projecting the line onto that member's variants gives
/// back its own node. Commuting children compare as a multiset, which is what the operator means.
/// For any class, no two members may be live in one variant, or the register holds two values.
fn prove(
    nodes: &HashMap<u64, Node>,
    presence: &BTreeMap<u64, Vec<bool>>,
    held: &Factoring,
) -> Result<(), Error> {
    let mut members: HashMap<u64, Vec<u64>> = HashMap::new();
    for value in presence.keys() {
        members
            .entry(held.slots.of(*value))
            .or_default()
            .push(*value);
    }
    for (slot, class) in &members {
        for (at, value) in class.iter().enumerate() {
            for other in &class[at + 1..] {
                if !disjoint(&presence[value], &presence[other]) {
                    return Err(Error::Pooling);
                }
            }
        }
        if !held.shared.contains(slot) {
            continue;
        }
        let Some(speaker) = nodes.get(slot) else {
            continue;
        };
        let want = shape(&held.slots, speaker);
        for value in class {
            let node = &nodes[value];
            if node.op != speaker.op || shape(&held.slots, node) != want {
                return Err(Error::Pooling);
            }
        }
    }
    Ok(())
}
