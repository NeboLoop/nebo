use std::sync::Arc;

use crate::domain::DomainInput;
use crate::errors;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};
use db::Store;

/// MessageTool handles outbound delivery to the owner (notifications, companion chat, SMS, TTS).
pub struct MessageTool {
    store: Arc<Store>,
}

impl MessageTool {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    fn infer_resource(&self, action: &str) -> &str {
        match action {
            "notify" => "owner",
            "alert" | "dnd_status" => "notify",
            "conversations" | "read" | "search" => "sms",
            _ => "",
        }
    }
}

impl DynTool for MessageTool {
    fn name(&self) -> &str {
        "message"
    }

    fn description(&self) -> String {
        "Outbound delivery — send notifications, alerts, and SMS to the owner.\n\
         USE THIS when: user wants to send a text, notification, or alert to someone outside NeboAI.\n\n\
         - message(resource: \"owner\", action: \"notify\", text: \"Task complete!\") — Notify the owner via companion chat\n\
         - message(resource: \"sms\", action: \"send\", phone: \"+15551234567\", text: \"Hello!\") — Send SMS (macOS)\n\
         - message(resource: \"sms\", action: \"conversations\") — List SMS conversations\n\
         - message(resource: \"sms\", action: \"read\", phone: \"+15551234567\") — Read SMS messages\n\
         - message(resource: \"sms\", action: \"search\", query: \"meeting\") — Search SMS messages\n\
         - message(resource: \"notify\", action: \"send\", title: \"Alert\", text: \"Something happened\") — System notification\n\
         - message(resource: \"notify\", action: \"alert\", title: \"Warning\", text: \"...\") — Show alert dialog\n\
         - message(resource: \"notify\", action: \"dnd_status\") — Check Do Not Disturb status\n\n\
         For text-to-speech: use os(resource: \"tts\", action: \"speak\", text: \"Hello\")\n\
         Use message for outbound delivery to humans outside NeboAI."
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "resource": {
                    "type": "string",
                    "description": "REQUIRED. The messaging resource category — determines which actions are available.",
                    "enum": ["owner", "notify", "sms"]
                },
                "action": {
                    "type": "string",
                    "description": "The operation to perform on the selected resource. Never put a resource name here.",
                    "enum": ["notify", "send", "alert", "dnd_status", "conversations", "read", "search"]
                },
                "text": { "type": "string", "description": "Message text" },
                "title": { "type": "string", "description": "Notification or alert title" },
                "phone": { "type": "string", "description": "Phone number or contact for SMS" },
                "query": { "type": "string", "description": "Search query for SMS search" },
                "limit": { "type": "integer", "description": "Max number of results to return", "default": 20 }
            },
            "required": ["resource", "action"]
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
            let domain_input: DomainInput = match serde_json::from_value(input.clone()) {
                Ok(v) => v,
                Err(e) => return ToolResult::error(format!("Failed to parse input: {}. Do not retry — this is a schema error.", e)),
            };

            let mut input = input;
            let resource = {
                let corrected = crate::domain::auto_correct_resource(
                    &domain_input,
                    &mut input,
                    &["owner", "sms", "notify"],
                );
                if corrected.is_empty() {
                    self.infer_resource(&domain_input.action).to_string()
                } else {
                    corrected
                }
            };

            match resource.as_str() {
                "owner" => {
                    let text = input["text"].as_str().unwrap_or("");
                    if text.is_empty() {
                        return ToolResult::error(errors::missing_param("notify", "text", "message(resource: \"owner\", action: \"notify\", text: \"Task complete!\")"));
                    }

                    // Get existing companion chat or create one
                    let msg_id = uuid::Uuid::new_v4().to_string();
                    let companion = match self.store.get_companion_chat_by_user("") {
                        Ok(Some(chat)) => Ok(chat),
                        _ => {
                            let chat_id = uuid::Uuid::new_v4().to_string();
                            self.store.create_companion_chat(&chat_id, "")
                        }
                    };

                    match companion {
                        Ok(chat) => {
                            let _ = self.store.create_chat_message(
                                &msg_id,
                                &chat.id,
                                "assistant",
                                text,
                                None,
                            );
                            // Fire OS notification
                            notify_crate::send("Nebo", text);
                            ToolResult::ok(format!("Notified owner: {}", text))
                        }
                        Err(e) => ToolResult::error(format!("Failed to notify: {}. Do not retry — this is a database error.", e)),
                    }
                }
                "notify" => handle_notify(&self.store, &domain_input.action, &input).await,
                "sms" => handle_sms(&domain_input.action, &input).await,
                other => ToolResult::error(format!(
                    "Resource {:?} not available. Available: owner, notify, sms",
                    other
                )),
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Notify resource handlers
// ---------------------------------------------------------------------------

async fn handle_notify(store: &Store, action: &str, input: &serde_json::Value) -> ToolResult {
    match action {
        "send" => {
            let text = input["text"].as_str().unwrap_or("");
            let title = input["title"].as_str().unwrap_or("Nebo");

            if text.is_empty() {
                return ToolResult::error(errors::missing_param("send", "text", "message(resource: \"notify\", action: \"send\", title: \"Alert\", text: \"Something happened\")"));
            }

            let id = uuid::Uuid::new_v4().to_string();
            match store.create_notification(
                &id,
                "", // user_id (single-user app)
                "info",
                title,
                Some(text),
                None,
                None,
            ) {
                Ok(_) => {
                    notify_crate::send(title, text);
                    ToolResult::ok(format!("Notification sent: {}", text))
                }
                Err(e) => ToolResult::error(format!("Failed to send notification: {}. Do not retry — this is a database error.", e)),
            }
        }
        "alert" => {
            let text = input["text"].as_str().unwrap_or("");
            let title = input["title"].as_str().unwrap_or("Nebo");

            if text.is_empty() {
                return ToolResult::error(errors::missing_param("alert", "text", "message(resource: \"notify\", action: \"alert\", title: \"Warning\", text: \"Something happened\")"));
            }

            handle_alert(title, text).await
        }
        "speak" => ToolResult::error(
            "speak has moved to the os tool: os(resource: \"tts\", action: \"speak\", text: \"...\")",
        ),
        "dnd_status" => handle_dnd_status().await,
        other => ToolResult::error(format!(
            "Unknown action '{}' for notify resource. Available: send, alert, speak, dnd_status",
            other
        )),
    }
}

// ---------------------------------------------------------------------------
// Alert (modal/urgent notification)
// ---------------------------------------------------------------------------

async fn handle_alert(title: &str, text: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            r#"display alert "{}" message "{}""#,
            title.replace('"', r#"\""#),
            text.replace('"', r#"\""#),
        );
        return run_osascript(&script).await;
    }

    #[cfg(target_os = "linux")]
    {
        if which_exists("notify-send") {
            return run_command("notify-send", &["--urgency=critical", title, text]).await;
        }
        return ToolResult::error("notify-send is not installed. Install libnotify (e.g. `sudo apt install libnotify-bin`) and try again.");
    }

    #[cfg(target_os = "windows")]
    {
        let script = format!(
            r#"Add-Type -AssemblyName PresentationFramework; [System.Windows.MessageBox]::Show("{}", "{}")"#,
            text.replace('"', "`\""),
            title.replace('"', "`\""),
        );
        return run_powershell(&script).await;
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Alert is not supported on this platform. Use message(resource: \"notify\", action: \"send\", ...) to send a standard notification instead.")
}

// ---------------------------------------------------------------------------
// DND status
// ---------------------------------------------------------------------------

async fn handle_dnd_status() -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        // Try Focus Modes first (macOS 12+), fall back to legacy DND prefs
        let output = tokio::process::Command::new("defaults")
            .args([
                "read",
                "com.apple.controlcenter",
                "NSStatusItem Visible FocusModes",
            ])
            .output()
            .await;

        match output {
            Ok(o) if o.status.success() => {
                let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
                let enabled = stdout == "1";
                return ToolResult::ok(
                    serde_json::json!({
                        "dnd_enabled": enabled,
                        "raw": stdout,
                    })
                    .to_string(),
                );
            }
            _ => {}
        }

        // Fallback: legacy dnd_prefs
        let output = tokio::process::Command::new("defaults")
            .args(["read", "com.apple.ncprefs", "dnd_prefs"])
            .output()
            .await;

        match output {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
                let enabled =
                    stdout.contains("dndDisplayLock = 1") || stdout.contains("dndMirrored = 1");
                return ToolResult::ok(
                    serde_json::json!({
                        "dnd_enabled": enabled,
                        "raw": stdout,
                    })
                    .to_string(),
                );
            }
            Err(e) => return ToolResult::error(format!("Failed to read DND status: {}. Do not retry — this is a system error.", e)),
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Try D-Bus to check GNOME DND
        let output = tokio::process::Command::new("dbus-send")
            .args([
                "--session",
                "--print-reply",
                "--dest=org.freedesktop.Notifications",
                "/org/freedesktop/Notifications",
                "org.freedesktop.DBus.Properties.Get",
                "string:org.freedesktop.Notifications",
                "string:DoNotDisturb",
            ])
            .output()
            .await;

        match output {
            Ok(o) if o.status.success() => {
                let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
                let enabled = stdout.contains("true");
                return ToolResult::ok(
                    serde_json::json!({
                        "dnd_enabled": enabled,
                        "raw": stdout,
                    })
                    .to_string(),
                );
            }
            _ => {
                return ToolResult::ok(
                    serde_json::json!({
                        "dnd_enabled": false,
                        "note": "Could not query D-Bus; assuming DND is off",
                    })
                    .to_string(),
                );
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        let script = r#"try { $val = Get-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\CloudStore\Store\DefaultAccount\Current\default$windows.data.notifications.quiethourssettings\windows.data.notifications.quiethourssettings' -ErrorAction Stop; Write-Output $val } catch { Write-Output 'unavailable' }"#;
        return run_powershell(script).await;
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Do Not Disturb status is not available on this platform. Do not retry.")
}

// ---------------------------------------------------------------------------
// SMS resource handlers (macOS Messages.app via chat.db)
// ---------------------------------------------------------------------------

async fn handle_sms(action: &str, input: &serde_json::Value) -> ToolResult {
    match action {
        "send" => handle_sms_send(input).await,
        "conversations" => handle_sms_conversations(input).await,
        "read" => handle_sms_read(input).await,
        "search" => handle_sms_search(input).await,
        other => ToolResult::error(format!(
            "Unknown action '{}' for sms resource. Available: send, conversations, read, search",
            other
        )),
    }
}

#[cfg(not(target_os = "macos"))]
async fn handle_sms_send(_input: &serde_json::Value) -> ToolResult {
    ToolResult::error("SMS is only available on macOS (via Messages.app). Do not retry on this platform.")
}

#[cfg(target_os = "macos")]
async fn handle_sms_send(input: &serde_json::Value) -> ToolResult {
    let text = input["text"].as_str().unwrap_or("");
    let phone = input["phone"].as_str().unwrap_or("");

    if text.is_empty() {
        return ToolResult::error(errors::missing_param("send", "text", "message(resource: \"sms\", action: \"send\", phone: \"+15551234567\", text: \"Hello!\")"));
    }
    if phone.is_empty() {
        return ToolResult::error(errors::missing_param("send", "phone", "message(resource: \"sms\", action: \"send\", phone: \"+15551234567\", text: \"Hello!\")"));
    }

    // Use variables and `service id` to avoid quoting issues and work on modern macOS.
    // Pipe via stdin to preserve emoji and multi-byte characters.
    let script = format!(
        "set theMessage to \"{text}\"\n\
         set theBuddy to \"{phone}\"\n\
         tell application \"Messages\"\n\
         \tset targetService to 1st account whose service type = iMessage\n\
         \tset targetBuddy to participant theBuddy of targetService\n\
         \tsend theMessage to targetBuddy\n\
         end tell",
        text = text.replace('\\', "\\\\").replace('"', "\\\""),
        phone = phone.replace('"', "\\\""),
    );
    run_osascript_stdin(&script).await
}

#[cfg(not(target_os = "macos"))]
async fn handle_sms_conversations(_input: &serde_json::Value) -> ToolResult {
    ToolResult::error("SMS is only available on macOS (via Messages.app). Do not retry on this platform.")
}

#[cfg(target_os = "macos")]
async fn handle_sms_conversations(input: &serde_json::Value) -> ToolResult {
    let limit = input["limit"].as_i64().unwrap_or(20);
    let db_path = match chat_db_path() {
        Some(p) => p,
        None => return ToolResult::error("Could not locate ~/Library/Messages/chat.db. Grant Full Disk Access to Nebo in System Settings > Privacy & Security."),
    };

    let query = format!(
        "SELECT c.chat_identifier, c.display_name, \
         (SELECT COUNT(*) FROM message m JOIN chat_message_join cmj ON m.ROWID = cmj.message_id WHERE cmj.chat_id = c.ROWID) as msg_count, \
         (SELECT datetime(m.date/1000000000 + 978307200, 'unixepoch', 'localtime') FROM message m JOIN chat_message_join cmj ON m.ROWID = cmj.message_id WHERE cmj.chat_id = c.ROWID ORDER BY m.date DESC LIMIT 1) as last_message_date \
         FROM chat c ORDER BY last_message_date DESC LIMIT {};",
        limit
    );

    run_sqlite3(&db_path, &query).await
}

#[cfg(not(target_os = "macos"))]
async fn handle_sms_read(_input: &serde_json::Value) -> ToolResult {
    ToolResult::error("SMS is only available on macOS (via Messages.app). Do not retry on this platform.")
}

#[cfg(target_os = "macos")]
async fn handle_sms_read(input: &serde_json::Value) -> ToolResult {
    let phone = input["phone"].as_str().unwrap_or("");
    if phone.is_empty() {
        return ToolResult::error(errors::missing_param("read", "phone", "message(resource: \"sms\", action: \"read\", phone: \"+15551234567\")"));
    }

    let limit = input["limit"].as_i64().unwrap_or(20);
    let db_path = match chat_db_path() {
        Some(p) => p,
        None => return ToolResult::error("Could not locate ~/Library/Messages/chat.db. Grant Full Disk Access to Nebo in System Settings > Privacy & Security."),
    };

    let escaped_phone = phone.replace('\'', "''");
    let query = format!(
        "SELECT m.is_from_me, \
         datetime(m.date/1000000000 + 978307200, 'unixepoch', 'localtime') as msg_date, \
         m.text \
         FROM message m \
         JOIN chat_message_join cmj ON m.ROWID = cmj.message_id \
         JOIN chat c ON cmj.chat_id = c.ROWID \
         WHERE c.chat_identifier = '{}' \
         ORDER BY m.date DESC LIMIT {};",
        escaped_phone, limit
    );

    run_sqlite3(&db_path, &query).await
}

#[cfg(not(target_os = "macos"))]
async fn handle_sms_search(_input: &serde_json::Value) -> ToolResult {
    ToolResult::error("SMS is only available on macOS (via Messages.app). Do not retry on this platform.")
}

#[cfg(target_os = "macos")]
async fn handle_sms_search(input: &serde_json::Value) -> ToolResult {
    let query_text = input["query"].as_str().unwrap_or("");
    if query_text.is_empty() {
        return ToolResult::error(errors::missing_param("search", "query", "message(resource: \"sms\", action: \"search\", query: \"meeting\")"));
    }

    let limit = input["limit"].as_i64().unwrap_or(20);
    let db_path = match chat_db_path() {
        Some(p) => p,
        None => return ToolResult::error("Could not locate ~/Library/Messages/chat.db. Grant Full Disk Access to Nebo in System Settings > Privacy & Security."),
    };

    let escaped_query = query_text.replace('\'', "''");
    let query = format!(
        "SELECT c.chat_identifier, m.is_from_me, \
         datetime(m.date/1000000000 + 978307200, 'unixepoch', 'localtime') as msg_date, \
         m.text \
         FROM message m \
         JOIN chat_message_join cmj ON m.ROWID = cmj.message_id \
         JOIN chat c ON cmj.chat_id = c.ROWID \
         WHERE m.text LIKE '%{}%' \
         ORDER BY m.date DESC LIMIT {};",
        escaped_query, limit
    );

    run_sqlite3(&db_path, &query).await
}

// ---------------------------------------------------------------------------
// Helper: macOS chat.db path
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn chat_db_path() -> Option<String> {
    dirs::home_dir()
        .map(|h| h.join("Library/Messages/chat.db"))
        .and_then(|p| p.to_str().map(|s| s.to_string()))
}

// ---------------------------------------------------------------------------
// Helper: run sqlite3 CLI
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
async fn run_sqlite3(db_path: &str, query: &str) -> ToolResult {
    let output = tokio::process::Command::new("sqlite3")
        .args(["-header", "-separator", "|", db_path, query])
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if stdout.is_empty() {
                ToolResult::ok("No results found.")
            } else {
                ToolResult::ok(stdout)
            }
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            ToolResult::error(format!("sqlite3 error: {}. Do not retry — this is a database error.", stderr))
        }
        Err(e) => ToolResult::error(format!("Failed to run sqlite3: {}. Do not retry — this is a system error.", e)),
    }
}

// ---------------------------------------------------------------------------
// Helper: run osascript (macOS)
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
async fn run_osascript_stdin(script: &str) -> ToolResult {
    use tokio::io::AsyncWriteExt;
    let mut child = match tokio::process::Command::new("osascript")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return ToolResult::error(format!("Failed to run osascript: {}. Do not retry — this is a system error.", e)),
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(script.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }
    let output = child.wait_with_output().await;

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if stdout.is_empty() {
                ToolResult::ok("SMS sent successfully")
            } else {
                ToolResult::ok(stdout)
            }
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            ToolResult::error(format!("osascript error: {}. Do not retry — this is a system error.", stderr))
        }
        Err(e) => ToolResult::error(format!("Failed to run osascript: {}. Do not retry — this is a system error.", e)),
    }
}

#[cfg(target_os = "macos")]
async fn run_osascript(script: &str) -> ToolResult {
    let output = tokio::process::Command::new("osascript")
        .args(["-e", script])
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if stdout.is_empty() {
                ToolResult::ok("OK")
            } else {
                ToolResult::ok(stdout)
            }
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            ToolResult::error(format!("osascript error: {}. Do not retry — this is a system error.", stderr))
        }
        Err(e) => ToolResult::error(format!("Failed to run osascript: {}. Do not retry — this is a system error.", e)),
    }
}

// ---------------------------------------------------------------------------
// Helper: run an arbitrary command
// ---------------------------------------------------------------------------

#[allow(dead_code)] // Used on Linux
async fn run_command(cmd: &str, args: &[&str]) -> ToolResult {
    let output = tokio::process::Command::new(cmd).args(args).output().await;

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if stdout.is_empty() {
                ToolResult::ok("OK")
            } else {
                ToolResult::ok(stdout)
            }
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            ToolResult::error(format!("{} error: {}. Do not retry — this is a system error.", cmd, stderr))
        }
        Err(e) => ToolResult::error(format!("Failed to run {}: {}. Do not retry — this is a system error.", cmd, e)),
    }
}

// ---------------------------------------------------------------------------
// Helper: run PowerShell (Windows)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
async fn run_powershell(script: &str) -> ToolResult {
    let output = tokio::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", script])
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if stdout.is_empty() {
                ToolResult::ok("OK")
            } else {
                ToolResult::ok(stdout)
            }
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            ToolResult::error(format!("PowerShell error: {}. Do not retry — this is a system error.", stderr))
        }
        Err(e) => ToolResult::error(format!("Failed to run PowerShell: {}. Do not retry — this is a system error.", e)),
    }
}

// ---------------------------------------------------------------------------
// Helper: check if a command exists on PATH (Linux/Windows)
// ---------------------------------------------------------------------------

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn which_exists(cmd: &str) -> bool {
    which::which(cmd).is_ok()
}
