//! What Unicode calls a codepoint.
//!
//! Two families are derived rather than stored: Unicode names CJK ideographs after their own
//! codepoint, and Hangul syllables after the jamo they decompose into. Between them that is every
//! glyph in a Chinese or Korean font and most of one in a Japanese font, so listing them would cost
//! tens of thousands of entries to say what a line of arithmetic already knows. The rest comes from
//! a table [`build.rs`](../build.rs) compiles over the blocks game fonts draw from: the whole of
//! Unicode's is an order of magnitude larger than a font viewer has any use for.

include!(concat!(env!("OUT_DIR"), "/glyph_names.rs"));

/// Jamo, in the order the syllable block composes them.
const LEAD: [&str; 19] = [
    "G", "GG", "N", "D", "DD", "R", "M", "B", "BB", "S", "SS", "", "J", "JJ", "C", "K", "T", "P",
    "H",
];
const VOWEL: [&str; 21] = [
    "A", "AE", "YA", "YAE", "EO", "E", "YEO", "YE", "O", "WA", "WAE", "OE", "YO", "U", "WEO", "WE",
    "WI", "YU", "EU", "YI", "I",
];
const TAIL: [&str; 28] = [
    "", "G", "GG", "GS", "N", "NJ", "NH", "D", "L", "LG", "LM", "LB", "LS", "LT", "LP", "LH", "M",
    "B", "BS", "S", "SS", "NG", "J", "C", "K", "T", "P", "H",
];

pub fn of(character: char) -> Option<String> {
    if let Some(derived) = derived(character) {
        return Some(derived);
    }
    let codepoint = u32::from(character);
    let (first, spans) = BLOCKS
        .iter()
        .find(|(first, spans)| (*first..first + spans.len() as u32 - 1).contains(&codepoint))?;
    let index = (codepoint - first) as usize;
    let (start, end) = (spans[index] as usize, spans[index + 1] as usize);
    (start < end).then(|| {
        TOKENS[start..end]
            .iter()
            .map(|token| word(*token))
            .collect::<Vec<_>>()
            .join(" ")
    })
}

fn word(index: u16) -> &'static str {
    let index = usize::from(index);
    &WORDS[usize::from(WORD_ENDS[index])..usize::from(WORD_ENDS[index + 1])]
}

/// The names Unicode states as a rule rather than as a list; ideograph runs come from the
/// generated table.
fn derived(character: char) -> Option<String> {
    let codepoint = u32::from(character);
    let within = |runs: &[(u32, u32)]| {
        runs.iter()
            .any(|(first, last)| (*first..=*last).contains(&codepoint))
    };

    if within(&UNIFIED) {
        return Some(format!("CJK UNIFIED IDEOGRAPH-{codepoint:04X}"));
    }
    if within(&COMPATIBILITY) {
        return Some(format!("CJK COMPATIBILITY IDEOGRAPH-{codepoint:04X}"));
    }

    match character {
        '\u{AC00}'..='\u{D7A3}' => {
            let syllable = (codepoint - 0xAC00) as usize;
            Some(format!(
                "HANGUL SYLLABLE {}{}{}",
                LEAD[syllable / (VOWEL.len() * TAIL.len())],
                VOWEL[syllable / TAIL.len() % VOWEL.len()],
                TAIL[syllable % TAIL.len()],
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod test {
    use super::of;

    #[test]
    fn names_come_from_the_table() {
        assert_eq!(of('A').as_deref(), Some("LATIN CAPITAL LETTER A"));
        assert_eq!(of('あ').as_deref(), Some("HIRAGANA LETTER A"));
        assert_eq!(of('♥').as_deref(), Some("BLACK HEART SUIT"));
        assert_eq!(
            of('Ａ').as_deref(),
            Some("FULLWIDTH LATIN CAPITAL LETTER A")
        );
    }

    #[test]
    fn ideographs_and_syllables_are_derived() {
        assert_eq!(of('一').as_deref(), Some("CJK UNIFIED IDEOGRAPH-4E00"));
        assert_eq!(
            of('\u{F92C}').as_deref(),
            Some("CJK COMPATIBILITY IDEOGRAPH-F92C")
        );
        // The compatibility supplement sits inside the extension planes: one span across them
        // would call these unified.
        assert_eq!(
            of('\u{2F800}').as_deref(),
            Some("CJK COMPATIBILITY IDEOGRAPH-2F800")
        );
        assert_eq!(of('가').as_deref(), Some("HANGUL SYLLABLE GA"));
        assert_eq!(of('힣').as_deref(), Some("HANGUL SYLLABLE HIH"));
    }

    /// Covering less than the whole of Unicode is the point; naming something Unicode does not, or
    /// naming it differently, is not. A rule stated as a codepoint range is the easy way to get
    /// this wrong, since every revision moves where the ideographs end.
    #[test]
    fn no_name_is_invented() {
        for codepoint in 0..=u32::from(char::MAX) {
            let Some(character) = char::from_u32(codepoint) else {
                continue;
            };
            match (of(character), unicode_names2::name(character)) {
                (Some(ours), Some(theirs)) => {
                    assert_eq!(ours, theirs.to_string(), "U+{codepoint:04X}");
                }
                (Some(ours), None) => {
                    panic!("U+{codepoint:04X} has no name, but we call it {ours}")
                }
                (None, _) => (),
            }
        }
    }

    #[test]
    fn the_unnamed_stay_unnamed() {
        // Private use and control codes, which Unicode names no more than it draws.
        assert_eq!(of('\u{E020}'), None);
        assert_eq!(of('\u{7F}'), None);
        // Past every block the table carries.
        assert_eq!(of('😀'), None);
    }
}
