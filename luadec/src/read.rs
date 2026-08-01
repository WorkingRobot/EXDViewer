//! The reading itself: the registers and jumps of one function, folded back into statements.
//!
//! Two rules carry most of it. A register holds a temporary while the expression that produced it is
//! still being built, and was a local all along the moment something reads or overwrites it out of
//! stack order, because that is the only way a value outlives the statement that made it. And a jump
//! is read by where it lands rather than by what precedes it, so one conditional shape serves an
//! `if`, a `while` and the tail of a `repeat`.

use crate::chunk::{Constant, Function, Instruction, Opcode, Operand};

use crate::expr::{Binary, Closure, Expr, Stat, Target, Unary, is_name};

/// Why a function could not be read. Each is a shape the reading does not recover rather than a
/// malformed chunk, so a caller falls back to disassembling that one function.
pub type Reading<T> = Result<T, &'static str>;

/// Nesting the reading will follow. Recovery is recursive and a stack overflow is not something a
/// caller can catch, so the depth is counted instead. An `elseif` chain nests once per arm, which is
/// what needs the room; twice this still runs the corpus inside half a megabyte of stack.
const MAX_DEPTH: usize = 200;

/// How many values one [`Opcode::SetList`] word carries.
const LIST_BLOCK: usize = 50;

/// What a register holds part-way through a statement.
#[derive(Clone, Default)]
enum Slot {
    #[default]
    Empty,
    Value(Expr),
    /// An object and the method [`Opcode::Self_`] looked up on it, waiting for its call.
    Method(Expr, String),
    /// A further result of the call or vararg that landed at the register this names.
    Rest,
}

/// One conditional and where it goes when it does not fall through.
#[derive(Clone, Copy)]
struct Test {
    /// Where the conditional itself sits; its jump is the word after.
    test: usize,
    target: usize,
    /// Where the next operand is evaluated, which is also where the body starts for the last test.
    after: usize,
}

struct Reader<'a> {
    function: &'a Function,
    /// Words that are operands of the instruction before them rather than instructions themselves.
    pseudo: Vec<bool>,
    /// Where the value a word writes has to be a local, and up to which register.
    sticky: Vec<Option<usize>>,
    slots: Vec<Slot>,
    /// Name of the local each register holds, for registers below [`Self::active`].
    names: Vec<String>,
    upvalues: Vec<String>,
    /// Registers below this hold locals; the rest are the expression under construction.
    active: usize,
    declared: usize,
    /// Where a call left a run of results whose length only the reader of them knows.
    open: Option<usize>,
    /// The condition a `repeat` ended on, once its body has been walked.
    until: Option<Expr>,
    /// Heads of the loops being read, so a body does not take its own jump back for another loop.
    heads: Vec<usize>,
    out: Vec<Stat>,
    depth: usize,
    counts: Counts,
}

/// How much of a chunk the reading resolved.
#[derive(Default)]
pub struct Counts {
    pub read: usize,
    pub raw: usize,
}

/// Read a function, falling back to its own disassembly where the reading does not resolve.
///
/// The fallback is per function rather than per chunk, so one shape the reading cannot follow costs
/// that function and not the file around it.
pub fn closure(
    held: &Function,
    upvalues: Vec<String>,
    depth: usize,
    counts: &mut Counts,
) -> Closure {
    let mut nested = Counts::default();
    match function(held, upvalues, depth, &mut nested) {
        Ok(read) => {
            counts.read += nested.read + 1;
            counts.raw += nested.raw;
            read
        }
        Err(reason) => {
            counts.raw += 1;
            disassembled(held, reason, depth, counts)
        }
    }
}

/// A function as its instructions, commented out so the source around it still reads as Lua.
fn disassembled(
    held: &Function,
    reason: &'static str,
    depth: usize,
    counts: &mut Counts,
) -> Closure {
    let mut lines = vec![format!("-- not read as source: {reason}")];
    crate::asm::listing(held, 0, &mut lines);
    let mut body: Vec<Stat> = lines
        .into_iter()
        .map(|line| Stat::Raw(format!("-- {line}")))
        .collect();

    // What the function held still reads on its own, so it is put back rather than lost inside the
    // comment. A name rather than a local keeps it clear of the limit on how many a function may
    // declare, however many the one that failed was holding.
    for (at, inner) in held.functions().iter().enumerate() {
        let upvalues = (0..usize::from(inner.upvalues()))
            .map(|at| format!("u{at}"))
            .collect();
        if at == 0 {
            body.push(Stat::Raw("-- the functions it held:".to_owned()));
        }
        let read = closure(inner, upvalues, depth + 1, counts);
        body.push(Stat::Assign(
            vec![Target::Name(format!("f{at}"))],
            vec![Expr::Function(Box::new(read))],
        ));
    }

    Closure {
        parameters: (0..usize::from(held.parameters()))
            .map(|at| format!("a{at}"))
            .collect(),
        vararg: held.is_vararg(),
        body,
    }
}

/// Read a function into a closure, naming its upvalues from where the enclosing function bound them.
fn function(
    held: &Function,
    upvalues: Vec<String>,
    depth: usize,
    counts: &mut Counts,
) -> Reading<Closure> {
    if depth > MAX_DEPTH {
        return Err("functions nest too deeply to read");
    }

    let parameters = usize::from(held.parameters());
    let marks = pseudo(held);
    let labels = labels(held, &marks);
    let mut reader = Reader {
        function: held,
        sticky: sticky(held, &marks, &labels),
        pseudo: marks,
        slots: vec![Slot::Empty; usize::from(held.max_stack()).max(parameters) + 3],
        names: (0..parameters).map(|at| format!("a{at}")).collect(),
        upvalues,
        active: parameters,
        declared: 0,
        open: None,
        until: None,
        heads: Vec::new(),
        out: Vec::new(),
        depth,
        counts: Counts::default(),
    };

    // The 5.0 `arg` table sits in the register past the fixed parameters, so it takes a name of its
    // own and the parameter list still ends in `...`.
    if held.has_arg() {
        reader.names.push("arg".to_owned());
        reader.active += 1;
    }

    let mut body = reader.block(0, held.code().len(), None, None)?;
    // Every function ends in a return the compiler writes for it, which nobody put in the source.
    if matches!(body.last(), Some(Stat::Return(values)) if values.is_empty()) {
        body.pop();
    }
    counts.read += reader.counts.read;
    counts.raw += reader.counts.raw;
    Ok(Closure {
        parameters: reader.names.get(..parameters).unwrap_or_default().to_vec(),
        vararg: held.is_vararg(),
        body,
    })
}

/// Which words are operands of the instruction before them. A closure is followed by one word per
/// upvalue saying where it binds, and a list whose length did not fit its own field is followed by
/// that length.
fn pseudo(held: &Function) -> Vec<bool> {
    let code = held.code();
    let mut pseudo = vec![false; code.len()];
    let mut pc = 0;
    while pc < code.len() {
        let instruction = code[pc];
        let extra = match instruction.opcode() {
            Opcode::Closure => usize::from(
                held.functions()
                    .get(instruction.bx() as usize)
                    .map_or(0, Function::upvalues),
            ),
            Opcode::SetList if instruction.c() == 0 => 1,
            _ => 0,
        };
        for at in pc + 1..(pc + 1 + extra).min(code.len()) {
            pseudo[at] = true;
        }
        pc += 1 + extra;
    }
    pseudo
}

/// Whether the instruction decides where control goes next, which is where a run of operand loads
/// has to stop.
fn branches(opcode: Opcode) -> bool {
    matches!(
        opcode,
        Opcode::Eq
            | Opcode::Lt
            | Opcode::Le
            | Opcode::Test
            | Opcode::TestSet
            | Opcode::Jmp
            | Opcode::ForPrep
            | Opcode::ForLoop
            | Opcode::TForLoop
            | Opcode::Return
            | Opcode::TailCall
    )
}

/// The registers an instruction reads, and the last one it writes.
///
/// A run of results whose length the instruction leaves open counts only its first register; nothing
/// downstream of this reads past what it can name.
fn touches(held: Instruction, reads: &mut Vec<usize>) -> Option<usize> {
    let a = usize::from(held.a());
    let (b, c) = (usize::from(held.b()), usize::from(held.c()));
    let register = |value: u16, reads: &mut Vec<usize>| {
        if let Operand::Register(held) = Operand::from(value) {
            reads.push(usize::from(held));
        }
    };
    match held.opcode() {
        Opcode::Move | Opcode::Unm | Opcode::Not | Opcode::Len | Opcode::TestSet => {
            reads.push(b);
            Some(a)
        }
        Opcode::LoadK
        | Opcode::LoadBool
        | Opcode::GetUpval
        | Opcode::GetGlobal
        | Opcode::NewTable
        | Opcode::Closure => Some(a),
        Opcode::LoadNil => Some(b.max(a)),
        Opcode::SetUpval | Opcode::SetGlobal | Opcode::Test => {
            reads.push(a);
            None
        }
        Opcode::GetTable => {
            reads.push(b);
            register(held.c(), reads);
            Some(a)
        }
        Opcode::Self_ => {
            reads.push(b);
            register(held.c(), reads);
            Some(a + 1)
        }
        Opcode::SetTable => {
            reads.push(a);
            register(held.b(), reads);
            register(held.c(), reads);
            None
        }
        Opcode::Add | Opcode::Sub | Opcode::Mul | Opcode::Div | Opcode::Mod | Opcode::Pow => {
            register(held.b(), reads);
            register(held.c(), reads);
            Some(a)
        }
        Opcode::Eq | Opcode::Lt | Opcode::Le => {
            register(held.b(), reads);
            register(held.c(), reads);
            None
        }
        Opcode::Concat => {
            reads.extend(b..=c.max(b));
            Some(a)
        }
        Opcode::Call | Opcode::TailCall => {
            reads.extend(a..a + b.max(1));
            Some(a + c.saturating_sub(2).max(0))
        }
        Opcode::Return => {
            reads.extend(a..a + b.saturating_sub(1));
            None
        }
        // A loop's three workings are the VM's, not the source's; counting them as reads would make
        // locals of registers the loop itself is holding.
        Opcode::ForLoop | Opcode::ForPrep => Some(a + 3),
        Opcode::TForLoop => Some(a + 2 + c.max(1)),
        Opcode::SetList => {
            reads.extend(a + 1..=a + b.max(1));
            None
        }
        Opcode::Vararg => Some(a + c.saturating_sub(1).max(0)),
        Opcode::Jmp | Opcode::Close | Opcode::Unknown(_) => None,
    }
}

/// Where a jump lands, which is where a walk of the words stops knowing what the registers hold.
fn labels(held: &Function, pseudo: &[bool]) -> Vec<bool> {
    let code = held.code();
    let mut labels = vec![false; code.len() + 1];
    for (pc, instruction) in code.iter().enumerate() {
        if !pseudo.get(pc).copied().unwrap_or(false)
            && matches!(
                instruction.opcode(),
                Opcode::Jmp | Opcode::ForLoop | Opcode::ForPrep
            )
            && let Ok(target) = usize::try_from(pc as i64 + 1 + i64::from(instruction.sbx()))
            && let Some(label) = labels.get_mut(target)
        {
            *label = true;
        }
    }

    labels
}

/// Whether the instruction finishes what it was part of, so the next word starts a statement.
fn settles(held: Instruction) -> bool {
    matches!(
        held.opcode(),
        Opcode::SetGlobal
            | Opcode::SetTable
            | Opcode::SetUpval
            | Opcode::Return
            | Opcode::TailCall
            | Opcode::Jmp
            | Opcode::ForPrep
            | Opcode::ForLoop
            | Opcode::TForLoop
            | Opcode::SetList
            | Opcode::Close
    ) || (held.opcode() == Opcode::Call && held.c() == 1)
}

/// Where a value has to be a local rather than a temporary, keyed by the word that wrote it.
///
/// A temporary is read once, by the instruction the compiler emitted next to consume it. Anything
/// read twice, or read on the far side of a jump, sat in the register while something else ran, and
/// only a local does that.
fn sticky(held: &Function, pseudo: &[bool], labels: &[bool]) -> Vec<Option<usize>> {
    let code = held.code();
    let mut sticky = vec![None; code.len()];
    let mut written: Vec<Option<(usize, usize)>> = vec![None; 256];
    let mut era = 0usize;
    let mut active = usize::from(held.parameters()) + usize::from(held.has_arg());
    let mut start = true;
    let mut seen = vec![0usize; 256];
    let mut reads = Vec::new();
    for (pc, instruction) in code.iter().enumerate() {
        if pseudo.get(pc).copied().unwrap_or(false) {
            continue;
        }
        if labels.get(pc).copied().unwrap_or(false) {
            era += 1;
        }
        // A method lookup reserves its two registers before it works out the key, so a key that
        // needed a register of its own lands above where the locals stop rather than at it.
        let keying = code
            .iter()
            .enumerate()
            .skip(pc + 1)
            .find(|(at, _)| !pseudo.get(*at).copied().unwrap_or(false))
            .is_some_and(|(_, next)| {
                next.opcode() == Opcode::Self_
                    && Operand::from(next.c()) == Operand::Register(instruction.a())
            });
        reads.clear();
        let writes = touches(*instruction, &mut reads);
        for register in reads.drain(..) {
            let Some(Some((at, when))) = written.get(register).copied() else {
                continue;
            };
            if let Some(count) = seen.get_mut(register) {
                *count += 1;
                if when == era && *count < 2 {
                    continue;
                }
            }
            if let Some(slot) = sticky.get_mut(at) {
                *slot = Some(slot.map_or(register, |held: usize| held.max(register)));
            }
        }
        // At the start of a statement the next free register is where the locals stop, so a word
        // that writes above where they were thought to stop says there are more of them, and the
        // values in between were locals all along.
        if start
            && !keying
            && let Some(top) = writes
        {
            let first = usize::from(instruction.a());
            if first > active {
                for register in active..first {
                    if let Some(Some((at, _))) = written.get(register).copied()
                        && let Some(slot) = sticky.get_mut(at)
                    {
                        *slot = Some(slot.map_or(register, |held: usize| held.max(register)));
                    }
                }
                active = first;
            }
            let _ = top;
        }
        start = settles(*instruction) || labels.get(pc + 1).copied().unwrap_or(false);
        // A loop opens a scope over its three workings and the variables it counts, and closes it
        // where it ends. A generic one opens at the jump into it, which is where its body begins.
        match instruction.opcode() {
            Opcode::ForPrep => active = usize::from(instruction.a()) + 4,
            Opcode::ForLoop | Opcode::TForLoop => active = usize::from(instruction.a()),
            Opcode::Jmp => {
                let target = usize::try_from(pc as i64 + 1 + i64::from(instruction.sbx()));
                if let Ok(target) = target
                    && let Some(held) = code.get(target)
                    && held.opcode() == Opcode::TForLoop
                {
                    active = usize::from(held.a()) + 3 + usize::from(held.c()).max(1);
                }
            }
            _ => (),
        }

        if branches(instruction.opcode()) {
            era += 1;
        }
        if let Some(top) = writes {
            for register in usize::from(instruction.a())..=top.min(255) {
                if let Some(slot) = written.get_mut(register) {
                    *slot = Some((pc, era));
                }
                if let Some(count) = seen.get_mut(register) {
                    *count = 0;
                }
            }
        }
    }
    // A table is not built until the list that fills it, and a method is not looked up until the
    // call that takes it, so a mark on the word that opens one moves to the word that closes it.
    for pc in 0..code.len() {
        if sticky.get(pc).copied().flatten().is_none() || pseudo.get(pc).copied().unwrap_or(false) {
            continue;
        }
        let opening = code[pc];
        let register = opening.a();
        let closing = match opening.opcode() {
            Opcode::NewTable => Opcode::SetList,
            Opcode::Self_ => Opcode::Call,
            _ => continue,
        };
        let mut found = None;
        for (at, held) in code.iter().enumerate().skip(pc + 1) {
            if pseudo.get(at).copied().unwrap_or(false) {
                continue;
            }
            if held.opcode() == closing && held.a() == register {
                found = Some(at);
                // A long list takes several words to fill, so the last of them is the end of it.
                if closing == Opcode::Call {
                    break;
                }
                continue;
            }
            let mut reads = Vec::new();
            let writes = touches(*held, &mut reads);
            if writes.is_some_and(|top| usize::from(register) <= top)
                && usize::from(held.a()) <= usize::from(register)
            {
                break;
            }
        }
        if let Some(found) = found
            && let Some(mark) = sticky.get_mut(pc).map(std::mem::take)
            && let Some(slot) = sticky.get_mut(found)
        {
            *slot = Some(match (*slot, mark) {
                (Some(held), Some(mark)) => held.max(mark),
                (held, mark) => held.or(mark).unwrap_or(usize::from(register)),
            });
        }
    }

    sticky
}

fn conditional(opcode: Opcode) -> bool {
    matches!(
        opcode,
        Opcode::Eq | Opcode::Lt | Opcode::Le | Opcode::Test | Opcode::TestSet
    )
}

impl<'a> Reader<'a> {
    fn code(&self) -> &'a [Instruction] {
        self.function.code()
    }

    fn at(&self, pc: usize) -> Reading<Instruction> {
        self.code()
            .get(pc)
            .copied()
            .ok_or("a jump lands off the end of the function")
    }

    /// The next word that is an instruction rather than an operand.
    fn after(&self, pc: usize) -> usize {
        let mut at = pc + 1;
        while self.pseudo.get(at) == Some(&true) {
            at += 1;
        }
        at
    }

    /// Where a jump at `pc` goes.
    fn target(&self, pc: usize) -> Reading<usize> {
        let offset = self.at(pc)?.sbx();
        usize::try_from(pc as i64 + 1 + i64::from(offset))
            .ok()
            .filter(|target| *target <= self.code().len())
            .ok_or("a jump lands off the end of the function")
    }

    // -- values ------------------------------------------------------------------------------

    fn constant(&self, at: usize) -> Reading<Expr> {
        match self.function.constants().get(at) {
            Some(Constant::Nil) => Ok(Expr::Nil),
            Some(Constant::Boolean(held)) => Ok(Expr::Bool(*held)),
            Some(Constant::Number(held)) => Ok(Expr::Number(*held)),
            Some(Constant::String(held)) => Ok(Expr::Str(held.clone())),
            None => Err("an instruction names a constant the function does not have"),
        }
    }

    fn text(&self, at: usize) -> Reading<String> {
        match self.function.constants().get(at) {
            Some(Constant::String(held)) if is_name(held) => {
                Ok(String::from_utf8_lossy(held).into_owned())
            }
            _ => Err("a name is held as something source cannot spell"),
        }
    }

    fn named(&self, register: usize) -> Reading<Expr> {
        match self.names.get(register).filter(|name| !name.is_empty()) {
            Some(name) => Ok(Expr::Name(name.clone())),
            None => Err("an instruction reads a register that holds nothing"),
        }
    }

    /// Take what a register holds. One below the expression under construction is a local and stays
    /// where it is; one above is a temporary, and taking it empties the slot.
    fn take(&mut self, register: usize) -> Reading<Expr> {
        if register < self.active {
            return self.named(register);
        }
        // A read that reaches under a temporary is not part of one expression, so the value it reads
        // outlived the statement that made it and was a local.
        if self
            .slots
            .iter()
            .skip(register + 1)
            .any(|slot| !matches!(slot, Slot::Empty))
        {
            self.declare(register + 1)?;
            return self.named(register);
        }
        match self.slots.get_mut(register) {
            Some(slot) => match std::mem::take(slot) {
                Slot::Value(held) => Ok(held),
                _ => Err("an instruction reads a register that holds nothing"),
            },
            None => Err("an instruction reads a register outside the stack"),
        }
    }

    fn operand(&mut self, held: u16) -> Reading<Expr> {
        match Operand::from(held) {
            Operand::Register(register) => self.take(usize::from(register)),
            Operand::Constant(at) => self.constant(usize::from(at)),
        }
    }

    /// Two operands, taken from the top of the stack down so neither is read past the other.
    fn operands(&mut self, b: u16, c: u16) -> Reading<(Expr, Expr)> {
        let right = self.operand(c)?;
        let left = self.operand(b)?;
        Ok((left, right))
    }

    // -- locals ------------------------------------------------------------------------------

    /// Turn every temporary below `top` into a local, which is what a value that outlived its
    /// statement always was.
    fn declare(&mut self, top: usize) -> Reading<()> {
        let top = top.min(self.slots.len());
        let mut names = Vec::new();
        let mut values = Vec::new();
        for register in self.active..top {
            match self.slots.get_mut(register).map(std::mem::take) {
                // A call's further results share the one expression that produced them.
                Some(Slot::Rest) => (),
                Some(Slot::Empty) => values.push(Expr::Nil),
                Some(Slot::Value(held)) => values.push(held),
                Some(Slot::Method(..)) => return Err("a method lookup outlived its call"),
                None => return Err("a local lands outside the stack"),
            }
            let name = format!("v{}", self.declared);
            self.declared += 1;
            while self.names.len() <= register {
                self.names.push(String::new());
            }
            self.names[register] = name.clone();
            names.push(name);
        }
        if !names.is_empty() {
            // A declaration without values reads as nils, so trailing ones say nothing.
            while matches!(values.last(), Some(Expr::Nil)) {
                values.pop();
            }
            self.out.push(Stat::Local(names, values));
        }
        self.active = top.max(self.active);
        self.open = None;
        Ok(())
    }

    /// Everything the registers still hold, as a declaration of its own.
    fn flush(&mut self) -> Reading<()> {
        let top = self
            .slots
            .iter()
            .rposition(|slot| !matches!(slot, Slot::Empty));
        match top {
            Some(top) => self.declare(top + 1),
            None => Ok(()),
        }
    }

    /// Put a value in a register, declaring anything a finished statement left above it.
    fn set(&mut self, register: usize, value: Expr) -> Reading<()> {
        if register < self.active {
            let target = Target::Name(self.names.get(register).cloned().unwrap_or_default());
            self.out.push(Stat::Assign(vec![target], vec![value]));
            return Ok(());
        }
        // Writing a register a temporary already sits at or above ends the statement that made
        // those temporaries, so they were locals.
        if self
            .slots
            .iter()
            .skip(register)
            .any(|slot| !matches!(slot, Slot::Empty))
        {
            self.declare(register)?;
            if register < self.active {
                let target = Target::Name(self.names.get(register).cloned().unwrap_or_default());
                self.out.push(Stat::Assign(vec![target], vec![value]));
                return Ok(());
            }
        }
        match self.slots.get_mut(register) {
            Some(slot) => {
                *slot = Slot::Value(value);
                Ok(())
            }
            None => Err("a value lands outside the stack"),
        }
    }

    /// Where the locals in scope stop, so a block can put them back as they were.
    fn scope(&self) -> (usize, usize) {
        (self.active, self.names.len())
    }

    fn close(&mut self, (active, names): (usize, usize)) {
        self.active = active;
        self.names.truncate(names.max(active));
    }

    /// Name the registers a loop's own variables sit in, and open the scope they live in.
    fn bind(&mut self, from: usize, count: usize) -> Reading<Vec<String>> {
        let mut names = Vec::with_capacity(count);
        for register in from..from + count {
            let name = format!("v{}", self.declared);
            self.declared += 1;
            while self.names.len() <= register {
                self.names.push(String::new());
            }
            self.names[register] = name.clone();
            names.push(name);
        }
        self.active = from + count;
        Ok(names)
    }

    // -- blocks ------------------------------------------------------------------------------

    /// Read the statements over `lo..hi`. `escape` is where a `break` of the enclosing loop lands,
    /// and `until` the head of the `repeat` whose condition ends this block.
    fn block(
        &mut self,
        lo: usize,
        hi: usize,
        escape: Option<usize>,
        until: Option<usize>,
    ) -> Reading<Vec<Stat>> {
        if self.depth > MAX_DEPTH {
            return Err("blocks nest too deeply to read");
        }
        self.depth += 1;

        let held = std::mem::take(&mut self.out);
        let scope = self.scope();

        let mut result = Ok(());
        let mut pc = lo;
        while pc < hi {
            match self.statement(pc, hi, escape, until) {
                Ok(next) if next > pc => pc = next,
                Ok(_) => {
                    result = Err("a statement did not move the reading forward");
                    break;
                }
                Err(error) => {
                    result = Err(error);
                    break;
                }
            }
        }
        if result.is_ok() {
            result = self.flush();
        }

        let body = std::mem::replace(&mut self.out, held);
        self.close(scope);
        for slot in &mut self.slots {
            *slot = Slot::Empty;
        }
        self.open = None;
        self.depth -= 1;
        result.map(|()| body)
    }

    fn statement(
        &mut self,
        pc: usize,
        hi: usize,
        escape: Option<usize>,
        until: Option<usize>,
    ) -> Reading<usize> {
        if let Some(end) = self.loops(pc, hi)? {
            return self.loop_statement(pc, end, hi);
        }

        let instruction = self.at(pc)?;
        match instruction.opcode() {
            Opcode::ForPrep => self.numeric_for(pc, hi),

            Opcode::Jmp => {
                let target = self.target(pc)?;
                if target > pc
                    && matches!(
                        self.at(target).map(Instruction::opcode),
                        Ok(Opcode::TForLoop)
                    )
                {
                    return self.generic_for(pc, target);
                }
                if self.escapes(target, escape) {
                    self.flush()?;
                    self.out.push(Stat::Break);
                    return Ok(pc + 1);
                }
                // A jump to where the block ends only falls out of it.
                match target == hi || self.threaded(target, hi) == hi {
                    true => Ok(pc + 1),
                    false => Err("a jump goes somewhere no statement explains"),
                }
            }

            opcode if conditional(opcode) => self.branch(pc, hi, escape, until),

            _ => self.step(pc),
        }
    }

    /// The word a loop starting at `pc` jumps back from, where one does.
    fn loops(&self, pc: usize, hi: usize) -> Reading<Option<usize>> {
        if self.heads.contains(&pc) {
            return Ok(None);
        }
        let mut found = None;
        let mut at = pc;
        while at < hi {
            if !self.pseudo.get(at).copied().unwrap_or(false)
                && self.at(at)?.opcode() == Opcode::Jmp
                && self.target(at)? == pc
            {
                found = Some(at);
            }
            at = self.after(at);
        }
        Ok(found)
    }

    fn loop_statement(&mut self, pc: usize, end: usize, hi: usize) -> Reading<usize> {
        self.flush()?;
        let escape = Some(end + 1);
        self.heads.push(pc);
        let held = self.loop_body(pc, end, hi, escape);
        self.heads.pop();
        held
    }

    fn loop_body(
        &mut self,
        pc: usize,
        end: usize,
        hi: usize,
        escape: Option<usize>,
    ) -> Reading<usize> {
        // A conditional at the head whose failure lands past the jump back is a `while`; a
        // conditional just before the jump back is a `repeat`; anything else loops forever.
        let tests = self.tests(pc, hi);
        if let Some((count, false_target)) = choose(&tests)
            && false_target == end + 1
            && tests.get(count - 1).is_some_and(|test| test.after <= end)
        {
            let condition = self.condition(&tests, count, pc)?;
            let body = self.block(tests[count - 1].after, end, escape, None)?;
            self.out.push(Stat::While(condition, body));
            return Ok(end + 1);
        }

        if end > 0 && conditional(self.at(end - 1)?.opcode()) {
            let body = self.block(pc, end + 1, escape, Some(pc))?;
            let condition = self
                .until
                .take()
                .ok_or("a repeat ended without a condition")?;
            self.out.push(Stat::Repeat(body, condition));
            return Ok(end + 1);
        }

        let body = self.block(pc, end, escape, None)?;
        self.out.push(Stat::While(Expr::Bool(true), body));
        Ok(end + 1)
    }

    fn numeric_for(&mut self, pc: usize, hi: usize) -> Reading<usize> {
        let instruction = self.at(pc)?;
        let end = self.target(pc)?;
        if end <= pc || end >= hi || self.at(end)?.opcode() != Opcode::ForLoop {
            return Err("a numeric for does not end in the loop it opened");
        }
        let control = usize::from(instruction.a());
        let step = self.take(control + 2)?;
        let limit = self.take(control + 1)?;
        let start = self.take(control)?;

        self.declare(control)?;
        let scope = self.scope();
        let names = self.bind(control, 4)?;
        let name = names.last().cloned().unwrap_or_default();
        let body = self.block(pc + 1, end, Some(end + 1), None)?;
        self.close(scope);
        self.out.push(Stat::NumericFor {
            name,
            start,
            limit,
            step,
            body,
        });
        Ok(end + 1)
    }

    fn generic_for(&mut self, pc: usize, end: usize) -> Reading<usize> {
        let instruction = self.at(end)?;
        let control = usize::from(instruction.a());
        let count = usize::from(instruction.c()).max(1);

        let mut values = Vec::new();
        for register in (control..control + 3).rev() {
            match std::mem::take(
                self.slots
                    .get_mut(register)
                    .ok_or("a for reaches past the stack")?,
            ) {
                Slot::Empty | Slot::Rest => (),
                Slot::Value(held) => values.push(held),
                Slot::Method(..) => return Err("a method lookup outlived its call"),
            }
        }
        values.reverse();

        self.declare(control)?;
        let scope = self.scope();
        let names = self.bind(control, 3 + count)?;
        let names = names.get(3..).unwrap_or_default().to_vec();
        let body = self.block(pc + 1, end, Some(end + 2), None)?;
        self.close(scope);
        self.out.push(Stat::GenericFor {
            names,
            values,
            body,
        });
        Ok(end + 2)
    }

    // -- conditionals ------------------------------------------------------------------------

    /// The run of conditionals starting at `pc`, with the operand loads between them stepped over.
    fn tests(&self, pc: usize, hi: usize) -> Vec<Test> {
        let mut tests = Vec::new();
        let mut at = pc;
        loop {
            let mut test = at;
            while test < hi
                && self
                    .at(test)
                    .map(|held| !branches(held.opcode()))
                    .unwrap_or(false)
            {
                test = self.after(test);
            }
            if test >= hi
                || !self
                    .at(test)
                    .map(|held| conditional(held.opcode()))
                    .unwrap_or(false)
            {
                return tests;
            }
            let jump = test + 1;
            if self.at(jump).map(Instruction::opcode) != Ok(Opcode::Jmp) {
                return tests;
            }
            let Ok(target) = self.target(jump) else {
                return tests;
            };
            tests.push(Test {
                test,
                target: self.threaded(target, hi),
                after: jump + 1,
            });
            if tests.len() >= MAX_DEPTH {
                return tests;
            }
            at = jump + 1;
        }
    }

    /// Where a jump would land if it went to the end of the block instead. The compiler collapses a
    /// jump to a jump, so one landing where the block's own last jump goes is that jump.
    fn threaded(&self, target: usize, hi: usize) -> usize {
        match target != hi
            && self.at(hi).map(Instruction::opcode) == Ok(Opcode::Jmp)
            && self.target(hi) == Ok(target)
        {
            true => hi,
            false => target,
        }
    }

    /// Whether a jump to `target` leaves the loop, which is what a `break` is.
    fn escapes(&self, target: usize, escape: Option<usize>) -> bool {
        escape.is_some_and(|at| target == at || self.threaded(target, at) == at)
    }

    /// The condition one conditional falls through on.
    fn test(&mut self, pc: usize) -> Reading<Expr> {
        let instruction = self.at(pc)?;
        let (a, b, c) = (instruction.a(), instruction.b(), instruction.c());
        let held = match instruction.opcode() {
            Opcode::Eq | Opcode::Lt | Opcode::Le => {
                let operator = match instruction.opcode() {
                    Opcode::Eq => Binary::Eq,
                    Opcode::Lt => Binary::Lt,
                    _ => Binary::Le,
                };
                let (left, right) = self.operands(b, c)?;
                let held = Expr::Binary(operator, Box::new(left), Box::new(right));
                match a {
                    0 => held,
                    _ => held.negate(),
                }
            }
            Opcode::Test => {
                let held = self.take(usize::from(a))?;
                match c {
                    0 => held,
                    _ => held.negate(),
                }
            }
            Opcode::TestSet => {
                let held = self.take(usize::from(b))?;
                self.set(usize::from(a), held.clone())?;
                match c {
                    0 => held,
                    _ => held.negate(),
                }
            }
            _ => return Err("a conditional is not one"),
        };
        Ok(held)
    }

    /// Evaluate the first `count` conditionals of a chain and fold them into one expression.
    fn condition(&mut self, tests: &[Test], count: usize, start: usize) -> Reading<Expr> {
        let mut held = Vec::with_capacity(count);
        let mut pc = start;
        for test in tests
            .get(..count)
            .ok_or("a chain is shorter than it was measured")?
        {
            while pc < test.test {
                let next = self.step(pc)?;
                if next <= pc {
                    return Err("a statement did not move the reading forward");
                }
                pc = next;
            }
            held.push(self.test(test.test)?);
            pc = test.after;
        }
        let true_target = tests[count - 1].after;
        let false_target = tests[..count]
            .iter()
            .map(|test| test.target)
            .max()
            .unwrap_or(0);
        combine(tests, &mut held, 0, count, true_target, false_target)
    }

    fn branch(
        &mut self,
        pc: usize,
        hi: usize,
        escape: Option<usize>,
        until: Option<usize>,
    ) -> Reading<usize> {
        let tests = self.tests(pc, hi);
        let Some((count, false_target)) = choose(&tests) else {
            // The chain runs backwards, which is how a `repeat` says where its body began.
            return match until {
                Some(head) => self.until_condition(&tests, pc, head),
                None => Err("a conditional goes somewhere no statement explains"),
            };
        };
        let body_at = tests[count - 1].after;

        // A pair of loads on the far side of the chain is a comparison standing as a value.
        if let Some(next) = self.boolean(&tests, count, pc, body_at, false_target)? {
            return Ok(next);
        }
        if let Some(next) = self.joined(&tests, count, pc, body_at, false_target)? {
            return Ok(next);
        }
        // Only where the chain is one conditional: a longer one the condition already explains is a
        // condition, and reading it as a value would swallow the arms it decides between.
        if count == 1
            && let Some(next) = self.shortcut(&tests, pc, hi)?
        {
            return Ok(next);
        }

        // A conditional leaving for where a `break` lands has one on the arm that fails it, and the
        // rest of the block is the arm that does not.
        if false_target > hi {
            if !self.escapes(false_target, escape) {
                return Err("a conditional leaves the block it is in");
            }
            let condition = self.condition(&tests, count, pc)?;
            self.flush()?;
            let body = self.block(body_at, hi, escape, None)?;
            self.out
                .push(Stat::If(vec![(condition, body)], Some(vec![Stat::Break])));
            return Ok(hi);
        }
        let condition = self.condition(&tests, count, pc)?;
        self.flush()?;

        // A jump over what follows the body is the `else`, unless it is a `break` out of a loop.
        let mut arms = Vec::new();
        let mut otherwise = None;
        let mut end = false_target;
        if false_target > body_at
            && self.at(false_target - 1).map(Instruction::opcode) == Ok(Opcode::Jmp)
            && !self.pseudo.get(false_target - 1).copied().unwrap_or(false)
        {
            let over = self.threaded(self.target(false_target - 1)?, hi);
            if over > false_target && over <= hi && escape != Some(over) {
                let body = self.block(body_at, false_target - 1, escape, None)?;
                arms.push((condition.clone(), body));
                otherwise = Some(self.block(false_target, over, escape, None)?);
                end = over;
            }
        }
        if arms.is_empty() {
            arms.push((condition, self.block(body_at, false_target, escape, None)?));
        }

        // An `else` holding one `if` and nothing else is what `elseif` compiles to.
        while let Some([Stat::If(inner, tail)]) = otherwise.as_deref() {
            arms.extend(inner.iter().cloned());
            otherwise = tail.clone();
        }

        self.out.push(Stat::If(arms, otherwise));
        Ok(end)
    }

    /// A chain jumping back to the head of a `repeat`, which is its `until`.
    fn until_condition(&mut self, tests: &[Test], pc: usize, head: usize) -> Reading<usize> {
        let count = tests
            .iter()
            .position(|test| test.target == head)
            .map(|at| at + 1)
            .ok_or("a conditional goes somewhere no statement explains")?;
        if tests[..count]
            .iter()
            .any(|test| test.target != head && test.target < tests[0].after)
        {
            return Err("a repeat's condition goes somewhere no statement explains");
        }
        let mut held = Vec::with_capacity(count);
        let mut at = pc;
        for test in &tests[..count] {
            while at < test.test {
                let next = self.step(at)?;
                if next <= at {
                    return Err("a statement did not move the reading forward");
                }
                at = next;
            }
            held.push(self.test(test.test)?);
            at = test.after;
        }
        let true_target = tests[count - 1].after;
        self.until = Some(combine(tests, &mut held, 0, count, true_target, head)?);
        Ok(true_target)
    }

    /// A conditional whose two paths leave a value in one register, which is `and` or `or` standing
    /// as a value rather than deciding a statement.
    fn shortcut(&mut self, tests: &[Test], pc: usize, hi: usize) -> Reading<Option<usize>> {
        let Some(test) = tests.first().copied() else {
            return Ok(None);
        };
        let instruction = self.at(test.test)?;
        let (register, target) = match instruction.opcode() {
            Opcode::Test => (usize::from(instruction.a()), usize::from(instruction.a())),
            Opcode::TestSet => (usize::from(instruction.b()), usize::from(instruction.a())),
            _ => return Ok(None),
        };
        let end = test.target;
        if end <= test.after || end > hi {
            return Ok(None);
        }

        // The far side has to build a value rather than run statements, and leave it where the
        // near side left its own.
        let mut at = test.after;
        let mut last = None;
        let mut reads = Vec::new();
        while at < end {
            let held = self.at(at)?;
            let settles = matches!(
                held.opcode(),
                Opcode::SetGlobal
                    | Opcode::SetTable
                    | Opcode::SetUpval
                    | Opcode::Return
                    | Opcode::TailCall
                    | Opcode::ForPrep
                    | Opcode::ForLoop
                    | Opcode::TForLoop
                    | Opcode::SetList
            ) || (held.opcode() == Opcode::Call && held.c() == 1);
            if settles {
                return Ok(None);
            }
            reads.clear();
            if let Some(top) = touches(held, &mut reads) {
                if top < target {
                    return Ok(None);
                }
                last = Some(top);
            }
            at = self.after(at);
        }
        if last != Some(target) {
            return Ok(None);
        }

        let mut at = pc;
        while at < test.test {
            let next = self.step(at)?;
            if next <= at {
                return Err("a statement did not move the reading forward");
            }
            at = next;
        }
        let left = self.take(register)?;

        let held = self.out.len();
        let mut walk = test.after;
        while walk < end {
            let next = self.statement(walk, end, None, None)?;
            if next <= walk {
                return Err("a statement did not move the reading forward");
            }
            walk = next;
        }
        if self.out.len() != held {
            return Err("a short circuit ran a statement");
        }

        let right = self.take(target)?;
        let operator = match instruction.c() {
            0 => Binary::And,
            _ => Binary::Or,
        };
        self.set(
            target,
            Expr::Binary(operator, Box::new(left), Box::new(right)),
        )?;
        Ok(Some(end))
    }

    /// A comparison used as a value, which lands as the two loads a jump picks between.
    fn boolean(
        &mut self,
        tests: &[Test],
        count: usize,
        pc: usize,
        body_at: usize,
        false_target: usize,
    ) -> Reading<Option<usize>> {
        let is_load = |held: Reading<Instruction>, b: u16| matches!(held, Ok(held) if held.opcode() == Opcode::LoadBool && held.b() == b);
        if false_target != body_at + 1
            || !is_load(self.at(body_at), 0)
            || !is_load(self.at(false_target), 1)
            || self.at(body_at)?.a() != self.at(false_target)?.a()
            || self.at(body_at)?.c() == 0
        {
            return Ok(None);
        }
        let condition = self.condition(tests, count, pc)?;
        let register = usize::from(self.at(body_at)?.a());
        // The load reached by falling through is the false one, so the value is the other way up.
        self.set(register, condition.negate())?;
        Ok(Some(false_target + 1))
    }

    /// A comparison joined by `and` to a value: failing it lands on the pair of loads, and passing
    /// it works the value out and jumps over them.
    fn joined(
        &mut self,
        tests: &[Test],
        count: usize,
        pc: usize,
        body_at: usize,
        false_target: usize,
    ) -> Reading<Option<usize>> {
        let pair = false_target;
        let is_load = |held: Reading<Instruction>, b: u16| matches!(held, Ok(held) if held.opcode() == Opcode::LoadBool && held.b() == b);
        if pair <= body_at
            || !is_load(self.at(pair), 0)
            || !is_load(self.at(pair.saturating_add(1)), 1)
            || self.at(pair)?.a() != self.at(pair + 1)?.a()
            || self.at(pair)?.c() == 0
        {
            return Ok(None);
        }
        // What the passing side works out has to end by jumping over both loads.
        let over = pair - 1;
        if over < body_at
            || self.at(over)?.opcode() != Opcode::Jmp
            || self.target(over)? != pair + 2
        {
            return Ok(None);
        }
        let register = usize::from(self.at(pair)?.a());
        if !self.settles_into(body_at, over, register)? {
            return Ok(None);
        }

        let condition = self.condition(tests, count, pc)?;
        let held = self.out.len();
        let mut at = body_at;
        while at < over {
            let next = self.statement(at, over, None, None)?;
            if next <= at {
                return Err("a statement did not move the reading forward");
            }
            at = next;
        }
        if self.out.len() != held {
            return Err("a short circuit ran a statement");
        }
        let right = self.take(register)?;
        self.set(
            register,
            Expr::Binary(Binary::And, Box::new(condition), Box::new(right)),
        )?;
        Ok(Some(pair + 2))
    }

    /// Whether `lo..hi` only works a value out, and leaves it in `register`.
    fn settles_into(&self, lo: usize, hi: usize, register: usize) -> Reading<bool> {
        let mut at = lo;
        let mut last = None;
        let mut reads = Vec::new();
        while at < hi {
            let held = self.at(at)?;
            let settles = matches!(
                held.opcode(),
                Opcode::SetGlobal
                    | Opcode::SetTable
                    | Opcode::SetUpval
                    | Opcode::Return
                    | Opcode::TailCall
                    | Opcode::ForPrep
                    | Opcode::ForLoop
                    | Opcode::TForLoop
                    | Opcode::SetList
            ) || (held.opcode() == Opcode::Call && held.c() == 1);
            if settles {
                return Ok(false);
            }
            reads.clear();
            if let Some(top) = touches(held, &mut reads) {
                if top < register {
                    return Ok(false);
                }
                last = Some(top);
            }
            at = self.after(at);
        }
        Ok(last == Some(register))
    }

    // -- straight-line code ------------------------------------------------------------------

    /// Read one instruction, and answer with where the reading goes next.
    fn step(&mut self, pc: usize) -> Reading<usize> {
        let next = self.walk(pc)?;
        // What the word left behind is a local rather than a temporary wherever the pass that
        // measured the registers said so.
        if let Some(Some(top)) = self.sticky.get(pc).copied() {
            self.declare(top + 1)?;
        }
        Ok(next)
    }

    fn walk(&mut self, pc: usize) -> Reading<usize> {
        let instruction = self.at(pc)?;
        let a = usize::from(instruction.a());
        let (b, c) = (instruction.b(), instruction.c());
        let next = self.after(pc);

        match instruction.opcode() {
            Opcode::Move => {
                let held = self.take(usize::from(b))?;
                self.set(a, held)?;
            }

            Opcode::LoadK => {
                let held = self.constant(instruction.bx() as usize)?;
                self.set(a, held)?;
            }

            Opcode::LoadBool => {
                self.set(a, Expr::Bool(b != 0))?;
                // The skip is what a comparison-as-a-value uses, and that shape is read elsewhere.
                if c != 0 {
                    return Err("a boolean load skips an instruction outside a comparison");
                }
            }

            Opcode::LoadNil => {
                for register in a..=usize::from(b).max(a) {
                    self.set(register, Expr::Nil)?;
                }
            }

            Opcode::GetUpval => {
                let held = self.upvalue(usize::from(b))?;
                self.set(a, held)?;
            }

            Opcode::SetUpval => {
                let held = self.take(a)?;
                let name = self.upvalue(usize::from(b))?;
                self.flush()?;
                let Expr::Name(name) = name else {
                    return Err("an upvalue has no name");
                };
                self.out
                    .push(Stat::Assign(vec![Target::Name(name)], vec![held]));
            }

            Opcode::GetGlobal => {
                let name = self.text(instruction.bx() as usize)?;
                self.set(a, Expr::Name(name))?;
            }

            Opcode::SetGlobal => {
                let name = self.text(instruction.bx() as usize)?;
                let held = self.take(a)?;
                self.flush()?;
                self.out
                    .push(Stat::Assign(vec![Target::Name(name)], vec![held]));
            }

            Opcode::GetTable => {
                let key = self.operand(c)?;
                let table = self.take(usize::from(b))?;
                self.set(a, Expr::Index(Box::new(table), Box::new(key)))?;
            }

            Opcode::SetTable => {
                let (key, value) = self.operands(b, c)?;
                let table = self.take(a)?;
                self.flush()?;
                self.out
                    .push(Stat::Assign(vec![Target::Index(table, key)], vec![value]));
            }

            Opcode::NewTable => {
                self.set(
                    a,
                    Expr::Table {
                        array: Vec::new(),
                        hash: Vec::new(),
                    },
                )?;
            }

            Opcode::Self_ => {
                // A function holding more than 256 constants cannot name a method in the
                // instruction, so the name arrives in a register instead.
                let name = match self.operand(c)? {
                    Expr::Str(held) if is_name(&held) => {
                        String::from_utf8_lossy(&held).into_owned()
                    }
                    _ => return Err("a method is named by something source cannot spell"),
                };
                let object = self.take(usize::from(b))?;
                if self
                    .slots
                    .iter()
                    .skip(a)
                    .any(|slot| !matches!(slot, Slot::Empty))
                {
                    self.declare(a)?;
                }
                if a < self.active {
                    return Err("a method lookup lands on a local");
                }
                *self
                    .slots
                    .get_mut(a)
                    .ok_or("a method lands outside the stack")? = Slot::Method(object, name);
                *self
                    .slots
                    .get_mut(a + 1)
                    .ok_or("a method lands outside the stack")? = Slot::Rest;
            }

            Opcode::Add | Opcode::Sub | Opcode::Mul | Opcode::Div | Opcode::Mod | Opcode::Pow => {
                let operator = match instruction.opcode() {
                    Opcode::Add => Binary::Add,
                    Opcode::Sub => Binary::Sub,
                    Opcode::Mul => Binary::Mul,
                    Opcode::Div => Binary::Div,
                    Opcode::Mod => Binary::Mod,
                    _ => Binary::Pow,
                };
                let (left, right) = self.operands(b, c)?;
                self.set(a, Expr::Binary(operator, Box::new(left), Box::new(right)))?;
            }

            Opcode::Unm | Opcode::Not | Opcode::Len => {
                let operator = match instruction.opcode() {
                    Opcode::Unm => Unary::Minus,
                    Opcode::Not => Unary::Not,
                    _ => Unary::Length,
                };
                let held = self.take(usize::from(b))?;
                let held = match operator {
                    Unary::Not => held.negate(),
                    _ => Expr::Unary(operator, Box::new(held)),
                };
                self.set(a, held)?;
            }

            Opcode::Concat => {
                let (first, last) = (usize::from(b), usize::from(c));
                if last < first {
                    return Err("a concatenation runs backwards");
                }
                let mut parts = Vec::with_capacity(last - first + 1);
                for register in (first..=last).rev() {
                    parts.push(self.take(register)?);
                }
                let mut held = parts.pop().ok_or("a concatenation joins nothing")?;
                while let Some(next) = parts.pop() {
                    held = Expr::Binary(Binary::Concat, Box::new(held), Box::new(next));
                }
                self.set(a, held)?;
            }

            Opcode::Call | Opcode::TailCall => return self.call(pc, next),

            Opcode::Return => {
                let values = self.values(a, usize::from(b))?;
                self.flush()?;
                self.out.push(Stat::Return(values));
            }

            Opcode::SetList => {
                let count = match b {
                    0 => self.open.map_or(0, |open| open.saturating_sub(a + 1)),
                    held => usize::from(held),
                };
                let block = match c {
                    0 => self.at(pc + 1)?.raw() as usize,
                    held => usize::from(held),
                };
                let mut values = Vec::with_capacity(count);
                for register in (a + 1..=a + count).rev() {
                    values.push(self.take(register)?);
                }
                values.reverse();
                let Some(Slot::Value(Expr::Table { array, .. })) = self.slots.get_mut(a) else {
                    return Err("a list is set on something that is not a table");
                };
                if array.len() + 1 != (block - 1) * LIST_BLOCK + 1 {
                    return Err("a list is set out of order");
                }
                array.extend(values);
                self.open = None;
            }

            Opcode::Closure => {
                let held = self.closure(pc, instruction.bx() as usize)?;
                self.set(a, Expr::Function(Box::new(held)))?;
            }

            Opcode::Vararg => {
                if !self.function.is_vararg() {
                    return Err("a function that takes no varargs reads them");
                }
                self.set(a, Expr::Vararg)?;
                match b {
                    0 => self.open = Some(a + 1),
                    held => self.rest(a, usize::from(held) - 1)?,
                }
            }

            // A scope closing over an upvalue leaves nothing for a reading to say.
            Opcode::Close => (),

            Opcode::Jmp
            | Opcode::Eq
            | Opcode::Lt
            | Opcode::Le
            | Opcode::Test
            | Opcode::TestSet
            | Opcode::ForPrep
            | Opcode::ForLoop
            | Opcode::TForLoop => return Err("a jump turned up where a value was expected"),

            Opcode::Unknown(_) => return Err("the function holds an opcode Lua 5.1 does not have"),
        }
        Ok(next)
    }

    fn upvalue(&self, at: usize) -> Reading<Expr> {
        match self.upvalues.get(at) {
            Some(name) => Ok(Expr::Name(name.clone())),
            None => Err("an instruction names an upvalue the function does not close over"),
        }
    }

    /// Mark the registers a run of results past the first covers.
    fn rest(&mut self, first: usize, count: usize) -> Reading<()> {
        for register in first + 1..first + count.max(1) {
            match self.slots.get_mut(register) {
                Some(slot) => *slot = Slot::Rest,
                None => return Err("a result lands outside the stack"),
            }
        }
        Ok(())
    }

    /// A run of values starting at `a`, where `count` is the encoded one that reads zero as
    /// everything up to the last call's open end.
    fn values(&mut self, a: usize, count: usize) -> Reading<Vec<Expr>> {
        let last = match count {
            0 => self.open.ok_or("a run of values has no end")?,
            held => a + held - 1,
        };
        if last < a {
            return Ok(Vec::new());
        }
        let mut values = Vec::with_capacity(last - a);
        for register in (a..last).rev() {
            // A further result is carried by the expression that made it, but its slot still has to
            // be given up or the next statement finds it and declares a local nobody wrote.
            match self.slots.get(register) {
                Some(Slot::Rest) => self.slots[register] = Slot::Empty,
                _ => values.push(self.take(register)?),
            }
        }
        values.reverse();
        self.open = None;
        Ok(values)
    }

    fn call(&mut self, pc: usize, next: usize) -> Reading<usize> {
        let instruction = self.at(pc)?;
        let a = usize::from(instruction.a());
        let arguments = self.values(a + 1, usize::from(instruction.b()))?;

        let call = match self.slots.get_mut(a).map(std::mem::take) {
            Some(Slot::Value(target)) => Expr::Call(Box::new(target), arguments),
            Some(Slot::Method(object, name)) => {
                let slot = self
                    .slots
                    .get_mut(a + 1)
                    .ok_or("a method lands outside the stack")?;
                *slot = Slot::Empty;
                Expr::Method(Box::new(object), name, arguments)
            }
            _ => return Err("a call has nothing to call"),
        };

        if instruction.opcode() == Opcode::TailCall {
            self.flush()?;
            self.out.push(Stat::Return(vec![call]));
            // The return the compiler writes after a tail call never runs.
            return match self.at(next).map(Instruction::opcode) {
                Ok(Opcode::Return) => Ok(next + 1),
                _ => Err("a tail call is not followed by the return the compiler writes"),
            };
        }

        match instruction.c() {
            // The call stands alone, so it is a statement rather than a value.
            1 => {
                self.flush()?;
                self.out.push(Stat::Call(call));
            }
            0 => {
                self.set(a, call)?;
                self.open = Some(a + 1);
            }
            held => {
                self.set(a, call)?;
                self.rest(a, usize::from(held) - 1)?;
            }
        }
        Ok(next)
    }

    /// A nested function, with its upvalues named by what the enclosing one bound them to.
    fn closure(&mut self, pc: usize, at: usize) -> Reading<Closure> {
        let function = self.function;
        let held = function
            .functions()
            .get(at)
            .ok_or("a closure names no function")?;
        let mut upvalues = Vec::with_capacity(usize::from(held.upvalues()));
        for step in 0..usize::from(held.upvalues()) {
            let binding = self.at(pc + 1 + step)?;
            let name = match binding.opcode() {
                Opcode::Move => self
                    .names
                    .get(usize::from(binding.b()))
                    .filter(|name| !name.is_empty())
                    .cloned()
                    .ok_or("a closure binds an upvalue to a register that holds nothing")?,
                Opcode::GetUpval => match self.upvalue(usize::from(binding.b()))? {
                    Expr::Name(name) => name,
                    _ => return Err("an upvalue has no name"),
                },
                _ => return Err("a closure binds an upvalue to something that is not one"),
            };
            upvalues.push(name);
        }
        Ok(closure(held, upvalues, self.depth + 1, &mut self.counts))
    }
}

/// How many conditionals of a chain belong to one condition, and where failing it lands. The longest
/// run whose jumps all land somewhere the condition explains is the one that reads as source.
fn choose(tests: &[Test]) -> Option<(usize, usize)> {
    for count in (1..=tests.len()).rev() {
        let true_target = tests[count - 1].after;
        let false_target = tests[..count].iter().map(|test| test.target).max()?;
        if false_target < true_target {
            continue;
        }
        // A jump either fails the whole condition, or skips to where a later operand is evaluated
        // because the part before it already settled.
        let explained = |(at, test): (usize, &Test)| {
            test.target == false_target
                || tests[at + 1..count]
                    .iter()
                    .any(|later| later.after == test.target)
        };
        if tests[..count].iter().enumerate().all(explained) {
            return Some((count, false_target));
        }
    }
    None
}

/// Fold a run of conditionals into the expression they test.
fn combine(
    tests: &[Test],
    held: &mut [Expr],
    at: usize,
    count: usize,
    true_target: usize,
    false_target: usize,
) -> Reading<Expr> {
    let test = tests
        .get(at)
        .ok_or("a chain is shorter than it was measured")?;
    let condition = held
        .get(at)
        .cloned()
        .ok_or("a chain is shorter than it was measured")?;
    if at + 1 == count {
        return match test.target == false_target {
            true => Ok(condition),
            false => Err("a condition ends somewhere it cannot"),
        };
    }
    let join =
        |operator, left: Expr, right| Expr::Binary(operator, Box::new(left), Box::new(right));

    if test.target == false_target {
        let rest = combine(tests, held, at + 1, count, true_target, false_target)?;
        return Ok(join(Binary::And, condition, rest));
    }
    if test.target == true_target {
        let rest = combine(tests, held, at + 1, count, true_target, false_target)?;
        return Ok(join(Binary::Or, condition.negate(), rest));
    }
    // The jump skips to a later operand, so everything before it settled the part of the condition
    // that operand is joined to.
    let split = tests[at + 1..count]
        .iter()
        .position(|later| later.after == test.target)
        .map(|found| at + 1 + found)
        .ok_or("a condition jumps somewhere it cannot")?;
    let group = combine(tests, held, at, split + 1, tests[split].after, false_target)?;
    let rest = combine(tests, held, split + 1, count, true_target, false_target)?;
    Ok(join(Binary::And, group, rest))
}
