//! How much of a tree of chunks reads back as source.
//!
//! `cargo run --release --example coverage -- <dir> [reasons]`

use std::collections::BTreeMap;
use std::path::PathBuf;

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

fn main() {
    let mut arguments = std::env::args().skip(1);
    let Some(root) = arguments.next() else {
        eprintln!("usage: coverage <dir> [reasons]");
        return;
    };
    let reasons = arguments.next().is_some();

    let (mut whole, mut partial, mut unread, mut broken) = (0usize, 0, 0, 0);
    let (mut read, mut raw) = (0usize, 0);
    let mut why = BTreeMap::<String, usize>::new();
    let files = chunks(&root);

    for file in &files {
        let Ok(bytes) = std::fs::read(file) else {
            continue;
        };
        let chunk = match luadec::Chunk::parse(&bytes) {
            Ok(chunk) => chunk,
            Err(error) => {
                broken += 1;
                println!("unreadable {}: {error}", file.display());
                continue;
            }
        };
        let held = luadec::decompile(&chunk);
        read += held.functions;
        raw += held.disassembled;
        match (held.functions, held.disassembled) {
            (_, 0) => whole += 1,
            (0, _) => unread += 1,
            _ => partial += 1,
        }
        if reasons {
            for line in &held.lines {
                if let Some(reason) = line.split("-- -- not read as source: ").nth(1) {
                    *why.entry(reason.to_owned()).or_default() += 1;
                }
            }
        }
    }

    println!("\nfiles {}", files.len());
    println!("  whole    {whole}");
    println!("  partial  {partial}");
    println!("  none     {unread}");
    println!("  broken   {broken}");
    println!("functions {} of {} read as source", read, read + raw);
    if reasons {
        let mut ordered: Vec<_> = why.into_iter().collect();
        ordered.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        println!("\nwhy a function stayed disassembly:");
        for (reason, count) in ordered {
            println!("  {count:>7}  {reason}");
        }
    }
}
