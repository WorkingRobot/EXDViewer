//! The game's own `.tex` / `.atex` textures, including their mipmap chain.

use anyhow::Result;
use std::io::Cursor;

use super::{Mip, Preview, upload};
use crate::assets::Bytes;
use crate::assets::Channels;

pub fn texture_kind_name(kind: ironworks::file::tex::TextureKind) -> &'static str {
    use ironworks::file::tex::TextureKind;
    match kind {
        TextureKind::D1 => "1D",
        TextureKind::D2 => "2D",
        TextureKind::D3 => "3D",
        TextureKind::Cube => "Cube map",
        TextureKind::D2Array => "2D array",
        TextureKind::Unknown => "Unknown",
    }
}

pub fn decode(
    ctx: &egui::Context,
    path: &str,
    bytes: &[u8],
    mip: u8,
    channels: Channels,
) -> Result<Preview> {
    use ironworks::file::{File as _, tex};

    let texture = tex::Texture::read(Cursor::new(bytes.to_vec()))?;
    let format = texture.format();
    let mip = mip.min(texture.mip_levels().saturating_sub(1));
    let facts = vec![
        ("Format", format!("{format:?}")),
        ("Format kind", format!("{:?}", format.kind())),
        ("Components", format.components().to_string()),
        ("Bits per pixel", format.bits_per_pixel().to_string()),
        ("Texture kind", texture_kind_name(texture.kind()).to_owned()),
        (
            "Dimensions",
            format!("{} x {}", texture.width(), texture.height()),
        ),
        ("Depth", texture.depth().to_string()),
        ("Mipmap levels", texture.mip_levels().to_string()),
        ("Array size", texture.array_size().to_string()),
        (
            "Pixel data",
            format!(
                "{} ({} bytes)",
                Bytes(texture.data().len()),
                texture.data().len()
            ),
        ),
        ("File size", Bytes(bytes.len()).to_string()),
    ];
    let mips = (0..texture.mip_levels())
        .map(|level| {
            let (width, height) = texture.mip_size(level);
            Mip {
                level,
                width,
                height,
                bytes: texture.mip_data(level).map_or(0, <[u8]>::len),
            }
        })
        .collect();
    let image = crate::utils::tex_loader::decode_mip(&texture, mip, path)?;
    Ok(upload(
        ctx,
        path,
        image,
        texture.layers(),
        texture.format().components(),
        facts,
        mips,
        channels,
    ))
}
