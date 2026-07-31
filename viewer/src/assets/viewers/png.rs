//! Ordinary image files (`.png`), decoded with the `image` crate.

use anyhow::Result;

use super::{Preview, upload};
use crate::assets::Bytes;
use crate::assets::Channels;

pub fn decode(
    ctx: &egui::Context,
    path: &str,
    bytes: &[u8],
    channels: Channels,
) -> Result<Preview> {
    let image = image::load_from_memory(bytes)?;
    let facts = vec![
        ("Format", "PNG".to_string()),
        (
            "Dimensions",
            format!("{} x {}", image.width(), image.height()),
        ),
        ("Colour", format!("{:?}", image.color())),
        ("File size", Bytes(bytes.len()).to_string()),
    ];
    Ok(upload(ctx, path, image, 1, 4, facts, Vec::new(), channels))
}
