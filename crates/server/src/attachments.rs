//! Attachments: ONE way in, ONE way to describe them.
//!
//! `store` takes bytes into the upload store (`agent::uploads`), whether they
//! arrived over `POST /files/upload` or as a path dragged into the composer.
//! `note` builds every "[Attached: …]" line the server appends to a prompt.
//!
//! The notes exist for the model, not for people: each client strips them out
//! of the bubble with ONE regex, and a shape this constructor cannot emit is a
//! shape no client ever has to learn. See `NOTE_RE` below — it is the desktop's
//! regex, verbatim, and the test at the bottom holds every note this module can
//! produce against it.

use comm::wire::Attachment;

/// How a note opens. The reader knows these two words by name, so no third
/// label may be invented at a call site.
pub(crate) enum Kind {
    /// A file the agent can open at the path the note names.
    File,
    /// Audio: transcribed, or explained if it could not be.
    Audio,
}

impl Kind {
    fn label(&self) -> &'static str {
        match self {
            Kind::File => "Attached",
            Kind::Audio => "Audio",
        }
    }

    /// What kind a landing file is. Asked once, here, so the word a workflow
    /// subscribes to is the same word the prompt note gives the employee —
    /// audio is what can be listened to, everything else is a file.
    pub(crate) fn of(filename: &str, mime_type: &str) -> Kind {
        if ai::transcribe::is_transcribable(filename, mime_type) {
            Kind::Audio
        } else {
            Kind::File
        }
    }

    /// The kind as it travels: the last segment of the event source, and the
    /// `kind` field of its payload.
    fn slug(&self) -> &'static str {
        match self {
            Kind::File => "file",
            Kind::Audio => "audio",
        }
    }
}

/// One attachment note, ready to push onto a prompt (it opens with a newline).
///
/// `body` says what happened to the file — "saved at /…", "download failed:
/// …", "transcript follows". A `]` anywhere inside would end the note early
/// for every reader, so it becomes `)`.
pub(crate) fn note(kind: Kind, filename: &str, size: u64, body: &str) -> String {
    format!(
        "\n[{}: {} ({}) — {}]",
        kind.label(),
        bracket_safe(filename),
        size_label(size),
        bracket_safe(body)
    )
}

/// An attachment's size in the words the notes use.
pub(crate) fn size_label(size: u64) -> String {
    let size_kb = size / 1024;
    if size_kb >= 1024 {
        format!("{:.1} MB", size_kb as f64 / 1024.0)
    } else {
        format!("{} KB", size_kb)
    }
}

fn bracket_safe(s: &str) -> String {
    s.replace(']', ")")
}

/// Take bytes into the upload store and describe them as an attachment. The
/// one ingest: an uploaded file and a path dragged into the composer land in
/// the same directory, under the same naming rule, with a real file id — so
/// everything downstream (the prompt note, the user row, the `<img>` proxy,
/// re-reading the picture on a later turn) works the same way for both.
pub(crate) fn store(filename: &str, mime_type: &str, bytes: &[u8]) -> Option<Attachment> {
    let file_id = uuid::Uuid::new_v4().to_string();
    let path = agent::uploads::dir()?.join(agent::uploads::file_name(&file_id, filename));
    if let Err(e) = std::fs::write(&path, bytes) {
        tracing::warn!(path = %path.display(), error = %e, "could not store attachment");
        return None;
    }
    Some(Attachment {
        url: format!("/api/v1/comm-files/{}", file_id),
        file_id,
        filename: filename.to_string(),
        mime_type: mime_type.to_string(),
        size: bytes.len() as u64,
        thumbnail_url: None,
        width: None,
        height: None,
        duration: None,
    })
}

/// A file already on this machine, named by path in the prompt (dragged or
/// pasted into the composer), taken into the store so it reaches the agent
/// through the attachment door instead of a second one of its own.
pub(crate) fn ingest_path(path: &std::path::Path) -> Option<Attachment> {
    let bytes = std::fs::read(path).ok()?;
    // Only genuine image bytes are lifted out of the text: a token that merely
    // ends in .png stays in the prompt as the user typed it.
    let mime = ai::sniff_image_mime(&bytes)?;
    let filename = path.file_name()?.to_string_lossy().to_string();
    store(&filename, mime, &bytes)
}

/// An attachment has landed on this bot: say so, so work can start without
/// anyone typing.
///
/// One event on the ONE bus every other trigger uses (`AppState::emit_lifecycle`,
/// the same door `agent.installed` and `account.connected` go through), so a
/// binding subscribes to it the way it subscribes to anything: an `event`
/// trigger naming the source. The source carries the kind — `attachment.audio`
/// for a recording, `attachment.file` for everything else — so a flow that only
/// wants recordings names `attachment.audio` and a PDF never wakes it;
/// `attachment.*` takes both.
///
/// The payload is what a workflow needs to go and act on the file: which file,
/// what kind, what it is called, how big, the employee it landed on and the
/// conversation it came from. The last two are null when the client did not
/// name them — an upload with no conversation behind it is still an arrival.
pub(crate) fn announce(
    state: &crate::state::AppState,
    att: &Attachment,
    agent_id: Option<&str>,
    chat_id: Option<&str>,
) {
    let kind = Kind::of(&att.filename, &att.mime_type);
    state.emit_lifecycle(
        &format!("attachment.{}", kind.slug()),
        serde_json::json!({
            "file_id": att.file_id,
            "kind": kind.slug(),
            "filename": att.filename,
            "size": att.size,
            "agent_id": agent_id,
            "chat_id": chat_id,
        }),
        format!("attachment:{}", att.file_id),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The desktop's stripper, verbatim — app/src/lib/types/attachment.ts:49
    /// (`ATTACHMENT_NOTE_RE`); mobile mirrors it in `chat_parse.dart`. If this
    /// constant and that regex ever disagree, a note shows up raw in a bubble.
    /// The third alternative matches notes stored before this module existed.
    const NOTE_RE: &str = r"\n?\[(?:Attached|Audio): [^\]]*\]|\n?\[The audio file is saved at [^\]]*\]";

    /// Every note this constructor can emit is strippable by the one reader —
    /// including one whose filename and body carry a `]` of their own.
    #[test]
    fn every_note_matches_the_readers_regex() {
        let re = regex::Regex::new(NOTE_RE).unwrap();
        let notes = [
            note(Kind::File, "photo.jpg", 1_300_000, "saved at /tmp/a/photo.jpg"),
            note(Kind::File, "deck.pdf", 900, "could not be saved to disk. Tell the user"),
            note(Kind::Audio, "memo.m4a", 30_000, "transcript follows"),
            note(Kind::Audio, "memo.m4a", 30_000, "the file itself is saved at /tmp/memo.m4a"),
            note(Kind::File, "weird].png", 2048, "saved at /tmp/od]d/weird].png"),
        ];
        for n in notes {
            assert_eq!(
                re.find(&n).map(|m| m.as_str()),
                Some(n.as_str()),
                "note not strippable by the client regex: {n}"
            );
        }
    }

    /// Sizes read the way they always did: KB under a megabyte, MB above.
    #[test]
    fn size_reads_in_kb_then_mb() {
        assert_eq!(size_label(2048), "2 KB");
        assert_eq!(size_label(1024 * 1024 * 3 / 2), "1.5 MB");
    }
}
