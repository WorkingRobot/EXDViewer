use crate::utils::Tag;

pub(crate) const PATH_LIST: Tag = Tag::new(*b"PTL", 4, "path list");
pub(crate) const PRESENCE: Tag = Tag::new(*b"PDB", 3, "presence map");

pub(crate) const GROUP_LITERAL: u8 = 0;
pub(crate) const GROUP_RANGE: u8 = 1;

#[cfg(feature = "encode")]
pub(crate) const DIR_RESTART: usize = 32;

#[cfg(feature = "encode")]
pub(crate) const MIN_RUN: usize = 8;

#[cfg(feature = "encode")]
pub(crate) const ZSTD_LEVEL: i32 = 19;
