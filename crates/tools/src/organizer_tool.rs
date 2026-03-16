//! Organizer tool: PIM management — mail, contacts, calendar, reminders.
//!
//! Multi-platform: macOS (AppleScript), Linux (CLI backends), Windows (Outlook COM).
//! Write actions (send, create, delete, complete) require user approval.

use crate::organizer;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

pub struct OrganizerTool;

impl OrganizerTool {
    pub fn new() -> Self {
        Self
    }

    fn infer_resource(&self, action: &str) -> &str {
        match action {
            "accounts" | "unread" | "send" => "mail",
            "today" | "upcoming" | "calendars" => "calendar",
            "groups" => "contacts",
            "lists" | "complete" => "reminders",
            _ => "",
        }
    }
}

/// Actions that modify data and require user approval.
const WRITE_ACTIONS: &[&str] = &["send", "create", "delete", "complete"];

impl DynTool for OrganizerTool {
    fn name(&self) -> &str {
        "organizer"
    }

    fn description(&self) -> String {
        "Personal information management — mail, contacts, calendar, reminders.\n\n\
         Resources:\n\
         - mail: accounts, unread, read, send, search\n\
         - contacts: search, get, create, groups\n\
         - calendar: calendars, today, upcoming, create, list\n\
         - reminders: lists, list, create, complete, delete\n\n\
         Examples:\n  \
         organizer(resource: \"mail\", action: \"unread\")\n  \
         organizer(resource: \"mail\", action: \"send\", to: [\"user@example.com\"], cc: [\"boss@example.com\"], subject: \"Hello\", body: \"Hi there\")\n  \
         organizer(resource: \"contacts\", action: \"search\", query: \"John\")\n  \
         organizer(resource: \"calendar\", action: \"upcoming\", days: 14)\n  \
         organizer(resource: \"calendar\", action: \"create\", title: \"Meeting\", date: \"2025-03-15 10:00\", location: \"Room 1\")\n  \
         organizer(resource: \"reminders\", action: \"create\", name: \"Buy milk\", list: \"Groceries\", priority: 5, due_date: \"tomorrow\")"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "resource": {
                    "type": "string",
                    "description": "PIM resource",
                    "enum": ["mail", "contacts", "calendar", "reminders"]
                },
                "action": {
                    "type": "string",
                    "description": "Action to perform on the resource",
                    "enum": [
                        "accounts", "unread", "read", "send", "search",
                        "get", "create", "groups",
                        "calendars", "today", "upcoming", "list",
                        "lists", "complete", "delete"
                    ]
                },
                "query": { "type": "string", "description": "Search query" },
                "name": { "type": "string", "description": "Contact, event, or reminder name" },
                "title": { "type": "string", "description": "Event or reminder title (alias for name)" },
                "email": { "type": "string", "description": "Contact email address" },
                "phone": { "type": "string", "description": "Contact phone number" },
                "company": { "type": "string", "description": "Contact company/organization" },
                "notes": { "type": "string", "description": "Notes or description" },
                "subject": { "type": "string", "description": "Email subject" },
                "body": { "type": "string", "description": "Email body" },
                "to": {
                    "oneOf": [
                        { "type": "string" },
                        { "type": "array", "items": { "type": "string" } }
                    ],
                    "description": "Email recipient(s)"
                },
                "cc": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "CC recipient(s)"
                },
                "mailbox": { "type": "string", "description": "Mailbox name (e.g. 'INBOX', 'Sent')" },
                "calendar": { "type": "string", "description": "Calendar name" },
                "date": { "type": "string", "description": "Start date (e.g. '2025-03-15 10:00', 'tomorrow')" },
                "end_date": { "type": "string", "description": "End date (defaults to start + 1 hour)" },
                "location": { "type": "string", "description": "Event location" },
                "days": { "type": "integer", "description": "Number of days to look ahead (default: 7)" },
                "list": { "type": "string", "description": "Reminder list or category name" },
                "due_date": { "type": "string", "description": "Due date (e.g. '2025-03-15', 'tomorrow', 'in 3 days')" },
                "priority": { "type": "integer", "description": "Priority: 1-3=high, 4-6=medium, 7-9=low" },
                "limit": { "type": "integer", "description": "Maximum number of results to return" }
            },
            "required": ["action"]
        })
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn requires_approval_for(&self, input: &serde_json::Value) -> bool {
        let action = input["action"].as_str().unwrap_or("");
        WRITE_ACTIONS.contains(&action)
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let parsed: organizer::OrganizerInput = match serde_json::from_value(input) {
                Ok(v) => v,
                Err(e) => return ToolResult::error(format!("Failed to parse input: {}", e)),
            };

            let resource = if parsed.resource.is_empty() {
                self.infer_resource(&parsed.action).to_string()
            } else {
                parsed.resource.clone()
            };

            match resource.as_str() {
                "mail" => organizer::handle_mail(&parsed.action, &parsed).await,
                "contacts" => organizer::handle_contacts(&parsed.action, &parsed).await,
                "calendar" => organizer::handle_calendar(&parsed.action, &parsed).await,
                "reminders" => organizer::handle_reminders(&parsed.action, &parsed).await,
                _ => ToolResult::error(format!(
                    "Unknown resource '{}'. Use: mail, contacts, calendar, reminders",
                    resource
                )),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_metadata() {
        let tool = OrganizerTool::new();
        assert_eq!(tool.name(), "organizer");
        assert!(tool.description().contains("mail"));
        assert!(tool.description().contains("contacts"));
        assert!(tool.description().contains("calendar"));
        assert!(tool.description().contains("reminders"));
        let schema = tool.schema();
        assert!(schema["properties"]["resource"].is_object());
        assert!(schema["properties"]["action"].is_object());
        assert!(schema["properties"]["cc"].is_object());
        assert!(schema["properties"]["priority"].is_object());
        assert!(schema["properties"]["days"].is_object());
        assert!(schema["properties"]["location"].is_object());
        assert!(schema["properties"]["due_date"].is_object());
        assert!(schema["properties"]["notes"].is_object());
    }

    #[test]
    fn test_infer_resource() {
        let tool = OrganizerTool::new();
        assert_eq!(tool.infer_resource("accounts"), "mail");
        assert_eq!(tool.infer_resource("unread"), "mail");
        assert_eq!(tool.infer_resource("send"), "mail");
        assert_eq!(tool.infer_resource("today"), "calendar");
        assert_eq!(tool.infer_resource("upcoming"), "calendar");
        assert_eq!(tool.infer_resource("calendars"), "calendar");
        assert_eq!(tool.infer_resource("groups"), "contacts");
        assert_eq!(tool.infer_resource("lists"), "reminders");
        assert_eq!(tool.infer_resource("complete"), "reminders");
        assert_eq!(tool.infer_resource("unknown"), "");
    }

    #[test]
    fn test_requires_approval_for_write_actions() {
        let tool = OrganizerTool::new();
        // Write actions → require approval
        assert!(tool.requires_approval_for(&serde_json::json!({"action": "send"})));
        assert!(tool.requires_approval_for(&serde_json::json!({"action": "create"})));
        assert!(tool.requires_approval_for(&serde_json::json!({"action": "delete"})));
        assert!(tool.requires_approval_for(&serde_json::json!({"action": "complete"})));
        // Read actions → auto-approve
        assert!(!tool.requires_approval_for(&serde_json::json!({"action": "unread"})));
        assert!(!tool.requires_approval_for(&serde_json::json!({"action": "read"})));
        assert!(!tool.requires_approval_for(&serde_json::json!({"action": "search"})));
        assert!(!tool.requires_approval_for(&serde_json::json!({"action": "accounts"})));
        assert!(!tool.requires_approval_for(&serde_json::json!({"action": "today"})));
        assert!(!tool.requires_approval_for(&serde_json::json!({"action": "upcoming"})));
        assert!(!tool.requires_approval_for(&serde_json::json!({"action": "lists"})));
        assert!(!tool.requires_approval_for(&serde_json::json!({"action": "list"})));
        assert!(!tool.requires_approval_for(&serde_json::json!({"action": "get"})));
        assert!(!tool.requires_approval_for(&serde_json::json!({"action": "groups"})));
        assert!(!tool.requires_approval_for(&serde_json::json!({"action": "calendars"})));
    }

    #[tokio::test]
    async fn test_unknown_resource() {
        let tool = OrganizerTool::new();
        let ctx = ToolContext::default();
        let input = serde_json::json!({"resource": "unknown", "action": "list"});
        let result = tool.execute_dyn(&ctx, input).await;
        assert!(result.is_error);
        assert!(result.content.contains("Unknown resource"));
    }

    #[tokio::test]
    async fn test_missing_action() {
        let tool = OrganizerTool::new();
        let ctx = ToolContext::default();
        let input = serde_json::json!({});
        let result = tool.execute_dyn(&ctx, input).await;
        // With OrganizerInput, action defaults to "" → resource inference returns "" → unknown resource
        assert!(result.is_error);
    }
}
