//! One chunk as source.
//!
//! `cargo run --release --example emit -- <file.luab> [asm]`

fn main() {
    let mut arguments = std::env::args().skip(1);
    let Some(file) = arguments.next() else {
        eprintln!("usage: emit <file.luab> [asm]");
        return;
    };
    let bytes = match std::fs::read(&file) {
        Ok(bytes) => bytes,
        Err(error) => return eprintln!("{file}: {error}"),
    };
    let chunk = match luadec::Chunk::parse(&bytes) {
        Ok(chunk) => chunk,
        Err(error) => return eprintln!("{file}: {error}"),
    };
    match arguments.next().is_some() {
        true => println!("{}", luadec::disassemble(&chunk).join("\n")),
        false => {
            let held = luadec::decompile(&chunk);
            eprintln!(
                "-- {} of {} functions read as source",
                held.functions,
                held.functions + held.disassembled
            );
            println!("{}", held.lines.join("\n"));
        }
    }
}
