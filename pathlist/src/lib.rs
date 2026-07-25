//! A directory listing for one game version.\
//!
//! ```text
//! "PTL" u8(4)                 four-byte tag: format name, then version
//! u64le list_id
//! varint dir_count
//! front-coded sorted dir paths, restarting every DIR_RESTART entries
//! dir_count × zigzag varint   delta of that directory's block id
//! varint block_count
//! block_count × varint        length of each block
//! concatenated block bytes
//! ```
//!
//! This is the *uncompressed* body. Compression is left to HTTP `Content-Encoding`, so browsers
//! inflate it with their own zstd.
//!
//! A block is `varint group_count` followed by groups:
//!
//! ```text
//! u8 0  literal: varint n, then n × (varint shared, varint tail_len, tail)
//! u8 1  range:   varint len + prefix, u8 digit_width, varint len + suffix, varint start, varint count
//! ```
//!
//! A range expands to `prefix + zero_pad(start + i, digit_width) + suffix` for `i` in `0..count`.
//! Groups are emitted in no particular order, so a reader with more than one group must sort the
//! union; single-group blocks are already sorted.

mod consts;
mod presence;
mod read;
mod utils;
#[cfg(feature = "encode")]
mod write;

pub use presence::{Presence, Unnamed, encode_presence};
pub use read::PathList;
#[cfg(feature = "encode")]
pub use write::{compress, decompress, encode};

/// Version of the presence map format this build writes. Callers that persist an encoded map have to
/// key their storage by it, since a stored map from another version cannot be decoded.
pub const PRESENCE_VERSION: u8 = consts::PRESENCE.version;
