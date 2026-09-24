//! A completion becomes a notification to the parent: heard at its next step
//! when it is busy, or the input of a new turn when it is idle. One format
//! for every path: a foreground result, a notification row, and the wake
//! rail's notification turn.

use super::{Completion, CompletionStatus};
use crate::session::SessionManager;

/// The wake-table kind of a notification row waiting for an owner session.
pub const WAKE_KIND: &str = "notification";

/// The header every notification opens with.
pub const HEADER: &str = "[Notification: not a message from the owner]";

/// The metadata of a notification row: the model reads it, the owner's
/// transcript never shows it.
pub const ROW_METADATA: &str = r#"{"notification":true,"isMeta":true}"#;

/// The one format: status line, then the result.
pub fn render_result(c: &Completion) -> String {
    let status = match &c.status {
        CompletionStatus::Done => "done".to_string(),
        CompletionStatus::Partial { why } => format!("partial ({why}; send_message to continue)"),
        CompletionStatus::Failed { error } => format!("failed ({error})"),
        CompletionStatus::Stopped => "stopped".to_string(),
    };
    format!("helper {} \"{}\": {status}\n{}", c.task_id, c.description, c.result)
}

/// The notification text for `c`.
pub fn render_notification(c: &Completion) -> String {
    format!("{HEADER}\n{}", render_result(c))
}

/// Write `text` into session `session_key` as a notification row: the
/// session's next step loads it with the rest of its conversation.
pub fn append_row(sessions: &SessionManager, session_key: &str, text: &str) -> Result<(), String> {
    let id = sessions.resolve_session_id_by_key(session_key).map_err(|e| e.to_string())?;
    sessions
        .append_message(&id, "user", text, None, None, Some(ROW_METADATA))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Whether a stored row is a notification.
pub fn is_notification_row(msg: &db::models::ChatMessage) -> bool {
    msg.metadata
        .as_deref()
        .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
        .and_then(|v| v.get("notification").and_then(|b| b.as_bool()))
        == Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completion(status: CompletionStatus) -> Completion {
        Completion {
            task_id: "h7".into(),
            description: "read the logs".into(),
            status,
            result: "Three errors, all from the nightly job.".into(),
            usage: ai::UsageInfo::default(),
        }
    }

    #[test]
    fn one_format_for_every_status() {
        assert_eq!(
            render_notification(&completion(CompletionStatus::Done)),
            "[Notification: not a message from the owner]\nhelper h7 \"read the logs\": done\nThree errors, all from the nightly job."
        );
        let second_line = |s: CompletionStatus| {
            render_notification(&completion(s))
                .lines()
                .nth(1)
                .unwrap()
                .to_string()
        };
        assert_eq!(
            second_line(CompletionStatus::Partial {
                why: "budget".into()
            }),
            "helper h7 \"read the logs\": partial (budget; send_message to continue)"
        );
        assert_eq!(
            second_line(CompletionStatus::Failed {
                error: "timeout".into()
            }),
            "helper h7 \"read the logs\": failed (timeout)"
        );
        assert_eq!(
            second_line(CompletionStatus::Stopped),
            "helper h7 \"read the logs\": stopped"
        );
        assert_eq!(
            render_result(&completion(CompletionStatus::Done)),
            render_notification(&completion(CompletionStatus::Done))
                .strip_prefix(&format!("{HEADER}\n"))
                .unwrap(),
            "a foreground result is the same body"
        );
    }
}
