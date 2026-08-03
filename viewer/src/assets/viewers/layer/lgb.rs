//! `.lgb` layer groups: where a zone's objects are placed, one file per group.

use std::io::Cursor;

use anyhow::Result;
use ironworks::file::{File, lgb};

use super::super::Preview;
use super::Source;

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    let file = lgb::LayerGroupFile::read(Cursor::new(bytes.to_vec()))?;
    let group = file.group();

    let identity = vec![
        ("Group", group.name().clone()),
        ("Group ID", group.id().to_string()),
    ];

    Ok(Preview::Layers(Box::new(super::rendered(
        path,
        identity,
        Source::Group(file),
    ))))
}
