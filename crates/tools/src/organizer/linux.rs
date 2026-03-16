//! Linux organizer: multi-backend CLI integration.
//!
//! Mail:      neomutt → mutt → s-nail → mail → sendmail (send)
//!            notmuch → mutt/neomutt (read)
//! Contacts:  khard → abook
//! Calendar:  khal → gcalcli → calcurse
//! Reminders: taskwarrior (task) → todo.sh

use super::shared::{run_command, run_command_with_stdin, which_exists};
use super::OrganizerInput;
use crate::registry::ToolResult;

// ═══════════════════════════════════════════════════════════════════════
// Backend detection
// ═══════════════════════════════════════════════════════════════════════

fn detect_mail_send() -> Option<&'static str> {
    for cmd in &["neomutt", "mutt", "s-nail", "mail", "sendmail"] {
        if which_exists(cmd) {
            return Some(cmd);
        }
    }
    None
}

fn detect_mail_read() -> Option<&'static str> {
    // Prefer notmuch if installed AND configured
    if which_exists("notmuch") {
        let home = std::env::var("HOME").unwrap_or_default();
        if std::path::Path::new(&format!("{}/.notmuch-config", home)).exists() {
            return Some("notmuch");
        }
    }
    // Fall back to mutt/neomutt
    let send = detect_mail_send();
    if matches!(send, Some("mutt" | "neomutt")) {
        return send;
    }
    None
}

fn detect_contacts() -> Option<&'static str> {
    for cmd in &["khard", "abook"] {
        if which_exists(cmd) {
            return Some(cmd);
        }
    }
    None
}

fn detect_calendar() -> Option<&'static str> {
    for cmd in &["khal", "gcalcli", "calcurse"] {
        if which_exists(cmd) {
            return Some(cmd);
        }
    }
    None
}

fn detect_reminders() -> Option<&'static str> {
    for cmd in &["task", "todo.sh"] {
        if which_exists(cmd) {
            return Some(cmd);
        }
    }
    None
}

// ═══════════════════════════════════════════════════════════════════════
// Mail
// ═══════════════════════════════════════════════════════════════════════

pub async fn handle_mail(action: &str, input: &OrganizerInput) -> ToolResult {
    match action {
        "send" => mail_send(input).await,
        "read" => mail_read(input).await,
        "unread" => mail_unread(input).await,
        "search" => mail_search(input).await,
        "accounts" => mail_accounts().await,
        _ => ToolResult::error(format!(
            "Unknown mail action '{}'. Use: accounts, unread, read, send, search",
            action
        )),
    }
}

async fn mail_send(input: &OrganizerInput) -> ToolResult {
    if input.to.is_empty() {
        return ToolResult::error("'to' parameter required for send");
    }
    if input.subject.is_empty() {
        return ToolResult::error("'subject' parameter required for send");
    }

    let backend = match detect_mail_send() {
        Some(b) => b,
        None => {
            return ToolResult::error(
                "No mail client found. Install one of:\n\
                 - neomutt (recommended): sudo apt install neomutt\n\
                 - mutt: sudo apt install mutt\n\
                 - s-nail: sudo apt install s-nail",
            )
        }
    };

    match backend {
        "neomutt" | "mutt" | "s-nail" | "mail" => {
            let mut args: Vec<&str> = vec!["-s", &input.subject];
            // CC recipients
            for cc in &input.cc {
                args.push("-c");
                args.push(cc);
            }
            // To recipients
            for to in &input.to {
                args.push(to);
            }
            // Body piped via stdin (safe from shell injection)
            run_command_with_stdin(backend, &args, &input.body).await
        }
        "sendmail" => {
            // Build RFC 2822 formatted email
            let mut email = String::new();
            email.push_str(&format!("To: {}\n", input.to.join(", ")));
            if !input.cc.is_empty() {
                email.push_str(&format!("Cc: {}\n", input.cc.join(", ")));
            }
            email.push_str(&format!("Subject: {}\n", input.subject));
            email.push_str("Content-Type: text/plain; charset=UTF-8\n\n");
            email.push_str(&input.body);

            run_command_with_stdin("sendmail", &["-t"], &email).await
        }
        _ => ToolResult::error(format!("Unsupported mail backend: {}", backend)),
    }
}

async fn mail_read(input: &OrganizerInput) -> ToolResult {
    let backend = match detect_mail_read() {
        Some(b) => b,
        None => {
            return ToolResult::error(
                "No mail reader found. Install notmuch for best results:\n\
                 sudo apt install notmuch\n\
                 notmuch setup",
            )
        }
    };

    let limit = input.limit.unwrap_or(10).clamp(1, 50);
    let limit_str = limit.to_string();

    match backend {
        "notmuch" => {
            let mut query = "date:1month..today".to_string();
            if !input.mailbox.is_empty() {
                query = format!("folder:{} and {}", input.mailbox, query);
            }
            run_command(
                "notmuch",
                &[
                    "search",
                    "--format=text",
                    "--output=summary",
                    &format!("--limit={}", limit_str),
                    &query,
                ],
            )
            .await
        }
        "mutt" | "neomutt" => {
            // Mutt batch mode is limited; suggest notmuch for better reading
            let mailbox = if input.mailbox.is_empty() {
                "INBOX".to_string()
            } else {
                input.mailbox.clone()
            };
            run_command(
                backend,
                &["-f", &mailbox, "-e", "set pager=cat", "-e", "push q"],
            )
            .await
        }
        _ => ToolResult::error("No suitable mail reader found"),
    }
}

async fn mail_unread(input: &OrganizerInput) -> ToolResult {
    if !which_exists("notmuch") {
        return ToolResult::error(
            "Unread count requires notmuch:\n\
             sudo apt install notmuch && notmuch setup",
        );
    }

    let mut query = "tag:unread".to_string();
    if !input.mailbox.is_empty() {
        query = format!("folder:{} and {}", input.mailbox, query);
    }
    run_command("notmuch", &["count", &query]).await
}

async fn mail_search(input: &OrganizerInput) -> ToolResult {
    if input.query.is_empty() {
        return ToolResult::error("'query' parameter required for search");
    }

    if !which_exists("notmuch") {
        return ToolResult::error(
            "Search requires notmuch:\n\
             sudo apt install notmuch && notmuch setup",
        );
    }

    let limit = input.limit.unwrap_or(20).clamp(1, 50);
    let mut query = input.query.clone();
    if !input.mailbox.is_empty() {
        query = format!("folder:{} and ({})", input.mailbox, query);
    }

    run_command(
        "notmuch",
        &[
            "search",
            "--format=text",
            "--output=summary",
            &format!("--limit={}", limit),
            &query,
        ],
    )
    .await
}

async fn mail_accounts() -> ToolResult {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut accounts = Vec::new();

    // Check notmuch config
    if which_exists("notmuch") {
        let result = tokio::process::Command::new("notmuch")
            .args(["config", "get", "user.primary_email"])
            .output()
            .await;
        if let Ok(output) = result {
            if output.status.success() {
                let email = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !email.is_empty() {
                    accounts.push(format!("notmuch: {}", email));
                }
            }
        }
    }

    // Check config files
    for (file, label) in &[
        (".muttrc", "mutt"),
        (".neomuttrc", "neomutt"),
        (".mailrc", "mail"),
    ] {
        let path = format!("{}/{}", home, file);
        if std::path::Path::new(&path).exists() {
            accounts.push(format!("{}: configured ({})", label, path));
        }
    }

    if accounts.is_empty() {
        ToolResult::error(
            "No mail accounts found. Configure one of:\n\
             - notmuch: notmuch setup\n\
             - mutt: create ~/.muttrc\n\
             - neomutt: create ~/.neomuttrc",
        )
    } else {
        ToolResult::ok(accounts.join("\n"))
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Contacts
// ═══════════════════════════════════════════════════════════════════════

pub async fn handle_contacts(action: &str, input: &OrganizerInput) -> ToolResult {
    let backend = match detect_contacts() {
        Some(b) => b,
        None => {
            return ToolResult::error(
                "No contacts backend found. Install one of:\n\
                 - khard (recommended): sudo apt install khard\n\
                 - abook: sudo apt install abook",
            )
        }
    };

    match (backend, action) {
        ("khard", "search") => {
            if input.query.is_empty() {
                return ToolResult::error("'query' parameter required for search");
            }
            run_command("khard", &["list", &input.query]).await
        }
        ("khard", "get") => {
            let name = &input.name;
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for get");
            }
            run_command("khard", &["show", name]).await
        }
        ("khard", "create") => {
            let name = &input.name;
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for create");
            }

            // Build vCard 3.0
            let parts: Vec<&str> = name.splitn(2, ' ').collect();
            let first = parts[0];
            let last = if parts.len() > 1 { parts[1] } else { "" };

            let mut vcard = String::new();
            vcard.push_str("BEGIN:VCARD\n");
            vcard.push_str("VERSION:3.0\n");
            vcard.push_str(&format!("FN:{}\n", name));
            vcard.push_str(&format!("N:{};{};;;\n", last, first));
            if !input.email.is_empty() {
                vcard.push_str(&format!("EMAIL;TYPE=WORK:{}\n", input.email));
            }
            if !input.phone.is_empty() {
                vcard.push_str(&format!("TEL;TYPE=CELL:{}\n", input.phone));
            }
            if !input.company.is_empty() {
                vcard.push_str(&format!("ORG:{}\n", input.company));
            }
            if !input.notes.is_empty() {
                // Fold long lines in vCard (escape newlines)
                vcard.push_str(&format!("NOTE:{}\n", input.notes.replace('\n', "\\n")));
            }
            vcard.push_str("END:VCARD\n");

            run_command_with_stdin("khard", &["new", "--vcard-version", "3.0", "-"], &vcard).await
        }
        ("khard", "groups") => run_command("khard", &["addressbooks"]).await,

        ("abook", "search") => {
            if input.query.is_empty() {
                return ToolResult::error("'query' parameter required for search");
            }
            run_command("abook", &["--mutt-query", &input.query]).await
        }
        ("abook", "get") => {
            let name = &input.name;
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for get");
            }
            run_command("abook", &["--mutt-query", name]).await
        }
        ("abook", "create") => ToolResult::error(
            "abook does not support non-interactive contact creation.\n\
             Install khard for full CRUD support: sudo apt install khard",
        ),
        ("abook", "groups") => ToolResult::error(
            "abook does not support contact groups.\n\
             Install khard for addressbook support: sudo apt install khard",
        ),

        (_, _) => ToolResult::error(format!(
            "Unknown contacts action '{}'. Use: search, get, create, groups",
            action
        )),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Calendar
// ═══════════════════════════════════════════════════════════════════════

pub async fn handle_calendar(action: &str, input: &OrganizerInput) -> ToolResult {
    let backend = match detect_calendar() {
        Some(b) => b,
        None => {
            return ToolResult::error(
                "No calendar backend found. Install one of:\n\
                 - khal (recommended): sudo apt install khal\n\
                 - gcalcli (Google Calendar): pip install gcalcli\n\
                 - calcurse: sudo apt install calcurse",
            )
        }
    };

    match (backend, action) {
        // ── khal ──
        ("khal", "calendars") => run_command("khal", &["printcalendars"]).await,
        ("khal", "today") => {
            run_command(
                "khal",
                &[
                    "list",
                    "today",
                    "today",
                    "--format",
                    "{calendar}: {title} | {start-time} - {end-time} | {location}",
                ],
            )
            .await
        }
        ("khal", "upcoming") => {
            let days = input.days.unwrap_or(7).clamp(1, 365);
            run_command(
                "khal",
                &[
                    "list",
                    "today",
                    &format!("{}d", days),
                    "--format",
                    "{calendar}: {title} | {start-time} - {end-time} | {location}",
                ],
            )
            .await
        }
        ("khal", "list") => {
            let days = input.days.unwrap_or(365).clamp(1, 365);
            run_command(
                "khal",
                &[
                    "list",
                    "today",
                    &format!("{}d", days),
                    "--format",
                    "{calendar}: {title} | {start-time} - {end-time} | {location}",
                ],
            )
            .await
        }
        ("khal", "create") => {
            let name = input.event_name();
            if name.is_empty() {
                return ToolResult::error("'name' or 'title' parameter required for create");
            }
            if input.date.is_empty() {
                return ToolResult::error("'date' parameter required for create");
            }

            let start = match super::shared::parse_date(&input.date) {
                Ok(dt) => dt,
                Err(e) => return ToolResult::error(e),
            };
            let end = if !input.end_date.is_empty() {
                match super::shared::parse_date(&input.end_date) {
                    Ok(dt) => dt,
                    Err(e) => return ToolResult::error(format!("Invalid end_date: {}", e)),
                }
            } else {
                start + chrono::Duration::hours(1)
            };

            let start_str = start.format("%Y-%m-%d %H:%M").to_string();
            let end_str = end.format("%Y-%m-%d %H:%M").to_string();

            let mut args: Vec<&str> = vec!["new"];
            if !input.calendar.is_empty() {
                args.push("-a");
                args.push(&input.calendar);
            }
            args.push(&start_str);
            args.push(&end_str);

            // khal new format: start end title [:: description]
            // khal doesn't have a location flag — append to title like Go does
            let title_with_loc = if !input.location.is_empty() {
                format!("{} @ {}", name, input.location)
            } else {
                name.to_string()
            };
            let title_and_desc = if !input.notes.is_empty() {
                format!("{} :: {}", title_with_loc, input.notes)
            } else {
                title_with_loc
            };
            args.push(&title_and_desc);

            run_command("khal", &args).await
        }

        // ── gcalcli ──
        ("gcalcli", "calendars") => run_command("gcalcli", &["list"]).await,
        ("gcalcli", "today") => {
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            let tomorrow = (chrono::Local::now() + chrono::Duration::days(1))
                .format("%Y-%m-%d")
                .to_string();
            run_command("gcalcli", &["agenda", "--nocolor", &today, &tomorrow]).await
        }
        ("gcalcli", "upcoming") => {
            let days = input.days.unwrap_or(7).clamp(1, 365);
            let start = chrono::Local::now().format("%Y-%m-%d").to_string();
            let end = (chrono::Local::now() + chrono::Duration::days(days))
                .format("%Y-%m-%d")
                .to_string();
            run_command("gcalcli", &["agenda", "--nocolor", &start, &end]).await
        }
        ("gcalcli", "list") => {
            let days = input.days.unwrap_or(30).clamp(1, 365);
            let start = chrono::Local::now().format("%Y-%m-%d").to_string();
            let end = (chrono::Local::now() + chrono::Duration::days(days))
                .format("%Y-%m-%d")
                .to_string();
            run_command("gcalcli", &["agenda", "--nocolor", &start, &end]).await
        }
        ("gcalcli", "create") => {
            let name = input.event_name();
            if name.is_empty() {
                return ToolResult::error("'name' or 'title' parameter required for create");
            }
            if input.date.is_empty() {
                return ToolResult::error("'date' parameter required for create");
            }

            let start = match super::shared::parse_date(&input.date) {
                Ok(dt) => dt,
                Err(e) => return ToolResult::error(e),
            };
            let duration_min = if !input.end_date.is_empty() {
                match super::shared::parse_date(&input.end_date) {
                    Ok(end) => {
                        let mins = (end - start).num_minutes();
                        if mins <= 0 { 60 } else { mins }
                    }
                    Err(e) => return ToolResult::error(format!("Invalid end_date: {}", e)),
                }
            } else {
                60 // 1 hour default
            };

            let start_str = start.format("%Y-%m-%d %H:%M").to_string();
            let duration_str = duration_min.to_string();

            let mut args = vec![
                "add",
                "--title",
                name,
                "--when",
                &start_str,
                "--duration",
                &duration_str,
                "--noprompt",
            ];

            if !input.calendar.is_empty() {
                args.push("--calendar");
                args.push(&input.calendar);
            }
            if !input.location.is_empty() {
                args.push("--where");
                args.push(&input.location);
            }
            if !input.notes.is_empty() {
                args.push("--description");
                args.push(&input.notes);
            }

            run_command("gcalcli", &args).await
        }

        // ── calcurse ──
        ("calcurse", "calendars") => {
            ToolResult::ok(
                "calcurse uses a single local calendar stored in ~/.local/share/calcurse/"
                    .to_string(),
            )
        }
        ("calcurse", "today") => {
            run_command("calcurse", &["-Q", "--filter-type", "apt", "-d", "1"]).await
        }
        ("calcurse", "upcoming") => {
            let days = input.days.unwrap_or(7).clamp(1, 365);
            let days_str = days.to_string();
            run_command(
                "calcurse",
                &["-Q", "--filter-type", "apt", "-d", &days_str],
            )
            .await
        }
        ("calcurse", "list") => {
            let days = input.days.unwrap_or(365).clamp(1, 365);
            let days_str = days.to_string();
            run_command(
                "calcurse",
                &["-Q", "--filter-type", "apt", "-d", &days_str],
            )
            .await
        }
        ("calcurse", "create") => {
            let name = input.event_name();
            if name.is_empty() {
                return ToolResult::error("'name' or 'title' parameter required for create");
            }
            if input.date.is_empty() {
                return ToolResult::error("'date' parameter required for create");
            }

            let start = match super::shared::parse_date(&input.date) {
                Ok(dt) => dt,
                Err(e) => return ToolResult::error(e),
            };
            let end = if !input.end_date.is_empty() {
                match super::shared::parse_date(&input.end_date) {
                    Ok(dt) => dt,
                    Err(e) => return ToolResult::error(format!("Invalid end_date: {}", e)),
                }
            } else {
                start + chrono::Duration::hours(1)
            };

            // calcurse -i reads ical from stdin — safer than bash -c
            let appointment = format!(
                "{} @ {} -> {} @ {} |{}",
                start.format("%m/%d/%Y"),
                start.format("%H:%M"),
                end.format("%m/%d/%Y"),
                end.format("%H:%M"),
                name,
            );
            run_command_with_stdin("calcurse", &["-i", "-"], &appointment).await
        }

        (_, _) => ToolResult::error(format!(
            "Unknown calendar action '{}'. Use: calendars, today, upcoming, create, list",
            action
        )),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Reminders
// ═══════════════════════════════════════════════════════════════════════

pub async fn handle_reminders(action: &str, input: &OrganizerInput) -> ToolResult {
    let backend = match detect_reminders() {
        Some(b) => b,
        None => {
            return ToolResult::error(
                "No task/reminder backend found. Install one of:\n\
                 - taskwarrior (recommended): sudo apt install taskwarrior\n\
                 - todo.sh: https://github.com/todotxt/todo.txt-cli",
            )
        }
    };

    match (backend, action) {
        // ── taskwarrior ──
        ("task", "lists") => run_command("task", &["projects"]).await,
        ("task", "list") => {
            let mut args = vec!["list"];
            let project_filter;
            if !input.list.is_empty() {
                project_filter = format!("project:{}", input.list);
                args.push(&project_filter);
            }
            run_command("task", &args).await
        }
        ("task", "create") => {
            let name = input.event_name();
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for create");
            }

            let mut args = vec!["add", name];
            let project_str;
            let due_str;
            let pri_str;

            if !input.list.is_empty() {
                project_str = format!("project:{}", input.list);
                args.push(&project_str);
            }
            if !input.due_date.is_empty() {
                match super::shared::parse_date(&input.due_date) {
                    Ok(dt) => {
                        due_str = format!("due:{}", dt.format("%Y-%m-%d"));
                        args.push(&due_str);
                    }
                    Err(e) => return ToolResult::error(format!("Invalid due_date: {}", e)),
                }
            }
            if let Some(pri) = input.priority {
                let p = match pri {
                    1..=3 => "H",
                    4..=6 => "M",
                    _ => "L",
                };
                pri_str = format!("priority:{}", p);
                args.push(&pri_str);
            }

            run_command("task", &args).await
        }
        ("task", "complete") => {
            let name = input.event_name();
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for complete");
            }
            let pattern = format!("/{}/", name);
            // Pipe "yes" to auto-confirm
            run_command_with_stdin("task", &[&pattern, "done"], "yes\n").await
        }
        ("task", "delete") => {
            let name = input.event_name();
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for delete");
            }
            let pattern = format!("/{}/", name);
            run_command_with_stdin("task", &[&pattern, "delete"], "yes\n").await
        }

        // ── todo.sh ──
        ("todo.sh", "lists") => run_command("todo.sh", &["listcon"]).await,
        ("todo.sh", "list") => {
            let mut args = vec!["list"];
            if !input.list.is_empty() {
                let ctx = format!("@{}", input.list);
                // todo.sh list @context
                return run_command("todo.sh", &["list", &ctx]).await;
            }
            run_command("todo.sh", &args).await
        }
        ("todo.sh", "create") => {
            let name = input.event_name();
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for create");
            }

            // Build todo.txt format: (A) Task @context due:2024-01-15
            let mut task = String::new();

            if let Some(pri) = input.priority {
                let p = match pri {
                    1..=3 => "A",
                    4..=6 => "B",
                    _ => "C",
                };
                task.push_str(&format!("({}) ", p));
            }

            task.push_str(name);

            if !input.list.is_empty() {
                task.push_str(&format!(" @{}", input.list));
            }

            if !input.due_date.is_empty() {
                match super::shared::parse_date(&input.due_date) {
                    Ok(dt) => {
                        task.push_str(&format!(" due:{}", dt.format("%Y-%m-%d")));
                    }
                    Err(e) => return ToolResult::error(format!("Invalid due_date: {}", e)),
                }
            }

            run_command("todo.sh", &["add", &task]).await
        }
        ("todo.sh", "complete") => {
            let name = input.event_name();
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for complete");
            }
            run_command_with_stdin("todo.sh", &["do", name], "y\n").await
        }
        ("todo.sh", "delete") => {
            let name = input.event_name();
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for delete");
            }
            run_command_with_stdin("todo.sh", &["del", name], "y\n").await
        }

        (_, _) => ToolResult::error(format!(
            "Unknown reminders action '{}'. Use: lists, list, create, complete, delete",
            action
        )),
    }
}
