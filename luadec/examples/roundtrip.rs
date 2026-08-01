//! Check a reading against the compiler that wrote the bytecode.
//!
//! Every function the reading resolved whole is written back out as source, compiled, and compared
//! against the function it came from. Where the two agree, the reading of that function says exactly
//! what the compiler was given.
//!
//! `cargo run --release --example roundtrip -- <luac> <dir> <workdir>`
//!
//! `luac` has to be a Lua 5.1 one that reads the 32-bit chunks the game ships. On a 64-bit host that
//! means `sizeof(size_t)` patched to four in `lundump.c` and `ldump.c`.

use std::path::{Path, PathBuf};
use std::process::Command;

use luadec::chunk::{Chunk, Constant, Function};

fn chunks(root: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![PathBuf::from(root)];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match path.is_dir() {
                true => stack.push(path),
                false if path.extension().is_some_and(|held| held == "luab") => found.push(path),
                false => (),
            }
        }
    }
    found.sort();
    found
}

fn protos(held: &Function) -> usize {
    1 + held.functions().iter().map(protos).sum::<usize>()
}

/// Whether two functions are the same but for the lines they were written on, which a reading
/// cannot put back because it does not know where anybody pressed return.
fn same(left: &Function, right: &Function) -> bool {
    left.parameters() == right.parameters()
        && left.is_vararg() == right.is_vararg()
        && left.has_arg() == right.has_arg()
        && left.needs_arg() == right.needs_arg()
        && left.max_stack() == right.max_stack()
        && left.code() == right.code()
        && left.constants().len() == right.constants().len()
        && left
            .constants()
            .iter()
            .zip(right.constants())
            .all(|(left, right)| match (left, right) {
                (Constant::Number(left), Constant::Number(right)) => {
                    left.to_bits() == right.to_bits()
                }
                (left, right) => left == right,
            })
        && left.functions().len() == right.functions().len()
        && left
            .functions()
            .iter()
            .zip(right.functions())
            .all(|(left, right)| same(left, right))
}

/// Compile one source file, and answer with the chunk it made.
fn compile(luac: &str, work: &Path, source: &str) -> Option<Chunk> {
    let lua = work.join("one.lua");
    let out = work.join("one.luab");
    std::fs::write(&lua, source).ok()?;
    let held = Command::new(luac)
        .arg("-s")
        .arg("-o")
        .arg(&out)
        .arg(&lua)
        .output()
        .ok()?;
    if !held.status.success() {
        return None;
    }
    Chunk::parse(&std::fs::read(&out).ok()?).ok()
}

/// What one file's functions came to.
#[derive(Default)]
struct Tally {
    /// Functions the reading resolved whole and offered for checking.
    offered: usize,
    /// Of those, the ones the compiler would not even parse.
    rejected: usize,
    /// Of those, the ones that came back as the function they were read from.
    matched: usize,
    /// Of those, the ones that compiled but came back as something else.
    differed: usize,
    /// Of those, the ones holding the same instructions in the same order, which is a reading that
    /// said the same thing with the registers or the constant pool laid out another way.
    shaped: usize,
}

impl Tally {
    fn add(&mut self, other: &Self) {
        self.offered += other.offered;
        self.rejected += other.rejected;
        self.matched += other.matched;
        self.differed += other.differed;
        self.shaped += other.shaped;
    }
}

/// Offer a function for checking, and where it cannot be offered, its children instead.
///
/// A function closing over an upvalue cannot stand on its own, and one the reading did not resolve
/// whole has nothing to check, so in both cases what is inside it is tried instead.
fn check(luac: &str, work: &Path, held: &Function, main: bool, tally: &mut Tally) {
    let source = luadec::source(held).filter(|_| main || held.upvalues() == 0);
    if let Some(source) = source {
        let text = match main {
            true => source.lines.join("\n"),
            false => {
                let mut parameters = source.parameters.join(", ");
                if source.vararg {
                    if !parameters.is_empty() {
                        parameters.push_str(", ");
                    }
                    parameters.push_str("...");
                }
                format!(
                    "return function({parameters})\n{}\nend",
                    source.lines.join("\n")
                )
            }
        };
        tally.offered += 1;
        let built = compile(luac, work, &text);
        let Some(built) = built else {
            tally.rejected += 1;
            return;
        };
        // A function was wrapped in a chunk that returns it, so it is the one that chunk holds.
        let read = match main {
            true => Some(built.main()),
            false => built.main().functions().first(),
        };
        if read.is_some_and(|read| same(read, held)) {
            tally.matched += protos(held);
            tally.offered += protos(held) - 1;
            return;
        }
        // Where a function came back as something else, what is inside it is tried on its own, so
        // one disagreement is charged to the function it is in rather than to everything under it.
        tally.differed += 1;
        if read.is_some_and(|read| {
            read.code().len() == held.code().len()
                && read
                    .code()
                    .iter()
                    .zip(held.code())
                    .all(|(read, held)| read.opcode() == held.opcode())
        }) {
            tally.shaped += 1;
        }
    }
    for nested in held.functions() {
        check(luac, work, nested, false, tally);
    }
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let (Some(luac), Some(root), Some(work)) =
        (arguments.next(), arguments.next(), arguments.next())
    else {
        eprintln!("usage: roundtrip <luac> <dir> <workdir>");
        return;
    };
    let work = PathBuf::from(work);
    if std::fs::create_dir_all(&work).is_err() {
        eprintln!("cannot make {}", work.display());
        return;
    }

    let files = chunks(&root);
    let mut total = Tally::default();
    let mut whole = 0usize;
    for (at, file) in files.iter().enumerate() {
        let Ok(bytes) = std::fs::read(file) else {
            continue;
        };
        let Ok(chunk) = Chunk::parse(&bytes) else {
            continue;
        };
        let mut tally = Tally::default();
        match luadec::units(&chunk) {
            Some(units) => {
                for unit in units {
                    check(&luac, &work, unit, true, &mut tally);
                }
            }
            None => check(&luac, &work, chunk.main(), true, &mut tally),
        }
        if tally.offered > 0 && tally.matched == tally.offered {
            whole += 1;
        }
        total.add(&tally);
        if at % 500 == 0 {
            eprintln!("{at}/{}", files.len());
        }
    }

    println!("\nfiles {}", files.len());
    println!("  every function offered came back the same: {whole}");
    println!("functions offered  {}", total.offered);
    println!("  matched          {}", total.matched);
    println!(
        "  differed         {} ({} the same instructions in the same order)",
        total.differed, total.shaped
    );
    println!("  did not compile  {}", total.rejected);
}
