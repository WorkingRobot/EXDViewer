use std::ops::Deref;

use compact_str::CompactString;
use ironworks::sestring::{SeStr, SeString};
use smallvec::SmallVec;

const MAX_SIZE: usize = core::mem::size_of::<String>();

#[repr(transparent)]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CompactSeString(SmallVec<[u8; MAX_SIZE]>);

impl CompactSeString {
    pub fn new(string: SeString) -> Self {
        Self(SmallVec::from_vec(string.into_inner()))
    }
}

impl Deref for CompactSeString {
    type Target = SeStr;

    fn deref(&self) -> &Self::Target {
        self.0.as_slice().into()
    }
}

impl From<&SeStr> for CompactSeString {
    fn from(sestring: &SeStr) -> Self {
        Self(SmallVec::from_slice(sestring.as_bytes()))
    }
}

impl From<CompactString> for CompactSeString {
    fn from(compact: CompactString) -> Self {
        Self(compact.into_bytes())
    }
}
