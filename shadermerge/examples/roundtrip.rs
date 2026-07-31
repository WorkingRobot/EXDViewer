// Does the emitter mean what the shader meant?
//
// Every other gate checks shape: that guards cover the right variants, that names resolve, that dxc
// accepts it. None of them can tell a shader from a different shader that also compiles, and that is
// twice now what went wrong — an `imul` reading one lane where it wanted two, a `discard` that lost
// the branch it sat under. Both were valid HLSL throughout.
//
// So: canonicalise a shader, rebuild a program from that graph alone, canonicalise *that*, and
// require the two to agree on what the shader leaves behind — every output, and every side effect
// with the condition it fires under. Value hashes are content-addressed, so agreement is exact.
//
// The reference is the shader's own graph, never the merged union: restricting the union to one
// variant and comparing it against the graph it was built from would pass whatever the emitter did.

use dxbc::chunks::ChunkData;
use ironworks::file::shpk::ShaderPackage;
use shadermerge::check;

fn main() {
    let dir = std::env::args().nth(1).unwrap();
    let mut packs: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            (path.extension()? == "shpk").then_some(path)
        })
        .collect();
    packs.sort();

    let (mut seen, mut agreed, mut skipped) = (0, 0, 0);
    let mut wrong: Vec<String> = Vec::new();
    for path in &packs {
        let Ok(raw) = std::fs::read(path) else {
            continue;
        };
        let Ok(package) = ShaderPackage::parse(&raw) else {
            continue;
        };
        let name = path.file_stem().unwrap().to_string_lossy();
        for (at, shader) in package.shaders().iter().enumerate() {
            let start = package.blobs_offset() + shader.blob_offset() as usize;
            let Some(bytes) = raw.get(start..start + shader.blob_size() as usize) else {
                continue;
            };
            let Some(program) = dxbc::scan_dxbc(bytes)
                .iter()
                .flat_map(|held| &held.chunks)
                .find_map(|chunk| match chunk.parse() {
                    ChunkData::Shader(program) => Some(program),
                    _ => None,
                })
            else {
                continue;
            };
            seen += 1;
            match check::roundtrip(&raw, &package, at, &program) {
                Ok(true) => agreed += 1,
                // A loop is walked once with no back edge, so neither side is a program and the
                // comparison would be of two things that are equally not one.
                Err(shadermerge::Error::Loops) => skipped += 1,
                Ok(false) => wrong.push(format!("{name} #{at}")),
                Err(why) => wrong.push(format!("{name} #{at}: {why}")),
            }
        }
    }
    println!(
        "shaders {seen}, agreed {agreed}, skipped for loops {skipped}, disagreed {}",
        wrong.len()
    );
    for held in wrong.iter().take(20) {
        println!("  {held}");
    }
    if !wrong.is_empty() {
        std::process::exit(1);
    }
}
