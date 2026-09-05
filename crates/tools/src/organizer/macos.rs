//! macOS organizer: AppleScript integration with Mail, Contacts, Calendar, Reminders.

use super::OrganizerInput;
use super::shared::{escape_applescript, run_osascript};
use crate::errors::missing_param;
use crate::origin::ToolContext;
use crate::registry::ToolResult;

// ═══════════════════════════════════════════════════════════════════════
// Mail
// ═══════════════════════════════════════════════════════════════════════

/// AppleScript can only return strings, so the scripts prefix diagnostic
/// (non-content) outcomes with "DIAG|": count-vs-content mismatches and
/// exact-name misses must surface as tool errors, never as an empty-looking
/// success. Used by the mail, contacts and reminders scripts.
fn diag(result: ToolResult) -> ToolResult {
    if !result.is_error {
        if let Some(msg) = result.content.strip_prefix("DIAG|") {
            return ToolResult::error(msg.to_string());
        }
    }
    result
}

/// `read` fetches bodies at ~0.8 s each via AppleScript, so it is capped
/// lower than `search` to stay inside the 30 s subprocess budget.
const MAIL_READ_LIMIT_CAP: i64 = 20;
const MAIL_SEARCH_LIMIT_CAP: i64 = 50;
const MAIL_SEND_EXAMPLE: &str = "organizer(resource: \"mail\", action: \"send\", to: [\"pat@example.com\"], subject: \"Invoice 42\", body: \"Attached is the invoice.\")";

pub async fn handle_mail(action: &str, input: &OrganizerInput) -> ToolResult {
    match action {
        "accounts" => {
            run_osascript(
                r#"tell application "Mail"
    set out to ""
    repeat with a in accounts
        set out to out & (name of a) & " = " & (email addresses of a as string) & linefeed
    end repeat
    return out
end tell"#,
            )
            .await
        }
        "unread" => {
            // Per-account breakdown so the caller can tell which account the
            // unread mail lives in (the unified count alone is ambiguous).
            run_osascript(
                r#"tell application "Mail"
    set total to 0
    set out to ""
    repeat with a in accounts
        try
            set mb to mailbox "INBOX" of a
            set u to count of (messages of mb whose read status is false)
            set total to total + u
            set out to out & (name of a) & " (" & (email addresses of a as string) & "): " & u & " unread" & linefeed
        end try
    end repeat
    return out & "Total: " & total & " unread"
end tell"#,
            )
            .await
        }
        "read" => {
            // Content snippets cost ~0.8s/message via AppleScript, so the cap is
            // 20 (not 50) to stay inside the 30s subprocess budget.
            let limit = input.limit.unwrap_or(10).clamp(1, MAIL_READ_LIMIT_CAP);
            // Read per-account INBOXes (newest-first), never the unified `inbox`:
            // the unified object orders by account, so a chatty secondary account
            // (e.g. iCloud onboarding mail) shadows the one that matters. An
            // optional `account` filter (name or address) narrows to one account;
            // `mailbox` names a non-INBOX mailbox within the account(s).
            //
            // Failure honesty: Mail's unread counters and its message enumeration
            // are different data sources — enumeration returns 0 while unread > 0
            // when Mail's automation session hasn't synced (Mail closed / mid-sync).
            // Every "nothing came back" shape must say WHY (DIAG| → tool error),
            // never masquerade as an empty inbox. One un-fetchable body must not
            // zero the whole batch, so content is fetched per-message under its
            // own try + 5s timeout.
            let script = format!(
                r#"tell application "Mail"
    set wantedAcct to "{acct}"
    set wantedBox to "{mbox}"
    if wantedBox is "" then set wantedBox to "INBOX"
    set targets to {{}}
    set skipped to ""
    repeat with a in accounts
        if wantedAcct is "" or (name of a is wantedAcct) or ((email addresses of a as string) contains wantedAcct) then
            try
                set end of targets to mailbox wantedBox of a
            on error errMsg
                set skipped to skipped & (name of a) & ": " & errMsg & "; "
            end try
        end if
    end repeat
    if (count of targets) is 0 then
        if skipped is not "" then return "DIAG|Accounts matched but mailbox " & wantedBox & " could not be opened — " & skipped
        return "DIAG|No matching account/mailbox. Use action 'accounts' to list accounts."
    end if
    set lim to {limit}
    set output to ""
    set taken to 0
    set unreadTotal to 0
    set enumerated to 0
    repeat with box in targets
        set acctName to name of account of box
        try
            set unreadTotal to unreadTotal + (unread count of box)
        end try
        set n to count of messages of box
        set enumerated to enumerated + n
        set i to 1
        repeat while i <= n and taken < lim
            set m to message i of box
            set c to ""
            try
                with timeout of 5 seconds
                    set c to content of m
                end timeout
                if c is missing value then set c to ""
                if length of c > 200 then set c to ((characters 1 through 200 of c) as string) & " …[body truncated at 200 characters]"
            on error errMsg
                set c to "[body unavailable: " & errMsg & "]"
            end try
            set output to output & "Account: " & acctName & linefeed & "From: " & (sender of m) & linefeed & "Subject: " & (subject of m) & linefeed & "Date: " & (date received of m as text) & linefeed & c & linefeed & "---" & linefeed
            set taken to taken + 1
            set i to i + 1
        end repeat
    end repeat
    if output is "" and unreadTotal > 0 then
        set n to count of messages of inbox
        set enumerated to n
        set i to 1
        repeat while i <= n and taken < lim
            set m to message i of inbox
            set acctName to "unknown"
            try
                set acctName to name of account of mailbox of m
            end try
            set c to ""
            try
                with timeout of 5 seconds
                    set c to content of m
                end timeout
                if c is missing value then set c to ""
                if length of c > 200 then set c to ((characters 1 through 200 of c) as string) & " …[body truncated at 200 characters]"
            on error errMsg
                set c to "[body unavailable: " & errMsg & "]"
            end try
            set output to output & "Account: " & acctName & linefeed & "From: " & (sender of m) & linefeed & "Subject: " & (subject of m) & linefeed & "Date: " & (date received of m as text) & linefeed & c & linefeed & "---" & linefeed
            set taken to taken + 1
            set i to i + 1
        end repeat
        if output is not "" then
            set output to output & "(account names unavailable for these messages)" & linefeed
            if enumerated > taken then set output to output & "(showing " & taken & " of " & enumerated & " messages; limit is capped at {cap})" & linefeed
            return output
        end if
        return "DIAG|Mail reports " & unreadTotal & " unread but message enumeration returned 0 messages (per-account and unified). Mail.app is not exposing any messages: open Mail, let it finish syncing, and call again."
    end if
    if output is "" and skipped is not "" then return "DIAG|0 messages in readable mailboxes; accounts skipped: " & skipped
    if output is "" then
        set emptyMsg to "0 messages in " & wantedBox & " across " & (count of targets) & " account(s)."
        if enumerated is 0 then set emptyMsg to emptyMsg & " Mail.app is not exposing any messages."
        return emptyMsg
    end if
    if enumerated > taken then set output to output & "(showing " & taken & " of " & enumerated & " messages in " & wantedBox & "; limit is capped at {cap})" & linefeed
    if skipped is not "" then set output to output & "(skipped accounts: " & skipped & ")" & linefeed
    return output
end tell"#,
                acct = escape_applescript(&input.account),
                mbox = escape_applescript(&input.mailbox),
                cap = MAIL_READ_LIMIT_CAP,
            );
            diag(run_osascript(&script).await)
        }
        "send" => {
            if input.to.is_empty() {
                return ToolResult::error(missing_param("send", "to", MAIL_SEND_EXAMPLE));
            }
            if input.subject.is_empty() {
                return ToolResult::error(missing_param("send", "subject", MAIL_SEND_EXAMPLE));
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

            script.push_str(&format!(
                "\n    send newMsg\n    return \"Handed to Mail for delivery to {}\"\nend tell",
                escape_applescript(&input.to.join(", "))
            ));
            run_osascript(&script).await
        }
        "search" => {
            let query = &input.query;
            if query.is_empty() {
                return ToolResult::error(missing_param(
                    "search",
                    "query",
                    "organizer(resource: \"mail\", action: \"search\", query: \"invoice\")",
                ));
            }
            // Mail's scripting suite has NO `search` verb (the previous
            // `search inbox for …` never even parsed) — a whose-clause over
            // subject + sender is the supported query form, and it's fast
            // (measured ~0.15s over a few-hundred-message inbox). Searched
            // per-account (see `read` for why the unified inbox misleads);
            // optional `account` narrows to one.
            let limit = input.limit.unwrap_or(20).clamp(1, MAIL_SEARCH_LIMIT_CAP);
            // Same failure honesty as `read`: when enumeration is dead (0 messages
            // visible while unread counters say otherwise), "no matches" would be
            // a lie — return a DIAG explaining the sync state instead.
            let script = format!(
                r#"tell application "Mail"
    set wantedAcct to "{acct}"
    set output to ""
    set taken to 0
    set unreadTotal to 0
    set enumerated to 0
    set matched to 0
    set searchedAccts to 0
    set skipped to ""
    repeat with a in accounts
        if wantedAcct is "" or (name of a is wantedAcct) or ((email addresses of a as string) contains wantedAcct) then
            try
                set mb to mailbox "INBOX" of a
                set searchedAccts to searchedAccts + 1
                try
                    set unreadTotal to unreadTotal + (unread count of mb)
                end try
                set enumerated to enumerated + (count of messages of mb)
                set found to (messages of mb whose subject contains "{query}" or sender contains "{query}")
                set matched to matched + (count of found)
                repeat with m in found
                    if taken >= {limit} then exit repeat
                    set output to output & (name of a) & " | From: " & (sender of m) & " | Subject: " & (subject of m) & " | " & (date received of m as text) & linefeed
                    set taken to taken + 1
                end repeat
            on error errMsg
                set skipped to skipped & (name of a) & ": " & errMsg & "; "
            end try
        end if
    end repeat
    if output is "" and enumerated is 0 and unreadTotal > 0 then return "DIAG|Mail reports " & unreadTotal & " unread but message enumeration returned 0 messages. Mail.app is not exposing any messages: open Mail, let it finish syncing, and call again."
    if output is "" and skipped is not "" then return "DIAG|No messages whose subject or sender contains '{query}', and some accounts could not be searched: " & skipped
    if output is "" then return "No messages whose subject or sender contains '{query}' (searched " & enumerated & " messages in the INBOX of " & searchedAccts & " account(s)). Search is a plain substring; operators like from: are not supported."
    if matched > taken then set output to output & "(showing " & taken & " of " & matched & " matching messages; limit is capped at {cap})" & linefeed
    if skipped is not "" then set output to output & "(skipped accounts: " & skipped & ")" & linefeed
    return output
end tell"#,
                acct = escape_applescript(&input.account),
                query = escape_applescript(query),
                cap = MAIL_SEARCH_LIMIT_CAP,
            );
            diag(run_osascript(&script).await)
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
                return ToolResult::error(missing_param(
                    "search",
                    "query",
                    "organizer(resource: \"contacts\", action: \"search\", query: \"Pat\")",
                ));
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
    if output is "" then return "No contacts whose name contains '{query}'."
    return output
end tell"#,
                query = escape_applescript(query)
            );
            run_osascript(&script).await
        }
        "get" => {
            let name = &input.name;
            if name.is_empty() {
                return ToolResult::error(missing_param(
                    "get",
                    "name",
                    "organizer(resource: \"contacts\", action: \"get\", name: \"Pat Smith\")",
                ));
            }
            let script = format!(
                r#"tell application "Contacts"
    try
        set p to first person whose name is "{name}"
    on error errMsg number errNum
        if errNum is -1728 then return "DIAG|No contact named exactly '{name}'. Use action 'search' for partial matches."
        error errMsg number errNum
    end try
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
            diag(run_osascript(&script).await)
        }
        "create" => {
            let name = &input.name;
            if name.is_empty() {
                return ToolResult::error(missing_param(
                    "create",
                    "name",
                    "organizer(resource: \"contacts\", action: \"create\", name: \"Pat Smith\", email: \"pat@example.com\")",
                ));
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

/// AppleScript handlers used by the calendar queries.
///
/// Defined outside the `tell` block (handlers must be top-level) and called
/// with `my …` from inside it. They only ever touch plain text, never app
/// objects, so no Apple event round trip is involved.
const CALENDAR_TEXT_HANDLERS: &str = r#"on flattenText(t)
    set flatOut to ""
    repeat with p in paragraphs of (t as text)
        set pt to p as text
        if flatOut is "" then
            set flatOut to pt
        else if pt is not "" then
            set flatOut to flatOut & ", " & pt
        end if
    end repeat
    return flatOut
end flattenText

on quoteLines(t)
    set quoteOut to ""
    repeat with p in paragraphs of (t as text)
        set quoteOut to quoteOut & "      > " & (p as text) & linefeed
    end repeat
    return quoteOut
end quoteLines
"#;

/// AppleScript that renders the event `e` of calendar `cal` into `output`.
///
/// Shared verbatim by both calendar queries so the single-calendar and
/// all-calendar paths can never drift. Every property is read into a local
/// first (`date string` etc. only work on values, not app references) and
/// guarded by its own `try` — Calendar.app raises on properties an account
/// doesn't expose, and one bad property must never kill the whole query.
/// Unset properties come back as `missing value`, which coerces to the
/// literal text "missing value", so each one is checked explicitly.
///
/// Parity with the native EventKit path (see pim_helper.swift), minus
/// organizer and free/busy availability — Calendar.app's scripting
/// dictionary exposes neither.
const CALENDAR_EVENT_RENDER: &str = r#"set evSummary to "Untitled"
                    try
                        set evTmp to summary of e
                        if evTmp is not missing value then set evSummary to my flattenText(evTmp)
                    end try
                    set evOut to "- [" & (name of cal) & "] " & evSummary
                    set evAllDay to false
                    try
                        set evAllDay to (allday event of e) is true
                    end try
                    set evStart to missing value
                    set evEnd to missing value
                    try
                        set evStart to start date of e
                    end try
                    try
                        set evEnd to end date of e
                    end try
                    if evStart is not missing value then
                        if evAllDay then
                            set evWhen to (date string of evStart)
                            if evEnd is not missing value then
                                set evLast to evEnd - 1
                                if (date string of evLast) is not (date string of evStart) then set evWhen to evWhen & " – " & (date string of evLast)
                            end if
                            set evWhen to evWhen & " (all day)"
                        else
                            set evWhen to (date string of evStart) & " " & (time string of evStart)
                            if evEnd is not missing value then
                                if (date string of evEnd) is (date string of evStart) then
                                    set evWhen to evWhen & " – " & (time string of evEnd)
                                else
                                    set evWhen to evWhen & " – " & (date string of evEnd) & " " & (time string of evEnd)
                                end if
                            end if
                        end if
                        set evOut to evOut & " — " & evWhen
                    end if
                    set evOut to evOut & linefeed
                    try
                        set evLoc to location of e
                        if evLoc is not missing value then
                            set evLoc to my flattenText(evLoc)
                            if evLoc is not "" then set evOut to evOut & "    Location: " & evLoc & linefeed
                        end if
                    end try
                    try
                        set evUrl to url of e
                        if evUrl is not missing value and (evUrl as text) is not "" then set evOut to evOut & "    URL: " & (evUrl as text) & linefeed
                    end try
                    try
                        set evAtt to ""
                        repeat with a in attendees of e
                            set attLine to ""
                            try
                                set attName to display name of a
                                if attName is not missing value then set attLine to attName as text
                            end try
                            set attMail to ""
                            try
                                set attTmp to email of a
                                if attTmp is not missing value then set attMail to attTmp as text
                            end try
                            if attLine is "" then
                                set attLine to attMail
                            else if attMail is not "" and attMail is not attLine then
                                set attLine to attLine & " <" & attMail & ">"
                            end if
                            if attLine is not "" then
                                try
                                    set attStat to (participation status of a) as text
                                    if attStat is not "unknown" then set attLine to attLine & " [" & attStat & "]"
                                end try
                                if evAtt is "" then
                                    set evAtt to attLine
                                else
                                    set evAtt to evAtt & "; " & attLine
                                end if
                            end if
                        end repeat
                        if evAtt is not "" then set evOut to evOut & "    Attendees: " & evAtt & linefeed
                    end try
                    try
                        set evStat to (status of e) as text
                        if evStat is not "confirmed" and evStat is not "none" then set evOut to evOut & "    Status: " & evStat & linefeed
                    end try
                    try
                        set evRec to recurrence of e
                        if evRec is not missing value and (evRec as text) is not "" then set evOut to evOut & "    Repeats: " & (evRec as text) & linefeed
                    end try
                    try
                        set evUid to uid of e
                        if evUid is not missing value and (evUid as text) is not "" then set evOut to evOut & "    ID: " & (evUid as text) & linefeed
                    end try
                    try
                        set evNotes to description of e
                        if evNotes is not missing value and (evNotes as text) is not "" then set evOut to evOut & "    Notes:" & linefeed & my quoteLines(evNotes as text)
                    end try
                    set output to output & evOut"#;

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
    let range = if days <= 1 {
        "today".to_string()
    } else {
        format!("in the next {} days", days)
    };

    // If a specific calendar is named, query just that one.
    if !calendar.is_empty() {
        let escaped = escape_applescript(calendar);
        let no_events_msg = escape_applescript(&format!(
            "No events {} in calendar '{}'.",
            range, calendar
        ));
        let script = format!(
            r#"{CALENDAR_TEXT_HANDLERS}
tell application "Calendar"
    set today to current date
    set time of today to 0
    set endDate to today + ({days} * days)
    set output to ""
    repeat with cal in (every calendar whose name is "{escaped}")
        set evts to (every event of cal whose start date >= today and start date < endDate)
        repeat with e in evts
                    {CALENDAR_EVENT_RENDER}
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
    // The empty result names which calendars were read, so "no events" is
    // never mistaken for "no events anywhere" when only tracked ones were.
    let no_events_plain = match saved_prefs {
        Some(ref prefs) => format!(
            "No events {} in the {} tracked calendar(s): {}. Untracked calendars were not read; change the set with organizer(resource: \"calendar\", action: \"configure\").",
            range,
            prefs.len(),
            prefs.join(", ")
        ),
        None => format!("No events {} in any calendar (all calendars read).", range),
    };
    let no_events_msg = escape_applescript(&no_events_plain);
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
        r#"{CALENDAR_TEXT_HANDLERS}
tell application "Calendar"
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
                    {CALENDAR_EVENT_RENDER}
                end repeat
            end timeout
        on error
            set skippedCals to skippedCals & (name of cal) & ", "
        end try
    end repeat
    if skippedCals is not "" then
        set output to output & linefeed & "(Calendars not read (error or 15 s timeout): " & text 1 thru -3 of skippedCals & ")"
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
                ToolResult::ok(no_events_plain)
            } else {
                ToolResult::ok(text)
            }
        }
        Ok(Ok(o)) => ToolResult::error(format!(
            "Calendar query failed: {}",
            super::shared::exit_error("osascript", &o)
        )),
        Ok(Err(e)) => ToolResult::error(format!("Calendar process error: {e}")),
        Err(_) => ToolResult::error(format!(
            "Calendar query exceeded {} s and was killed. Narrow it: name one calendar, or choose which calendars to track with organizer(resource: \"calendar\", action: \"configure\").",
            overall_timeout.as_secs()
        )),
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

const CALENDAR_CREATE_EXAMPLE: &str = "organizer(resource: \"calendar\", action: \"create\", title: \"Dentist\", date: \"2026-09-15 14:00\", end_date: \"2026-09-15 15:00\")";

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
                return ToolResult::error(missing_param("create", "title", CALENDAR_CREATE_EXAMPLE));
            }
            if input.date.is_empty() {
                return ToolResult::error(missing_param("create", "date", CALENDAR_CREATE_EXAMPLE));
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
                return ToolResult::error(missing_param(
                    "delete",
                    "title",
                    "organizer(resource: \"calendar\", action: \"delete\", title: \"Dentist\")",
                ));
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
        "pending" | "accept" | "decline" => ToolResult::error(format!(
            "'{}' needs the native calendar helper, which is not available on this machine. Calendar.app's scripting does not expose invitations.",
            action
        )),
        _ => ToolResult::error(format!(
            "Unknown calendar action '{}'. Use: calendars, today, upcoming, list, create, delete, auto_accept, configure",
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
                return ToolResult::error(missing_param(
                    "create",
                    "name",
                    "organizer(resource: \"reminders\", action: \"create\", name: \"Call the plumber\", due_date: \"tomorrow\")",
                ));
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
                return ToolResult::error(missing_param(
                    "complete",
                    "name",
                    "organizer(resource: \"reminders\", action: \"complete\", name: \"Call the plumber\")",
                ));
            }
            let list = if input.list.is_empty() {
                "Reminders"
            } else {
                &input.list
            };
            let script = format!(
                r#"tell application "Reminders"
    try
        set completed of (first reminder of list "{list}" whose name is "{name}") to true
    on error errMsg number errNum
        if errNum is -1728 then return "DIAG|No reminder named exactly '{name}' in list '{list}'. Use action 'list' to see the exact names."
        error errMsg number errNum
    end try
    return "Completed: {name}"
end tell"#,
                list = escape_applescript(list),
                name = escape_applescript(name)
            );
            diag(run_osascript(&script).await)
        }
        "delete" => {
            let name = input.event_name();
            if name.is_empty() {
                return ToolResult::error(missing_param(
                    "delete",
                    "name",
                    "organizer(resource: \"reminders\", action: \"delete\", name: \"Call the plumber\")",
                ));
            }
            let list = if input.list.is_empty() {
                "Reminders"
            } else {
                &input.list
            };
            let script = format!(
                r#"tell application "Reminders"
    try
        delete (first reminder of list "{list}" whose name is "{name}")
    on error errMsg number errNum
        if errNum is -1728 then return "DIAG|No reminder named exactly '{name}' in list '{list}'. Use action 'list' to see the exact names."
        error errMsg number errNum
    end try
    return "Deleted reminder: {name}"
end tell"#,
                list = escape_applescript(list),
                name = escape_applescript(name)
            );
            diag(run_osascript(&script).await)
        }
        _ => ToolResult::error(format!(
            "Unknown reminders action '{}'. Use: lists, list, create, complete, delete",
            action
        )),
    }
}
