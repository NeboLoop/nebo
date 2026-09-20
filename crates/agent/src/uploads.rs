//! The upload store: where an attachment's bytes live on this machine.
//!
//! ONE directory, ONE naming rule, ONE lookup. The server writes here when a
//! file is attached (`server::attachments::store`), and the runner reads back
//! from here when a picture from an earlier turn has to be shown to the model
//! again — the row keeps the attachment's id, never a second copy of the bytes.

use std::path::PathBuf;

/// Directory holding attachments this machine has bytes for — ones uploaded
/// here, and ones downloaded from the loop.
pub fn dir() -> Option<PathBuf> {
    let dir = config::data_dir().ok()?.join("files").join("uploads");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Stored name for an attachment. The id prefix keeps it unique across
/// re-sends while leaving the original filename readable on disk.
pub fn file_name(file_id: &str, filename: &str) -> String {
    let short_id: String = file_id.chars().take(8).collect();
    format!("{}-{}", short_id, filename)
}

/// Find a locally-held attachment by id alone (the filename isn't always known
/// at the call site — a rendering `<img>` has only the id).
///
/// ponytail: linear scan of the uploads dir. Index it if that directory ever
/// grows past a few thousand files.
pub fn by_id(file_id: &str) -> Option<PathBuf> {
    let short_id: String = file_id.chars().take(8).collect();
    if short_id.is_empty() {
        return None;
    }
    let prefix = format!("{}-", short_id);
    std::fs::read_dir(dir()?)
        .ok()?
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().starts_with(&prefix))
        .map(|e| e.path())
}

/// The picture a stored image attachment holds, re-read from the store and
/// normalized for a provider. This is why a user row keeps only the
/// attachment (id, name, size) and never a base64 copy of the same image:
/// replaying the conversation reads the file again.
///
/// `None` when the attachment isn't an image, its bytes are gone, or they
/// don't decode — history then carries the "[Attached: …]" note alone, which
/// is what the model needs to say the picture is no longer available.
pub fn image(att: &comm::wire::Attachment) -> Option<ai::ImageContent> {
    if att.file_id.is_empty() || !att.mime_type.starts_with("image/") {
        return None;
    }
    let bytes = std::fs::read(by_id(&att.file_id)?).ok()?;
    let (media_type, data) = ai::image_norm::normalize_for_llm(&bytes)?;
    Some(ai::ImageContent { media_type, data })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(file_id: &str, mime: &str) -> comm::wire::Attachment {
        comm::wire::Attachment {
            file_id: file_id.into(),
            filename: "job.png".into(),
            mime_type: mime.into(),
            size: 68,
            url: String::new(),
            thumbnail_url: None,
            width: None,
            height: None,
            duration: None,
        }
    }

    /// The store is the one copy of an attached picture: written by its id,
    /// read back by that id alone. This is what a user row leans on when it
    /// keeps the attachment and not the bytes.
    #[test]
    fn a_stored_picture_comes_back_by_its_id() {
        use base64::Engine;
        crate::test_home::with_home(|_| {
            let png = base64::engine::general_purpose::STANDARD
                .decode("iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAIAAAAmkwkpAAAAEElEQVR4nGM4YWMDRwzEcQAREhQBbrqBkwAAAABJRU5ErkJggg==")
                .unwrap();
            let file_id = "abcd1234-0000";
            std::fs::write(dir().unwrap().join(file_name(file_id, "job.png")), &png).unwrap();

            let shown = image(&attachment(file_id, "image/png")).expect("the picture comes back");
            assert!(shown.media_type.starts_with("image/"));
            assert!(!shown.data.is_empty());

            // Gone from disk, or never an image: the note in the text stands alone.
            assert!(image(&attachment("no-such-id", "image/png")).is_none());
            assert!(image(&attachment(file_id, "application/pdf")).is_none());
        });
    }
}
