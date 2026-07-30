//! Names for the shader resources a material refers to by hash.

use std::collections::HashMap;
use std::sync::LazyLock;

/// Names whose id is the crc32 of the name itself.
static DERIVED: &str = include_str!("names.txt");
/// `hash name` for ids that cannot be derived.
static EXPLICIT: &str = include_str!("hashes.txt");

static SHADER_NAME_CRC: crc::Algorithm<u32> = crc::Algorithm {
    width: 32,
    poly: 0x04C1_1DB7,
    init: 0x0000_0000,
    refin: true,
    refout: true,
    xorout: 0x0000_0000,
    check: 0x2DFD_2D88,
    residue: 0x0000_0000,
};

/// The id the game would give a name, for matching a string back to a table keyed by hash.
pub fn hash(bytes: &[u8]) -> u32 {
    crc::Crc::<u32>::new(&SHADER_NAME_CRC).checksum(bytes)
}

fn entries(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
}

static NAMES: LazyLock<HashMap<u32, &'static str>> = LazyLock::new(|| {
    let derived = entries(DERIVED).map(|name| (hash(name.as_bytes()), name));
    let explicit = entries(EXPLICIT).filter_map(|line| {
        let (hash, name) = line.split_once(' ')?;
        Some((u32::from_str_radix(hash, 16).ok()?, name))
    });
    derived.chain(explicit).collect()
});

/// The name behind a shader resource id, if it is one of the known ones.
pub fn resolve(id: u32) -> Option<&'static str> {
    NAMES.get(&id).copied()
}

#[cfg(test)]
mod tests {
    use super::{DERIVED, EXPLICIT, NAMES, entries, hash, resolve};

    /// Every line has to be usable; a malformed one would drop a name rather than fail.
    #[test]
    fn the_whole_table_loads() {
        let lines = entries(DERIVED).count() + entries(EXPLICIT).count();
        assert_eq!(NAMES.len(), lines);
    }

    /// The hash file is only for ids that genuinely cannot be derived. Anything in it that *does*
    /// hash to its own name belongs in the name list instead.
    #[test]
    fn explicit_hashes_are_not_derivable() {
        for line in entries(EXPLICIT) {
            let (listed, name) = line.split_once(' ').expect(line);
            let listed = u32::from_str_radix(listed, 16).expect(line);
            assert_ne!(hash(name.as_bytes()), listed, "{name} could be derived");
        }
    }

    /// Ids observed in real materials, so the table is checked against the game rather than only
    /// against itself.
    #[test]
    fn resolves_ids_seen_in_the_wild() {
        for (id, name) in [
            (0x29ac_0223, "g_AlphaThreshold"),
            (0xb554_5fbb, "g_NormalScale"),
            (0x600e_f9df, "GetValuesCompatibility"),
            (0x0c5e_c1f1, "g_SamplerNormal"),
        ] {
            assert_eq!(resolve(id), Some(name), "{id:#010x}");
        }
        assert_eq!(resolve(0xdead_beef), None);
    }
}
