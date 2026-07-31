//! What a file is, read from its bytes rather than from its name.
//!
//! Most of what the game ships leads with a magic. The two formats here that do not are `.tex`,
//! recognized by the shape of its fixed header, and `.mtrl`, which is small enough to read outright
//! and names a shader package that gives it away. Anything left over that decodes cleanly is text.

use std::io::Cursor;

use binrw::BinRead;
use ironworks::file::{File, mtrl, tex};

use super::viewers::Viewer;

/// How much of a file is looked at before calling it text.
const SAMPLE: usize = 1024;

/// A `.tex` header, which the first surface follows immediately.
const TEX_HEADER: u32 = 80;

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
    (b"ShPk", Format::Shown(Viewer::Shpk)),
    (b"ShCd", Format::Shown(Viewer::Shcd)),
    (b"blks", Format::Named("Skeleton")),
    (b"SEDBSSCF", Format::Named("Sound")),
    (b"EXHF", Format::Named("Sheet header")),
    (b"EXDF", Format::Named("Sheet page")),
];

/// What the bytes say the file is, or `None` where they say nothing. Ordered strongest test first:
/// a magic settles it outright, and the two guesses below only ever see what no magic claimed.
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
    is_text(bytes).then_some(Format::Shown(Viewer::Text))
}

/// Read as a material and believed only if it names a shader package, which nothing else would. The
/// header states its own size as a `u16` and every count in it is a byte or a short, so this reads a
/// bounded amount however little of the file is really a material.
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
        assert_eq!(label(b"ShCd\0\0\0\0"), Some("Shader code"));
        assert_eq!(label(b"\x89PNG\r\n\x1a\n\0\0\0\0"), Some("Image"));
        assert_eq!(label(b"SEDBSSCF\0\0\0\0"), Some("Sound"));
        assert_eq!(label(b"blks\0\0\0\0"), Some("Skeleton"));
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

    #[test]
    fn tells_text_from_binary() {
        assert_eq!(label(b"name,value\r\nfoo,1\r\n"), Some("Text"));
        assert!(sniff(b"\0\0\0\0nul first").is_none());
        assert!(sniff(&[0xff, 0xfe, 0x01, 0x02]).is_none());
    }
}
