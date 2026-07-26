use anyhow::{Context, Result, bail};

use crate::consts::{GROUP_LITERAL, GROUP_RANGE, PATH_LIST};
use crate::utils::Reader;

pub struct PathList {
    dirs: Vec<Box<str>>,
    block_of_dir: Vec<(u32, u32)>,
    list_id: u64,
    name_offset: Vec<u32>,
    blocks: Box<[u8]>,
}

impl PathList {
    pub fn decode(body: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(body);
        reader.tag(&PATH_LIST)?;
        let list_id = reader.u64_le()?;
        let dir_count = reader.varint()? as usize;
        let mut dirs: Vec<Box<str>> = Vec::with_capacity(dir_count);
        let mut prev: Vec<u8> = Vec::new();
        for _ in 0..dir_count {
            prev = reader.front_coded(&prev)?;
            dirs.push(str::from_utf8(&prev)?.into());
        }

        let mut ids = Vec::with_capacity(dir_count);
        let mut id = 0i64;
        for _ in 0..dir_count {
            id += reader.zigzag()?;
            ids.push(usize::try_from(id).context("negative block id in path list")?);
        }

        let block_count = reader.varint()? as usize;
        let mut bounds = Vec::with_capacity(block_count);
        let mut at = 0u32;
        for _ in 0..block_count {
            let len = u32::try_from(reader.varint()?).context("block longer than 4 GiB")?;
            bounds.push((at, at + len));
            at += len;
        }
        let blocks: Box<[u8]> = reader.take(at as usize)?.into();

        let block_of_dir = ids
            .into_iter()
            .map(|id| {
                bounds
                    .get(id)
                    .copied()
                    .context("directory points past the last block")
            })
            .collect::<Result<Vec<_>>>()?;

        // One entry per directory plus a terminator, so a name count is a subtraction and this is
        // the only walk over the blocks.
        let mut name_offset = Vec::with_capacity(block_of_dir.len() + 1);
        let mut running = 0u32;
        for (start, end) in &block_of_dir {
            name_offset.push(running);
            running += count_names(&blocks[*start as usize..*end as usize])?;
        }
        name_offset.push(running);

        Ok(Self {
            dirs,
            block_of_dir,
            list_id,
            name_offset,
            blocks,
        })
    }

    /// How many names precede this directory in canonical order. Added to a name's position within
    /// the directory it gives the index a [`Presence`](crate::Presence) map is keyed by.
    pub fn name_offset(&self, dir: usize) -> Result<usize> {
        if dir >= self.dirs.len() {
            bail!("directory index out of range");
        }
        Ok(self.name_offset[dir] as usize)
    }

    /// Names in one directory. The offsets are cumulative and carry a terminating entry, so this is
    /// the gap to the next directory rather than another walk over the block.
    pub fn name_count(&self, dir: usize) -> Result<usize> {
        let start = self.name_offset(dir)?;
        Ok(self.name_offset[dir + 1] as usize - start)
    }

    /// Total names across every directory.
    pub fn len(&self) -> usize {
        self.name_offset.last().copied().unwrap_or(0) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Identifies the master list this was built from. A [`Presence`](crate::Presence) map is indexed
    /// by position in that list.
    pub fn list_id(&self) -> u64 {
        self.list_id
    }

    pub fn dirs(&self) -> &[Box<str>] {
        &self.dirs
    }

    pub fn resident_bytes(&self) -> usize {
        self.blocks.len()
            + self.block_of_dir.len() * size_of::<(u32, u32)>()
            + self
                .dirs
                .iter()
                .map(|dir| dir.len() + size_of::<Box<str>>())
                .sum::<usize>()
    }

    /// The sorted file names in one directory.
    pub fn names(&self, dir: usize) -> Result<Vec<String>> {
        let (start, end) = *self
            .block_of_dir
            .get(dir)
            .context("directory index out of range")?;
        let mut reader = Reader::new(&self.blocks[start as usize..end as usize]);
        let groups = reader.varint()?;
        let mut out: Vec<String> = Vec::new();
        for _ in 0..groups {
            match reader.byte()? {
                GROUP_LITERAL => {
                    let count = reader.varint()?;
                    let mut prev: Vec<u8> = Vec::new();
                    for _ in 0..count {
                        prev = reader.front_coded(&prev)?;
                        out.push(String::from_utf8(prev.clone())?);
                    }
                }
                GROUP_RANGE => {
                    let prefix = str::from_utf8(reader.section()?)?.to_owned();
                    let width = usize::from(reader.byte()?);
                    let suffix = str::from_utf8(reader.section()?)?.to_owned();
                    let first = reader.varint()?;
                    let count = reader.varint()?;
                    out.reserve(count as usize);
                    for i in 0..count {
                        out.push(format!("{prefix}{:0width$}{suffix}", first + i));
                    }
                }
                other => bail!("unknown group kind {other} in path list"),
            }
        }
        // Groups are emitted unordered; a single group is already sorted.
        if groups > 1 {
            out.sort();
        }
        Ok(out)
    }
}

/// Names a block holds, read from the group headers without expanding anything.
fn count_names(block: &[u8]) -> Result<u32> {
    let mut reader = Reader::new(block);
    let groups = reader.varint()?;
    let mut total = 0u32;
    for _ in 0..groups {
        match reader.byte()? {
            GROUP_LITERAL => {
                let count = reader.varint()?;
                for _ in 0..count {
                    reader.varint()?;
                    let tail = reader.varint()? as usize;
                    reader.take(tail)?;
                }
                total += count as u32;
            }
            GROUP_RANGE => {
                reader.section()?;
                reader.byte()?;
                reader.section()?;
                reader.varint()?;
                total += reader.varint()? as u32;
            }
            other => bail!("unknown group kind {other} in path list"),
        }
    }
    Ok(total)
}
