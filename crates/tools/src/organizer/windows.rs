//! Windows organizer: Outlook COM automation via PowerShell.
//!
//! Outlook folder IDs:
//!   6=Inbox, 5=SentItems, 16=Drafts, 3=Deleted, 4=Outbox,
//!   9=Calendar, 10=Contacts, 13=Tasks
//!
//! Fallback: Task Scheduler (`Register-ScheduledTask` + `msg.exe`)
//! for reminders when Outlook is not available.

use super::shared::{escape_powershell, run_powershell};
use super::OrganizerInput;
use crate::registry::ToolResult;

// ═══════════════════════════════════════════════════════════════════════
// Outlook detection (cached, 10-second timeout)
// ═══════════════════════════════════════════════════════════════════════

fn has_outlook() -> bool {
    use std::sync::OnceLock;
    static HAS_OUTLOOK: OnceLock<bool> = OnceLock::new();

    *HAS_OUTLOOK.get_or_init(|| {
        let script = r#"try { $null = New-Object -ComObject Outlook.Application; Write-Output "true" } catch { Write-Output "false" }"#;
        // Use .output() in a thread with a 10-second timeout — Outlook COM can hang
        let (tx, rx) = std::sync::mpsc::channel();
        let script = script.to_string();
        std::thread::spawn(move || {
            let result = std::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", &script])
                .output();
            let _ = tx.send(result);
        });

        match rx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(Ok(output)) => {
                String::from_utf8_lossy(&output.stdout).trim() == "true"
            }
            _ => false,
        }
    })
}

/// Map mailbox name to Outlook folder ID, or return name for custom folder lookup.
fn folder_id(mailbox: &str) -> &str {
    match mailbox.to_lowercase().as_str() {
        "" | "inbox" => "6",
        "sent" | "sentitems" | "sent items" => "5",
        "drafts" => "16",
        "deleted" | "trash" => "3",
        "outbox" => "4",
        _ => mailbox, // Custom folder — looked up by name
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Mail
// ═══════════════════════════════════════════════════════════════════════

pub async fn handle_mail(action: &str, input: &OrganizerInput) -> ToolResult {
    if !has_outlook() {
        return ToolResult::error("Mail requires Microsoft Outlook on Windows");
    }

    match action {
        "accounts" => {
            let script = r#"
$ol = New-Object -ComObject Outlook.Application
$ns = $ol.GetNamespace("MAPI")
$accounts = $ns.Accounts
$output = ""
for ($i = 1; $i -le $accounts.Count; $i++) {
    $a = $accounts.Item($i)
    $output += $a.DisplayName + " <" + $a.SmtpAddress + ">`n"
}
if ($output -eq "") { Write-Output "No email accounts configured in Outlook" } else { Write-Output $output }"#;
            run_powershell(script).await
        }
        "unread" => {
            let script = r#"
$ol = New-Object -ComObject Outlook.Application
$ns = $ol.GetNamespace("MAPI")
$inbox = $ns.GetDefaultFolder(6)
Write-Output $inbox.UnReadItemCount"#;
            run_powershell(script).await
        }
        "read" => {
            let limit = input.limit.unwrap_or(10).clamp(1, 50);
            let fid = folder_id(&input.mailbox);

            let script = if fid.chars().all(|c| c.is_ascii_digit()) {
                format!(
                    r#"
$ol = New-Object -ComObject Outlook.Application
$ns = $ol.GetNamespace("MAPI")
$folder = $ns.GetDefaultFolder({fid})
$items = $folder.Items
$items.Sort("[ReceivedTime]", $true)
$count = [Math]::Min({limit}, $items.Count)
$output = ""
for ($i = 1; $i -le $count; $i++) {{
    $m = $items.Item($i)
    $output += "From: " + $m.SenderName + " | Subject: " + $m.Subject + " | Date: " + $m.ReceivedTime.ToString("yyyy-MM-dd HH:mm") + "`n---`n"
}}
if ($output -eq "") {{ Write-Output "No messages" }} else {{ Write-Output $output }}"#,
                )
            } else {
                // Custom folder lookup by name
                format!(
                    r#"
$ol = New-Object -ComObject Outlook.Application
$ns = $ol.GetNamespace("MAPI")
$inbox = $ns.GetDefaultFolder(6)
$folder = $inbox.Parent.Folders | Where-Object {{ $_.Name -eq "{name}" }} | Select-Object -First 1
if ($null -eq $folder) {{ $folder = $inbox }}

$items = $folder.Items
$items.Sort("[ReceivedTime]", $true)
$count = [Math]::Min({limit}, $items.Count)
$output = ""
for ($i = 1; $i -le $count; $i++) {{
    $m = $items.Item($i)
    $output += "From: " + $m.SenderName + " | Subject: " + $m.Subject + " | Date: " + $m.ReceivedTime.ToString("yyyy-MM-dd HH:mm") + "`n---`n"
}}
if ($output -eq "") {{ Write-Output "No messages" }} else {{ Write-Output $output }}"#,
                    name = escape_powershell(&input.mailbox),
                )
            };
            run_powershell(&script).await
        }
        "send" => {
            if input.to.is_empty() {
                return ToolResult::error("'to' parameter required for send");
            }
            if input.subject.is_empty() {
                return ToolResult::error("'subject' parameter required for send");
            }

            let to_str = escape_powershell(&input.to.join(";"));
            let cc_str = escape_powershell(&input.cc.join(";"));
            let subject = escape_powershell(&input.subject);
            let body = escape_powershell(&input.body);

            let mut script = format!(
                r#"
$ol = New-Object -ComObject Outlook.Application
$mail = $ol.CreateItem(0)
$mail.To = "{to_str}"
$mail.Subject = "{subject}"
$mail.Body = "{body}""#,
            );

            if !input.cc.is_empty() {
                script.push_str(&format!("\n$mail.CC = \"{cc_str}\""));
            }

            script.push_str(
                "\n$mail.Send()\nWrite-Output \"Email sent\"",
            );
            run_powershell(&script).await
        }
        "search" => {
            if input.query.is_empty() {
                return ToolResult::error("'query' parameter required for search");
            }
            let limit = input.limit.unwrap_or(20).clamp(1, 50);
            let query = escape_powershell(&input.query);

            let script = format!(
                r#"
$ol = New-Object -ComObject Outlook.Application
$ns = $ol.GetNamespace("MAPI")
$inbox = $ns.GetDefaultFolder(6)
$filter = "@SQL=""urn:schemas:httpmail:subject"" LIKE '%{query}%' OR ""urn:schemas:httpmail:textdescription"" LIKE '%{query}%'"
$items = $inbox.Items.Restrict($filter)
$items.Sort("[ReceivedTime]", $true)
$count = [Math]::Min({limit}, $items.Count)
$output = ""
for ($i = 1; $i -le $count; $i++) {{
    $m = $items.Item($i)
    $output += $m.ReceivedTime.ToString("yyyy-MM-dd HH:mm") + " | From: " + $m.SenderName + " | Subject: " + $m.Subject + "`n"
}}
if ($output -eq "") {{ Write-Output "No messages found" }} else {{ Write-Output $output }}"#,
            );
            run_powershell(&script).await
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
    if !has_outlook() {
        return ToolResult::error("Contacts requires Microsoft Outlook on Windows");
    }

    match action {
        "search" => {
            if input.query.is_empty() {
                return ToolResult::error("'query' parameter required for search");
            }
            let query = escape_powershell(&input.query);

            let script = format!(
                r#"
$ol = New-Object -ComObject Outlook.Application
$ns = $ol.GetNamespace("MAPI")
$contacts = $ns.GetDefaultFolder(10).Items
$output = ""
foreach ($c in $contacts) {{
    if ($c.FullName -like "*{query}*" -or $c.Email1Address -like "*{query}*") {{
        $phone = if ($c.MobileTelephoneNumber) {{ $c.MobileTelephoneNumber }} elseif ($c.BusinessTelephoneNumber) {{ $c.BusinessTelephoneNumber }} else {{ "" }}
        $output += $c.FullName + " | " + $c.Email1Address + " | " + $phone + "`n"
    }}
}}
if ($output -eq "") {{ Write-Output "No contacts found" }} else {{ Write-Output $output }}"#,
            );
            run_powershell(&script).await
        }
        "get" => {
            let name = &input.name;
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for get");
            }
            let name_escaped = escape_powershell(name);

            let script = format!(
                r#"
$ol = New-Object -ComObject Outlook.Application
$ns = $ol.GetNamespace("MAPI")
$contacts = $ns.GetDefaultFolder(10).Items
foreach ($c in $contacts) {{
    if ($c.FullName -like "*{name_escaped}*") {{
        $output = "Name: " + $c.FullName + "`n"
        if ($c.CompanyName) {{ $output += "Company: " + $c.CompanyName + "`n" }}
        if ($c.Email1Address) {{ $output += "Email: " + $c.Email1Address + "`n" }}
        if ($c.Email2Address) {{ $output += "Email2: " + $c.Email2Address + "`n" }}
        if ($c.BusinessTelephoneNumber) {{ $output += "Business Phone: " + $c.BusinessTelephoneNumber + "`n" }}
        if ($c.MobileTelephoneNumber) {{ $output += "Mobile: " + $c.MobileTelephoneNumber + "`n" }}
        if ($c.HomeTelephoneNumber) {{ $output += "Home Phone: " + $c.HomeTelephoneNumber + "`n" }}
        if ($c.BusinessAddress) {{ $output += "Address: " + $c.BusinessAddress + "`n" }}
        if ($c.Body) {{ $output += "Notes: " + $c.Body + "`n" }}
        Write-Output $output
        break
    }}
}}
if (-not $output) {{ Write-Output "Contact not found" }}"#,
            );
            run_powershell(&script).await
        }
        "create" => {
            let name = &input.name;
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for create");
            }
            let parts: Vec<&str> = name.splitn(2, ' ').collect();
            let first = escape_powershell(parts[0]);
            let last = if parts.len() > 1 {
                escape_powershell(parts[1])
            } else {
                String::new()
            };

            let mut script = format!(
                r#"
$ol = New-Object -ComObject Outlook.Application
$c = $ol.CreateItem(2)
$c.FirstName = "{first}"
$c.LastName = "{last}""#,
            );

            if !input.email.is_empty() {
                script.push_str(&format!(
                    "\n$c.Email1Address = \"{}\"",
                    escape_powershell(&input.email)
                ));
            }
            if !input.phone.is_empty() {
                script.push_str(&format!(
                    "\n$c.MobileTelephoneNumber = \"{}\"",
                    escape_powershell(&input.phone)
                ));
            }
            if !input.company.is_empty() {
                script.push_str(&format!(
                    "\n$c.CompanyName = \"{}\"",
                    escape_powershell(&input.company)
                ));
            }
            if !input.notes.is_empty() {
                script.push_str(&format!(
                    "\n$c.Body = \"{}\"",
                    escape_powershell(&input.notes)
                ));
            }

            script.push_str(
                "\n$c.Save()\nWrite-Output (\"Contact created: \" + $c.FullName)",
            );
            run_powershell(&script).await
        }
        "groups" => {
            let script = r#"
$ol = New-Object -ComObject Outlook.Application
$ns = $ol.GetNamespace("MAPI")
$contactsFolder = $ns.GetDefaultFolder(10)
$output = ""
foreach ($f in $contactsFolder.Folders) {
    $output += $f.Name + " (" + $f.Items.Count + " contacts)`n"
}
if ($output -eq "") { Write-Output "No contact folders" } else { Write-Output $output }"#;
            run_powershell(script).await
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
    if !has_outlook() {
        return ToolResult::error("Calendar requires Microsoft Outlook on Windows");
    }

    match action {
        "calendars" => {
            let script = r#"
$ol = New-Object -ComObject Outlook.Application
$ns = $ol.GetNamespace("MAPI")
$output = ""
foreach ($store in $ns.Stores) {
    $root = $store.GetRootFolder()
    foreach ($f in $root.Folders) {
        if ($f.DefaultItemType -eq 9) {
            $output += $store.DisplayName + " / " + $f.Name + "`n"
        }
    }
}
if ($output -eq "") { Write-Output "No calendars found" } else { Write-Output $output }"#;
            run_powershell(script).await
        }
        "today" => {
            let today = chrono::Local::now().format("%m/%d/%Y").to_string();
            let tomorrow = (chrono::Local::now() + chrono::Duration::days(1))
                .format("%m/%d/%Y")
                .to_string();

            let script = format!(
                r#"
$ol = New-Object -ComObject Outlook.Application
$ns = $ol.GetNamespace("MAPI")
$cal = $ns.GetDefaultFolder(9)
$items = $cal.Items
$items.IncludeRecurrences = $true
$items.Sort("[Start]")
$filter = "[Start] >= '{today}' AND [Start] < '{tomorrow}'"
$filtered = $items.Restrict($filter)
$output = ""
foreach ($e in $filtered) {{
    $output += $e.Subject + " | " + $e.Start.ToString("yyyy-MM-dd HH:mm") + " - " + $e.End.ToString("HH:mm")
    if ($e.Location) {{ $output += " @ " + $e.Location }}
    $output += "`n"
}}
if ($output -eq "") {{ Write-Output "No events today" }} else {{ Write-Output $output }}"#,
            );
            run_powershell(&script).await
        }
        "upcoming" => {
            let days = input.days.unwrap_or(7).clamp(1, 365);
            let start = chrono::Local::now().format("%m/%d/%Y").to_string();
            let end = (chrono::Local::now() + chrono::Duration::days(days))
                .format("%m/%d/%Y")
                .to_string();

            let script = format!(
                r#"
$ol = New-Object -ComObject Outlook.Application
$ns = $ol.GetNamespace("MAPI")
$cal = $ns.GetDefaultFolder(9)
$items = $cal.Items
$items.IncludeRecurrences = $true
$items.Sort("[Start]")
$filter = "[Start] >= '{start}' AND [Start] < '{end}'"
$filtered = $items.Restrict($filter)
$output = ""
foreach ($e in $filtered) {{
    $output += $e.Subject + " | " + $e.Start.ToString("yyyy-MM-dd HH:mm") + " - " + $e.End.ToString("HH:mm")
    if ($e.Location) {{ $output += " @ " + $e.Location }}
    $output += "`n"
}}
if ($output -eq "") {{ Write-Output "No upcoming events in the next {days} days" }} else {{ Write-Output $output }}"#,
            );
            run_powershell(&script).await
        }
        "list" => {
            let days = input.days.unwrap_or(30).clamp(1, 365);
            let start = chrono::Local::now().format("%m/%d/%Y").to_string();
            let end = (chrono::Local::now() + chrono::Duration::days(days))
                .format("%m/%d/%Y")
                .to_string();

            let script = format!(
                r#"
$ol = New-Object -ComObject Outlook.Application
$ns = $ol.GetNamespace("MAPI")
$cal = $ns.GetDefaultFolder(9)
$items = $cal.Items
$items.IncludeRecurrences = $true
$items.Sort("[Start]")
$filter = "[Start] >= '{start}' AND [Start] < '{end}'"
$filtered = $items.Restrict($filter)
$output = ""
foreach ($e in $filtered) {{
    $output += $e.Subject + " | " + $e.Start.ToString("yyyy-MM-dd HH:mm") + " - " + $e.End.ToString("HH:mm")
    if ($e.Location) {{ $output += " @ " + $e.Location }}
    $output += "`n"
}}
if ($output -eq "") {{ Write-Output "No events in the next {days} days" }} else {{ Write-Output $output }}"#,
            );
            run_powershell(&script).await
        }
        "create" => {
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

            let start_str = start.format("%Y-%m-%d %H:%M:%S").to_string();
            let end_str = end.format("%Y-%m-%d %H:%M:%S").to_string();

            let mut script = format!(
                r#"
$ol = New-Object -ComObject Outlook.Application
$apt = $ol.CreateItem(1)
$apt.Subject = "{subject}"
$apt.Start = [DateTime]"{start_str}"
$apt.End = [DateTime]"{end_str}""#,
                subject = escape_powershell(name),
            );

            if !input.location.is_empty() {
                script.push_str(&format!(
                    "\n$apt.Location = \"{}\"",
                    escape_powershell(&input.location)
                ));
            }
            if !input.notes.is_empty() {
                script.push_str(&format!(
                    "\n$apt.Body = \"{}\"",
                    escape_powershell(&input.notes)
                ));
            }

            script.push_str(
                "\n$apt.Save()\nWrite-Output (\"Event created: \" + $apt.Subject)",
            );
            run_powershell(&script).await
        }
        _ => ToolResult::error(format!(
            "Unknown calendar action '{}'. Use: calendars, today, upcoming, create, list",
            action
        )),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Reminders (Outlook Tasks + Task Scheduler fallback)
// ═══════════════════════════════════════════════════════════════════════

pub async fn handle_reminders(action: &str, input: &OrganizerInput) -> ToolResult {
    if has_outlook() {
        outlook_reminders(action, input).await
    } else {
        scheduler_reminders(action, input).await
    }
}

async fn outlook_reminders(action: &str, input: &OrganizerInput) -> ToolResult {
    match action {
        "lists" => {
            let script = r#"
$ol = New-Object -ComObject Outlook.Application
$ns = $ol.GetNamespace("MAPI")
$tasks = $ns.GetDefaultFolder(13).Items
$cats = @{}
foreach ($t in $tasks) {
    if ($t.Categories) {
        foreach ($c in $t.Categories.Split(",")) {
            $c = $c.Trim()
            if ($cats.ContainsKey($c)) { $cats[$c]++ } else { $cats[$c] = 1 }
        }
    }
}
$output = ""
foreach ($k in $cats.Keys) { $output += $k + " (" + $cats[$k] + " tasks)`n" }
if ($output -eq "") { Write-Output "No task categories" } else { Write-Output $output }"#;
            run_powershell(script).await
        }
        "list" => {
            let mut filter = "$_.Complete -eq $false".to_string();
            if !input.list.is_empty() {
                filter.push_str(&format!(
                    " -and $_.Categories -like '*{}*'",
                    escape_powershell(&input.list)
                ));
            }

            let script = format!(
                r#"
$ol = New-Object -ComObject Outlook.Application
$ns = $ol.GetNamespace("MAPI")
$tasks = $ns.GetDefaultFolder(13).Items | Where-Object {{ {filter} }}
$output = ""
foreach ($t in $tasks) {{
    $pri = switch ($t.Importance) {{ 2 {{ "[HIGH]" }} 1 {{ "[NORMAL]" }} 0 {{ "[LOW]" }} default {{ "" }} }}
    $due = if ($t.DueDate.Year -lt 4501) {{ " | Due: " + $t.DueDate.ToString("yyyy-MM-dd") }} else {{ "" }}
    $output += $t.Subject + " " + $pri + $due + "`n"
}}
if ($output -eq "") {{ Write-Output "No tasks" }} else {{ Write-Output $output }}"#,
            );
            run_powershell(&script).await
        }
        "create" => {
            let name = input.event_name();
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for create");
            }

            let mut script = format!(
                r#"
$ol = New-Object -ComObject Outlook.Application
$task = $ol.CreateItem(3)
$task.Subject = "{name}""#,
                name = escape_powershell(name),
            );

            if !input.notes.is_empty() {
                script.push_str(&format!(
                    "\n$task.Body = \"{}\"",
                    escape_powershell(&input.notes)
                ));
            }
            if !input.list.is_empty() {
                script.push_str(&format!(
                    "\n$task.Categories = \"{}\"",
                    escape_powershell(&input.list)
                ));
            }

            if !input.due_date.is_empty() {
                match super::shared::parse_date(&input.due_date) {
                    Ok(dt) => {
                        let date_str = dt.format("%Y-%m-%d").to_string();
                        script.push_str(&format!(
                            "\n$task.DueDate = \"{date_str}\"\n$task.ReminderSet = $true\n$task.ReminderTime = \"{date_str} 09:00:00\""
                        ));
                    }
                    Err(e) => return ToolResult::error(format!("Invalid due_date: {}", e)),
                }
            }

            if let Some(pri) = input.priority {
                let importance = match pri {
                    1..=3 => "2",  // High
                    4..=6 => "1",  // Normal
                    _ => "0",     // Low
                };
                script.push_str(&format!("\n$task.Importance = {importance}"));
            }

            script.push_str(
                "\n$task.Save()\nWrite-Output (\"Task created: \" + $task.Subject)",
            );
            run_powershell(&script).await
        }
        "complete" => {
            let name = input.event_name();
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for complete");
            }
            let name_escaped = escape_powershell(name);

            let script = format!(
                r#"
$ol = New-Object -ComObject Outlook.Application
$ns = $ol.GetNamespace("MAPI")
$tasks = $ns.GetDefaultFolder(13).Items
$found = $false
foreach ($t in $tasks) {{
    if ($t.Subject -like "*{name_escaped}*" -and $t.Complete -eq $false) {{
        $t.Complete = $true
        $t.Save()
        Write-Output ("Completed: " + $t.Subject)
        $found = $true
        break
    }}
}}
if (-not $found) {{ Write-Output "No matching task found" }}"#,
            );
            run_powershell(&script).await
        }
        "delete" => {
            let name = input.event_name();
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for delete");
            }
            let name_escaped = escape_powershell(name);

            let script = format!(
                r#"
$ol = New-Object -ComObject Outlook.Application
$ns = $ol.GetNamespace("MAPI")
$tasks = $ns.GetDefaultFolder(13).Items
$found = $false
foreach ($t in $tasks) {{
    if ($t.Subject -like "*{name_escaped}*") {{
        $subj = $t.Subject
        $t.Delete()
        Write-Output ("Deleted: " + $subj)
        $found = $true
        break
    }}
}}
if (-not $found) {{ Write-Output "No matching task found" }}"#,
            );
            run_powershell(&script).await
        }
        _ => ToolResult::error(format!(
            "Unknown reminders action '{}'. Use: lists, list, create, complete, delete",
            action
        )),
    }
}

/// Task Scheduler fallback when Outlook is not available.
async fn scheduler_reminders(action: &str, input: &OrganizerInput) -> ToolResult {
    match action {
        "lists" => {
            run_powershell(
                r#"Get-ScheduledTask | Where-Object { $_.TaskPath -eq "\Nebo\" } | ForEach-Object { $_.TaskName } | Sort-Object"#,
            )
            .await
        }
        "list" => {
            run_powershell(
                r#"Get-ScheduledTask | Where-Object { $_.TaskPath -eq "\Nebo\" } | Select-Object TaskName, State | Format-Table -AutoSize"#,
            )
            .await
        }
        "create" => {
            let name = input.event_name();
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for create");
            }

            let trigger_time = if !input.due_date.is_empty() {
                match super::shared::parse_date(&input.due_date) {
                    Ok(dt) => dt.format("%Y-%m-%dT%H:%M:%S").to_string(),
                    Err(e) => return ToolResult::error(format!("Invalid due_date: {}", e)),
                }
            } else {
                // Default: 1 hour from now
                let dt = chrono::Local::now().naive_local() + chrono::Duration::hours(1);
                dt.format("%Y-%m-%dT%H:%M:%S").to_string()
            };

            let name_escaped = escape_powershell(name);
            let script = format!(
                r#"
$action = New-ScheduledTaskAction -Execute "msg.exe" -Argument "* Reminder: {name_escaped}"
$trigger = New-ScheduledTaskTrigger -Once -At "{trigger_time}"
Register-ScheduledTask -TaskPath "\Nebo\" -TaskName "{name_escaped}" -Action $action -Trigger $trigger -Force | Out-Null
Write-Output "Reminder scheduled: {name_escaped} at {trigger_time}""#,
            );
            run_powershell(&script).await
        }
        "complete" | "delete" => {
            let name = input.event_name();
            if name.is_empty() {
                return ToolResult::error("'name' parameter required");
            }
            let name_escaped = escape_powershell(name);
            let script = format!(
                r#"Unregister-ScheduledTask -TaskPath "\Nebo\" -TaskName "{name_escaped}" -Confirm:$false; Write-Output "Removed: {name_escaped}""#,
            );
            run_powershell(&script).await
        }
        _ => ToolResult::error(format!(
            "Unknown reminders action '{}'. Use: lists, list, create, complete, delete",
            action
        )),
    }
}
