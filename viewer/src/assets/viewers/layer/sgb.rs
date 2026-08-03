//! `.sgb` shared groups: a prefab of placed objects a zone can put down many times over.

use std::io::Cursor;

use anyhow::Result;
use ironworks::file::{File, sgb};

use super::super::Preview;
use super::Source;

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    let file = sgb::SharedGroupFile::read(Cursor::new(bytes.to_vec()))?;
    let scene = file.scene();

    let mut identity = vec![("Assets", scene.bg_path().clone())];
    if !scene.environments().is_empty() {
        identity.push(("Environments", scene.environments().len().to_string()));
    }

    Ok(Preview::Layers(Box::new(super::rendered(
        path,
        identity,
        Source::Shared(file),
    ))))
}
