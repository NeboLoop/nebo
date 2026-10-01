//! A published app's review outcome, as the hub reports it.
//!
//! When a marketplace review finishes, the hub sends each of the
//! publisher's bots an `artifact_reviewed` notification on the installs
//! stream. The bot that submitted the app finds the listing by its artifact
//! (`app_listings`), records the outcome, says it in the conversation the
//! listing was submitted from, and tells every open app (`app_listing`
//! event) so the listing card there shows it. Nothing asks the hub again
//! and again: the hub says when.

use serde_json::{Value, json};

use crate::state::AppState;

/// The notification's type on the installs stream.
pub const ARTIFACT_REVIEWED: &str = "artifact_reviewed";

/// Whether an installs-stream message is a review outcome.
pub fn is_review(content: &str) -> Option<Value> {
    let v: Value = serde_json::from_str(content).ok()?;
    (v.get("type").and_then(|t| t.as_str()) == Some(ARTIFACT_REVIEWED)).then_some(v)
}

/// The listing as the outcome leaves it, and what to say in its chat.
/// `None` when the outcome is about an artifact this bot did not submit.
pub fn review_outcome(
    row: Option<db::AppListing>,
    review: &Value,
) -> Option<(db::AppListing, String)> {
    let mut row = row?;
    let status = review.get("status").and_then(|v| v.as_str()).unwrap_or("");
    let version = review
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or(&row.version)
        .to_string();
    let notes = review
        .get("notes")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let draft: Value = serde_json::from_str(&row.draft).unwrap_or_default();
    let name = draft
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|n| !n.is_empty())
        .or_else(|| review.get("name").and_then(|v| v.as_str()))
        .unwrap_or("Your app")
        .to_string();
    let visibility = draft
        .get("visibility")
        .and_then(|v| v.as_str())
        .unwrap_or("public");
    let message = match status {
        "approved" => {
            row.status = "approved".into();
            let where_ = match visibility {
                "unlisted" => "Anyone with its link or code can install it now.",
                "private" => "You can install it from your account now.",
                _ => "It is listed on the marketplace now.",
            };
            format!("**{name} v{version} passed review.** {where_}")
        }
        "rejected" => {
            row.status = "rejected".into();
            let notes_line = if notes.is_empty() {
                "The reviewer left no notes.".to_string()
            } else {
                format!("The reviewer's notes: {notes}")
            };
            format!(
                "**{name} v{version} needs changes before it can be listed.** {notes_line}\n\nTell me what to change, or \
                 ask me to fix what the reviewer found and submit it again."
            )
        }
        _ => return None,
    };
    row.version = version;
    row.notes = notes;
    Some((row, message))
}

/// Handle an `artifact_reviewed` notification: record it, say it in the
/// listing's chat, and tell the open apps.
pub async fn handle_review(state: &AppState, review: Value) {
    let artifact_id = review
        .get("artifactId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let row = state
        .store
        .app_listing_by_artifact(artifact_id)
        .ok()
        .flatten();
    let Some((row, message)) = review_outcome(row, &review) else {
        tracing::debug!(
            artifact_id,
            "review outcome for a listing this bot did not submit"
        );
        return;
    };
    if let Err(e) = state.store.put_app_listing(&row) {
        tracing::warn!(app = %row.app_id, error = %e, "review outcome not recorded");
    }
    let session_key = if row.chat_session.is_empty() {
        types::keyparser::build_agent_session_key(&row.app_id, "web")
    } else {
        row.chat_session.clone()
    };
    let metadata = json!({
        "appListing": {
            "appId": row.app_id,
            "artifactId": row.artifact_id,
            "status": row.status,
            "version": row.version,
            "notes": row.notes,
        }
    })
    .to_string();
    let sessions = state.harness.sessions();
    match sessions.resolve_session_id_by_key(&session_key) {
        Ok(session_id) => {
            if let Err(e) = sessions.append_message(
                &session_id,
                "assistant",
                &message,
                None,
                None,
                Some(&metadata),
            ) {
                tracing::warn!(session_key, error = %e, "review outcome not posted in its chat");
            }
        }
        Err(e) => {
            tracing::warn!(session_key, error = %e, "review outcome: the listing's chat is gone")
        }
    }
    state.hub.broadcast(
        "app_listing",
        json!({
            "appId": row.app_id,
            "artifactId": row.artifact_id,
            "status": row.status,
            "version": row.version,
            "notes": row.notes,
        }),
    );
    // The open chat reads the posted outcome (it has no reply streaming).
    state.hub.broadcast(
        "chat_complete",
        json!({ "session_id": session_key, "agentId": row.app_id }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn submitted() -> db::AppListing {
        db::AppListing {
            app_id: "app-1".into(),
            artifact_id: "art-1".into(),
            draft: r#"{"name":"Kart Racer","visibility":"unlisted"}"#.into(),
            status: "in_review".into(),
            version: "1.0.0".into(),
            chat_session: "agent:app-1:web".into(),
            ..Default::default()
        }
    }

    #[test]
    fn only_a_review_notification_is_a_review() {
        assert!(is_review(r#"{"type":"artifact_reviewed","artifactId":"a"}"#).is_some());
        assert!(is_review(r#"{"type":"tool_installed","tool_id":"a"}"#).is_none());
        assert!(is_review("not json").is_none());
    }

    /// An approval says where the app can be installed from; a rejection
    /// carries the reviewer's notes and offers to fix it; an outcome for an
    /// artifact this bot did not submit, or a status that is no decision,
    /// changes nothing.
    #[test]
    fn the_outcome_is_recorded_and_said_plainly() {
        let (row, msg) = review_outcome(
            Some(submitted()),
            &json!({"status": "approved", "version": "1.0.0"}),
        )
        .unwrap();
        assert_eq!(row.status, "approved");
        assert!(
            msg.contains("Kart Racer v1.0.0 passed review") && msg.contains("link or code"),
            "{msg}"
        );

        let (row, msg) = review_outcome(
            Some(submitted()),
            &json!({"status": "rejected", "version": "1.0.0", "notes": "The first screenshot is blank."}),
        )
        .unwrap();
        assert_eq!(
            (row.status.as_str(), row.notes.as_str()),
            ("rejected", "The first screenshot is blank.")
        );
        assert!(
            msg.contains("needs changes") && msg.contains("The first screenshot is blank."),
            "{msg}"
        );

        assert!(review_outcome(None, &json!({"status": "approved"})).is_none());
        assert!(review_outcome(Some(submitted()), &json!({"status": "scanning"})).is_none());
    }
}
