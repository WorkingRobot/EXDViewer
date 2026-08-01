use luadec::chunk::{Chunk, Function, Opcode};
fn walk<'a>(h: &'a Function, out: &mut Vec<&'a Function>) {
    out.push(h);
    for n in h.functions() {
        walk(n, out)
    }
}
fn main() {
    let mut a = std::env::args().skip(1);
    let file = a.next().unwrap();
    let want: usize = a.next().unwrap_or("0".into()).parse().unwrap();
    let chunk = Chunk::parse(&std::fs::read(&file).unwrap()).unwrap();
    let mut all = Vec::new();
    walk(chunk.main(), &mut all);
    let h = all[want];
    println!(
        "proto {want}: {} instructions, {} params",
        h.code().len(),
        h.parameters()
    );
    for (pc, i) in h.code().iter().enumerate() {
        let t = match i.opcode() {
            Opcode::Jmp | Opcode::ForPrep | Opcode::ForLoop => {
                format!(" -> {}", pc as i64 + 1 + i64::from(i.sbx()))
            }
            _ => String::new(),
        };
        println!(
            "{pc:>4} {:<9} a={} b={} c={}{}",
            i.opcode().name(),
            i.a(),
            i.b(),
            i.c(),
            t
        );
    }
}
