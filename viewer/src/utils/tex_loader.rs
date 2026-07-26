use anyhow::{Context, Result};
use image::{DynamicImage, ImageBuffer, ImageFormat};
use image_dds::Surface;
use ironworks::{Error, Ironworks};
use ironworks::{Resource, file::tex};
use itertools::Itertools;
use std::io::Cursor;

// https://github.com/ackwell/boilmaster/blob/3d180aae4a3b5719324f5a16d22b392e4859ac07/crates/bm_asset/src/texture.rs
pub fn read<R: Resource>(ironworks: &Ironworks<R>, path: &str) -> Result<DynamicImage> {
    let texture = match ironworks.file::<tex::Texture>(path) {
        Ok(value) => value,
        Err(ironworks::Error::NotFound(a)) => Err(Error::NotFound(a))?,
        other => other.context("read file")?,
    };
    decode(texture, path)
}

/// Decode an already-read texture. The web backend hands out bytes rather than an
/// [`Ironworks`], so it comes in this way instead.
pub fn decode(texture: tex::Texture, path: &str) -> Result<DynamicImage> {
    decode_mip(&texture, 0, path)
}

/// Decode one mipmap level. Level 0 is the full-size image.
pub fn decode_mip(texture: &tex::Texture, level: u8, path: &str) -> Result<DynamicImage> {
    if !matches!(texture.kind(), tex::TextureKind::D2) {
        anyhow::bail!(
            "unsupported texture dimension {:?} for path {path}",
            texture.kind()
        );
    }

    // Block-compressed formats keep their whole mip chain in one surface, so image_dds picks the
    // level out itself. The uncompressed ones are decoded from that level's slice of the data.
    let bc = |image_format| read_texture_bc(texture, level, image_format);
    let (width, height) = texture.mip_size(level);
    let plain = || {
        texture
            .mip_data(level)
            .with_context(|| format!("texture {path} has no mipmap level {level}"))
    };

    let buffer = match texture.format() {
        tex::Format::A8Unorm => read_a8(width, height, plain()?)?,

        tex::Format::Bgra4Unorm => read_bgra4(width, height, plain()?)?,
        tex::Format::Bgr5a1Unorm => read_bgr5a1(width, height, plain()?)?,
        tex::Format::Bgra8Unorm => read_bgra8(width, height, plain()?)?,

        tex::Format::Bc1Unorm => bc(image_dds::ImageFormat::BC1RgbaUnorm)?,
        tex::Format::Bc2Unorm => bc(image_dds::ImageFormat::BC2RgbaUnorm)?,
        tex::Format::Bc3Unorm => bc(image_dds::ImageFormat::BC3RgbaUnorm)?,
        tex::Format::Bc4Unorm => bc(image_dds::ImageFormat::BC4RUnorm)?,
        tex::Format::Bc5Unorm => bc(image_dds::ImageFormat::BC5RgUnorm)?,
        tex::Format::Bc6hFloat => bc(image_dds::ImageFormat::BC6hRgbSfloat)?,
        tex::Format::Bc7Unorm => bc(image_dds::ImageFormat::BC7RgbaUnorm)?,

        other => {
            anyhow::bail!("unsupported texture format {other:?} for path {path}");
        }
    };

    Ok(buffer)
}

fn read_a8(width: u16, height: u16, data: &[u8]) -> Result<DynamicImage> {
    let buffer = ImageBuffer::from_raw(width.into(), height.into(), data.to_owned())
        .context("failed to build image buffer")?;
    Ok(DynamicImage::ImageLuma8(buffer))
}

fn read_bgra4(width: u16, height: u16, data: &[u8]) -> Result<DynamicImage> {
    let data = data
        .iter()
        .tuples()
        .flat_map(|(gb, ar)| {
            let b = (gb & 0x0F) * 0x11;
            let g = (gb >> 4) * 0x11;
            let r = (ar & 0x0F) * 0x11;
            let a = (ar >> 4) * 0x11;
            [r, g, b, a]
        })
        .collect::<Vec<_>>();

    let buffer = ImageBuffer::from_raw(width.into(), height.into(), data)
        .context("failed to build image buffer")?;
    Ok(DynamicImage::ImageRgba8(buffer))
}

fn read_bgr5a1(width: u16, height: u16, data: &[u8]) -> Result<DynamicImage> {
    let data = data
        .iter()
        .tuples()
        .flat_map(|(b, a)| {
            let pixel = u16::from(*b) | (u16::from(*a) << 8);
            let r = (pixel & 0x7C00) >> 7;
            let g = (pixel & 0x03E0) >> 2;
            let b = (pixel & 0x001F) << 3;
            let a = ((pixel & 0x8000) >> 15) * 0xFF;
            [r, g, b, a]
        })
        .map(|value| u8::try_from(value).unwrap())
        .collect::<Vec<_>>();

    let buffer = ImageBuffer::from_raw(width.into(), height.into(), data)
        .context("failed to build image buffer")?;
    Ok(DynamicImage::ImageRgba8(buffer))
}

fn read_bgra8(width: u16, height: u16, data: &[u8]) -> Result<DynamicImage> {
    // TODO: seems really wasteful to copy the entire image in memory just to reassign the channels. think of a better way to do this.
    // TODO: use array_chunks once it hits stable
    let data = data
        .iter()
        .tuples()
        .flat_map(|(b, g, r, a)| [r, g, b, a])
        .copied()
        .collect::<Vec<_>>();

    let buffer = ImageBuffer::from_raw(width.into(), height.into(), data)
        .context("failed to build image buffer")?;
    Ok(DynamicImage::ImageRgba8(buffer))
}

fn read_texture_bc(
    texture: &tex::Texture,
    level: u8,
    image_format: image_dds::ImageFormat,
) -> Result<DynamicImage> {
    let surface = Surface {
        width: texture.width().into(),
        height: texture.height().into(),
        depth: texture.depth().into(),
        layers: match texture.kind() {
            tex::TextureKind::Cube => 6,
            tex::TextureKind::D2Array => texture.array_size().into(),
            _other => 1,
        },
        mipmaps: texture.mip_levels().into(),
        image_format,
        data: texture.data(),
    };

    let image = surface
        .decode_rgba8()
        .with_context(|| format!("failed to decode {image_format:?}"))?
        .to_image(level.into())
        .with_context(|| format!("failed to build image from mipmap level {level}"))?;

    Ok(image.into())
}

pub fn write(image: impl Into<DynamicImage>, format: ImageFormat) -> Result<Vec<u8>> {
    fn inner(mut image: DynamicImage, format: ImageFormat) -> Result<Vec<u8>> {
        // JPEG encoder errors out on anything with an alpha channel.
        if format == ImageFormat::Jpeg {
            image = match image {
                image @ (DynamicImage::ImageLumaA8(..) | DynamicImage::ImageLuma16(..)) => {
                    image.into_luma8().into()
                }
                other => other.into_rgb8().into(),
            }
        }

        // TODO: are there any non-failure cases here?
        let mut bytes = Cursor::new(vec![]);
        image
            .write_to(&mut bytes, format)
            .context("failed to write output buffer")?;

        Ok(bytes.into_inner())
    }

    inner(image.into(), format)
}
