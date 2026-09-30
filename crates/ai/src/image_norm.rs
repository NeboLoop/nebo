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

/// An image's width and height in pixels, read from its header without
/// decoding it. None when the bytes aren't an image this crate can read.
pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// An image ready for the vision helper, with its size as it was found.
#[derive(Debug, Clone)]
pub struct Picture {
    pub image: crate::types::ImageContent,
    pub width: u32,
    pub height: u32,
}

impl Picture {
    /// The picture in `bytes`: sent as it is when a provider takes it as it
    /// is, else resized and re-encoded ([`normalize_for_llm`]). None when
    /// the bytes aren't an image.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let (width, height) = dimensions(bytes)?;
        let as_is = crate::types::sniff_image_mime(bytes).filter(|_| base64_len(bytes.len()) <= MAX_BASE64);
        let (media_type, data) = match as_is {
            Some(mime) => (mime.to_string(), base64::engine::general_purpose::STANDARD.encode(bytes)),
            None => normalize_for_llm(bytes)?,
        };
        Some(Self { image: crate::types::ImageContent { media_type, data }, width, height })
    }

    /// The picture a tool result points at: a `data:` URI or a local file.
    /// None for anything else: a web URL, or a file that isn't an image (the
    /// same channel carries a deck or a PDF a tool produced).
    pub fn load(raw: &str) -> Option<Self> {
        if let Some(rest) = raw.strip_prefix("data:") {
            let (_, data) = rest.split_once(',')?;
            return Self::from_bytes(&base64::engine::general_purpose::STANDARD.decode(data).ok()?);
        }
        if raw.starts_with("http://") || raw.starts_with("https://") {
            return None;
        }
        Self::from_bytes(&std::fs::read(raw).ok()?)
    }

    /// The picture as it arrived with the owner's message (already sized
    /// for a provider when the message was built).
    pub fn attached(image: &crate::types::ImageContent) -> Option<Self> {
        let bytes = base64::engine::general_purpose::STANDARD.decode(&image.data).ok()?;
        let (width, height) = dimensions(&bytes)?;
        Some(Self { image: image.clone(), width, height })
    }
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
    fn dimensions_are_read_from_the_header() {
        assert_eq!(dimensions(&png_bytes(4000, 3000, 255)), Some((4000, 3000)));
        assert_eq!(dimensions(b"definitely not an image"), None);
    }

    /// A picture from a data URI or a file keeps the size it was found at;
    /// one too large to send as it is is resized; anything that isn't an
    /// image (a deck on the same channel) is none.
    #[test]
    fn a_picture_loads_from_a_data_uri_or_a_file_and_nothing_else_does() {
        let bytes = png_bytes(640, 480, 255);
        let uri = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&bytes));
        let p = Picture::load(&uri).expect("data uri");
        assert_eq!((p.width, p.height, p.image.media_type.as_str()), (640, 480, "image/png"));

        let path = std::env::temp_dir().join(format!("nebo-picture-{}.png", std::process::id()));
        std::fs::write(&path, &bytes).unwrap();
        assert!(Picture::load(path.to_str().unwrap()).is_some());
        std::fs::write(&path, b"PK\x03\x04 a deck, not a picture").unwrap();
        assert!(Picture::load(path.to_str().unwrap()).is_none());
        let _ = std::fs::remove_file(&path);
        assert!(Picture::load("https://example.com/a.png").is_none());
    }

    #[test]
    fn a_picture_too_large_to_send_as_it_is_is_resized_and_keeps_its_size() {
        // Noise does not compress: a 3000×3000 PNG of it is far past 5 MB.
        let mut state = 7u32;
        let img = image::RgbImage::from_fn(3000, 3000, |_, _| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            image::Rgb([(state >> 16) as u8, (state >> 8) as u8, state as u8])
        });
        let mut big = Vec::new();
        image::DynamicImage::ImageRgb8(img).write_to(&mut Cursor::new(&mut big), image::ImageFormat::Png).unwrap();
        assert!(base64_len(big.len()) > MAX_BASE64);
        let p = Picture::from_bytes(&big).expect("resized");
        assert_eq!((p.width, p.height), (3000, 3000), "the size as found");
        assert!(p.image.data.len() <= MAX_BASE64);
        assert!(decode(&p.image.data).width() <= MAX_EDGE);
    }

    #[test]
    fn non_image_bytes_return_none() {
        assert!(normalize_for_llm(b"definitely not an image").is_none());
        assert!(normalize_for_llm(&[]).is_none());
    }
}
