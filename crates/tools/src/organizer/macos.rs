//! macOS organizer: AppleScript integration with Mail, Contacts, Calendar, Reminders.

use super::shared::{escape_applescript, run_osascript};
use super::OrganizerInput;
use crate::registry::ToolResult;

// ═══════════════════════════════════════════════════════════════════════
// Mail
// ═══════════════════════════════════════════════════════════════════════

pub async fn handle_mail(action: &str, input: &OrganizerInput) -> ToolResult {
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
            let limit = input.limit.unwrap_or(10).clamp(1, 50);
            let mailbox = if input.mailbox.is_empty() {
                "inbox".to_string()
            } else {
                format!("mailbox \"{}\"", escape_applescript(&input.mailbox))
            };
            let script = format!(
                r#"tell application "Mail"
    set msgs to (messages 1 through {limit} of {mailbox})
    set output to ""
    repeat with m in msgs
        set output to output & "From: " & (sender of m) & linefeed & "Subject: " & (subject of m) & linefeed & "Date: " & (date received of m as text) & linefeed & "---" & linefeed
    end repeat
    return output
end tell"#,
            );
            run_osascript(&script).await
        }
        "send" => {
            if input.to.is_empty() {
                return ToolResult::error("'to' parameter required for send");
            }
            if input.subject.is_empty() {
                return ToolResult::error("'subject' parameter required for send");
            }

            let mut script = format!(
                r#"tell application "Mail"
    set newMsg to make new outgoing message with properties {{subject:"{subject}", content:"{body}", visible:true}}"#,
                subject = escape_applescript(&input.subject),
                body = escape_applescript(&input.body),
            );

            // To recipients
            for addr in &input.to {
                script.push_str(&format!(
                    "\n    tell newMsg to make new to recipient with properties {{address:\"{}\"}}",
                    escape_applescript(addr)
                ));
            }

            // CC recipients
            for addr in &input.cc {
                script.push_str(&format!(
                    "\n    tell newMsg to make new cc recipient with properties {{address:\"{}\"}}",
                    escape_applescript(addr)
                ));
            }

            script.push_str(
                "\n    send newMsg\nend tell",
            );
            run_osascript(&script).await
        }
        "search" => {
            let query = &input.query;
            if query.is_empty() {
                return ToolResult::error("'query' parameter required for search");
            }
            let limit = input.limit.unwrap_or(20).clamp(1, 50);
            let script = format!(
                r#"tell application "Mail"
    set found to (search inbox for "{query}")
    set output to ""
    set i to 0
    repeat with m in found
        if i >= {limit} then exit repeat
        set output to output & "From: " & (sender of m) & " | Subject: " & (subject of m) & linefeed
        set i to i + 1
    end repeat
    if output is "" then return "No messages found"
    return output
end tell"#,
                query = escape_applescript(query),
            );
            run_osascript(&script).await
        }
        _ => ToolResult::error(format!(
            "Unknown mail action '{}'. Use: accounts, unread, read, send, search",
            action
        )),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Contacts
// ═══════════════════════════════════════════════════════════════════════

pub async fn handle_contacts(action: &str, input: &OrganizerInput) -> ToolResult {
    match action {
        "search" => {
            let query = &input.query;
            if query.is_empty() {
                return ToolResult::error("'query' parameter required for search");
            }
            let script = format!(
                r#"tell application "Contacts"
    set found to every person whose name contains "{query}"
    set output to ""
    repeat with p in found
        set output to output & (name of p)
        try
            set output to output & " | " & (value of first email of p)
        end try
        set output to output & linefeed
    end repeat
    if output is "" then return "No contacts found"
    return output
end tell"#,
                query = escape_applescript(query)
            );
            run_osascript(&script).await
        }
        "get" => {
            let name = &input.name;
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for get");
            }
            let script = format!(
                r#"tell application "Contacts"
    set p to first person whose name is "{name}"
    set output to "Name: " & (name of p) & linefeed
    try
        set emails to every email of p
        repeat with e in emails
            set output to output & "Email (" & (label of e) & "): " & (value of e) & linefeed
        end repeat
    end try
    try
        set phones to every phone of p
        repeat with ph in phones
            set output to output & "Phone (" & (label of ph) & "): " & (value of ph) & linefeed
        end repeat
    end try
    try
        set output to output & "Company: " & (organization of p) & linefeed
    end try
    try
        set output to output & "Notes: " & (note of p) & linefeed
    end try
    return output
end tell"#,
                name = escape_applescript(name)
            );
            run_osascript(&script).await
        }
        "create" => {
            let name = &input.name;
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

            let mut script = format!(
                r#"tell application "Contacts"
    set newPerson to make new person with properties {{first name:"{first}", last name:"{last}"}}"#,
            );

            if !input.email.is_empty() {
                script.push_str(&format!(
                    "\n    tell newPerson to make new email at end of emails with properties {{label:\"work\", value:\"{}\"}}",
                    escape_applescript(&input.email)
                ));
            }
            if !input.phone.is_empty() {
                script.push_str(&format!(
                    "\n    tell newPerson to make new phone at end of phones with properties {{label:\"mobile\", value:\"{}\"}}",
                    escape_applescript(&input.phone)
                ));
            }
            if !input.company.is_empty() {
                script.push_str(&format!(
                    "\n    set organization of newPerson to \"{}\"",
                    escape_applescript(&input.company)
                ));
            }
            if !input.notes.is_empty() {
                script.push_str(&format!(
                    "\n    set note of newPerson to \"{}\"",
                    escape_applescript(&input.notes)
                ));
            }

            script.push_str(
                "\n    save\n    return \"Contact created: \" & (name of newPerson)\nend tell",
            );
            run_osascript(&script).await
        }
        "groups" => {
            run_osascript("tell application \"Contacts\" to return name of every group").await
        }
        _ => ToolResult::error(format!(
            "Unknown contacts action '{}'. Use: search, get, create, groups",
            action
        )),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Calendar
// ═══════════════════════════════════════════════════════════════════════

pub async fn handle_calendar(action: &str, input: &OrganizerInput) -> ToolResult {
    match action {
        "calendars" => {
            run_osascript("tell application \"Calendar\" to return name of every calendar").await
        }
        "today" => {
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
            let days = input.days.unwrap_or(7).clamp(1, 365);
            let script = format!(
                r#"tell application "Calendar"
    set today to current date
    set endDate to today + ({days} * days)
    set output to ""
    repeat with cal in every calendar
        set evts to (every event of cal whose start date >= today and start date < endDate)
        repeat with e in evts
            set output to output & (name of cal) & " | " & (summary of e) & " | " & (start date of e as text) & linefeed
        end repeat
    end repeat
    if output is "" then return "No upcoming events in the next {days} days"
    return output
end tell"#,
            );
            run_osascript(&script).await
        }
        "list" => {
            let days = input.days.unwrap_or(30).clamp(1, 365);
            let script = format!(
                r#"tell application "Calendar"
    set today to current date
    set endDate to today + ({days} * days)
    set output to ""
    repeat with cal in every calendar
        set evts to (every event of cal whose start date >= today and start date < endDate)
        repeat with e in evts
            set output to output & (name of cal) & " | " & (summary of e) & " | " & (start date of e as text) & linefeed
        end repeat
    end repeat
    if output is "" then return "No events in the next {days} days"
    return output
end tell"#,
            );
            run_osascript(&script).await
        }
        "create" => {
            let name = input.event_name();
            if name.is_empty() {
                return ToolResult::error("'name' or 'title' parameter required for create");
            }
            if input.date.is_empty() {
                return ToolResult::error("'date' parameter required for create (e.g. '2024-06-15 14:00')");
            }

            let start_dt = match super::shared::parse_date(&input.date) {
                Ok(dt) => dt,
                Err(e) => return ToolResult::error(e),
            };

            let end_dt = if !input.end_date.is_empty() {
                match super::shared::parse_date(&input.end_date) {
                    Ok(dt) => dt,
                    Err(e) => return ToolResult::error(format!("Invalid end_date: {}", e)),
                }
            } else {
                start_dt + chrono::Duration::hours(1)
            };

            let calendar = if input.calendar.is_empty() {
                "Calendar".to_string()
            } else {
                input.calendar.clone()
            };

            // AppleScript date format: "January 2, 2006 at 3:04:05 PM"
            let start_str = start_dt.format("%B %e, %Y at %I:%M:%S %p").to_string();
            let end_str = end_dt.format("%B %e, %Y at %I:%M:%S %p").to_string();

            let mut props = format!(
                "summary:\"{}\", start date:date \"{}\", end date:date \"{}\"",
                escape_applescript(name),
                escape_applescript(&start_str),
                escape_applescript(&end_str),
            );

            if !input.notes.is_empty() {
                props.push_str(&format!(
                    ", description:\"{}\"",
                    escape_applescript(&input.notes)
                ));
            }
            if !input.location.is_empty() {
                props.push_str(&format!(
                    ", location:\"{}\"",
                    escape_applescript(&input.location)
                ));
            }

            let script = format!(
                r#"tell application "Calendar"
    tell calendar "{calendar}"
        set newEvent to make new event with properties {{{props}}}
        return "Event created: " & (summary of newEvent)
    end tell
end tell"#,
                calendar = escape_applescript(&calendar),
            );
            run_osascript(&script).await
        }
        _ => ToolResult::error(format!(
            "Unknown calendar action '{}'. Use: calendars, today, upcoming, create, list",
            action
        )),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Reminders
// ═══════════════════════════════════════════════════════════════════════

pub async fn handle_reminders(action: &str, input: &OrganizerInput) -> ToolResult {
    match action {
        "lists" => {
            run_osascript("tell application \"Reminders\" to return name of every list").await
        }
        "list" => {
            let list = if input.list.is_empty() {
                "Reminders"
            } else {
                &input.list
            };
            let script = format!(
                r#"tell application "Reminders"
    set output to ""
    set rems to every reminder of list "{list}" whose completed is false
    repeat with r in rems
        set line to (name of r)
        try
            if due date of r is not missing value then
                set line to line & " | Due: " & (due date of r as text)
            end if
        end try
        try
            if priority of r > 0 then
                set line to line & " | Priority: " & (priority of r)
            end if
        end try
        set output to output & line & linefeed
    end repeat
    if output is "" then return "No reminders in list '{list}'"
    return output
end tell"#,
                list = escape_applescript(list)
            );
            run_osascript(&script).await
        }
        "create" => {
            let name = input.event_name();
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for create");
            }

            let list = if input.list.is_empty() {
                "Reminders"
            } else {
                &input.list
            };

            let mut props = format!("name:\"{}\"", escape_applescript(name));

            if !input.notes.is_empty() {
                props.push_str(&format!(
                    ", body:\"{}\"",
                    escape_applescript(&input.notes)
                ));
            }

            // Priority: 1-4 = high (1), 5 = medium (5), 6-9 = low (9)
            if let Some(pri) = input.priority {
                let as_pri = match pri {
                    1..=4 => 1,
                    5 => 5,
                    _ => 9,
                };
                props.push_str(&format!(", priority:{}", as_pri));
            }

            let mut script = format!(
                r#"tell application "Reminders"
    tell list "{list}"
        set newReminder to make new reminder with properties {{{props}}}"#,
                list = escape_applescript(list),
            );

            // Due date (parsed separately since AppleScript needs a date object)
            if !input.due_date.is_empty() {
                match super::shared::parse_date(&input.due_date) {
                    Ok(dt) => {
                        let date_str = dt.format("%B %e, %Y at %I:%M:%S %p").to_string();
                        script.push_str(&format!(
                            "\n        set due date of newReminder to date \"{}\"",
                            escape_applescript(&date_str)
                        ));
                    }
                    Err(e) => return ToolResult::error(format!("Invalid due_date: {}", e)),
                }
            }

            script.push_str(
                "\n        return \"Reminder created: \" & (name of newReminder)\n    end tell\nend tell",
            );
            run_osascript(&script).await
        }
        "complete" => {
            let name = input.event_name();
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for complete");
            }
            let list = if input.list.is_empty() {
                "Reminders"
            } else {
                &input.list
            };
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
            let name = input.event_name();
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for delete");
            }
            let list = if input.list.is_empty() {
                "Reminders"
            } else {
                &input.list
            };
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
