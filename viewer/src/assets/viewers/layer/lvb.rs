//! `.lvb` levels: the scene naming the layer groups and settings a zone is built from.

use std::io::Cursor;

use anyhow::Result;
use ironworks::file::{File, lvb};

use super::super::Preview;
use super::Source;

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    let file = lvb::LevelFile::read(Cursor::new(bytes.to_vec()))?;

    let mut identity = Vec::new();
    if !file.scene().bg_path().is_empty() {
        identity.push(("Assets", file.scene().bg_path().clone()));
    }

    Ok(Preview::Layers(Box::new(super::rendered(
        path,
        identity,
        Source::Level(file),
    ))))
}
