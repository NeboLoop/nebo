//! Organizer sub-modules: shared types and platform-specific implementations.

mod shared;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;

pub use shared::{parse_date, which_exists};

use crate::registry::ToolResult;

// ═══════════════════════════════════════════════════════════════════════
// Platform dispatch
// ═══════════════════════════════════════════════════════════════════════

#[cfg(target_os = "macos")]
pub use macos::{handle_calendar, handle_contacts, handle_mail, handle_reminders};

#[cfg(target_os = "linux")]
pub use linux::{handle_calendar, handle_contacts, handle_mail, handle_reminders};

#[cfg(target_os = "windows")]
pub use windows::{handle_calendar, handle_contacts, handle_mail, handle_reminders};

// Fallback for unsupported platforms
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub async fn handle_mail(_action: &str, _input: &OrganizerInput) -> ToolResult {
    ToolResult::error("Mail is not supported on this platform")
}
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub async fn handle_contacts(_action: &str, _input: &OrganizerInput) -> ToolResult {
    ToolResult::error("Contacts is not supported on this platform")
}
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub async fn handle_calendar(_action: &str, _input: &OrganizerInput) -> ToolResult {
    ToolResult::error("Calendar is not supported on this platform")
}
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub async fn handle_reminders(_action: &str, _input: &OrganizerInput) -> ToolResult {
    ToolResult::error("Reminders is not supported on this platform")
}

// ═══════════════════════════════════════════════════════════════════════
// Shared input type
// ═══════════════════════════════════════════════════════════════════════

/// Typed input for organizer tool calls.
///
/// All fields are optional and action-specific. The `to` field accepts
/// both a single string and an array for backward compatibility.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct OrganizerInput {
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub resource: String,

    // Mail fields
    #[serde(default, deserialize_with = "string_or_vec")]
    pub to: Vec<String>,
    #[serde(default)]
    pub cc: Vec<String>,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub mailbox: String,

    // Contacts fields
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub phone: String,
    #[serde(default)]
    pub company: String,
    #[serde(default)]
    pub notes: String,

    // Calendar fields
    #[serde(default)]
    pub calendar: String,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub end_date: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub days: Option<i64>,

    // Reminder fields
    #[serde(default)]
    pub list: String,
    #[serde(default)]
    pub due_date: String,
    #[serde(default)]
    pub priority: Option<i32>,

    // Shared
    #[serde(default)]
    pub limit: Option<i64>,
}

impl OrganizerInput {
    /// Get the event/reminder name — prefers `title`, falls back to `name`.
    pub fn event_name(&self) -> &str {
        if !self.title.is_empty() {
            &self.title
        } else {
            &self.name
        }
    }
}

/// Deserialize `to` as either a single string or an array of strings.
fn string_or_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct StringOrVec;

    impl<'de> de::Visitor<'de> for StringOrVec {
        type Value = Vec<String>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a string or array of strings")
        }

        fn visit_str<E: de::Error>(self, s: &str) -> Result<Vec<String>, E> {
            if s.is_empty() {
                Ok(vec![])
            } else {
                Ok(vec![s.to_string()])
            }
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<String>, A::Error> {
            let mut v = Vec::new();
            while let Some(s) = seq.next_element::<String>()? {
                if !s.is_empty() {
                    v.push(s);
                }
            }
            Ok(v)
        }

        fn visit_none<E: de::Error>(self) -> Result<Vec<String>, E> {
            Ok(vec![])
        }

        fn visit_unit<E: de::Error>(self) -> Result<Vec<String>, E> {
            Ok(vec![])
        }
    }

    deserializer.deserialize_any(StringOrVec)
}

// ═══════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_input_to_as_string() {
        let input: OrganizerInput =
            serde_json::from_str(r#"{"action":"send","to":"user@example.com"}"#).unwrap();
        assert_eq!(input.to, vec!["user@example.com"]);
    }

    #[test]
    fn test_input_to_as_array() {
        let input: OrganizerInput =
            serde_json::from_str(r#"{"action":"send","to":["a@b.com","c@d.com"]}"#).unwrap();
        assert_eq!(input.to, vec!["a@b.com", "c@d.com"]);
    }

    #[test]
    fn test_input_to_empty_string() {
        let input: OrganizerInput = serde_json::from_str(r#"{"action":"send","to":""}"#).unwrap();
        assert!(input.to.is_empty());
    }

    #[test]
    fn test_input_to_missing() {
        let input: OrganizerInput = serde_json::from_str(r#"{"action":"send"}"#).unwrap();
        assert!(input.to.is_empty());
    }

    #[test]
    fn test_input_defaults() {
        let input: OrganizerInput = serde_json::from_str(r#"{"action":"unread"}"#).unwrap();
        assert_eq!(input.action, "unread");
        assert!(input.resource.is_empty());
        assert!(input.cc.is_empty());
        assert!(input.priority.is_none());
        assert!(input.days.is_none());
        assert!(input.limit.is_none());
    }

    #[test]
    fn test_input_all_fields() {
        let input: OrganizerInput = serde_json::from_str(
            r#"{
            "action": "create",
            "resource": "calendar",
            "title": "Meeting",
            "date": "2024-06-15 14:00",
            "end_date": "2024-06-15 15:00",
            "location": "Room 1",
            "notes": "Discuss Q3",
            "calendar": "Work",
            "days": 7,
            "priority": 3,
            "limit": 10
        }"#,
        )
        .unwrap();
        assert_eq!(input.title, "Meeting");
        assert_eq!(input.location, "Room 1");
        assert_eq!(input.days, Some(7));
        assert_eq!(input.priority, Some(3));
    }

    #[test]
    fn test_event_name_prefers_title() {
        let input: OrganizerInput = serde_json::from_str(
            r#"{"action":"create","title":"From Title","name":"From Name"}"#,
        )
        .unwrap();
        assert_eq!(input.event_name(), "From Title");
    }

    #[test]
    fn test_event_name_falls_back_to_name() {
        let input: OrganizerInput =
            serde_json::from_str(r#"{"action":"create","name":"From Name"}"#).unwrap();
        assert_eq!(input.event_name(), "From Name");
    }
}
