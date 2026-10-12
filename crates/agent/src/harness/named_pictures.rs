//! Pictures the owner names by path in a message: looked at as an attached
//! picture is, by the vision helper, as the message is stored.
//!
//! Live 2026-10-08: the owner wrote "I don't want these in the video:
//! /Users/…/Desktop/Screenshot 2026-10-08 at 2.26.29 PM.png" (black title
//! cards), and the employee answered "that screenshot file is not part of
//! the video" without looking. A path to an image on this computer the run
//! may read is a picture he showed: it meets the limits a `read_file` of it
//! meets, and nothing else.

use std::path::{Path, PathBuf};

/// The picture formats a path is taken as a picture for.
const PICTURE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "heic", "webp", "gif"];

/// At most this many pictures are looked at for one message.
pub(crate) const MAX_NAMED: usize = 4;

/// Longest path looked for.
const MAX_PATH: usize = 1024;

/// The image files the text names by an absolute or `~/` path that exist
/// here, in order, each once, at most [`MAX_NAMED`]. A path may hold spaces
/// (a macOS screenshot's name does): it runs to the first picture extension
/// that makes it a file. Files under `skip` (the uploads folder, which the
/// attachment notes name) are left out: those are attachments already.
pub(crate) fn in_text(text: &str, skip: Option<&Path>) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    let mut from = 0;
    while found.len() < MAX_NAMED {
        let Some((start, end, path)) = next_path(text, from) else {
            break;
        };
        from = end.max(start + 1);
        if skip.is_some_and(|dir| path.starts_with(dir)) || found.contains(&path) {
            continue;
        }
        found.push(path);
    }
    found
}

/// The first path at or after byte `from`: its span and the file.
fn next_path(text: &str, from: usize) -> Option<(usize, usize, PathBuf)> {
    let mut prev: Option<char> = text[..from].chars().next_back();
    for (i, c) in text[from..].char_indices().map(|(i, c)| (i + from, c)) {
        let opens = prev.is_none_or(|p| p.is_whitespace() || "\"'`([<:".contains(p));
        prev = Some(c);
        if !opens || !(text[i..].starts_with('/') || text[i..].starts_with("~/")) {
            continue;
        }
        if let Some((end, path)) = file_from(text, i) {
            return Some((i, end, path));
        }
    }
    None
}

/// The shortest run of `text` from `start` that ends in a picture
/// extension and names an existing file.
fn file_from(text: &str, start: usize) -> Option<(usize, PathBuf)> {
    let rest = &text[start..];
    let line = rest.find('\n').unwrap_or(rest.len()).min(MAX_PATH);
    let line = (0..=line)
        .rev()
        .find(|&n| rest.is_char_boundary(n))
        .unwrap_or(0);
    let lower = rest[..line].to_ascii_lowercase();
    for (dot, _) in lower.match_indices('.') {
        let after = &lower[dot + 1..];
        let Some(ext) = PICTURE_EXTENSIONS.iter().find(|e| after.starts_with(*e)) else {
            continue;
        };
        let end = dot + 1 + ext.len();
        if lower[end..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            continue;
        }
        let Ok(path) = types::pathres::resolve(&rest[..end]) else {
            continue;
        };
        if path.is_file() {
            return Some((start + end, path));
        }
    }
    None
}

/// Whether a run may read `path`: the limits a `read_file` of it meets (the
/// sensitive paths, Nebo's own files, the run's folders).
pub(crate) fn readable(ctx: &tools::ToolContext, path: &Path) -> bool {
    let shown = path.to_string_lossy().into_owned();
    tools::file_tool::validate_file_path(&shown, "read").is_ok()
        && tools::safeguard::check_safeguard(
            "read_file",
            &serde_json::json!({ "path": shown }),
            ctx,
        )
        .is_none()
        && ctx
            .outside_folders("read", std::slice::from_ref(&shown))
            .is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        let mut img = image::RgbImage::new(4, 4);
        img.put_pixel(0, 0, image::Rgb([255, 0, 0]));
        img.save(&path).unwrap();
        path
    }

    /// The owner's message of 2026-10-08: a screenshot's path, spaces and
    /// all, after a colon.
    #[test]
    fn a_pasted_screenshot_path_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let shot = png(dir.path(), "Screenshot 2026-10-08 at 2.26.29 PM.png");
        let text = format!("I don't want these in the video: {}", shot.display());
        assert_eq!(in_text(&text, None), vec![shot.clone()]);
        // Quoted, with a full stop after it, in upper case.
        let upper = png(dir.path(), "CARDS.PNG");
        let text = format!("Look at \"{}\" and {}.", shot.display(), upper.display());
        assert_eq!(in_text(&text, None), vec![shot, upper]);
    }

    #[test]
    fn a_path_that_is_no_picture_here_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let notes = dir.path().join("notes.txt");
        std::fs::write(&notes, "x").unwrap();
        let missing = dir.path().join("gone.png");
        let text = format!(
            "See {} and {} and https://example.com/a.png and a/relative.png",
            notes.display(),
            missing.display()
        );
        assert!(in_text(&text, None).is_empty());
    }

    /// An attachment's note names its file in the uploads folder: that
    /// picture is an attachment already, never looked at twice.
    #[test]
    fn an_attachments_own_file_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let uploads = dir.path().join("uploads");
        std::fs::create_dir_all(&uploads).unwrap();
        let attached = png(&uploads, "abc-photo.jpg");
        let text = format!(
            "[Attached: photo.jpg — an image, shown above — saved at {}]",
            attached.display()
        );
        assert!(in_text(&text, Some(&uploads)).is_empty());
    }

    #[test]
    fn at_most_four_each_once() {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<PathBuf> = (0..6)
            .map(|n| png(dir.path(), &format!("p{n}.png")))
            .collect();
        let mut text: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
        text.insert(1, paths[0].display().to_string());
        assert_eq!(in_text(&text.join(" "), None), paths[..MAX_NAMED].to_vec());
    }

    /// Read as `read_file` reads: Nebo's own files and the sensitive paths
    /// stay closed.
    #[test]
    fn readable_meets_the_read_file_limits() {
        let ctx = tools::ToolContext::new(tools::Origin::User);
        let dir = tempfile::tempdir().unwrap();
        assert!(readable(&ctx, &png(dir.path(), "ok.png")));
        assert!(!readable(&ctx, &types::pathres::expand("~/.ssh/id.png")));
    }
}
