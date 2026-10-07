//! The AI-generated tag on made images and video, in each file's own
//! comment metadata, as made audio carries it (`audio::tag_ai_generated`):
//! a PNG's `tEXt` Comment, a JPEG's COM segment, an MP4's `udta/©cmt`
//! (ffprobe's `comment`, which the Nebo Media plugin's probe reads).

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use super::audio::AI_TAG;

/// `bytes` with the AI-generated comment written into the image's own
/// metadata: a PNG `tEXt` chunk (keyword `Comment`) after its header, or a
/// JPEG COM segment after its APPn segments. Any other format, or bytes
/// that don't read as one, are returned untouched, with `false`.
pub fn image(bytes: Vec<u8>) -> (Vec<u8>, bool) {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return png(bytes);
    }
    if bytes.starts_with(&[0xff, 0xd8]) {
        return jpeg(bytes);
    }
    (bytes, false)
}

fn png(mut bytes: Vec<u8>) -> (Vec<u8>, bool) {
    // The signature, then IHDR (length 13): the text chunk goes after it.
    const IHDR_END: usize = 8 + 4 + 4 + 13 + 4;
    if bytes.len() < IHDR_END || &bytes[12..16] != b"IHDR" || bytes[8..12] != 13u32.to_be_bytes() {
        return (bytes, false);
    }
    let mut data = b"Comment\0".to_vec();
    data.extend_from_slice(AI_TAG.as_bytes());
    let mut chunk = (data.len() as u32).to_be_bytes().to_vec();
    chunk.extend_from_slice(b"tEXt");
    chunk.extend_from_slice(&data);
    let crc = crc32fast::hash(&chunk[4..]);
    chunk.extend_from_slice(&crc.to_be_bytes());
    bytes.splice(IHDR_END..IHDR_END, chunk);
    (bytes, true)
}

fn jpeg(mut bytes: Vec<u8>) -> (Vec<u8>, bool) {
    // After SOI and any APPn segments (JFIF and Exif must come first).
    let mut at = 2;
    while at + 4 <= bytes.len() && bytes[at] == 0xff && (0xe0..=0xef).contains(&bytes[at + 1]) {
        let len = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        if len < 2 || at + 2 + len > bytes.len() {
            return (bytes, false);
        }
        at += 2 + len;
    }
    if at + 2 > bytes.len() || bytes[at] != 0xff {
        return (bytes, false);
    }
    let mut com = vec![0xff, 0xfe];
    com.extend_from_slice(&((AI_TAG.len() + 2) as u16).to_be_bytes());
    com.extend_from_slice(AI_TAG.as_bytes());
    bytes.splice(at..at, com);
    (bytes, true)
}

/// The largest `moov` read into memory: the index of a long film, never
/// its media.
const MAX_MOOV: u64 = 64 * 1024 * 1024;

/// Tags the MP4 at `path` AI-generated: a `©cmt` text atom in `moov/udta`
/// (an existing one replaced). The media data is never read into memory: a
/// `moov` at the end is rewritten in place; one before the media is
/// rewritten into a copy, its chunk offsets moved by what the tag added.
/// `false`, with the file untouched, when it does not read as a plain MP4
/// (a fragmented one included).
pub fn mp4(path: &Path) -> std::io::Result<bool> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let mut boxes = Vec::new();
    let mut at = 0u64;
    while at < len {
        let Some((kind, size)) = box_header(&mut file, at, len)? else {
            return Ok(false);
        };
        boxes.push((kind, at, size));
        at += size;
    }
    let moovs: Vec<_> = boxes.iter().filter(|b| &b.0 == b"moov").collect();
    if moovs.len() != 1 || boxes.iter().any(|b| &b.0 == b"moof") {
        return Ok(false);
    }
    let (_, moov_at, moov_size) = *moovs[0];
    if moov_size > MAX_MOOV {
        return Ok(false);
    }
    let mut moov = vec![0u8; moov_size as usize];
    file.seek(SeekFrom::Start(moov_at))?;
    file.read_exact(&mut moov)?;
    let Some(mut tagged) = tag_moov(&moov) else {
        return Ok(false);
    };
    let moov_end = moov_at + moov_size;
    let delta = tagged.len() as i64 - moov.len() as i64;
    if moov_end < len && !shift_offsets(&mut tagged, moov_end, delta) {
        return Ok(false);
    }
    drop(file);
    if moov_end == len {
        let mut file = std::fs::OpenOptions::new().write(true).open(path)?;
        file.set_len(moov_at)?;
        file.seek(SeekFrom::Start(moov_at))?;
        file.write_all(&tagged)?;
        file.sync_all()?;
        return Ok(true);
    }
    let part = path.with_extension("tag.part");
    let copy = || -> std::io::Result<()> {
        let mut from = std::fs::File::open(path)?;
        let mut to = std::fs::File::create(&part)?;
        std::io::copy(&mut (&mut from).take(moov_at), &mut to)?;
        to.write_all(&tagged)?;
        from.seek(SeekFrom::Start(moov_end))?;
        std::io::copy(&mut from, &mut to)?;
        to.sync_all()
    };
    if let Err(e) = copy().and_then(|()| std::fs::rename(&part, path)) {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    Ok(true)
}

/// The box at `at`: its type and whole size (a 64-bit size and a size of
/// 0, "to the end", read), or `None` when it doesn't fit in `len`.
fn box_header(file: &mut std::fs::File, at: u64, len: u64) -> std::io::Result<Option<([u8; 4], u64)>> {
    if at + 8 > len {
        return Ok(None);
    }
    let mut head = [0u8; 16];
    file.seek(SeekFrom::Start(at))?;
    file.read_exact(&mut head[..8])?;
    let kind: [u8; 4] = head[4..8].try_into().unwrap_or_default();
    let size = match u32::from_be_bytes(head[..4].try_into().unwrap_or_default()) {
        0 => len - at,
        1 => {
            if at + 16 > len {
                return Ok(None);
            }
            file.read_exact(&mut head[8..16])?;
            u64::from_be_bytes(head[8..16].try_into().unwrap_or_default())
        }
        n => n as u64,
    };
    Ok((size >= 8 && at + size <= len).then_some((kind, size)))
}

/// The children of a container's body: (type, start, end) of each, start
/// at the child's header. `None` when they don't tile the body.
fn children(body: &[u8]) -> Option<Vec<([u8; 4], usize, usize)>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < body.len() {
        if at + 8 > body.len() {
            return None;
        }
        let size = match u32::from_be_bytes(body[at..at + 4].try_into().ok()?) {
            0 => body.len() - at,
            1 => usize::try_from(u64::from_be_bytes(body.get(at + 8..at + 16)?.try_into().ok()?)).ok()?,
            n => n as usize,
        };
        if size < 8 || at + size > body.len() {
            return None;
        }
        out.push((body[at + 4..at + 8].try_into().ok()?, at, at + size));
        at += size;
    }
    Some(out)
}

/// A box of `kind` around `body`.
fn boxed(kind: &[u8; 4], body: &[u8]) -> Option<Vec<u8>> {
    let size = u32::try_from(body.len() + 8).ok()?;
    let mut out = size.to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    Some(out)
}

/// The header length of the box starting at `b`.
fn header_len(b: &[u8]) -> usize {
    if b[..4] == 1u32.to_be_bytes() { 16 } else { 8 }
}

/// `moov` with `udta/©cmt` saying AI-generated.
fn tag_moov(moov: &[u8]) -> Option<Vec<u8>> {
    const CMT: [u8; 4] = *b"\xa9cmt";
    // A QuickTime text atom: length, language ("und"), text.
    let mut text = (AI_TAG.len() as u16).to_be_bytes().to_vec();
    text.extend_from_slice(&0x55c4u16.to_be_bytes());
    text.extend_from_slice(AI_TAG.as_bytes());
    let cmt = boxed(&CMT, &text)?;

    let body = &moov[header_len(moov)..];
    let mut out = Vec::with_capacity(body.len() + cmt.len() + 8);
    let mut has_udta = false;
    for (kind, start, end) in children(body)? {
        if &kind == b"udta" {
            has_udta = true;
            let udta = &body[start..end];
            let inner = &udta[header_len(udta)..];
            let mut kept = Vec::with_capacity(inner.len() + cmt.len());
            for (k, s, e) in children(inner)? {
                if k != CMT {
                    kept.extend_from_slice(&inner[s..e]);
                }
            }
            kept.extend_from_slice(&cmt);
            out.extend(boxed(b"udta", &kept)?);
        } else {
            out.extend_from_slice(&body[start..end]);
        }
    }
    if !has_udta {
        out.extend(boxed(b"udta", &cmt)?);
    }
    boxed(b"moov", &out)
}

/// Moves every chunk offset at or past `from` by `delta`, in each track's
/// `stco` / `co64`. `false` when an offset no longer fits its field.
fn shift_offsets(moov: &mut [u8], from: u64, delta: i64) -> bool {
    fn walk(b: &mut [u8], from: u64, delta: i64) -> bool {
        let Some(kids) = children(b) else { return false };
        for (kind, start, end) in kids {
            let child = &mut b[start..end];
            let h = header_len(child);
            let ok = match &kind {
                b"trak" | b"mdia" | b"minf" | b"stbl" => walk(&mut child[h..], from, delta),
                b"stco" | b"co64" => {
                    let wide = &kind == b"co64";
                    let body = &mut child[h..];
                    let Some(count) = body.get(4..8).and_then(|c| c.try_into().ok()).map(u32::from_be_bytes) else {
                        return false;
                    };
                    let width = if wide { 8 } else { 4 };
                    if body.len() < 8 + count as usize * width {
                        return false;
                    }
                    for i in 0..count as usize {
                        let field = &mut body[8 + i * width..8 + (i + 1) * width];
                        let offset = if wide {
                            u64::from_be_bytes(field.try_into().unwrap_or_default())
                        } else {
                            u32::from_be_bytes(field.try_into().unwrap_or_default()) as u64
                        };
                        if offset < from {
                            continue;
                        }
                        let Some(moved) = offset.checked_add_signed(delta) else { return false };
                        if wide {
                            field.copy_from_slice(&moved.to_be_bytes());
                        } else {
                            let Ok(moved) = u32::try_from(moved) else { return false };
                            field.copy_from_slice(&moved.to_be_bytes());
                        }
                    }
                    true
                }
                _ => true,
            };
            if !ok {
                return false;
            }
        }
        true
    }
    let h = header_len(moov);
    walk(&mut moov[h..], from, delta)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(format: image::ImageFormat) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(4, 4, image::Rgb([200, 30, 40]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    /// A made PNG and JPEG carry the comment and still decode; bytes that
    /// are not one stay as they were.
    #[test]
    fn images_carry_the_ai_comment_and_still_decode() {
        for format in [image::ImageFormat::Png, image::ImageFormat::Jpeg] {
            let plain = encoded(format);
            let (tagged, ok) = image(plain.clone());
            assert!(ok, "{format:?}");
            assert_eq!(tagged.len(), plain.len() + if format == image::ImageFormat::Png { 32 } else { 16 });
            assert!(tagged.windows(AI_TAG.len()).any(|w| w == AI_TAG.as_bytes()));
            let back = image::load_from_memory_with_format(&tagged, format).unwrap();
            assert_eq!(back.to_rgb8().dimensions(), (4, 4));
        }
        let png = image(encoded(image::ImageFormat::Png)).0;
        // The tEXt chunk sits right after IHDR, with a correct CRC.
        assert_eq!(&png[37..41], b"tEXt");
        assert_eq!(&png[41..49], b"Comment\0");
        let crc = crc32fast::hash(&png[37..41 + 8 + AI_TAG.len()]);
        assert_eq!(png[41 + 8 + AI_TAG.len()..][..4], crc.to_be_bytes());
        for not_one in [b"\x89PNG\r\n\x1a\nfake".to_vec(), b"\xff\xd8\xff\xe0fake jpeg".to_vec(), b"RIFF....WEBP".to_vec()] {
            assert_eq!(image(not_one.clone()), (not_one, false));
        }
    }

    fn mp4_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        boxed(kind, body).unwrap()
    }

    /// A tiny MP4: `ftyp`, then `moov` (one track whose `stco` points at
    /// the media) and `mdat`, in `order`.
    fn tiny_mp4(moov_first: bool, udta: Option<&[u8]>) -> (Vec<u8>, Vec<u8>) {
        let media = b"MEDIA-BYTES".to_vec();
        let ftyp = mp4_box(b"ftyp", b"isom\0\0\0\0isom");
        let build = |offset: u32| {
            let mut stco_body = vec![0, 0, 0, 0];
            stco_body.extend_from_slice(&1u32.to_be_bytes());
            stco_body.extend_from_slice(&offset.to_be_bytes());
            let stbl = mp4_box(b"stbl", &mp4_box(b"stco", &stco_body));
            let trak = mp4_box(b"trak", &mp4_box(b"mdia", &mp4_box(b"minf", &stbl)));
            let mut body = mp4_box(b"mvhd", &[0u8; 20]);
            body.extend(trak);
            if let Some(u) = udta {
                body.extend(mp4_box(b"udta", u));
            }
            mp4_box(b"moov", &body)
        };
        let mdat = mp4_box(b"mdat", &media);
        let mut file = ftyp.clone();
        if moov_first {
            let moov_len = build(0).len();
            let offset = (ftyp.len() + moov_len + 8) as u32;
            file.extend(build(offset));
            file.extend(mdat);
        } else {
            let offset = (ftyp.len() + 8) as u32;
            file.extend(mdat);
            file.extend(build(offset));
        }
        (file, media)
    }

    /// The chunk offset the file's one `stco` holds.
    fn stco_offset(file: &[u8]) -> usize {
        let at = file.windows(4).position(|w| w == b"stco").unwrap();
        u32::from_be_bytes(file[at + 12..at + 16].try_into().unwrap()) as usize
    }

    /// Either layout gets the tag, and the track's chunk offset still lands
    /// on its media: moved by the tag's size when `moov` comes first.
    #[test]
    fn an_mp4_is_tagged_and_its_media_still_found() {
        let dir = tempfile::tempdir().unwrap();
        for moov_first in [true, false] {
            for udta in [None, Some(&b"\0\0\0\x0c\xa9cmtold!"[..])] {
                let (file, media) = tiny_mp4(moov_first, udta);
                let path = dir.path().join("clip.mp4");
                std::fs::write(&path, &file).unwrap();
                assert!(mp4(&path).unwrap(), "moov first {moov_first}, udta {udta:?}");
                let out = std::fs::read(&path).unwrap();
                let at = stco_offset(&out);
                assert_eq!(&out[at..at + media.len()], &media[..], "moov first {moov_first}");
                let tag = out.windows(AI_TAG.len()).filter(|w| *w == AI_TAG.as_bytes()).count();
                assert_eq!(tag, 1);
                assert!(!out.windows(4).any(|w| w == b"old!"), "the old comment is replaced");
                assert!(!path.with_extension("tag.part").exists());
            }
        }
        let path = dir.path().join("not.mp4");
        std::fs::write(&path, b"MP4DATA").unwrap();
        assert!(!mp4(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"MP4DATA");
    }

    /// A real film, when ffmpeg is here: ffprobe reads the tag as its
    /// `comment` (what the Nebo Media plugin's probe checks) and the film
    /// still decodes end to end, fast-start or not.
    #[test]
    fn ffprobe_reads_the_tag_on_a_real_film() {
        let (Ok(ffmpeg), Ok(ffprobe)) = (which::which("ffmpeg"), which::which("ffprobe")) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        for faststart in [false, true] {
            let path = dir.path().join(format!("film-{faststart}.mp4"));
            let mut args: Vec<String> = "-y -loglevel error -f lavfi -i testsrc=duration=1:size=64x64:rate=10 -c:v mpeg4"
                .split(' ')
                .map(str::to_string)
                .collect();
            if faststart {
                args.extend(["-movflags".into(), "+faststart".into()]);
            }
            args.push(path.to_string_lossy().into_owned());
            assert!(command::new::<std::process::Command>(&ffmpeg, command::Console::Hidden).args(&args).status().unwrap().success());
            assert!(mp4(&path).unwrap());
            let probe = command::new::<std::process::Command>(&ffprobe, command::Console::Hidden)
                .args(["-v", "error", "-show_entries", "format_tags=comment", "-of", "default=nw=1:nk=1"])
                .arg(&path)
                .output()
                .unwrap();
            assert_eq!(String::from_utf8_lossy(&probe.stdout).trim(), AI_TAG, "faststart {faststart}");
            let decode = command::new::<std::process::Command>(&ffmpeg, command::Console::Hidden)
                .args(["-v", "error", "-xerror", "-i"])
                .arg(&path)
                .args(["-f", "null", "-"])
                .output()
                .unwrap();
            assert!(decode.status.success() && decode.stderr.is_empty(), "{}", String::from_utf8_lossy(&decode.stderr));
        }
    }
}
