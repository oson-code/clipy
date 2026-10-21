//! PNG encoding, thumbnails and colour swatches for history images.

use std::io::Cursor;

use anyhow::{Context, Result};
use image::{imageops::FilterType, DynamicImage, ImageFormat, RgbaImage};

/// Largest side of the thumbnail stored with each image; the menu scales it down further to
/// the configured preview size (at most 512 px).
pub const STORED_THUMBNAIL: u32 = 512;

pub fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let img = rgba_image(rgba, width, height)?;
    to_png(&DynamicImage::ImageRgba8(img))
}

pub fn decode_png(png: &[u8]) -> Result<(Vec<u8>, u32, u32)> {
    let img = image::load_from_memory_with_format(png, ImageFormat::Png)
        .context("decoding PNG")?
        .into_rgba8();
    let (w, h) = img.dimensions();
    Ok((img.into_raw(), w, h))
}

/// A PNG of `rgba` shrunk to fit `max`×`max`; small images are kept as they are.
pub fn thumbnail_png(rgba: &[u8], width: u32, height: u32, max: u32) -> Result<Vec<u8>> {
    let img = DynamicImage::ImageRgba8(rgba_image(rgba, width, height)?);
    let img = if width > max || height > max {
        img.resize(max, max, FilterType::Triangle)
    } else {
        img
    };
    to_png(&img)
}

/// Decodes `png` and scales it to fit `max_w`×`max_h` keeping its aspect ratio. Images are never
/// enlarged. Returns RGBA pixels and the final size.
pub fn fit_png(png: &[u8], max_w: u32, max_h: u32) -> Result<(Vec<u8>, u32, u32)> {
    let img = image::load_from_memory_with_format(png, ImageFormat::Png).context("decoding PNG")?;
    let img = if img.width() > max_w || img.height() > max_h {
        img.resize(max_w.max(1), max_h.max(1), FilterType::Triangle)
    } else {
        img
    };
    let img = img.into_rgba8();
    let (w, h) = img.dimensions();
    Ok((img.into_raw(), w, h))
}

/// A square of solid colour, used as the preview of colour codes such as `#FF8800`.
pub fn swatch(color: [u8; 4], size: u32) -> (Vec<u8>, u32, u32) {
    let size = size.max(1);
    (color.repeat((size * size) as usize), size, size)
}

fn rgba_image(rgba: &[u8], width: u32, height: u32) -> Result<RgbaImage> {
    RgbaImage::from_raw(width, height, rgba.to_vec())
        .with_context(|| format!("image buffer does not match {width}x{height}"))
}

fn to_png(img: &DynamicImage) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .context("encoding PNG")?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(w: u32, h: u32) -> Vec<u8> {
        (0..w * h)
            .flat_map(|i| {
                if i % 2 == 0 {
                    [255, 0, 0, 255]
                } else {
                    [0, 0, 255, 128]
                }
            })
            .collect()
    }

    #[test]
    fn png_round_trip_is_lossless() {
        let rgba = checker(7, 5);
        let png = encode_png(&rgba, 7, 5).unwrap();
        assert_eq!(decode_png(&png).unwrap(), (rgba, 7, 5));
    }

    #[test]
    fn thumbnail_shrinks_large_images_only() {
        let big = checker(1024, 256);
        let (_, w, h) = decode_png(&thumbnail_png(&big, 1024, 256, 512).unwrap()).unwrap();
        assert_eq!((w, h), (512, 128));
        let small = checker(10, 10);
        let (_, w, h) = decode_png(&thumbnail_png(&small, 10, 10, 512).unwrap()).unwrap();
        assert_eq!((w, h), (10, 10));
    }

    #[test]
    fn fit_keeps_aspect_ratio() {
        let png = encode_png(&checker(400, 100), 400, 100).unwrap();
        let (_, w, h) = fit_png(&png, 100, 32).unwrap();
        assert_eq!((w, h), (100, 25));
    }

    #[test]
    fn swatch_is_solid() {
        let (rgba, w, h) = swatch([1, 2, 3, 4], 2);
        assert_eq!((w, h), (2, 2));
        assert_eq!(rgba, [1, 2, 3, 4].repeat(4));
    }

    #[test]
    fn mismatched_buffer_is_an_error() {
        assert!(encode_png(&[0; 3], 1, 1).is_err());
    }
}
