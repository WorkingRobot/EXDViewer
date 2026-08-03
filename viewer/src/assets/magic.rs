//! What a file is, read from its bytes rather than from its name.
//!
//! Most of what the game ships leads with a magic. The three formats here that do not are `.tex`
//! and `.tera`, recognized by the shape of their fixed headers, and `.mtrl`, which is small enough
//! to read outright and names a shader package that gives it away. Anything left over that decodes
//! cleanly is text.

use std::io::Cursor;

use binrw::BinRead;
use ironworks::file::{File, mtrl, tex};

use super::viewers::Viewer;

/// How much of a file is looked at before calling it text.
const SAMPLE: usize = 1024;

/// A `.tex` header, which the first surface follows immediately.
const TEX_HEADER: u32 = 80;

/// A `.tera` header, which the plate list follows immediately.
const TERA_HEADER: usize = 52;

/// The only `.tera` version the game ships. Earlier ones lay the header out differently.
const TERA_VERSION: u32 = 0x0100_0003;

/// A format as its bytes identify it.
#[derive(Clone, Copy)]
pub enum Format {
    /// One a viewer here can render.
    Shown(Viewer),
    /// One nothing here renders, named so a file can still be said to be something.
    Named(&'static str),
}

impl Format {
    pub fn viewer(self) -> Viewer {
        match self {
            Self::Shown(viewer) => viewer,
            Self::Named(_) => Viewer::Raw,
        }
    }

    /// What to call it. Taken from the viewer where there is one, so the two can never disagree.
    pub fn label(self) -> &'static str {
        match self {
            Self::Shown(viewer) => viewer.label(),
            Self::Named(name) => name,
        }
    }
}

const MAGIC: &[(&[u8], Format)] = &[
    (b"\x89PNG\r\n\x1a\n", Format::Shown(Viewer::Image)),
    (b"uldh", Format::Shown(Viewer::Uld)),
    (b"fcsv", Format::Shown(Viewer::Font)),
    (b"gftd0100", Format::Shown(Viewer::Icons)),
    (b"\x1bLua", Format::Shown(Viewer::Luab)),
    (b"ShPk", Format::Shown(Viewer::Shpk)),
    (b"ShCd", Format::Shown(Viewer::Shcd)),
    (b"die\0", Format::Shown(Viewer::Eid)),
    (b"plks", Format::Shown(Viewer::Skp)),
    (b"blks", Format::Named("Skeleton")),
    (b"SEDBSSCF", Format::Named("Sound")),
    (b"EXHF", Format::Named("Sheet header")),
    (b"EXDF", Format::Named("Sheet page")),
    (b"EXLT", Format::Named("Sheet list")),
    (b"LGB1", Format::Shown(Viewer::Lgb)),
    (b"SGB1", Format::Shown(Viewer::Sgb)),
    (b"LVB1", Format::Shown(Viewer::Lvb)),
    (b"SVB1", Format::Shown(Viewer::Svb)),
    (b"LCB1", Format::Shown(Viewer::Lcb)),
    (b"UWB1", Format::Shown(Viewer::Uwb)),
    (b"ENVB", Format::Shown(Viewer::Envb)),
    (b"OBSB", Format::Shown(Viewer::Obsb)),
    (b"ESSB", Format::Shown(Viewer::Essb)),
    (b"AMB\0", Format::Shown(Viewer::Amb)),
    (b" dgg", Format::Shown(Viewer::Ggd)),
    (b"dzg\0", Format::Shown(Viewer::Gzd)),
    (b"pap ", Format::Named("Animation")),
    (b"TMLB", Format::Named("Timeline")),
    (b"CUTB", Format::Named("Cutscene")),
    (b"XFVA", Format::Shown(Viewer::Avfx)),
];

/// What the bytes say the file is, or `None` where they say nothing. Ordered strongest test first:
/// a magic settles it outright, and the three guesses below only ever see what no magic claimed.
pub fn sniff(bytes: &[u8]) -> Option<Format> {
    if let Some((_, format)) = MAGIC.iter().find(|(magic, _)| bytes.starts_with(magic)) {
        return Some(*format);
    }
    if is_material(bytes) {
        return Some(Format::Shown(Viewer::Material));
    }
    if is_texture(bytes) {
        return Some(Format::Shown(Viewer::Texture));
    }
    if is_terrain(bytes) {
        return Some(Format::Shown(Viewer::Tera));
    }
    is_text(bytes).then_some(Format::Shown(Viewer::Text))
}

/// Read as a material and believed only if it names a shader package, which nothing else would.
fn is_material(bytes: &[u8]) -> bool {
    bytes.len() <= usize::from(u16::MAX)
        && mtrl::Material::read(Cursor::new(bytes.to_vec()))
            .is_ok_and(|material| material.shader().ends_with(".shpk"))
}

/// A texture's header is a fixed 80 bytes holding a pixel format the file cannot be read without,
/// dimensions, a mip count, and the offset of the first surface -- which is the end of the header.
fn is_texture(bytes: &[u8]) -> bool {
    let Some(header) = bytes.get(..TEX_HEADER as usize) else {
        return false;
    };
    let short = |at: usize| u16::from_le_bytes(header[at..at + 2].try_into().unwrap());
    let word = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().unwrap());

    tex::Format::read_le(&mut Cursor::new(&header[4..8])).is_ok()
        && short(8) > 0
        && short(10) > 0
        && (1..=13).contains(&(header[14] & 127))
        && word(28) == TEX_HEADER
}

/// Terrain leads with a version where the formats above lead with a magic, so it is taken on the
/// shape of its header instead: that one version, a texture slot mask with only three bits defined,
/// and 28 bytes of padding that is zero in every file the game ships.
fn is_terrain(bytes: &[u8]) -> bool {
    let Some(header) = bytes.get(..TERA_HEADER) else {
        return false;
    };
    let word = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().unwrap());

    word(0) == TERA_VERSION && word(20) <= 0b111 && header[24..].iter().all(|byte| *byte == 0)
}

/// The text the game ships is ASCII, so a control byte that is not whitespace rules it out. Only the
/// start is read, which is enough to reject anything binary: none of the formats above get far
/// without a zero.
fn is_text(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(SAMPLE)];
    let text = match std::str::from_utf8(sample) {
        Ok(text) => text,
        // Cutting the sample can split a character in two; any other invalid byte is not text.
        Err(e) if e.error_len().is_none() => {
            std::str::from_utf8(&sample[..e.valid_up_to()]).expect("prefix decoded")
        }
        Err(_) => return false,
    };
    !text.is_empty()
        && !text
            .chars()
            .any(|c| c.is_control() && !"\t\r\n".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(bytes: &[u8]) -> Option<&'static str> {
        sniff(bytes).map(Format::label)
    }

    #[test]
    fn reads_a_magic() {
        assert_eq!(label(b"uldh0100rest of it"), Some("Layout"));
        assert_eq!(label(b"fcsv0100\0\0\0\0"), Some("Font"));
        assert_eq!(label(b"gftd0100\0\0\0\0"), Some("Icons"));
        assert_eq!(label(b"ShPk\0\0\0\0"), Some("Shader package"));
        assert_eq!(label(b"\x1bLua\x51\0\x01\x04"), Some("Lua"));
        assert_eq!(label(b"ShCd\0\0\0\0"), Some("Shader code"));
        assert_eq!(label(b"\x89PNG\r\n\x1a\n\0\0\0\0"), Some("Image"));
        assert_eq!(label(b"SEDBSSCF\0\0\0\0"), Some("Sound"));
        assert_eq!(label(b"blks\0\0\0\0"), Some("Skeleton"));
        assert_eq!(label(b"die\0" as &[u8]), Some("Bind points"));
        assert_eq!(label(b"plks0031"), Some("Skeleton parameters"));
        assert_eq!(label(b"LGB1\0\0\0\0"), Some("Layer group"));
        assert_eq!(label(b"SGB1\0\0\0\0"), Some("Shared group"));
        assert_eq!(label(b"LVB1\0\0\0\0"), Some("Level"));
        assert_eq!(label(b"SVB1\0\0\0\0"), Some("Sky visibility"));
        assert_eq!(label(b"LCB1\0\0\0\0"), Some("Light culling"));
        assert_eq!(label(b"UWB1\0\0\0\0"), Some("Underwater"));
        assert_eq!(label(b"ENVB\0\0\0\0"), Some("Environment"));
        assert_eq!(label(b"OBSB\0\0\0\0"), Some("Object behavior"));
        assert_eq!(label(b"ESSB\0\0\0\0"), Some("Environment sound"));
        assert_eq!(label(b"AMB\0\x01\0\0\0"), Some("Ambient light"));
        assert_eq!(label(b" dgg\0\0\0\0"), Some("Grass grid"));
        assert_eq!(label(b"dzg\0\0\0\0\0"), Some("Grass zone"));
        assert_eq!(label(b"pap \0\0\0\0"), Some("Animation"));
        assert_eq!(label(b"TMLB\0\0\0\0"), Some("Timeline"));
        assert_eq!(label(b"CUTB\0\0\0\0"), Some("Cutscene"));
        assert_eq!(label(b"XFVA\0\0\0\0"), Some("Visual effect"));
    }

    /// A sheet list is the one magic that also passes as text, so the magic has to win.
    #[test]
    fn names_a_sheet_list_rather_than_calling_it_text() {
        assert_eq!(label(b"EXLT,2\r\nAchievement,0\r\n"), Some("Sheet list"));
    }

    #[test]
    fn says_nothing_about_a_short_or_empty_file() {
        assert!(sniff(b"").is_none());
        assert!(sniff(b"\x89").is_none());
        assert!(sniff(&[0u8; 4]).is_none());
        // Part of a magic is not the magic.
        assert!(sniff(b"uld\0").is_none());
    }

    /// The one guess with no magic behind it, so both directions are worth pinning down.
    #[test]
    fn reads_a_texture_header() {
        let mut header = vec![0u8; 128];
        header[4..8].copy_from_slice(&0x3420u32.to_le_bytes()); // Bc1Unorm
        header[8..10].copy_from_slice(&64u16.to_le_bytes());
        header[10..12].copy_from_slice(&64u16.to_le_bytes());
        header[14] = 7;
        header[28..32].copy_from_slice(&TEX_HEADER.to_le_bytes());
        assert_eq!(label(&header), Some("Texture"));

        for (at, byte) in [(4, 0x99), (8, 0), (14, 0), (28, 0)] {
            let mut broken = header.clone();
            broken[at] = byte;
            assert!(
                label(&broken) != Some("Texture"),
                "byte {at} went unchecked"
            );
        }
    }

    /// The other guess with no magic behind it. Terrain is all zeroes but for three words, so the
    /// checks that reject a file matter more here than the ones that accept it.
    #[test]
    fn reads_a_terrain_header() {
        let mut header = vec![0u8; TERA_HEADER];
        header[..4].copy_from_slice(&TERA_VERSION.to_le_bytes());
        header[20] = 0b111;
        assert_eq!(label(&header), Some("Terrain"));

        for (at, byte) in [(0, 0x04), (20, 0x08), (24, 0x01), (51, 0x01)] {
            let mut broken = header.clone();
            broken[at] = byte;
            assert!(
                label(&broken) != Some("Terrain"),
                "byte {at} went unchecked"
            );
        }
        // A header cut short is not a terrain file, however well the part that survived reads.
        assert!(label(&header[..TERA_HEADER - 1]) != Some("Terrain"));
    }

    #[test]
    fn tells_text_from_binary() {
        assert_eq!(label(b"name,value\r\nfoo,1\r\n"), Some("Text"));
        assert!(sniff(b"\0\0\0\0nul first").is_none());
        assert!(sniff(&[0xff, 0xfe, 0x01, 0x02]).is_none());
    }
}
