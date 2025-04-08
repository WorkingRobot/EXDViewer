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

    if !matches!(texture.kind(), tex::TextureKind::D2) {
        anyhow::bail!(
            "unsupported texture dimension {:?} for path {path}",
            texture.kind()
        );
    }

    let buffer = match texture.format() {
        tex::Format::A8Unorm => read_a8(texture)?,

        tex::Format::Bgra4Unorm => read_bgra4(texture)?,
        tex::Format::Bgr5a1Unorm => read_bgr5a1(texture)?,
        tex::Format::Bgra8Unorm => read_bgra8(texture)?,

        tex::Format::Bc1Unorm => read_texture_bc(texture, image_dds::ImageFormat::BC1RgbaUnorm)?,
        tex::Format::Bc2Unorm => read_texture_bc(texture, image_dds::ImageFormat::BC2RgbaUnorm)?,
        tex::Format::Bc3Unorm => read_texture_bc(texture, image_dds::ImageFormat::BC3RgbaUnorm)?,
        tex::Format::Bc4Unorm => read_texture_bc(texture, image_dds::ImageFormat::BC4RUnorm)?,
        tex::Format::Bc5Unorm => read_texture_bc(texture, image_dds::ImageFormat::BC5RgUnorm)?,
        tex::Format::Bc6hFloat => read_texture_bc(texture, image_dds::ImageFormat::BC6hRgbSfloat)?,
        tex::Format::Bc7Unorm => read_texture_bc(texture, image_dds::ImageFormat::BC7RgbaUnorm)?,

        other => {
            anyhow::bail!("unsupported texture format {:?} for path {path}", other);
        }
    };

    Ok(buffer)
}

fn read_a8(texture: tex::Texture) -> Result<DynamicImage> {
    let buffer = ImageBuffer::from_raw(
        texture.width().into(),
        texture.height().into(),
        texture.data().to_owned(),
    )
    .context("failed to build image buffer")?;
    Ok(DynamicImage::ImageLuma8(buffer))
}

fn read_bgra4(texture: tex::Texture) -> Result<DynamicImage> {
    let data = texture
        .data()
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

    let buffer = ImageBuffer::from_raw(texture.width().into(), texture.height().into(), data)
        .context("failed to build image buffer")?;
    Ok(DynamicImage::ImageRgba8(buffer))
}

fn read_bgr5a1(texture: tex::Texture) -> Result<DynamicImage> {
    let data = texture
        .data()
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

    let buffer = ImageBuffer::from_raw(texture.width().into(), texture.height().into(), data)
        .context("failed to build image buffer")?;
    Ok(DynamicImage::ImageRgba8(buffer))
}

fn read_bgra8(texture: tex::Texture) -> Result<DynamicImage> {
    // TODO: seems really wasteful to copy the entire image in memory just to reassign the channels. think of a better way to do this.
    // TODO: use array_chunks once it hits stable
    let data = texture
        .data()
        .iter()
        .tuples()
        .flat_map(|(b, g, r, a)| [r, g, b, a])
        .copied()
        .collect::<Vec<_>>();

    let buffer = ImageBuffer::from_raw(texture.width().into(), texture.height().into(), data)
        .context("failed to build image buffer")?;
    Ok(DynamicImage::ImageRgba8(buffer))
}

fn read_texture_bc(
    texture: tex::Texture,
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
        .to_image(0)
        .context("failed to build image from buffer")?;

    Ok(image.into())
}

pub fn write(image: impl Into<DynamicImage>, format: ImageFormat) -> Result<Vec<u8>> {
    fn inner(mut image: DynamicImage, format: ImageFormat) -> Result<Vec<u8>> {
        // JPEG encoder errors out on anything with an alpha channel.
        if format == ImageFormat::Jpeg {
            image = match image {
                image @ DynamicImage::ImageLumaA8(..) | image @ DynamicImage::ImageLuma16(..) => {
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
