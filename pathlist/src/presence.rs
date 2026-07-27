//! Which paths of the global list a particular game version actually ships.
//!
//! ```text
//! "PDB" u8(3)                 four-byte tag: format name, then version
//! u64le list_id
//! varint count
//! ceil(count / 8) bytes, bit `i` (LSB first) set when path `i` is present
//! varint unnamed_count
//! per unnamed file: u8 repository, u8 category, u8 split, varint hash
//! ```

use anyhow::Result;
use ironworks::sqpack::IndexHash;

use crate::consts::PRESENCE;
use crate::utils::{Reader, Writer};

/// An unnamed file (doesn't exist in the path list)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unnamed {
    pub repository: u8,
    pub category: u8,
    pub hash: u64,
    /// Whether `hash` is the `.index` split form; otherwise it is the `.index2` whole-path crc32.
    pub split: bool,
}

pub struct Presence {
    list_id: u64,
    count: usize,
    bits: Box<[u8]>,
    unnamed: Vec<Unnamed>,
}

impl Presence {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(bytes);
        reader.tag(&PRESENCE)?;
        let list_id = reader.u64_le()?;
        let count = reader.varint()? as usize;
        let bits = reader.take(count.div_ceil(8))?.into();
        let unnamed_count = reader.varint()? as usize;
        let mut unnamed = Vec::with_capacity(unnamed_count);
        for _ in 0..unnamed_count {
            let repository = reader.byte()?;
            let category = reader.byte()?;
            let split = reader.byte()? != 0;
            unnamed.push(Unnamed {
                repository,
                category,
                hash: reader.varint()?,
                split,
            });
        }
        Ok(Self {
            list_id,
            count,
            bits,
            unnamed,
        })
    }

    /// The master list this was built against; must match [`PathList::list_id`](crate::PathList::list_id).
    pub fn list_id(&self) -> u64 {
        self.list_id
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn unnamed(&self) -> &[Unnamed] {
        &self.unnamed
    }

    /// Whether the path at `index` in the global list ships in this version.
    pub fn contains(&self, index: usize) -> bool {
        index < self.count && self.bits[index / 8] & (1 << (index % 8)) != 0
    }
}

/// `present[i]` says whether path `i` of the global list ships in this version.
pub fn encode_presence(present: &[bool], unnamed: &[Unnamed], list_id: u64) -> Vec<u8> {
    let mut bits = vec![0u8; present.len().div_ceil(8)];
    for (index, _) in present.iter().enumerate().filter(|(_, set)| **set) {
        bits[index / 8] |= 1 << (index % 8);
    }

    let mut out = Writer::with_capacity(bits.len() + 16);
    out.tag(&PRESENCE);
    out.u64_le(list_id);
    out.varint(present.len() as u64);
    out.raw(&bits);

    out.varint(unnamed.len() as u64);
    for file in unnamed {
        out.byte(file.repository);
        out.byte(file.category);
        out.byte(u8::from(file.split));
        out.varint(file.hash);
    }
    out.finish()
}

/// Which listed paths a particular install actually ships, plus the files it has that the list does
/// not name.
///
/// `walk` yields every `(directory, name)` in list order; the join, lowercasing and hashing happen
/// here so that the server and the client cannot disagree about what "present" means. `locate`
/// resolves a path to its `(repository, category)`, returning `None` for a path no package could
/// hold. Returns the encoded map, uncompressed.
pub fn build_presence(
    path_count: usize,
    installed: &std::collections::HashSet<(u8, u8, IndexHash)>,
    locate: impl Fn(&str) -> Option<(u8, u8)>,
    list_id: u64,
    walk: impl FnOnce(&mut dyn FnMut(&str, &str)),
) -> Vec<u8> {
    let mut named = std::collections::HashSet::with_capacity(installed.len());
    let mut present = Vec::with_capacity(path_count);
    let mut full = String::new();

    walk(&mut |dir, name| {
        full.clear();
        if !dir.is_empty() {
            full.push_str(dir);
            full.push('/');
        }
        full.push_str(name);
        // Packages hash lowercased paths, and the list is not uniformly lowercase.
        full.make_ascii_lowercase();

        let Some((repository, category)) = locate(&full) else {
            present.push(false);
            return;
        };
        let (split, whole) = IndexHash::of(&full);
        let mut found = false;
        for key in split
            .map(|hash| (repository, category, hash))
            .into_iter()
            .chain(std::iter::once((repository, category, whole)))
        {
            if installed.contains(&key) {
                named.insert(key);
                found = true;
            }
        }
        present.push(found);
    });

    let mut unnamed: Vec<Unnamed> = installed
        .iter()
        .filter(|key| !named.contains(*key))
        .map(|(repository, category, hash)| Unnamed {
            repository: *repository,
            category: *category,
            hash: match hash {
                IndexHash::Split(hash) => *hash,
                IndexHash::Whole(hash) => u64::from(*hash),
            },
            split: matches!(hash, IndexHash::Split(_)),
        })
        .collect();
    unnamed.sort_unstable_by_key(|file| (file.repository, file.category, file.split, file.hash));

    encode_presence(&present, &unnamed, list_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_arbitrary_runs() {
        // spans byte boundaries, ends mid-byte, and mixes long runs with isolated bits
        let present: Vec<bool> = (0..1000)
            .map(|i| matches!(i, 0 | 7 | 8 | 63..=130 | 999))
            .collect();
        let unnamed = [
            Unnamed {
                repository: 0,
                category: 10,
                hash: 0xdead_beef_cafe,
                split: true,
            },
            Unnamed {
                repository: 2,
                category: 6,
                hash: 0x1234,
                split: false,
            },
        ];
        let map = Presence::decode(&encode_presence(&present, &unnamed, 7)).unwrap();
        assert_eq!(map.list_id(), 7);
        assert_eq!(map.unnamed(), unnamed);
        assert_eq!(map.len(), present.len());
        for (i, want) in present.iter().enumerate() {
            assert_eq!(map.contains(i), *want, "bit {i}");
        }
        assert!(!map.contains(present.len()), "past the end reads as absent");
    }

    /// The sibling format is the realistic mix-up, and its tag has to be rejected on the name rather
    /// than the version byte.
    #[test]
    fn rejects_a_foreign_blob() {
        assert!(Presence::decode(b"PTL\x04nope").is_err());
        assert!(Presence::decode(b"PDB\x02nope").is_err());
    }
}
