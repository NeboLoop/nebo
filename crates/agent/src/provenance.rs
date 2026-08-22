//! Engine-side provenance classification — the static tool→class table the
//! runner uses to accumulate a run's taint set (coworker trust boundaries
//! design, 2026-08-22). Deterministic and model-invisible: the mapping is
//! keyed on tool name + input shape only; nothing a model writes can add,
//! remove, or forge a class.
//!
//! v1 covers the built-in domain tools. Plugin-declared taint (manifest
//! `taint: [...]`) is a follow-up; until it lands, content read through a
//! plugin (e.g. cloud mail) is NOT stamped — the design doc records this gap.

use types::provenance::ProvenanceClass;

/// Classes the given tool call brings into the run, per the static table.
/// Read-only tool *sends* are not taint — reading external content is.
pub fn classify_tool(name: &str, input: &serde_json::Value) -> Vec<ProvenanceClass> {
    let resource = input["resource"].as_str().unwrap_or("");
    let action = input["action"].as_str().unwrap_or("");
    match name {
        // Any web surface (fetch/search/browser/devtools) is web content.
        "web" | "browser" => vec![ProvenanceClass::Web],
        // Mailbox reads. Contacts/calendar/reminders are the owner's own data.
        "organizer" if resource == "mail" => vec![ProvenanceClass::ExternalEmail],
        // Loop reads pull remote messages (other bots / loop members) into
        // context. Sends are not taint.
        "loop" if matches!(action, "messages" | "get") => vec![ProvenanceClass::Channel],
        // SMS reads are external interlocutors.
        "message" if resource == "sms" && matches!(action, "read" | "search" | "conversations") => {
            vec![ProvenanceClass::Channel]
        }
        // Agent-ingested files (the attachment root under <data_dir>/files).
        // Owner-uploaded files elsewhere on disk are owner-authored and carry
        // no taint (decision 2026-08-22).
        "os" if action == "read" => {
            let path = input["path"].as_str().unwrap_or("");
            if !path.is_empty() && is_ingested_file(path) {
                vec![ProvenanceClass::Document]
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

/// True when the path lies under the attachment/ingestion root
/// (`<data_dir>/files/`) — files the agent pulled in, not files the owner
/// placed.
fn is_ingested_file(path: &str) -> bool {
    let Ok(root) = config::data_dir() else {
        return false;
    };
    let files_root = root.join("files");
    std::path::Path::new(path).starts_with(&files_root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_and_browser_are_web() {
        assert_eq!(
            classify_tool("web", &serde_json::json!({"resource": "search"})),
            vec![ProvenanceClass::Web]
        );
        assert_eq!(
            classify_tool("browser", &serde_json::json!({})),
            vec![ProvenanceClass::Web]
        );
    }

    #[test]
    fn organizer_mail_only() {
        assert_eq!(
            classify_tool(
                "organizer",
                &serde_json::json!({"resource": "mail", "action": "read"})
            ),
            vec![ProvenanceClass::ExternalEmail]
        );
        assert!(
            classify_tool(
                "organizer",
                &serde_json::json!({"resource": "calendar", "action": "list"})
            )
            .is_empty()
        );
    }

    #[test]
    fn loop_and_sms_reads_are_channel_sends_are_not() {
        assert_eq!(
            classify_tool(
                "loop",
                &serde_json::json!({"resource": "channel", "action": "messages"})
            ),
            vec![ProvenanceClass::Channel]
        );
        assert!(
            classify_tool(
                "loop",
                &serde_json::json!({"resource": "channel", "action": "send"})
            )
            .is_empty()
        );
        assert_eq!(
            classify_tool(
                "message",
                &serde_json::json!({"resource": "sms", "action": "read"})
            ),
            vec![ProvenanceClass::Channel]
        );
        assert!(
            classify_tool(
                "message",
                &serde_json::json!({"resource": "coworker", "action": "send"})
            )
            .is_empty()
        );
    }

    #[test]
    fn plain_file_reads_are_clean() {
        assert!(
            classify_tool(
                "os",
                &serde_json::json!({"resource": "file", "action": "read", "path": "/tmp/notes.md"})
            )
            .is_empty()
        );
    }
}
