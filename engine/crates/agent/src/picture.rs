//! Shrinking an attached picture: endpoints cap the size of a request, and a pasted
//! screenshot as base64 easily passes it.

use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat, ImageReader, Rgb, RgbImage};

use crate::params::Picture;
use crate::request::Image;

/// Smallest side a picture is shrunk to while it is still over the byte budget.
const FLOOR: u32 = 128;

/// The picture scaled to `params.max_side` and, when `max_bytes` is set, re-encoded
/// until its base64 text fits. A picture already small enough is returned as it came;
/// one that cannot be read is too (the endpoint will say what it makes of it).
pub fn prepare(image: Image, params: &Picture, max_bytes: Option<u32>) -> Image {
    let budget = max_bytes.map_or(usize::MAX, |n| n as usize);
    let Some(bytes) = api::media::base64_decode(&image.data_base64) else {
        return image;
    };
    let Some(decoded) = decode(&bytes) else {
        return image;
    };
    let side = decoded.width().max(decoded.height());
    if side <= params.max_side && image.data_base64.len() <= budget {
        return image;
    }
    let flat = flatten(&decoded);
    let mut side = side.min(params.max_side.max(FLOOR));
    let mut quality = params.quality.clamp(1, 100);
    loop {
        let scaled = if side < decoded.width().max(decoded.height()) {
            DynamicImage::ImageRgb8(flat.clone())
                .resize(side, side, FilterType::Triangle)
                .to_rgb8()
        } else {
            flat.clone()
        };
        let text = base64_encode(&jpeg(&scaled, quality));
        if text.len() <= budget || side <= FLOOR {
            return Image {
                mime: "image/jpeg".into(),
                data_base64: text,
            };
        }
        // Quality first, then size: a screenshot's text survives a lower quality longer
        // than it survives fewer pixels.
        if quality > 40 {
            quality -= 20;
        } else {
            side = (side * 3 / 4).max(FLOOR);
        }
    }
}

fn decode(bytes: &[u8]) -> Option<DynamicImage> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    // A decompression bomb is a reason to send the picture untouched, not to allocate.
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(256 << 20);
    reader.limits(limits);
    match reader.format()? {
        ImageFormat::Png | ImageFormat::Jpeg => reader.decode().ok(),
        _ => None,
    }
}

/// Transparent pixels become white: JPEG has no alpha, and black is unreadable.
fn flatten(img: &DynamicImage) -> RgbImage {
    let rgba = img.to_rgba8();
    let mut out = RgbImage::new(rgba.width(), rgba.height());
    for (x, y, p) in rgba.enumerate_pixels() {
        let a = u32::from(p[3]);
        let mix = |c: u8| ((u32::from(c) * a + 255 * (255 - a)) / 255) as u8;
        out.put_pixel(x, y, Rgb([mix(p[0]), mix(p[1]), mix(p[2])]));
    }
    out
}

fn jpeg(img: &RgbImage, quality: u8) -> Vec<u8> {
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, quality)
        .encode_image(img)
        .expect("an RGB image encodes to memory");
    out
}

fn base64_encode(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A noisy picture: compresses badly, like a photo or a busy screenshot.
    fn noisy(w: u32, h: u32) -> Image {
        let mut seed = 7u32;
        let img = RgbImage::from_fn(w, h, |_, _| {
            let mut next = || {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed >> 24) as u8
            };
            Rgb([next(), next(), next()])
        });
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        Image {
            mime: "image/png".into(),
            data_base64: base64_encode(&png),
        }
    }

    fn size(image: &Image) -> (u32, u32) {
        let bytes = api::media::base64_decode(&image.data_base64).unwrap();
        let img = decode(&bytes).unwrap();
        (img.width(), img.height())
    }

    #[test]
    fn base64_round_trips() {
        for n in 0..8 {
            let bytes: Vec<u8> = (0..n).map(|i| i * 37).collect();
            assert_eq!(
                api::media::base64_decode(&base64_encode(&bytes)).unwrap(),
                bytes
            );
        }
    }

    #[test]
    fn small_pictures_are_untouched() {
        let img = noisy(16, 16);
        assert_eq!(prepare(img.clone(), &Picture::default(), None), img);
    }

    #[test]
    fn large_pictures_are_scaled_to_the_longest_side() {
        let out = prepare(noisy(1400, 700), &Picture::default(), None);
        assert_eq!(out.mime, "image/jpeg");
        assert_eq!(size(&out), (1024, 512));
    }

    #[test]
    fn the_byte_budget_is_kept() {
        let img = noisy(1200, 900);
        assert!(img.data_base64.len() > 128_000);
        let out = prepare(img, &Picture::default(), Some(60_000));
        assert!(out.data_base64.len() <= 60_000, "{}", out.data_base64.len());
        assert_eq!(out.mime, "image/jpeg");
    }

    #[test]
    fn unreadable_pictures_pass_through() {
        let img = Image {
            mime: "image/png".into(),
            data_base64: base64_encode(b"not a picture"),
        };
        assert_eq!(prepare(img.clone(), &Picture::default(), Some(10)), img);
    }
}
