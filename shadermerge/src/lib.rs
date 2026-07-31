//! One HLSL source per `(stage, pass)` of a shader package, merged from every variant it compiled.
//!
//! A package ships the same shader many times over, once per combination of the keys it is compiled
//! under, and the differences between two of them are usually a few lines. This unions their value
//! graphs and prints the result once, with the lines a variant does not compute wrapped in `#if`s
//! over that package's own key names. The output compiles: every loop-free group in the shipped
//! files builds under `dxc` once per variant it claims.
//!
//! Values are content-addressed rather than compared as text, because one added `#ifdef` makes the
//! compiler reallocate every register and reschedule the whole shader.

pub(crate) mod canon;
pub mod check;
mod factor;
pub(crate) mod merge;
pub(crate) mod pick;
pub(crate) mod synth;

use ironworks::file::shpk::ShaderPackage;

pub use merge::merge as source;
pub use pick::{Group, group, stage_of, tag};

/// Why a group could not be merged. Every one of these is a group the merger does not handle yet
/// rather than a malformed file, so they are worth telling the reader apart.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    NoSuchGroup,
    /// The group contains a loop. The canonicaliser walks a loop body once with no back edge and
    /// nothing prints the construct, so a listing for it would read as a program without being one.
    Loops,
    NoProgram,
    Truncated,
    /// The `#if` regions and the blocks the decompiler produced stopped lining up.
    Regions,
    /// Two values were given one register while some variant computes both.
    Pooling,
    /// A line reads a register that nothing writes in every variant that line runs in.
    Unwritten,
    Scheduling,
    /// Variants of the group carry different immediate constant buffers, and a program has one.
    Tables,
}

impl std::fmt::Display for Error {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(match self {
            Self::NoSuchGroup => "no shaders for this stage and pass",
            Self::Loops => "this pass contains a loop, which merging does not reconstruct yet",
            Self::NoProgram => "a shader in this pass holds no program",
            Self::Truncated => "a shader in this pass runs past the end of the file",
            Self::Regions => "the guarded blocks did not line up with their conditions",
            Self::Pooling => "two values were pooled into one register but share a variant",
            Self::Unwritten => "a line reads a register nothing writes",
            Self::Scheduling => "the values could not be ordered",
            Self::Tables => "this pass mixes shaders with different constant tables",
        })
    }
}

impl std::error::Error for Error {}

/// A merged source, and what it stands for.
pub struct Merged {
    /// The whole file: a banner, the slot table for repacking, one `// variant` line per key
    /// combination, then the body as HLSL.
    pub lines: Vec<String>,
    /// The same, with the merged program as assembly instead. The guards are the `if` blocks over
    /// the key buffer that the HLSL reading turns into `#if`.
    pub asm: Vec<String>,
    /// Where the body starts in either reading, so a reader can fold the header away.
    pub body: usize,
    /// Key combinations the source covers.
    pub variants: usize,
    /// Distinct compiled shaders behind those combinations.
    pub blobs: usize,
    pub regions: usize,
}

/// The merged source for one pass.
///
/// Pooling is what separates a merged source from a pile of near-copies: it writes a tail once where
/// several branches compute it from different inputs. It does not yet emit every group, so a group
/// it cannot lay out falls back to the plain layout rather than failing.
pub fn pass(package: &ShaderPackage, raw: &[u8], stage: usize, pass: u32) -> Result<Merged, Error> {
    match merge::merge(package, raw, stage, pass, 1) {
        Ok(held) => Ok(held),
        Err(_) => merge::merge(package, raw, stage, pass, 0),
    }
}

/// The same, pooling values that differ in at most `holes` of their inputs.
///
/// Pooling writes a tail once where several branches compute it from different inputs, which is what
/// separates a merged source from a pile of near-copies. It shortens a group that duplicated whole
/// tails and lengthens one that did not, so a caller wanting the shorter answer asks for both.
pub fn pooled(
    package: &ShaderPackage,
    raw: &[u8],
    stage: usize,
    pass: u32,
    holes: usize,
) -> Result<Merged, Error> {
    merge::merge(package, raw, stage, pass, holes)
}
