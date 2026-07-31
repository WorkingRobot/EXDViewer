// Prints the merged source for one pass, so the corpus gate can compile it.
use ironworks::file::shpk::ShaderPackage;

/// The gate names a pass the way the tables do; the crate wants its id.
fn package_pass(name: &str) -> u32 {
    shaders::names::hash(name.as_bytes())
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap();
    let stage = shadermerge::stage_of(&args.next().unwrap());
    let want = package_pass(&args.next().unwrap());
    let raw = std::fs::read(&path).unwrap();
    let package = ShaderPackage::parse(&raw).unwrap();
    match shadermerge::pass(&package, &raw, stage, want) {
        Ok(merged) => {
            let lines = match args.next().as_deref() {
                Some("asm") => merged.asm,
                _ => merged.lines,
            };
            println!("{}", lines.join("\n"));
        }
        Err(why) => {
            eprintln!("{why}");
            std::process::exit(1);
        }
    }
}
