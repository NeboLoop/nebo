//! Image normalization — the ONE gate every user-supplied image passes
//! through before a provider ever sees it. Whatever arrives (any size, any
//! decodable format), what leaves is a right-sized, provider-friendly image:
//! oversized inputs are resized instead of silently demoted to disk paths,
//! and everything is re-encoded to a canonical format so provider format
//! quirks can't surface per-upload.

use base64::Engine;
use std::io::Cursor;

/// Longest edge providers actually use. Anthropic downsamples past ~1568px,
/// so pixels beyond this are pure token waste.
const MAX_EDGE: u32 = 1568;

/// Anthropic's per-image base64 ceiling — the strictest we ship against.
const MAX_BASE64: usize = 5 * 1024 * 1024;

/// JPEG quality ladder: start high, step down only if the encode is still
/// over the ceiling (effectively unreachable at MAX_EDGE, but images with
/// pathological noise exist).
const JPEG_QUALITIES: [u8; 3] = [85, 70, 50];

/// Normalize raw image bytes for LLM consumption. Returns the media type and
/// BASE64-ENCODED payload ready for an `ImageContent`, or None when the bytes
/// aren't a decodable image (callers keep their existing save-to-disk path —
/// that branch is for genuinely non-image files, no longer for big ones).
///
/// Canonical outputs: PNG when the image carries meaningful alpha (JPEG would
/// flatten it onto an arbitrary background), JPEG otherwise. Anything already
/// small AND canonical passes through untouched — no generational quality
/// loss on repeat sends.
pub fn normalize_for_llm(bytes: &[u8]) -> Option<(String, String)> {
    let sniffed = crate::types::sniff_image_mime(bytes);

    // Fast path: already canonical, already within provider limits.
    if let Some(mime @ ("image/jpeg" | "image/png")) = sniffed {
        if base64_len(bytes.len()) <= MAX_BASE64 {
            if let Ok(reader) =
                image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()
            {
                if let Ok((w, h)) = reader.into_dimensions() {
                    if w.max(h) <= MAX_EDGE {
                        let data =
                            base64::engine::general_purpose::STANDARD.encode(bytes);
                        return Some((mime.to_string(), data));
                    }
                }
            }
        }
    }

    let img = image::load_from_memory(bytes).ok()?;
    let (w, h) = (img.width(), img.height());
    let img = if w.max(h) > MAX_EDGE {
        img.resize(MAX_EDGE, MAX_EDGE, image::imageops::FilterType::Triangle)
    } else {
        img
    };

    let has_alpha = img.color().has_alpha() && image_uses_alpha(&img);
    if has_alpha {
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .ok()?;
        if base64_len(out.len()) <= MAX_BASE64 {
            let data = base64::engine::general_purpose::STANDARD.encode(&out);
            return Some(("image/png".to_string(), data));
        }
        // Alpha image too large even resized — flatten to JPEG below rather
        // than fail; a visible image beats a perfect one that never arrives.
    }

    let rgb = img.to_rgb8();
    for q in JPEG_QUALITIES {
        let mut out = Vec::new();
        let mut cursor = Cursor::new(&mut out);
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, q);
        if rgb.write_with_encoder(encoder).is_err() {
            return None;
        }
        if base64_len(out.len()) <= MAX_BASE64 {
            let data = base64::engine::general_purpose::STANDARD.encode(&out);
            return Some(("image/jpeg".to_string(), data));
        }
    }
    None
}

/// True when any pixel is actually transparent — images with an alpha channel
/// that is fully opaque are photos in disguise and should take the JPEG path.
fn image_uses_alpha(img: &image::DynamicImage) -> bool {
    let rgba = img.to_rgba8();
    rgba.pixels().any(|p| p.0[3] != u8::MAX)
}

fn base64_len(raw: usize) -> usize {
    raw.div_ceil(3) * 4
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn png_bytes(w: u32, h: u32, alpha: u8) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([120, 40, 200, alpha]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn decode(data: &str) -> image::DynamicImage {
        let bytes = base64::engine::general_purpose::STANDARD.decode(data).unwrap();
        image::load_from_memory(&bytes).unwrap()
    }

    #[test]
    fn oversized_image_is_resized_not_rejected() {
        let big = png_bytes(4000, 3000, 255);
        let (mime, data) = normalize_for_llm(&big).expect("oversized image must normalize");
        assert_eq!(mime, "image/jpeg");
        let img = decode(&data);
        assert!(img.width().max(img.height()) <= MAX_EDGE);
        assert!(data.len() <= MAX_BASE64);
    }

    #[test]
    fn small_canonical_image_passes_through_untouched() {
        let small = png_bytes(200, 100, 255);
        let (mime, data) = normalize_for_llm(&small).expect("small png must pass");
        assert_eq!(mime, "image/png");
        let round = base64::engine::general_purpose::STANDARD.decode(&data).unwrap();
        assert_eq!(round, small, "no re-encode for already-canonical input");
    }

    #[test]
    fn transparency_survives_as_png() {
        let translucent = png_bytes(2500, 400, 128);
        let (mime, data) = normalize_for_llm(&translucent).expect("alpha image must normalize");
        assert_eq!(mime, "image/png", "alpha must not be flattened to jpeg");
        let img = decode(&data);
        assert!(img.width().max(img.height()) <= MAX_EDGE);
        assert!(image_uses_alpha(&img));
    }

    #[test]
    fn opaque_alpha_channel_takes_jpeg_path() {
        let opaque = png_bytes(3000, 3000, 255);
        let (mime, _) = normalize_for_llm(&opaque).unwrap();
        assert_eq!(mime, "image/jpeg");
    }

    #[test]
    fn non_image_bytes_return_none() {
        assert!(normalize_for_llm(b"definitely not an image").is_none());
        assert!(normalize_for_llm(&[]).is_none());
    }
}
