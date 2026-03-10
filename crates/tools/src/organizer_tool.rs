use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// Organizer tool: PIM management — mail, contacts, calendar, reminders.
/// macOS-first via AppleScript; returns platform error on other OSes.
pub struct OrganizerTool;

impl OrganizerTool {
    pub fn new() -> Self {
        Self
    }

    /// Infer the resource from the action when the caller omits the resource field.
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
         organizer(resource: \"mail\", action: \"send\", to: \"user@example.com\", subject: \"Hello\", body: \"Hi there\")\n  \
         organizer(resource: \"contacts\", action: \"search\", query: \"John\")\n  \
         organizer(resource: \"calendar\", action: \"today\")\n  \
         organizer(resource: \"reminders\", action: \"create\", list: \"Groceries\", name: \"Buy milk\")"
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
                "email": { "type": "string", "description": "Email address" },
                "subject": { "type": "string", "description": "Email subject" },
                "body": { "type": "string", "description": "Email or event body" },
                "to": { "type": "string", "description": "Email recipient address" },
                "calendar": { "type": "string", "description": "Calendar name" },
                "list": { "type": "string", "description": "Reminder list name" },
                "date": { "type": "string", "description": "Date for events (e.g. '2025-03-15 10:00')" },
                "limit": { "type": "integer", "description": "Maximum number of results to return" }
            },
            "required": ["action"]
        })
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let action = input["action"].as_str().unwrap_or("");
            let resource = input["resource"]
                .as_str()
                .unwrap_or_else(|| self.infer_resource(action));

            match resource {
                "mail" => handle_mail(action, &input).await,
                "contacts" => handle_contacts(action, &input).await,
                "calendar" => handle_calendar(action, &input).await,
                "reminders" => handle_reminders(action, &input).await,
                _ => ToolResult::error(format!(
                    "Unknown resource '{}'. Use: mail, contacts, calendar, reminders",
                    resource
                )),
            }
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Mail
// ═══════════════════════════════════════════════════════════════════════

#[cfg(target_os = "macos")]
async fn handle_mail(action: &str, input: &serde_json::Value) -> ToolResult {
    match action {
        "accounts" => {
            run_osascript("tell application \"Mail\" to return name of every account").await
        }
        "unread" => {
            run_osascript(
                "tell application \"Mail\" to return count of \
                 (messages of inbox whose read status is false)",
            )
            .await
        }
        "read" => {
            let limit = input["limit"].as_i64().unwrap_or(10).clamp(1, 50);
            let script = format!(
                r#"tell application "Mail"
    set msgs to (messages 1 through {limit} of inbox)
    set output to ""
    repeat with m in msgs
        set output to output & "From: " & (sender of m) & linefeed & "Subject: " & (subject of m) & linefeed & "Date: " & (date received of m as text) & linefeed & "---" & linefeed
    end repeat
    return output
end tell"#,
                limit = limit
            );
            run_osascript(&script).await
        }
        "send" => {
            let to = input["to"].as_str().unwrap_or("");
            let subject = input["subject"].as_str().unwrap_or("");
            let body = input["body"].as_str().unwrap_or("");
            if to.is_empty() {
                return ToolResult::error("'to' parameter required for send");
            }
            if subject.is_empty() {
                return ToolResult::error("'subject' parameter required for send");
            }
            let script = format!(
                r#"tell application "Mail"
    set newMsg to make new outgoing message with properties {{subject:"{subject}", content:"{body}", visible:true}}
    tell newMsg to make new to recipient with properties {{address:"{to}"}}
    send newMsg
end tell"#,
                subject = escape_applescript(subject),
                body = escape_applescript(body),
                to = escape_applescript(to)
            );
            run_osascript(&script).await
        }
        "search" => {
            let query = input["query"].as_str().unwrap_or("");
            if query.is_empty() {
                return ToolResult::error("'query' parameter required for search");
            }
            let script = format!(
                r#"tell application "Mail"
    set found to (search inbox for "{query}")
    set output to ""
    set maxResults to 20
    set i to 0
    repeat with m in found
        if i >= maxResults then exit repeat
        set output to output & "From: " & (sender of m) & " | Subject: " & (subject of m) & linefeed
        set i to i + 1
    end repeat
    return output
end tell"#,
                query = escape_applescript(query)
            );
            run_osascript(&script).await
        }
        _ => ToolResult::error(format!(
            "Unknown mail action '{}'. Use: accounts, unread, read, send, search",
            action
        )),
    }
}

#[cfg(target_os = "linux")]
async fn handle_mail(action: &str, _input: &serde_json::Value) -> ToolResult {
    let _ = action;
    ToolResult::error("Mail integration requires a supported mail client on Linux")
}

#[cfg(target_os = "windows")]
async fn handle_mail(action: &str, _input: &serde_json::Value) -> ToolResult {
    let _ = action;
    ToolResult::error("Mail integration requires Outlook on Windows")
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
async fn handle_mail(action: &str, _input: &serde_json::Value) -> ToolResult {
    let _ = action;
    ToolResult::error("Mail integration is not supported on this platform")
}

// ═══════════════════════════════════════════════════════════════════════
// Contacts
// ═══════════════════════════════════════════════════════════════════════

#[cfg(target_os = "macos")]
async fn handle_contacts(action: &str, input: &serde_json::Value) -> ToolResult {
    match action {
        "search" => {
            let query = input["query"].as_str().unwrap_or("");
            if query.is_empty() {
                return ToolResult::error("'query' parameter required for search");
            }
            let script = format!(
                r#"tell application "Contacts"
    set found to every person whose name contains "{query}"
    set output to ""
    repeat with p in found
        set output to output & (name of p) & linefeed
    end repeat
    return output
end tell"#,
                query = escape_applescript(query)
            );
            run_osascript(&script).await
        }
        "get" => {
            let name = input["name"].as_str().unwrap_or("");
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for get");
            }
            let script = format!(
                r#"tell application "Contacts"
    set p to first person whose name is "{name}"
    set output to "Name: " & (name of p) & linefeed
    try
        set output to output & "Email: " & (value of first email of p) & linefeed
    end try
    try
        set output to output & "Phone: " & (value of first phone of p) & linefeed
    end try
    try
        set output to output & "Company: " & (organization of p) & linefeed
    end try
    return output
end tell"#,
                name = escape_applescript(name)
            );
            run_osascript(&script).await
        }
        "create" => {
            let name = input["name"].as_str().unwrap_or("");
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for create");
            }
            // Split name into first/last
            let parts: Vec<&str> = name.splitn(2, ' ').collect();
            let first = escape_applescript(parts[0]);
            let last = if parts.len() > 1 {
                escape_applescript(parts[1])
            } else {
                String::new()
            };
            let email = input["email"].as_str().unwrap_or("");

            let mut script = format!(
                r#"tell application "Contacts"
    set newPerson to make new person with properties {{first name:"{first}", last name:"{last}"}}"#,
                first = first,
                last = last
            );
            if !email.is_empty() {
                script.push_str(&format!(
                    "\n    tell newPerson to make new email at end of emails with properties {{label:\"work\", value:\"{}\"}}",
                    escape_applescript(email)
                ));
            }
            script.push_str(
                "\n    save\n    return \"Contact created: \" & (name of newPerson)\nend tell",
            );
            run_osascript(&script).await
        }
        "groups" => {
            run_osascript(
                "tell application \"Contacts\" to return name of every group",
            )
            .await
        }
        _ => ToolResult::error(format!(
            "Unknown contacts action '{}'. Use: search, get, create, groups",
            action
        )),
    }
}

#[cfg(target_os = "linux")]
async fn handle_contacts(action: &str, _input: &serde_json::Value) -> ToolResult {
    let _ = action;
    ToolResult::error("Contacts integration is not supported on Linux")
}

#[cfg(target_os = "windows")]
async fn handle_contacts(action: &str, _input: &serde_json::Value) -> ToolResult {
    let _ = action;
    ToolResult::error("Contacts integration is not supported on Windows")
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
async fn handle_contacts(action: &str, _input: &serde_json::Value) -> ToolResult {
    let _ = action;
    ToolResult::error("Contacts integration is not supported on this platform")
}

// ═══════════════════════════════════════════════════════════════════════
// Calendar
// ═══════════════════════════════════════════════════════════════════════

#[cfg(target_os = "macos")]
async fn handle_calendar(action: &str, input: &serde_json::Value) -> ToolResult {
    match action {
        "calendars" => {
            run_osascript(
                "tell application \"Calendar\" to return name of every calendar",
            )
            .await
        }
        "today" | "list" => {
            let script = r#"tell application "Calendar"
    set today to current date
    set time of today to 0
    set tomorrow to today + (1 * days)
    set output to ""
    repeat with cal in every calendar
        set evts to (every event of cal whose start date >= today and start date < tomorrow)
        repeat with e in evts
            set output to output & (name of cal) & " | " & (summary of e) & " | " & (start date of e as text) & linefeed
        end repeat
    end repeat
    if output is "" then return "No events today"
    return output
end tell"#;
            run_osascript(script).await
        }
        "upcoming" => {
            let script = r#"tell application "Calendar"
    set today to current date
    set nextWeek to today + (7 * days)
    set output to ""
    repeat with cal in every calendar
        set evts to (every event of cal whose start date >= today and start date < nextWeek)
        repeat with e in evts
            set output to output & (name of cal) & " | " & (summary of e) & " | " & (start date of e as text) & linefeed
        end repeat
    end repeat
    if output is "" then return "No upcoming events in the next 7 days"
    return output
end tell"#;
            run_osascript(script).await
        }
        "create" => {
            let calendar = input["calendar"].as_str().unwrap_or("Calendar");
            let name = input["name"].as_str().unwrap_or("");
            let date = input["date"].as_str().unwrap_or("");
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for create");
            }
            if date.is_empty() {
                return ToolResult::error("'date' parameter required for create");
            }
            let body = input["body"].as_str().unwrap_or("");
            let script = format!(
                r#"tell application "Calendar"
    tell calendar "{calendar}"
        set startDate to date "{date}"
        set endDate to startDate + (1 * hours)
        set newEvent to make new event with properties {{summary:"{name}", start date:startDate, end date:endDate, description:"{body}"}}
        return "Event created: " & (summary of newEvent)
    end tell
end tell"#,
                calendar = escape_applescript(calendar),
                name = escape_applescript(name),
                date = escape_applescript(date),
                body = escape_applescript(body)
            );
            run_osascript(&script).await
        }
        _ => ToolResult::error(format!(
            "Unknown calendar action '{}'. Use: calendars, today, upcoming, create, list",
            action
        )),
    }
}

#[cfg(target_os = "linux")]
async fn handle_calendar(action: &str, _input: &serde_json::Value) -> ToolResult {
    let _ = action;
    ToolResult::error("Calendar integration is not supported on Linux")
}

#[cfg(target_os = "windows")]
async fn handle_calendar(action: &str, _input: &serde_json::Value) -> ToolResult {
    let _ = action;
    ToolResult::error("Calendar integration is not supported on Windows")
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
async fn handle_calendar(action: &str, _input: &serde_json::Value) -> ToolResult {
    let _ = action;
    ToolResult::error("Calendar integration is not supported on this platform")
}

// ═══════════════════════════════════════════════════════════════════════
// Reminders
// ═══════════════════════════════════════════════════════════════════════

#[cfg(target_os = "macos")]
async fn handle_reminders(action: &str, input: &serde_json::Value) -> ToolResult {
    match action {
        "lists" => {
            run_osascript(
                "tell application \"Reminders\" to return name of every list",
            )
            .await
        }
        "list" => {
            let list = input["list"].as_str().unwrap_or("Reminders");
            let script = format!(
                r#"tell application "Reminders"
    set output to ""
    set rems to every reminder of list "{list}" whose completed is false
    repeat with r in rems
        set output to output & (name of r) & linefeed
    end repeat
    if output is "" then return "No reminders in list '{list}'"
    return output
end tell"#,
                list = escape_applescript(list)
            );
            run_osascript(&script).await
        }
        "create" => {
            let list = input["list"].as_str().unwrap_or("Reminders");
            let name = input["name"].as_str().unwrap_or("");
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for create");
            }
            let script = format!(
                r#"tell application "Reminders"
    tell list "{list}"
        set newReminder to make new reminder with properties {{name:"{name}"}}
        return "Reminder created: " & (name of newReminder)
    end tell
end tell"#,
                list = escape_applescript(list),
                name = escape_applescript(name)
            );
            run_osascript(&script).await
        }
        "complete" => {
            let list = input["list"].as_str().unwrap_or("Reminders");
            let name = input["name"].as_str().unwrap_or("");
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for complete");
            }
            let script = format!(
                r#"tell application "Reminders"
    set completed of (first reminder of list "{list}" whose name is "{name}") to true
    return "Completed: {name}"
end tell"#,
                list = escape_applescript(list),
                name = escape_applescript(name)
            );
            run_osascript(&script).await
        }
        "delete" => {
            let list = input["list"].as_str().unwrap_or("Reminders");
            let name = input["name"].as_str().unwrap_or("");
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for delete");
            }
            let script = format!(
                r#"tell application "Reminders"
    delete (first reminder of list "{list}" whose name is "{name}")
    return "Deleted reminder: {name}"
end tell"#,
                list = escape_applescript(list),
                name = escape_applescript(name)
            );
            run_osascript(&script).await
        }
        _ => ToolResult::error(format!(
            "Unknown reminders action '{}'. Use: lists, list, create, complete, delete",
            action
        )),
    }
}

#[cfg(target_os = "linux")]
async fn handle_reminders(action: &str, _input: &serde_json::Value) -> ToolResult {
    let _ = action;
    ToolResult::error("Reminders integration is not supported on Linux")
}

#[cfg(target_os = "windows")]
async fn handle_reminders(action: &str, _input: &serde_json::Value) -> ToolResult {
    let _ = action;
    ToolResult::error("Reminders integration is not supported on Windows")
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
async fn handle_reminders(action: &str, _input: &serde_json::Value) -> ToolResult {
    let _ = action;
    ToolResult::error("Reminders integration is not supported on this platform")
}

// ═══════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════

#[cfg(target_os = "macos")]
async fn run_osascript(script: &str) -> ToolResult {
    match tokio::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            ToolResult::ok(if text.is_empty() { "OK".to_string() } else { text })
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            ToolResult::error(format!("AppleScript error: {}", stderr))
        }
        Err(e) => ToolResult::error(format!("Failed to run osascript: {}", e)),
    }
}

#[cfg(target_os = "macos")]
fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
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
        assert!(result.is_error);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_escape_applescript() {
        assert_eq!(escape_applescript("hello"), "hello");
        assert_eq!(escape_applescript("say \"hi\""), "say \\\"hi\\\"");
        assert_eq!(escape_applescript("path\\to"), "path\\\\to");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn test_mail_send_missing_to() {
        let result = handle_mail("send", &serde_json::json!({"subject": "test"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("'to' parameter required"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn test_mail_send_missing_subject() {
        let result =
            handle_mail("send", &serde_json::json!({"to": "test@example.com"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("'subject' parameter required"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn test_contacts_search_missing_query() {
        let result = handle_contacts("search", &serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("'query' parameter required"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn test_calendar_create_missing_name() {
        let result = handle_calendar("create", &serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("'name' parameter required"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn test_reminders_create_missing_name() {
        let result = handle_reminders("create", &serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("'name' parameter required"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn test_unknown_mail_action() {
        let result = handle_mail("unknown", &serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Unknown mail action"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn test_unknown_contacts_action() {
        let result = handle_contacts("unknown", &serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Unknown contacts action"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn test_unknown_calendar_action() {
        let result = handle_calendar("unknown", &serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Unknown calendar action"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn test_unknown_reminders_action() {
        let result = handle_reminders("unknown", &serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Unknown reminders action"));
    }
}
