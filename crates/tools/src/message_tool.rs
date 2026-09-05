use std::sync::Arc;

use crate::domain::DomainInput;
use crate::errors;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};
use db::Store;

/// Broadcast callback injected by the server (wired to ClientHub). Lets the
/// message tool surface owner notifications to the frontend (bell + desktop HUD)
/// without crates/tools depending on the server's hub — the same boundary-clean
/// pattern the agent worker uses (`agent::agent_worker::NotifyFn`).
pub type NotifyFn = Arc<dyn Fn(&str, serde_json::Value) + Send + Sync>;

/// MessageTool handles outbound delivery to the owner (notifications, companion chat, SMS, TTS)
/// and to coworkers (named AI employees on this bot).
pub struct MessageTool {
    store: Arc<Store>,
    /// Shared cell (NOT a snapshot): the server late-wires the broadcaster after the
    /// hub exists, so we read it at execution time — same pattern as `code_installer`.
    notify_fn: Arc<std::sync::RwLock<Option<NotifyFn>>>,
    /// Coworker message rail (server-implemented). Late-wired like `notify_fn`.
    coworker_rail: crate::coworker::CoworkerRailCell,
}

impl MessageTool {
    pub fn new(
        store: Arc<Store>,
        notify_fn: Arc<std::sync::RwLock<Option<NotifyFn>>>,
        coworker_rail: crate::coworker::CoworkerRailCell,
    ) -> Self {
        Self {
            store,
            notify_fn,
            coworker_rail,
        }
    }

    fn infer_resource(&self, action: &str) -> &str {
        match action {
            "notify" => "owner",
            "alert" | "dnd_status" => "notify",
            "conversations" | "read" | "search" => "sms",
            _ => "",
        }
    }

    async fn handle_coworker(&self, ctx: &ToolContext, input: &serde_json::Value) -> ToolResult {
        let to = input["to"].as_str().unwrap_or("");
        let text = input["text"].as_str().unwrap_or("");
        if to.is_empty() {
            return ToolResult::error(errors::missing_param(
                "send",
                "to",
                "message(resource: \"coworker\", action: \"send\", to: \"receptionist\", text: \"...\")",
            ));
        }
        if text.is_empty() {
            return ToolResult::error(errors::missing_param(
                "send",
                "text",
                "message(resource: \"coworker\", action: \"send\", to: \"receptionist\", text: \"...\")",
            ));
        }

        let rail = self.coworker_rail.read().unwrap().clone();
        let Some(rail) = rail else {
            return ToolResult::error(
                "Coworker messaging is not available in this environment. Do not retry.",
            );
        };

        let msg = crate::coworker::CoworkerMessage {
            from_agent_id: types::keyparser::extract_agent_id(&ctx.session_key),
            sender_session_key: ctx.session_key.clone(),
            to: to.to_string(),
            text: text.to_string(),
            // Verbatim resolved scope — the rail derives the matter from it;
            // the tool never re-derives scopes (the canonical derivation lives
            // in agent::memory::resolve_memory_scope and the runner).
            requester_scope: ctx.user_id.clone(),
            handoff_depth: ctx.handoff_depth,
            provenance: ctx.run_taint.clone(),
            wait: input["wait"].as_bool().unwrap_or(true),
        };

        match rail.send(msg).await {
            Ok(delivery) => {
                // Structured payload → the chat renders a first-class
                // "Messaged {name}" event (clickable through to the coworker
                // thread) instead of a bare tool chip. threadKey identifies
                // the delivered-into thread for the view-only transcript.
                let payload = serde_json::json!({
                    "kind": "coworker_message",
                    "to": delivery.to_name,
                    "toAgentId": delivery.to_agent_id,
                    "threadKey": delivery.thread_key,
                    "text": text,
                    "reply": delivery.reply.clone(),
                });
                match delivery.reply {
                    Some(ref reply) => ToolResult::ok(format!(
                        "Message delivered to {}. Their reply:\n\n{}",
                        delivery.to_name, reply
                    ))
                    .with_payload(payload),
                    None => ToolResult::ok(format!(
                        "Message delivered to {} — they are handling it in their own session; \
                         when their reply arrives you will be woken automatically to act on it \
                         and report. Until then, report this as \"asked {} — waiting\", never as \
                         done.",
                        delivery.to_name, delivery.to_name
                    ))
                    .with_payload(payload),
                }
            }
            Err(e) => ToolResult::error(e),
        }
    }
}

impl DynTool for MessageTool {
    fn name(&self) -> &str {
        "message"
    }

    fn description(&self) -> String {
        "Outbound delivery — message coworkers (named AI employees), and send notifications, alerts, and SMS to the owner.\n\
         USE THIS when: handing work to a named coworker, or when the user wants to send a text, notification, or alert to someone outside NeboAI.\n\n\
         Coworkers (named employees on this bot):\n\
         - message(resource: \"coworker\", action: \"send\", to: \"receptionist\", text: \"Can you confirm tomorrow's 2pm?\") — Message a coworker and wait for their reply\n\
         - message(resource: \"coworker\", action: \"send\", to: \"receptionist\", text: \"FYI: the Smith file moved.\", wait: false) — Fire-and-forget (delivery is acknowledged; their reply wakes you automatically to act on it)\n\
         The message is delivered into the coworker's own session — their persona, their memory, their connected accounts, their receipt — and the conversation is visible to the owner on both sides. \
         Work for a coworker? Message them by name. Extra hands for your own work? Spawn a task: agent(resource: \"task\", action: \"spawn\", ...).\n\
         Never claim a coworker's work is done — report \"asked X — waiting\" or relay their actual reply.\n\n\
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
                    "enum": ["coworker", "owner", "notify", "sms"]
                },
                "action": {
                    "type": "string",
                    "description": "The operation to perform on the selected resource. Never put a resource name here.",
                    "enum": ["notify", "send", "alert", "dnd_status", "conversations", "read", "search"]
                },
                "text": { "type": "string", "description": "Message text" },
                "to": { "type": "string", "description": "Coworker to message — an installed employee's name (e.g. \"receptionist\") or id" },
                "wait": { "type": "boolean", "description": "Coworker send: wait for their reply (default true). false = fire-and-forget; their reply wakes you automatically.", "default": true },
                "title": { "type": "string", "description": "Notification or alert title" },
                "phone": { "type": "string", "description": "Phone number or contact for SMS" },
                "from": { "type": "string", "description": "SMS send: which of your phone lines to text from (E.164). Omit to use your first texting line." },
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
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            // Attribute persisted notifications to the calling AI employee.
            let agent_id =
                Some(types::keyparser::extract_agent_id(&ctx.session_key)).filter(|s| !s.is_empty());
            let domain_input: DomainInput = match serde_json::from_value(input.clone()) {
                Ok(v) => v,
                Err(e) => return ToolResult::error(format!("Input did not match the schema: {}. Every call needs resource (coworker, owner, notify or sms) and action; fix the call and send it again.", e)),
            };

            let mut input = input;
            let resource = {
                let corrected = crate::domain::auto_correct_resource(
                    &domain_input,
                    &mut input,
                    &["coworker", "owner", "sms", "notify"],
                );
                if corrected.is_empty() {
                    self.infer_resource(&domain_input.action).to_string()
                } else {
                    corrected
                }
            };

            match resource.as_str() {
                "coworker" => match domain_input.action.as_str() {
                    "send" => self.handle_coworker(ctx, &input).await,
                    other => ToolResult::error(format!(
                        "Unknown action '{}' for coworker resource. Available: send",
                        other
                    )),
                },
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
                "notify" => {
                    let nf = self.notify_fn.read().unwrap().clone();
                    handle_notify(&self.store, nf.as_ref(), &domain_input.action, &input, agent_id.as_deref()).await
                }
                "sms" => handle_sms(&self.store, agent_id.as_deref(), &domain_input.action, &input).await,
                other => ToolResult::error(format!(
                    "Resource {:?} not available. Available: coworker, owner, notify, sms",
                    other
                )),
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Notify resource handlers
// ---------------------------------------------------------------------------

async fn handle_notify(store: &Store, notify_fn: Option<&NotifyFn>, action: &str, input: &serde_json::Value, agent_id: Option<&str>) -> ToolResult {
    match action {
        "send" => {
            let text = input["text"].as_str().unwrap_or("");
            let title = input["title"].as_str().unwrap_or("Nebo");

            if text.is_empty() {
                return ToolResult::error(errors::missing_param("send", "text", "message(resource: \"notify\", action: \"send\", title: \"Alert\", text: \"Something happened\")"));
            }

            let id = uuid::Uuid::new_v4().to_string();
            crate::owner_notify::emit(
                store,
                None,
                &crate::owner_notify::OwnerNotification {
                    id: &id,
                    kind: "info",
                    title,
                    body: Some(text),
                    action_url: None,
                    agent_id,
                    loud: false,
                },
            );
            notify_crate::send(title, text);
            ToolResult::ok(format!("Notification sent: {}", text))
        }
        "alert" => {
            let text = input["text"].as_str().unwrap_or("");
            let title = input["title"].as_str().unwrap_or("Nebo");

            if text.is_empty() {
                return ToolResult::error(errors::missing_param("alert", "text", "message(resource: \"notify\", action: \"alert\", title: \"Warning\", text: \"Something happened\")"));
            }

            handle_alert(store, notify_fn, title, text, agent_id).await
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
// Alert (urgent owner notification → bell + desktop HUD)
// ---------------------------------------------------------------------------

/// Surface an urgent alert to the owner via the canonical notification pathway:
/// a persisted row (the bell) plus a `notification` broadcast that the desktop
/// frontend turns into the branded auto-dismissing HUD. Replaces the old
/// osascript `display alert` modal (blocking, generic icon, never auto-dismisses).
/// Falls back to a persisted-only notification when no broadcaster is wired
/// (headless / no frontend) — never a modal.
async fn handle_alert(store: &Store, notify_fn: Option<&NotifyFn>, title: &str, text: &str, agent_id: Option<&str>) -> ToolResult {
    let id = uuid::Uuid::new_v4().to_string();
    let n = crate::owner_notify::OwnerNotification {
        id: &id,
        kind: "warning",
        title,
        body: Some(text),
        action_url: None,
        agent_id,
        loud: true,
    };
    match notify_fn {
        Some(f) => crate::owner_notify::emit(store, Some(&|ev, payload| f(ev, payload)), &n),
        None => crate::owner_notify::emit(store, None, &n),
    }

    ToolResult::ok(format!("Alerted the owner: {}", title))
}

// ---------------------------------------------------------------------------
// DND status
// ---------------------------------------------------------------------------

async fn handle_dnd_status() -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        // Focus (macOS 12+) records every active mode as an assertion in
        // ~/Library/DoNotDisturb/DB/Assertions.json; an empty record list
        // means no Focus is on. That is the state itself, not a menu-bar
        // preference.
        let assertions = dirs::home_dir().map(|h| h.join("Library/DoNotDisturb/DB/Assertions.json"));
        let read = match assertions {
            Some(ref p) => tokio::fs::read_to_string(p).await.map_err(|e| e.to_string()),
            None => Err("home directory unknown".to_string()),
        };
        match read.map(|s| focus_assertions(&s)) {
            Ok(Some(modes)) => ToolResult::ok(
                serde_json::json!({
                    "dnd_enabled": !modes.is_empty(),
                    "active_focus_modes": modes,
                    "source": "~/Library/DoNotDisturb/DB/Assertions.json",
                })
                .to_string(),
            ),
            Ok(None) => ToolResult::ok(
                serde_json::json!({
                    "dnd_enabled": null,
                    "note": "DND state unknown: ~/Library/DoNotDisturb/DB/Assertions.json was read but is not in the expected shape",
                })
                .to_string(),
            ),
            Err(e) => {
                // Legacy (pre-Focus) preference, still a real DND flag on
                // old systems; on new ones the key is absent.
                let legacy = tokio::process::Command::new("defaults")
                    .args(["read", "com.apple.ncprefs", "dnd_prefs"])
                    .output()
                    .await;
                if let Ok(o) = legacy
                    && o.status.success()
                {
                    let stdout = String::from_utf8_lossy(&o.stdout);
                    let enabled = stdout.contains("dndDisplayLock = 1") || stdout.contains("dndMirrored = 1");
                    return ToolResult::ok(
                        serde_json::json!({
                            "dnd_enabled": enabled,
                            "source": "defaults read com.apple.ncprefs dnd_prefs (legacy)",
                        })
                        .to_string(),
                    );
                }
                ToolResult::ok(
                    serde_json::json!({
                        "dnd_enabled": null,
                        "note": format!("DND state unknown: could not read ~/Library/DoNotDisturb/DB/Assertions.json ({}); ask the owner to grant Nebo Full Disk Access if this persists", e),
                    })
                    .to_string(),
                )
            }
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
                        "source": "org.freedesktop.Notifications DoNotDisturb property via dbus-send",
                    })
                    .to_string(),
                );
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
                return ToolResult::ok(
                    serde_json::json!({
                        "dnd_enabled": null,
                        "note": format!("DND state unknown: D-Bus query failed (dbus-send exited {}: {})", o.status.code().unwrap_or(-1), stderr),
                    })
                    .to_string(),
                );
            }
            Err(e) => {
                return ToolResult::ok(
                    serde_json::json!({
                        "dnd_enabled": null,
                        "note": format!("DND state unknown: D-Bus query failed (dbus-send could not run: {})", e),
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

async fn handle_sms(store: &Store, agent_id: Option<&str>, action: &str, input: &serde_json::Value) -> ToolResult {
    match action {
        "send" => match send_from_phone_line(store, agent_id, input).await {
            Some(r) => r,
            None => handle_sms_send(input).await,
        },
        "conversations" => handle_sms_conversations(input).await,
        "read" => handle_sms_read(input).await,
        "search" => handle_sms_search(input).await,
        other => ToolResult::error(format!(
            "Unknown action '{}' for sms resource. Available: send, conversations, read, search",
            other
        )),
    }
}

/// An employee with a texting-enabled phone line texts from that line — the
/// business number the caller already knows — through the hub. `None` means
/// this employee has no such line, and the send falls through to the
/// owner's Messages.app (the pre-existing personal-device path).
async fn send_from_phone_line(store: &Store, agent_id: Option<&str>, input: &serde_json::Value) -> Option<ToolResult> {
    let agent_id = agent_id?;
    let text = input["text"].as_str().unwrap_or("");
    let phone = input["phone"].as_str().unwrap_or("");
    let api = crate::build_neboai_api(store).ok()?;
    let lines = api.list_phone_lines().await.ok()?;
    let wanted = input["from"].as_str().filter(|s| !s.is_empty());
    let mine: Vec<&serde_json::Value> = lines["numbers"]
        .as_array()?
        .iter()
        .filter(|l| l["agentId"].as_str() == Some(agent_id) && l["smsEnabled"].as_bool() == Some(true))
        .collect();
    let line = match wanted {
        Some(w) => mine.iter().copied().find(|l| l["number"].as_str() == Some(w)),
        None => mine.first().copied(),
    };
    if wanted.is_some() && line.is_none() {
        return Some(ToolResult::error(format!(
            "{} is not one of your texting lines. Omit `from` to use your first texting line.",
            wanted.unwrap_or("")
        )));
    }
    let line = line?;
    let from = line["number"].as_str()?.to_string();
    if text.is_empty() {
        return Some(ToolResult::error(errors::missing_param("send", "text", "message(resource: \"sms\", action: \"send\", phone: \"+15551234567\", text: \"Hello!\")")));
    }
    if phone.is_empty() {
        return Some(ToolResult::error(errors::missing_param("send", "phone", "message(resource: \"sms\", action: \"send\", phone: \"+15551234567\", text: \"Hello!\")")));
    }
    Some(match api.send_phone_sms(&from, phone, text).await {
        Ok(_) => ToolResult::ok(format!("Sent by text from your line {from} to {phone}.")),
        Err(e) => ToolResult::error(format!("Could not text from line {from}: {e}. Do not retry with a different resource; tell the owner if this persists.")),
    })
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
    // Media (images/screenshots) is NOT in m.text — it lives in the attachment
    // table; without this column an MMS reads as an empty message (data loss).
    // The filenames are on-disk paths (~/Library/Messages/Attachments/…) the
    // agent can read or view directly.
    let query = format!(
        "SELECT m.is_from_me, \
         datetime(m.date/1000000000 + 978307200, 'unixepoch', 'localtime') as msg_date, \
         m.text, \
         (SELECT GROUP_CONCAT(a.filename, ', ') FROM message_attachment_join maj \
          JOIN attachment a ON maj.attachment_id = a.ROWID \
          WHERE maj.message_id = m.ROWID) as attachments \
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
         m.text, \
         (SELECT GROUP_CONCAT(a.filename, ', ') FROM message_attachment_join maj \
          JOIN attachment a ON maj.attachment_id = a.ROWID \
          WHERE maj.message_id = m.ROWID) as attachments \
         FROM message m \
         JOIN chat_message_join cmj ON m.ROWID = cmj.message_id \
         JOIN chat c ON cmj.chat_id = c.ROWID \
         WHERE m.text LIKE '%{}%' \
         ORDER BY m.date DESC LIMIT {};",
        escaped_query, limit
    );

    run_sqlite3(
        &db_path,
        &query,
        &format!("No messages whose text contains '{}' (plain substring).", query_text),
    )
    .await
}

// ---------------------------------------------------------------------------
// Helper: macOS Focus assertions
// ---------------------------------------------------------------------------

/// Active Focus mode identifiers from the contents of
/// `~/Library/DoNotDisturb/DB/Assertions.json`: every `storeAssertionRecords`
/// entry under `data` is one active mode. `None` when the document is not in
/// that shape (so the caller reports "unknown", never a guessed boolean).
#[cfg(target_os = "macos")]
fn focus_assertions(json: &str) -> Option<Vec<String>> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let data = v.get("data")?.as_array()?;
    let mut modes = Vec::new();
    for entry in data {
        let records = entry
            .get("storeAssertionRecords")
            .and_then(|r| r.as_array())
            .map(|r| r.as_slice())
            .unwrap_or(&[]);
        for record in records {
            let mode = record
                .get("assertionDetails")
                .and_then(|d| d.get("assertionDetailsModeIdentifier"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown");
            modes.push(mode.to_string());
        }
    }
    Some(modes)
}

#[cfg(all(test, target_os = "macos"))]
mod focus_tests {
    use super::focus_assertions;

    #[test]
    fn active_focus_lists_mode_identifiers() {
        let json = r#"{"data":[{"storeAssertionRecords":[{"assertionDetails":{"assertionDetailsModeIdentifier":"com.apple.donotdisturb.mode.default"}}]}]}"#;
        assert_eq!(
            focus_assertions(json),
            Some(vec!["com.apple.donotdisturb.mode.default".to_string()])
        );
    }

    #[test]
    fn no_focus_is_empty_not_unknown() {
        let json = r#"{"data":[{"storeAssertionRecords":[]}]}"#;
        assert_eq!(focus_assertions(json), Some(vec![]));
    }

    #[test]
    fn unexpected_shape_is_unknown() {
        assert_eq!(focus_assertions("not json"), None);
        assert_eq!(focus_assertions(r#"{"other":1}"#), None);
    }
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
                ToolResult::ok(empty.to_string())
            } else {
                ToolResult::ok(stdout)
            }
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            let hint = if stderr.contains("unable to open database file") || stderr.contains("authorization denied") {
                " Nebo cannot open chat.db: ask the owner to grant Nebo Full Disk Access (System Settings > Privacy & Security > Full Disk Access)."
            } else {
                ""
            };
            ToolResult::error(format!(
                "sqlite3 exited {} reading {}: {}.{}",
                o.status.code().unwrap_or(-1),
                db_path,
                stderr,
                hint
            ))
        }
        Err(e) => ToolResult::error(format!("Failed to run sqlite3: {}. Do not retry — this is a system error.", e)),
    }
}

// ---------------------------------------------------------------------------
// Helper: run osascript (macOS)
// ---------------------------------------------------------------------------

/// Run an AppleScript from stdin. `ok_text` is the result when the script
/// exits 0 without printing anything (what was handed off, to whom).
#[cfg(target_os = "macos")]
async fn run_osascript_stdin(script: &str, ok_text: &str) -> ToolResult {
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
                ToolResult::ok(ok_text.to_string())
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

