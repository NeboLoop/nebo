//! Sensitive argument redaction for slash commands.
//!
//! Prevents secrets (API keys, tokens, passwords) passed as slash command
//! arguments from being stored in conversation history or logs.

/// Slash command prefixes whose arguments are considered sensitive.
/// The match is case-insensitive against the first token of the message.
const SENSITIVE_COMMANDS: &[&str] = &[
    "/auth",
    "/login",
    "/token",
    "/key",
    "/secret",
    "/password",
    "/apikey",
    "/api-key",
    "/api_key",
    "/credential",
    "/credentials",
    "/oauth",
    "/connect",
    "/register",
    "/signup",
    "/signin",
];

/// A client's socket frame that answers a sign-in card (an `ask_response`
/// to one, or a chat message sent while its conversation has one open,
/// which answers it), as the log shows it: the answer (`data.value`,
/// `data.prompt`) left out. The owner's code for a sign-in never reaches a
/// log.
pub fn without_sign_in_answer(mut frame: serde_json::Value) -> serde_json::Value {
    if let Some(data) = frame.get_mut("data").and_then(serde_json::Value::as_object_mut) {
        for key in ["value", "prompt"] {
            if let Some(answer) = data.get_mut(key) {
                *answer = serde_json::json!("[sign-in answer]");
            }
        }
    }
    frame
}

/// If `prompt` is a sensitive slash command, return a copy with arguments
/// replaced by `[redacted]`. Otherwise return `None` (caller should use
/// the original prompt unchanged).
///
/// Only the first whitespace-delimited token is checked. When it matches
/// a known sensitive command, everything after it is replaced:
///
/// ```text
/// "/auth sk-abc123-xyz"  →  Some("/auth [redacted]")
/// "/help me"             →  None
/// "hello world"          →  None
/// ```
pub fn redact_sensitive_args(prompt: &str) -> Option<String> {
    let trimmed = prompt.trim();
    if !trimmed.starts_with('/') {
        return None;
    }

    let cmd = trimmed.split_whitespace().next().unwrap_or("");
    let has_args = trimmed.len() > cmd.len();
    if !has_args {
        return None;
    }

    let cmd_lower = cmd.to_lowercase();
    if SENSITIVE_COMMANDS.iter().any(|&s| s == cmd_lower) {
        Some(format!("{} [redacted]", cmd))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The answer to a sign-in card is left out of the logged frame, and
    /// nothing else is.
    #[test]
    fn a_sign_in_answer_is_left_out_of_the_logged_frame() {
        let answer = serde_json::json!({ "type": "ask_response", "data": { "request_id": "r1", "value": "4/0Secret" } });
        let logged = without_sign_in_answer(answer).to_string();
        assert!(!logged.contains("4/0Secret") && logged.contains("r1"), "{logged}");
        let chat = serde_json::json!({ "type": "chat", "data": { "session_id": "s1", "prompt": "4/0Secret" } });
        let logged = without_sign_in_answer(chat).to_string();
        assert!(!logged.contains("4/0Secret") && logged.contains("s1"), "{logged}");
    }

    #[test]
    fn redacts_auth_command() {
        assert_eq!(
            redact_sensitive_args("/auth sk-abc123-xyz"),
            Some("/auth [redacted]".to_string())
        );
    }

    #[test]
    fn redacts_case_insensitive() {
        assert_eq!(
            redact_sensitive_args("/AUTH my-secret"),
            Some("/AUTH [redacted]".to_string())
        );
        assert_eq!(
            redact_sensitive_args("/Token bearer-xyz"),
            Some("/Token [redacted]".to_string())
        );
    }

    #[test]
    fn redacts_multiple_args() {
        assert_eq!(
            redact_sensitive_args("/login user@example.com p@ssw0rd"),
            Some("/login [redacted]".to_string())
        );
    }

    #[test]
    fn no_redaction_for_non_sensitive() {
        assert_eq!(redact_sensitive_args("/help me"), None);
        assert_eq!(redact_sensitive_args("/gmail triage"), None);
    }

    #[test]
    fn no_redaction_for_non_slash() {
        assert_eq!(redact_sensitive_args("hello world"), None);
    }

    #[test]
    fn no_redaction_without_args() {
        assert_eq!(redact_sensitive_args("/auth"), None);
        assert_eq!(redact_sensitive_args("/login"), None);
    }

    #[test]
    fn redacts_password_and_key() {
        assert_eq!(
            redact_sensitive_args("/password hunter2"),
            Some("/password [redacted]".to_string())
        );
        assert_eq!(
            redact_sensitive_args("/key AKIA1234567890ABCDEF"),
            Some("/key [redacted]".to_string())
        );
        assert_eq!(
            redact_sensitive_args("/secret my-client-secret"),
            Some("/secret [redacted]".to_string())
        );
    }

    #[test]
    fn redacts_api_key_variants() {
        assert_eq!(
            redact_sensitive_args("/apikey abc123"),
            Some("/apikey [redacted]".to_string())
        );
        assert_eq!(
            redact_sensitive_args("/api-key abc123"),
            Some("/api-key [redacted]".to_string())
        );
        assert_eq!(
            redact_sensitive_args("/api_key abc123"),
            Some("/api_key [redacted]".to_string())
        );
    }
}
