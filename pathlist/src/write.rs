use std::collections::HashMap;

use anyhow::{Context, Result};

use crate::consts::{DIR_RESTART, GROUP_LITERAL, GROUP_RANGE, MIN_RUN, PATH_LIST, ZSTD_LEVEL};
use crate::utils::Writer;

type Families<'a> = HashMap<(&'a str, u8, &'a str), Vec<(u64, &'a str)>>;

/// A name split around its first digit run: `000123_hr1.tex` -> `("", 6, "_hr1.tex")` + 123.
fn split_numeric(name: &str) -> Option<((&str, u8, &str), u64)> {
    let start = name.find(|c: char| c.is_ascii_digit())?;
    let end = name[start..]
        .find(|c: char| !c.is_ascii_digit())
        .map_or(name.len(), |i| start + i);
    let digits = &name[start..end];
    // Wider than u64 can hold; those stay literal.
    if digits.len() > 19 {
        return None;
    }
    let value = digits.parse().ok()?;
    Some(((&name[..start], digits.len() as u8, &name[end..]), value))
}

fn encode_literal_group(out: &mut Writer, names: &[impl AsRef<str>]) {
    out.byte(GROUP_LITERAL);
    out.varint(names.len() as u64);
    let mut prev: &[u8] = b"";
    for name in names {
        let bytes = name.as_ref().as_bytes();
        out.front_coded(prev, bytes);
        prev = bytes;
    }
}

/// Collapse dense numeric families (`ui/icon` and friends) into `start..start+count` runs.
fn encode_carved(names: &[&str]) -> Option<Writer> {
    // Keeping the source name beside its number lets a run that turns out too short drop straight
    // back to the literal group.
    let mut families: Families<'_> = HashMap::new();
    let mut rest: Vec<&str> = Vec::new();
    for name in names {
        match split_numeric(name) {
            Some((key, value)) => families.entry(key).or_default().push((value, name)),
            None => rest.push(name),
        }
    }

    let mut ranges = Vec::new();
    for ((prefix, width, suffix), mut members) in families {
        members.sort_unstable();
        let mut i = 0;
        while i < members.len() {
            let mut j = i;
            while j + 1 < members.len() && members[j + 1].0 == members[j].0 + 1 {
                j += 1;
            }
            if j - i + 1 >= MIN_RUN {
                ranges.push((prefix, width, suffix, members[i].0, j - i + 1));
            } else {
                rest.extend(members[i..=j].iter().map(|(_, name)| *name));
            }
            i = j + 1;
        }
    }
    if ranges.is_empty() {
        return None;
    }
    rest.sort_unstable();

    let mut out = Writer::default();
    out.varint((ranges.len() + usize::from(!rest.is_empty())) as u64);
    for (prefix, width, suffix, start, count) in &ranges {
        out.byte(GROUP_RANGE);
        out.section(prefix.as_bytes());
        out.byte(*width);
        out.section(suffix.as_bytes());
        out.varint(*start);
        out.varint(*count as u64);
    }
    if !rest.is_empty() {
        encode_literal_group(&mut out, &rest);
    }
    Some(out)
}

fn encode_block(names: &[&str]) -> Vec<u8> {
    let mut literal = Writer::default();
    literal.varint(1);
    encode_literal_group(&mut literal, names);
    let literal = literal.finish();
    match encode_carved(names).map(Writer::finish) {
        Some(carved) if carved.len() < literal.len() => carved,
        _ => literal,
    }
}

fn encode_dirs(out: &mut Writer, dirs: &[&str]) {
    out.varint(dirs.len() as u64);
    let mut prev: &[u8] = b"";
    for (i, dir) in dirs.iter().enumerate() {
        let bytes = dir.as_bytes();
        // Restarting against nothing spends the shared prefix to bound how far back a reader scans.
        let base: &[u8] = if i % DIR_RESTART == 0 { b"" } else { prev };
        out.front_coded(base, bytes);
        prev = bytes;
    }
}

fn dedup_blocks(entries: &[(&str, Vec<&str>)]) -> (Vec<Vec<u8>>, Vec<u32>) {
    let mut blocks: Vec<Vec<u8>> = Vec::new();
    let mut seen: HashMap<Vec<u8>, u32> = HashMap::new();
    let mut block_ids: Vec<u32> = Vec::with_capacity(entries.len());
    for (_, names) in entries {
        let block = encode_block(names);
        let id = *seen.entry(block.clone()).or_insert_with(|| {
            blocks.push(block);
            (blocks.len() - 1) as u32
        });
        block_ids.push(id);
    }
    (blocks, block_ids)
}

/// `entries` must be sorted by directory, and each directory's names sorted.
pub fn encode(entries: &[(&str, Vec<&str>)], list_id: u64) -> Vec<u8> {
    let (blocks, block_ids) = dedup_blocks(entries);

    let dirs: Vec<&str> = entries.iter().map(|(dir, _)| *dir).collect();
    let mut out = Writer::with_capacity(1 << 20);
    out.tag(&PATH_LIST);
    out.u64_le(list_id);
    encode_dirs(&mut out, &dirs);

    let mut prev = 0i64;
    for id in &block_ids {
        let id = i64::from(*id);
        out.zigzag(id - prev);
        prev = id;
    }

    out.varint(blocks.len() as u64);
    for block in &blocks {
        out.varint(block.len() as u64);
    }
    for block in &blocks {
        out.raw(block);
    }
    out.finish()
}

/// Stored and served as a zstd frame under `Content-Encoding`.
pub fn compress(body: &[u8]) -> Result<Vec<u8>> {
    zstd::encode_all(body, ZSTD_LEVEL).context("compressing path list")
}

pub fn decompress(frame: &[u8]) -> Result<Vec<u8>> {
    zstd::decode_all(frame).context("decompressing path list")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::Reader;

    fn decode_block(block: &[u8]) -> Vec<String> {
        let mut reader = Reader::new(block);
        let groups = reader.varint().unwrap();
        let mut out = Vec::new();
        for _ in 0..groups {
            match reader.byte().unwrap() {
                GROUP_LITERAL => {
                    let count = reader.varint().unwrap();
                    let mut prev: Vec<u8> = Vec::new();
                    for _ in 0..count {
                        prev = reader.front_coded(&prev).unwrap();
                        out.push(String::from_utf8(prev.clone()).unwrap());
                    }
                }
                GROUP_RANGE => {
                    let prefix = str::from_utf8(reader.section().unwrap())
                        .unwrap()
                        .to_owned();
                    let width = usize::from(reader.byte().unwrap());
                    let suffix = str::from_utf8(reader.section().unwrap())
                        .unwrap()
                        .to_owned();
                    let start = reader.varint().unwrap();
                    let count = reader.varint().unwrap();
                    for i in 0..count {
                        out.push(format!("{prefix}{:0width$}{suffix}", start + i));
                    }
                }
                other => panic!("unknown group kind {other}"),
            }
        }
        if groups > 1 {
            out.sort();
        }
        out
    }

    /// Mirrors what the client has to do to read an encoded list, independently of [`crate::PathList`].
    fn decode_all(body: &[u8]) -> Vec<(String, Vec<String>)> {
        let mut reader = Reader::new(body);
        reader.tag(&PATH_LIST).unwrap();
        reader.u64_le().unwrap();

        let dir_count = reader.varint().unwrap() as usize;
        let mut dirs = Vec::with_capacity(dir_count);
        let mut prev: Vec<u8> = Vec::new();
        for _ in 0..dir_count {
            prev = reader.front_coded(&prev).unwrap();
            dirs.push(String::from_utf8(prev.clone()).unwrap());
        }

        let mut ids = Vec::with_capacity(dir_count);
        let mut id = 0i64;
        for _ in 0..dir_count {
            id += reader.zigzag().unwrap();
            ids.push(id as usize);
        }

        let block_count = reader.varint().unwrap() as usize;
        let lens: Vec<usize> = (0..block_count)
            .map(|_| reader.varint().unwrap() as usize)
            .collect();
        let blocks: Vec<&[u8]> = lens
            .into_iter()
            .map(|len| reader.take(len).unwrap())
            .collect();

        dirs.into_iter()
            .zip(ids)
            .map(|(dir, id)| (dir, decode_block(blocks[id])))
            .collect()
    }

    fn assert_round_trips(entries: &[(&str, Vec<&str>)]) {
        let decoded = decode_all(&encode(entries, 7));
        assert_eq!(decoded.len(), entries.len());
        for ((dir, names), (want_dir, want_names)) in decoded.iter().zip(entries) {
            assert_eq!(dir, want_dir);
            assert_eq!(names, want_names);
        }
    }

    /// A presence map is keyed by canonical position, so `name_offset` has to agree with the order
    /// a reader walks. If these drift, versions silently hide or reveal the wrong files.
    #[test]
    fn name_offsets_match_canonical_order() {
        let entries: Vec<(&str, Vec<&str>)> = vec![
            ("bg", vec!["a.mdl", "b.mdl"]),
            ("exd", vec!["root.exl"]),
            // shares a block with "bg" but must not share its offset
            ("music", vec!["a.mdl", "b.mdl"]),
            (
                "ui/icon/000000",
                vec!["000001.tex", "000002.tex", "000003.tex"],
            ),
        ];
        let list = crate::PathList::decode(&encode(&entries, 7)).unwrap();
        assert_eq!(list.len(), 8);

        let mut expected = 0usize;
        for (dir, (_, names)) in entries.iter().enumerate() {
            assert_eq!(
                list.name_offset(dir).unwrap(),
                expected,
                "offset for dir {dir}"
            );
            expected += names.len();
        }
        assert!(list.name_offset(entries.len()).is_err());
    }

    #[test]
    fn round_trips_small_corpus() {
        let entries: Vec<(&str, Vec<&str>)> = vec![
            ("bg/ffxiv/sea_s1", vec!["a.mdl", "ab.mdl", "abc.tex"]),
            (
                "music/ex1",
                vec!["BGM_EX1_Alex03.scd", "BGM_EX1_Answers.scd"],
            ),
            (
                "ui/icon/000000",
                vec!["000001.tex", "000001_hr1.tex", "000002.tex"],
            ),
            // shares a block with the first entry, exercising dedup
            ("ui/icon/000100", vec!["a.mdl", "ab.mdl", "abc.tex"]),
        ];
        assert_round_trips(&entries);
    }

    /// The `ui/icon` shape: two families interleaved in sort order.
    #[test]
    fn carves_interleaved_numeric_runs() {
        let names: Vec<String> = (1..=500)
            .flat_map(|i| [format!("{i:06}.tex"), format!("{i:06}_hr1.tex")])
            .collect();
        let mut sorted: Vec<&str> = names.iter().map(String::as_str).collect();
        sorted.sort_unstable();

        let carved = encode_block(&sorted);
        let mut literal = Writer::default();
        literal.varint(1);
        encode_literal_group(&mut literal, &sorted);
        let literal = literal.finish();
        assert!(
            carved.len() * 10 < literal.len(),
            "carve-out barely helped: {} vs literal {}",
            carved.len(),
            literal.len()
        );
        assert_eq!(decode_block(&carved), sorted);
    }

    #[test]
    fn leaves_sparse_names_literal() {
        let sorted = vec!["a.mdl", "b1c2.tex", "x0100.tex", "x9000.tex", "z.tex"];
        let block = encode_block(&sorted);
        assert_eq!(decode_block(&block), sorted);
    }

    #[test]
    fn round_trips_large_corpus() {
        let dirs: Vec<String> = (0..2000).map(|i| format!("ui/icon/{i:06}")).collect();
        let names: Vec<Vec<String>> = (0..2000)
            .map(|i| {
                (0..8)
                    .map(|n| format!("{:06}_hr1.tex", i * 8 + n))
                    .collect()
            })
            .collect();
        let entries: Vec<(&str, Vec<&str>)> = dirs
            .iter()
            .zip(&names)
            .map(|(d, ns)| (d.as_str(), ns.iter().map(String::as_str).collect()))
            .collect();
        assert_round_trips(&entries);
    }
}
