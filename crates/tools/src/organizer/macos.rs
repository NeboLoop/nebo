//! macOS organizer: AppleScript integration with Mail, Contacts, Calendar, Reminders.

use super::OrganizerInput;
use super::shared::{escape_applescript, run_osascript};
use crate::origin::ToolContext;
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
            // Content snippets cost ~0.8s/message via AppleScript, so the cap is
            // 20 (not 50) to stay inside the 30s subprocess budget.
            let limit = input.limit.unwrap_or(10).clamp(1, 20);
            // A bare `mailbox "X"` only resolves "On My Mac" mailboxes — account
            // mailboxes (the normal case) need resolving through their account,
            // so non-inbox reads search every account for the name.
            let resolve = if input.mailbox.is_empty() {
                "set box to inbox".to_string()
            } else {
                format!(
                    r#"set box to missing value
    set wanted to "{name}"
    repeat with acct in accounts
        repeat with mb in (mailboxes of acct)
            if name of mb is wanted then
                set box to mb
                exit repeat
            end if
        end repeat
        if box is not missing value then exit repeat
    end repeat
    if box is missing value then return "Mailbox '" & wanted & "' not found in any account""#,
                    name = escape_applescript(&input.mailbox)
                )
            };
            let script = format!(
                r#"tell application "Mail"
    {resolve}
    set n to count of messages of box
    if n is 0 then return "No messages"
    set lim to {limit}
    if n < lim then set lim to n
    set output to ""
    repeat with i from 1 to lim
        set m to message i of box
        set c to content of m
        if c is missing value then set c to ""
        if length of c > 200 then set c to (characters 1 through 200 of c) as string
        set output to output & "From: " & (sender of m) & linefeed & "Subject: " & (subject of m) & linefeed & "Date: " & (date received of m as text) & linefeed & c & linefeed & "---" & linefeed
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

            script.push_str("\n    send newMsg\nend tell");
            run_osascript(&script).await
        }
        "search" => {
            let query = &input.query;
            if query.is_empty() {
                return ToolResult::error("'query' parameter required for search");
            }
            // Mail's scripting suite has NO `search` verb (the previous
            // `search inbox for …` never even parsed) — a whose-clause over
            // subject + sender is the supported query form, and it's fast
            // (measured ~0.15s over a few-hundred-message inbox).
            let limit = input.limit.unwrap_or(20).clamp(1, 50);
            let script = format!(
                r#"tell application "Mail"
    set found to (messages of inbox whose subject contains "{query}" or sender contains "{query}")
    if (count of found) is 0 then return "No messages found"
    set output to ""
    set i to 0
    repeat with m in found
        if i >= {limit} then exit repeat
        set output to output & "From: " & (sender of m) & " | Subject: " & (subject of m) & " | " & (date received of m as text) & linefeed
        set i to i + 1
    end repeat
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
    // Try native Contacts framework (fast, no Contacts.app activation needed)
    {
        let limit_str = input.limit.map(|l| l.to_string());
        let mut args: Vec<(&str, &str)> = vec![];
        if !input.query.is_empty() {
            args.push(("query", &input.query));
        }
        if !input.name.is_empty() {
            args.push(("name", &input.name));
        }
        if !input.email.is_empty() {
            args.push(("email", &input.email));
        }
        if !input.phone.is_empty() {
            args.push(("phone", &input.phone));
        }
        if !input.company.is_empty() {
            args.push(("company", &input.company));
        }
        if !input.notes.is_empty() {
            args.push(("notes", &input.notes));
        }
        if let Some(ref s) = limit_str {
            args.push(("limit", s));
        }
        if let Some(result) = super::native::run_pim("contacts", action, &args).await {
            return result;
        }
    }

    // AppleScript fallback
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

/// Query events from specific calendars over a date range.
///
/// Uses a single osascript process with AppleScript's `with timeout`
/// per calendar. This avoids spawning 18+ separate processes (which
/// overwhelmed Calendar.app while it was syncing) and lets the app
/// warm up during the first calendar query.
///
/// When preferences are saved, only the selected calendars are queried.
async fn query_calendar_events(
    calendar: &str,
    days: u32,
    store: Option<&std::sync::Arc<db::Store>>,
) -> ToolResult {
    let no_events_msg = if days <= 1 {
        "No events today".to_string()
    } else {
        format!("No upcoming events in the next {} days", days)
    };

    // If a specific calendar is named, query just that one.
    if !calendar.is_empty() {
        let escaped = escape_applescript(calendar);
        let script = format!(
            r#"tell application "Calendar"
    set today to current date
    set time of today to 0
    set endDate to today + ({days} * days)
    set output to ""
    repeat with cal in (every calendar whose name is "{escaped}")
        set evts to (every event of cal whose start date >= today and start date < endDate)
        repeat with e in evts
            set output to output & (name of cal) & " | " & (summary of e) & " | " & (start date of e as text) & linefeed
        end repeat
    end repeat
    if output is "" then return "{no_events_msg}"
    return output
end tell"#,
        );
        return run_osascript(&script).await;
    }

    // Build the calendar filter: use saved preferences if available,
    // otherwise query all calendars.
    let saved_prefs = load_calendar_prefs(store);
    let cal_filter = if let Some(ref prefs) = saved_prefs {
        // AppleScript list literal: {"cal1", "cal2", ...}
        let items: Vec<String> = prefs
            .iter()
            .map(|n| format!("\"{}\"", escape_applescript(n)))
            .collect();
        format!(
            "set targetCals to {{{}}}\n    set allCals to {{}}\n    repeat with cName in targetCals\n        set allCals to allCals & (every calendar whose name is cName)\n    end repeat",
            items.join(", ")
        )
    } else {
        "set allCals to every calendar".to_string()
    };

    // Single osascript process — Calendar.app activates once, warms up
    // during the first calendar, and subsequent queries are fast.
    // `with timeout of 15` gives each calendar's `whose` clause 15s
    // to respond (Apple Event timeout, not wall-clock).
    let script = format!(
        r#"tell application "Calendar"
    set today to current date
    set time of today to 0
    set endDate to today + ({days} * days)
    set output to ""
    set skippedCals to ""
    {cal_filter}
    repeat with cal in allCals
        try
            with timeout of 15 seconds
                set evts to (every event of cal whose start date >= today and start date < endDate)
                repeat with e in evts
                    set output to output & (name of cal) & " | " & (summary of e) & " | " & (start date of e as text) & linefeed
                end repeat
            end timeout
        on error
            set skippedCals to skippedCals & (name of cal) & ", "
        end try
    end repeat
    if skippedCals is not "" then
        set output to output & linefeed & "(Skipped slow calendars: " & text 1 thru -3 of skippedCals & ")"
    end if
    if output is "" then return "{no_events_msg}"
    return output
end tell"#,
    );

    // Overall timeout: generous enough for Calendar.app warmup + all calendars.
    // With preferences (3-5 calendars) this completes in seconds.
    // Without preferences, worst case ~18 calendars × 15s = 4.5min, but in
    // practice most respond in <2s after the first one warms up the app.
    let overall_timeout = if saved_prefs.is_some() {
        std::time::Duration::from_secs(60)
    } else {
        std::time::Duration::from_secs(180)
    };

    let child = match tokio::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return ToolResult::error(format!("Failed to run osascript: {e}")),
    };

    match tokio::time::timeout(overall_timeout, child.wait_with_output()).await {
        Ok(Ok(o)) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if text.is_empty() {
                ToolResult::ok(no_events_msg)
            } else {
                ToolResult::ok(text)
            }
        }
        Ok(Ok(o)) => {
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            ToolResult::error(format!("Calendar query failed: {stderr}"))
        }
        Ok(Err(e)) => ToolResult::error(format!("Calendar process error: {e}")),
        Err(_) => ToolResult::error(
            "Calendar query timed out. Try configuring which calendars to track: \
             organizer(resource: \"calendar\", action: \"configure\")"
                .to_string(),
        ),
    }
}

/// Calendar preferences stored in plugin_settings DB table.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CalendarPrefs {
    #[serde(default)]
    calendars: Vec<String>,
    #[serde(default)]
    auto_accept: bool,
}

const CALENDAR_PLUGIN_NAME: &str = "organizer";
const CALENDAR_PREFS_KEY: &str = "calendar_prefs";

/// Load saved calendar preferences from DB, with migration from legacy file.
fn load_calendar_prefs(store: Option<&std::sync::Arc<db::Store>>) -> Option<Vec<String>> {
    let prefs = load_full_calendar_prefs(store)?;
    if prefs.calendars.is_empty() {
        None
    } else {
        Some(prefs.calendars)
    }
}

fn load_full_calendar_prefs(store: Option<&std::sync::Arc<db::Store>>) -> Option<CalendarPrefs> {
    let store = store?;

    // Try DB first
    if let Ok(Some(json)) = store.get_plugin_setting(CALENDAR_PLUGIN_NAME, CALENDAR_PREFS_KEY) {
        if let Ok(prefs) = serde_json::from_str::<CalendarPrefs>(&json) {
            return Some(prefs);
        }
    }

    // Migrate from legacy file if it exists
    if let Ok(dir) = config::data_dir() {
        let path = dir.join("calendar_preferences.json");
        if let Ok(data) = std::fs::read_to_string(&path) {
            let prefs = if let Ok(p) = serde_json::from_str::<CalendarPrefs>(&data) {
                Some(p)
            } else if let Ok(cals) = serde_json::from_str::<Vec<String>>(&data) {
                Some(CalendarPrefs {
                    calendars: cals,
                    auto_accept: false,
                })
            } else {
                None
            };
            if let Some(ref p) = prefs {
                // Write to DB and remove legacy file
                if save_full_calendar_prefs(Some(store), p).is_ok() {
                    let _ = std::fs::remove_file(&path);
                    tracing::info!("migrated calendar preferences from file to DB");
                }
            }
            return prefs;
        }
    }

    None
}

/// Save calendar preferences to DB.
fn save_calendar_prefs(
    store: Option<&std::sync::Arc<db::Store>>,
    calendars: &[String],
) -> Result<(), String> {
    let mut prefs = load_full_calendar_prefs(store).unwrap_or(CalendarPrefs {
        calendars: vec![],
        auto_accept: false,
    });
    prefs.calendars = calendars.to_vec();
    save_full_calendar_prefs(store, &prefs)
}

fn save_full_calendar_prefs(
    store: Option<&std::sync::Arc<db::Store>>,
    prefs: &CalendarPrefs,
) -> Result<(), String> {
    let store = store.ok_or("DB store not available")?;
    store
        .ensure_skill_plugin(CALENDAR_PLUGIN_NAME)
        .map_err(|e| format!("ensure plugin entry: {e}"))?;
    let json = serde_json::to_string(prefs).map_err(|e| format!("serialize prefs: {e}"))?;
    store
        .set_plugin_setting(CALENDAR_PLUGIN_NAME, CALENDAR_PREFS_KEY, &json)
        .map_err(|e| format!("save to DB: {e}"))?;
    Ok(())
}

pub async fn handle_calendar(
    action: &str,
    input: &OrganizerInput,
    ctx: &ToolContext,
    store: Option<&std::sync::Arc<db::Store>>,
) -> ToolResult {
    // Auto-accept: silently accept pending invites before any read operation
    if matches!(action, "today" | "upcoming" | "list") {
        if let Some(prefs) = load_full_calendar_prefs(store) {
            if prefs.auto_accept {
                // Fire-and-forget: accept pending invites via native helper
                if let Some(result) = super::native::run_pim("calendar", "accept", &[]).await {
                    if !result.is_error {
                        let content = result.content.trim();
                        if !content.is_empty() && !content.contains("No pending") {
                            tracing::info!("auto-accepted calendar invites: {}", content);
                        }
                    }
                }
            }
        }
    }

    // Handle auto_accept toggle
    if action == "auto_accept" {
        let mut prefs = load_full_calendar_prefs(store).unwrap_or(CalendarPrefs {
            calendars: vec![],
            auto_accept: false,
        });
        prefs.auto_accept = !prefs.auto_accept;
        if let Err(e) = save_full_calendar_prefs(store, &prefs) {
            return ToolResult::error(format!("Failed to save preferences: {e}"));
        }
        return ToolResult::ok(format!(
            "Calendar auto-accept is now {}. Pending invitations will be {} accepted when you check your calendar.",
            if prefs.auto_accept { "ON" } else { "OFF" },
            if prefs.auto_accept {
                "automatically"
            } else {
                "not automatically"
            },
        ));
    }

    // Try native EventKit path (fast, reads local SQLite cache, no Calendar.app activation)
    // Skip `configure` which needs ctx.ask_user() (Nebo-specific, not in Swift helper)
    if action != "configure" {
        let days_val = match action {
            "today" => 1i64,
            "upcoming" => input.days.unwrap_or(7).clamp(1, 365),
            "list" => input.days.unwrap_or(30).clamp(1, 365),
            _ => input.days.unwrap_or(1),
        };
        let days_str = days_val.to_string();
        let name = input.event_name();
        let mut args: Vec<(&str, &str)> = vec![];
        if !input.calendar.is_empty() {
            args.push(("calendar", &input.calendar));
        }
        if !input.date.is_empty() {
            args.push(("date", &input.date));
        }
        if !input.end_date.is_empty() {
            args.push(("end_date", &input.end_date));
        }
        if !input.location.is_empty() {
            args.push(("location", &input.location));
        }
        if !input.notes.is_empty() {
            args.push(("notes", &input.notes));
        }
        if !name.is_empty() {
            args.push(("title", name));
        }
        if !input.repeat.is_empty() {
            args.push(("repeat", &input.repeat));
        }
        if !input.repeat_days.is_empty() {
            args.push(("days", &input.repeat_days));
        }
        if !input.end_repeat.is_empty() {
            args.push(("end_repeat", &input.end_repeat));
        }
        let interval_str = input.interval.map(|i| i.to_string());
        if let Some(ref s) = interval_str {
            args.push(("interval", s));
        }
        if matches!(action, "today" | "upcoming" | "list") {
            args.push(("days", &days_str));
        }
        if let Some(result) = super::native::run_pim("calendar", action, &args).await {
            return result;
        }
    }

    // AppleScript fallback
    match action {
        "configure" => {
            // List all calendars via AppleScript
            let result =
                run_osascript("tell application \"Calendar\" to return name of every calendar")
                    .await;
            if result.is_error {
                return result;
            }

            let all_cals: Vec<String> = result
                .content
                .split(", ")
                .map(|s| s.trim().to_string())
                .collect();
            if all_cals.is_empty() {
                return ToolResult::error("No calendars found on this system");
            }

            // Build checkbox widget
            let current_prefs = load_calendar_prefs(store);
            let prompt = if current_prefs.is_some() {
                "Select which calendars Nebo should track (updating your saved preferences):"
            } else {
                "Select which calendars Nebo should track:"
            };

            let widgets = serde_json::json!([{
                "type": "checkbox",
                "label": "Calendars",
                "options": all_cals,
            }]);

            match ctx.ask_user(prompt, widgets).await {
                Some(response) if !response.is_empty() => {
                    let selected: Vec<String> =
                        response.split(", ").map(|s| s.trim().to_string()).collect();
                    if let Err(e) = save_calendar_prefs(store, &selected) {
                        return ToolResult::error(format!("Failed to save preferences: {e}"));
                    }
                    ToolResult::ok(format!(
                        "Calendar preferences saved. Now tracking {} calendar(s): {}",
                        selected.len(),
                        selected.join(", ")
                    ))
                }
                _ => ToolResult::ok(
                    "Calendar configuration cancelled — no changes made.".to_string(),
                ),
            }
        }
        "calendars" => {
            run_osascript("tell application \"Calendar\" to return name of every calendar").await
        }
        "today" => query_calendar_events(&input.calendar, 1, store).await,
        "upcoming" => {
            let days = input.days.unwrap_or(7).clamp(1, 365) as u32;
            query_calendar_events(&input.calendar, days, store).await
        }
        "list" => {
            let days = input.days.unwrap_or(30).clamp(1, 365) as u32;
            query_calendar_events(&input.calendar, days, store).await
        }
        "create" => {
            let name = input.event_name();
            if name.is_empty() {
                return ToolResult::error("'name' or 'title' parameter required for create");
            }
            if input.date.is_empty() {
                return ToolResult::error(
                    "'date' parameter required for create (e.g. '2024-06-15 14:00')",
                );
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
        "delete" => {
            let name = input.event_name();
            if name.is_empty() {
                return ToolResult::error("'name' or 'title' parameter required for delete");
            }
            let calendar_filter = if input.calendar.is_empty() {
                String::new()
            } else {
                format!(
                    " whose name of its calendar is \"{}\"",
                    escape_applescript(&input.calendar)
                )
            };
            let script = format!(
                r#"tell application "Calendar"
    set matchingEvents to every event of every calendar{filter} whose summary is "{name}"
    set deletedCount to 0
    repeat with evList in matchingEvents
        repeat with ev in evList
            delete ev
            set deletedCount to deletedCount + 1
        end repeat
    end repeat
    return "Deleted " & deletedCount & " event(s) matching '{name}'"
end tell"#,
                filter = calendar_filter,
                name = escape_applescript(name),
            );
            run_osascript(&script).await
        }
        _ => ToolResult::error(format!(
            "Unknown calendar action '{}'. Use: calendars, today, upcoming, create, delete, pending, accept, decline, auto_accept, list, configure",
            action
        )),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Reminders
// ═══════════════════════════════════════════════════════════════════════

pub async fn handle_reminders(action: &str, input: &OrganizerInput) -> ToolResult {
    // Try native EventKit path (fast, no Reminders.app activation needed)
    {
        let name = input.event_name();
        let pri_str = input.priority.map(|p| p.to_string());
        let mut args: Vec<(&str, &str)> = vec![];
        if !name.is_empty() {
            args.push(("name", name));
        }
        if !input.list.is_empty() {
            args.push(("list", &input.list));
        }
        if !input.notes.is_empty() {
            args.push(("notes", &input.notes));
        }
        if !input.due_date.is_empty() {
            args.push(("due_date", &input.due_date));
        }
        if let Some(ref s) = pri_str {
            args.push(("priority", s));
        }
        if let Some(result) = super::native::run_pim("reminders", action, &args).await {
            return result;
        }
    }

    // AppleScript fallback
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
                props.push_str(&format!(", body:\"{}\"", escape_applescript(&input.notes)));
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
