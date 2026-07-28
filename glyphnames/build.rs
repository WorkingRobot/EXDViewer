use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;

/// Where a game font's glyphs live, outside the families whose names are derived rather than
/// listed.
const BLOCKS: [(u32, u32); 5] = [
    (0x0000, 0x04FF), // Latin, Greek, Cyrillic and their combining marks
    (0x1100, 0x11FF), // Hangul jamo
    (0x2000, 0x2BFF), // punctuation, currency, arrows, enclosed alphanumerics, dingbats
    (0x3000, 0x33FF), // CJK punctuation, kana, compatibility jamo, enclosed CJK
    (0xFF00, 0xFFEF), // fullwidth and halfwidth forms
];

/// Compile the names into a word dictionary, a stream of word indices, and one dense span table per
/// block. Names repeat their words heavily -- five thousand of them are built from two thousand
/// words -- so storing each word once and pointing at it costs a third of the text.
fn main() {
    let (unified, compatibility) = derived_ranges();

    let mut words: Vec<String> = Vec::new();
    let mut indices: HashMap<String, u16> = HashMap::new();
    let mut tokens: Vec<u16> = Vec::new();
    let mut blocks: Vec<Vec<u16>> = Vec::new();

    for (first, last) in BLOCKS {
        // One entry per codepoint plus a sentinel, so a name is the stretch of stream between its
        // own start and the next: a codepoint with no name is an empty span, needing no marker.
        let mut spans = Vec::with_capacity((last - first + 2) as usize);
        for codepoint in first..=last {
            spans.push(u16::try_from(tokens.len()).expect("token stream fits in u16"));
            let name = char::from_u32(codepoint)
                .and_then(unicode_names2::name)
                .map(|name| name.to_string());
            for word in name.iter().flat_map(|name| name.split(' ')) {
                let index = *indices.entry(word.to_owned()).or_insert_with(|| {
                    words.push(word.to_owned());
                    u16::try_from(words.len() - 1).expect("dictionary fits in u16")
                });
                tokens.push(index);
            }
        }
        spans.push(u16::try_from(tokens.len()).expect("token stream fits in u16"));
        blocks.push(spans);
    }

    let mut ends = vec![0u16];
    for word in &words {
        let end = ends.last().unwrap() + u16::try_from(word.len()).unwrap();
        ends.push(end);
    }

    let mut out = String::new();
    writeln!(
        out,
        "static UNIFIED: [(u32, u32); {}] = {unified:?};",
        unified.len()
    )
    .unwrap();
    writeln!(
        out,
        "static COMPATIBILITY: [(u32, u32); {}] = {compatibility:?};",
        compatibility.len()
    )
    .unwrap();
    writeln!(out, "static WORDS: &str = {:?};", words.concat()).unwrap();
    write_array(&mut out, "WORD_ENDS", &ends);
    write_array(&mut out, "TOKENS", &tokens);
    for (index, spans) in blocks.iter().enumerate() {
        write_array(&mut out, &format!("BLOCK_{index}"), spans);
    }
    writeln!(out, "static BLOCKS: [(u32, &[u16]); {}] = [", BLOCKS.len()).unwrap();
    for (index, (first, _)) in BLOCKS.iter().enumerate() {
        writeln!(out, "    ({first:#06x}, &BLOCK_{index}),").unwrap();
    }
    writeln!(out, "];").unwrap();

    let path = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("glyph_names.rs");
    std::fs::write(path, out).unwrap();
}

fn write_array(out: &mut String, name: &str, values: &[u16]) {
    writeln!(out, "static {name}: [u16; {}] = {values:?};", values.len()).unwrap();
}

/// Inclusive first and last codepoint of a run.
type Runs = Vec<(u32, u32)>;

/// The codepoints Unicode names by rule rather than by listing, as runs. Which ones those are moves
/// with every revision that adds an ideograph, so they are read back off the name table rather than
/// written down: a codepoint belongs to a family when the name it has is the name the rule makes.
fn derived_ranges() -> (Runs, Runs) {
    let (mut unified, mut compatibility) = (Vec::new(), Vec::new());
    for codepoint in 0..=u32::from(char::MAX) {
        let Some(name) = char::from_u32(codepoint).and_then(unicode_names2::name) else {
            continue;
        };
        let name = name.to_string();
        let runs = if name.starts_with("CJK UNIFIED IDEOGRAPH-") {
            &mut unified
        } else if name.starts_with("CJK COMPATIBILITY IDEOGRAPH-") {
            &mut compatibility
        } else {
            continue;
        };
        match runs.last_mut() {
            Some((_, last)) if *last + 1 == codepoint => *last = codepoint,
            _ => runs.push((codepoint, codepoint)),
        }
    }
    (unified, compatibility)
}
