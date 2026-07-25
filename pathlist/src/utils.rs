use anyhow::{Context, Result, bail};

pub(crate) struct Tag {
    pub name: [u8; 3],
    pub version: u8,
    /// Human readable format name.
    pub label: &'static str,
}

impl Tag {
    pub(crate) const fn new(name: [u8; 3], version: u8, label: &'static str) -> Self {
        Self {
            name,
            version,
            label,
        }
    }

    const fn bytes(&self) -> [u8; 4] {
        let [a, b, c] = self.name;
        [a, b, c, self.version]
    }
}

pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    pub(crate) fn tag(&mut self, want: &Tag) -> Result<()> {
        let found = self
            .take(4)
            .with_context(|| format!("not a {}", want.label))?;
        if found[..3] != want.name {
            bail!("not a {}", want.label);
        }
        if found[3] != want.version {
            bail!(
                "{} version {} is not the version this build understands",
                want.label,
                found[3]
            );
        }
        Ok(())
    }

    pub(crate) fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self.pos + len;
        let out = self.bytes.get(self.pos..end).context("truncated section")?;
        self.pos = end;
        Ok(out)
    }

    pub(crate) fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u64_le(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("8 bytes"),
        ))
    }

    pub(crate) fn varint(&mut self) -> Result<u64> {
        let mut value = 0u64;
        let mut shift = 0;
        loop {
            let byte = self.byte().context("truncated varint")?;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
            if shift > 63 {
                bail!("varint too long");
            }
        }
    }

    pub(crate) fn zigzag(&mut self) -> Result<i64> {
        let raw = self.varint()?;
        Ok((raw >> 1) as i64 ^ -((raw & 1) as i64))
    }

    /// A varint length followed by that many bytes.
    pub(crate) fn section(&mut self) -> Result<&'a [u8]> {
        let len = self.varint()? as usize;
        self.take(len)
    }

    /// One front-coded entry: how much it shares with `prev`, then the bytes that differ.
    pub(crate) fn front_coded(&mut self, prev: &[u8]) -> Result<Vec<u8>> {
        let shared = self.varint()? as usize;
        let tail = self.varint()? as usize;
        if shared > prev.len() {
            bail!("front-coded entry shares more than the previous one has");
        }
        let mut out = prev[..shared].to_vec();
        out.extend_from_slice(self.take(tail)?);
        Ok(out)
    }
}

#[derive(Default)]
pub(crate) struct Writer(Vec<u8>);

impl Writer {
    pub(crate) fn with_capacity(bytes: usize) -> Self {
        Self(Vec::with_capacity(bytes))
    }

    pub(crate) fn finish(self) -> Vec<u8> {
        self.0
    }

    pub(crate) fn tag(&mut self, tag: &Tag) {
        self.raw(&tag.bytes());
    }

    pub(crate) fn byte(&mut self, byte: u8) {
        self.0.push(byte);
    }

    pub(crate) fn u64_le(&mut self, value: u64) {
        self.raw(&value.to_le_bytes());
    }

    pub(crate) fn varint(&mut self, mut value: u64) {
        while value >= 0x80 {
            self.byte(value as u8 | 0x80);
            value >>= 7;
        }
        self.byte(value as u8);
    }

    /// Bytes as they are, with nothing to say how many there are.
    pub(crate) fn raw(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }
}

/// The parts only an encoder reaches for. A read-only build still writes presence maps, so the rest
/// of [`Writer`] stays ungated.
#[cfg(feature = "encode")]
impl Writer {
    pub(crate) fn zigzag(&mut self, value: i64) {
        self.varint(((value << 1) ^ (value >> 63)) as u64);
    }

    /// A varint length followed by the bytes, for a reader's [`Reader::section`].
    pub(crate) fn section(&mut self, bytes: &[u8]) {
        self.varint(bytes.len() as u64);
        self.raw(bytes);
    }

    /// `next` against `prev`, keeping only what differs. Pass an empty `prev` to restart a run.
    pub(crate) fn front_coded(&mut self, prev: &[u8], next: &[u8]) {
        let shared = prev.iter().zip(next).take_while(|(a, b)| a == b).count();
        self.varint(shared as u64);
        self.section(&next[shared..]);
    }
}
