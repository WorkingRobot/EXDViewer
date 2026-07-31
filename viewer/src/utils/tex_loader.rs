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

/// Decode a `.tex` straight from its bytes, for callers holding a file rather than an
/// [`Ironworks`].
pub fn decode_bytes(bytes: &[u8], path: &str) -> Result<DynamicImage> {
    let texture = <tex::Texture as ironworks::file::File>::read(Cursor::new(bytes.to_vec()))?;
    decode(texture, path)
}

/// Decode the smallest mipmap that still covers `max_dim`, for previews that will be drawn far below
/// full size; `None` decodes at full size. Uploading mip 0 for a thumbnail costs megabytes of
/// texture memory per file.
///
/// The texture's full-resolution size comes back alongside the image. Anything indexing into a
/// texture -- a `.uld` part rectangle, say -- is expressed in that space rather than the decoded
/// mipmap's, so a caller cropping the result needs both.
pub fn decode_preview_sized(
    bytes: &[u8],
    path: &str,
    max_dim: Option<u16>,
) -> Result<(DynamicImage, [u16; 2])> {
    let texture = <tex::Texture as ironworks::file::File>::read(Cursor::new(bytes.to_vec()))?;
    let level = max_dim
        .and_then(|max_dim| {
            (0..texture.mip_levels())
                .take_while(|level| {
                    let (width, height) = texture.mip_size(*level);
                    width.max(height) >= max_dim
                })
                .last()
        })
        .unwrap_or(0);
    let size = [texture.width(), texture.height()];
    Ok((decode_mip(&texture, level, path)?, size))
}

/// Decode an already-read texture. The web backend hands out bytes rather than an
/// [`Ironworks`], so it comes in this way instead.
pub fn decode(texture: tex::Texture, path: &str) -> Result<DynamicImage> {
    decode_mip(&texture, 0, path)
}

/// Decode one mipmap level. A volume texture comes back with its slices stacked vertically; picking
/// one is a matter of which band the caller draws, not of decoding again.
pub fn decode_mip(texture: &tex::Texture, level: u8, path: &str) -> Result<DynamicImage> {
    if matches!(
        texture.kind(),
        tex::TextureKind::Unknown | tex::TextureKind::D1
    ) {
        anyhow::bail!(
            "unsupported texture dimension {:?} for path {path}",
            texture.kind()
        );
    }

    // Block-compressed formats keep their whole mip chain in one surface, so image_dds picks the
    // level out itself. The uncompressed ones are decoded from that level's slice of the data.
    let bc = |image_format| read_texture_bc(texture, level, image_format);
    // Volume slices, cube faces and array elements are all stored one after another, so they decode
    // as a single tall image and the caller picks which band to show.
    let (width, slice_height) = texture.mip_size(level);
    let height = slice_height.saturating_mul(texture.layers());
    let plain = || {
        texture
            .mip_data(level)
            .with_context(|| format!("texture {path} has no mipmap level {level}"))
    };

    let buffer = match texture.format() {
        tex::Format::A8Unorm | tex::Format::L8Unorm => read_gray8(width, height, plain()?)?,
        tex::Format::R8Unorm | tex::Format::R8Uint => read_channels8(width, height, plain()?, 1)?,
        tex::Format::Rg8Unorm => read_channels8(width, height, plain()?, 2)?,
        tex::Format::Bgrx8Unorm => read_bgrx8(width, height, plain()?)?,

        tex::Format::R16Unorm => read_unorm16(width, height, plain()?, 1)?,
        tex::Format::Rg16Unorm => read_unorm16(width, height, plain()?, 2)?,

        tex::Format::R16Float => read_half(width, height, plain()?, 1)?,
        tex::Format::Rg16Float => read_half(width, height, plain()?, 2)?,
        tex::Format::Rgba16Float => read_half(width, height, plain()?, 4)?,
        tex::Format::R32Float => read_float(width, height, plain()?, 1)?,
        tex::Format::Rg32Float => read_float(width, height, plain()?, 2)?,
        tex::Format::Rgba32Float => read_float(width, height, plain()?, 4)?,

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

fn read_gray8(width: u16, height: u16, data: &[u8]) -> Result<DynamicImage> {
    let buffer = ImageBuffer::from_raw(width.into(), height.into(), data.to_owned())
        .context("failed to build image buffer")?;
    Ok(DynamicImage::ImageLuma8(buffer))
}

/// Widen an 8-bit-per-channel image to RGBA. One channel shows as gray rather than red, since these
/// are masks and lookups far more often than they are color.
fn read_channels8(width: u16, height: u16, data: &[u8], channels: usize) -> Result<DynamicImage> {
    let pixels = data
        .chunks_exact(channels)
        .flat_map(|texel| match channels {
            1 => [texel[0], texel[0], texel[0], u8::MAX],
            _ => [texel[0], texel[1], 0, u8::MAX],
        })
        .collect::<Vec<_>>();
    let buffer = ImageBuffer::from_raw(width.into(), height.into(), pixels)
        .context("failed to build image buffer")?;
    Ok(DynamicImage::ImageRgba8(buffer))
}

/// BGRX: the fourth byte is padding rather than alpha, so the result is opaque.
fn read_bgrx8(width: u16, height: u16, data: &[u8]) -> Result<DynamicImage> {
    let pixels = data
        .chunks_exact(4)
        .flat_map(|texel| [texel[2], texel[1], texel[0], u8::MAX])
        .collect::<Vec<_>>();
    let buffer = ImageBuffer::from_raw(width.into(), height.into(), pixels)
        .context("failed to build image buffer")?;
    Ok(DynamicImage::ImageRgba8(buffer))
}

fn read_unorm16(width: u16, height: u16, data: &[u8], channels: usize) -> Result<DynamicImage> {
    let values = |texel: &[u8]| -> Vec<u8> {
        (0..channels)
            .map(|i| (u16::from_le_bytes([texel[i * 2], texel[i * 2 + 1]]) >> 8) as u8)
            .collect()
    };
    to_rgba(width, height, data, channels * 2, channels, values)
}

/// Half and single precision are scene values rather than colors, so they are clamped into the
/// unit range instead of being scaled by whatever the maximum in the image happens to be.
fn read_half(width: u16, height: u16, data: &[u8], channels: usize) -> Result<DynamicImage> {
    let values = |texel: &[u8]| -> Vec<u8> {
        (0..channels)
            .map(|i| {
                let bits = u16::from_le_bytes([texel[i * 2], texel[i * 2 + 1]]);
                to_u8(f32::from(half::f16::from_bits(bits)))
            })
            .collect()
    };
    to_rgba(width, height, data, channels * 2, channels, values)
}

fn read_float(width: u16, height: u16, data: &[u8], channels: usize) -> Result<DynamicImage> {
    let values = |texel: &[u8]| -> Vec<u8> {
        (0..channels)
            .map(|i| {
                let bytes = [
                    texel[i * 4],
                    texel[i * 4 + 1],
                    texel[i * 4 + 2],
                    texel[i * 4 + 3],
                ];
                to_u8(f32::from_le_bytes(bytes))
            })
            .collect()
    };
    to_rgba(width, height, data, channels * 4, channels, values)
}

fn to_u8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0) as u8
}

/// Lay decoded channels out as RGBA: one channel grays, two fill red and green, four map straight.
fn to_rgba(
    width: u16,
    height: u16,
    data: &[u8],
    stride: usize,
    channels: usize,
    values: impl Fn(&[u8]) -> Vec<u8>,
) -> Result<DynamicImage> {
    let pixels = data
        .chunks_exact(stride)
        .flat_map(|texel| {
            let v = values(texel);
            match channels {
                1 => [v[0], v[0], v[0], u8::MAX],
                2 => [v[0], v[1], 0, u8::MAX],
                _ => [v[0], v[1], v[2], v[3]],
            }
        })
        .collect::<Vec<_>>();
    let buffer = ImageBuffer::from_raw(width.into(), height.into(), pixels)
        .context("failed to build image buffer")?;
    Ok(DynamicImage::ImageRgba8(buffer))
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
        // image_dds wants a flat surface, and the game stores a volume's slices stacked, so the
        // depth is folded into the height rather than declared.
        height: u32::from(texture.height()) * u32::from(texture.layers()),
        depth: 1,
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
