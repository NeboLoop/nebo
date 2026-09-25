//! Reaching the owner outside a conversation: `message_owner` writes into
//! their chat, `push_notification` sends a notification (an urgent one
//! raises the alert), `check_dnd` reads whether Do Not Disturb is on.

use std::sync::Arc;

use db::Store;
use serde_json::{Value, json};

use crate::message_tool::NotifyFn;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

pub struct Owner {
    store: Arc<Store>,
    /// The registry's broadcast cell, late-wired by the server: an alert
    /// reaches the bell and the desktop HUD through it.
    notify_fn: Arc<std::sync::RwLock<Option<NotifyFn>>>,
}

impl Owner {
    pub fn new(store: Arc<Store>, notify_fn: Arc<std::sync::RwLock<Option<NotifyFn>>>) -> Self {
        Self { store, notify_fn }
    }

    pub fn tools(self) -> Vec<Box<dyn DynTool>> {
        let owner = Arc::new(self);
        [OwnerOp::Message, OwnerOp::Notify, OwnerOp::CheckDnd]
            .into_iter()
            .map(|op| {
                Box::new(OwnerTool {
                    op,
                    owner: owner.clone(),
                }) as Box<dyn DynTool>
            })
            .collect()
    }

    /// Into the owner's companion chat, with an OS notification.
    fn message(&self, input: &Value) -> ToolResult {
        let text = input["message"].as_str().unwrap_or("");
        let companion = match self.store.get_companion_chat_by_user("") {
            Ok(Some(chat)) => Ok(chat),
            _ => self
                .store
                .create_companion_chat(&uuid::Uuid::new_v4().to_string(), ""),
        };
        match companion {
            Ok(chat) => {
                let _ = self.store.create_chat_message(
                    &uuid::Uuid::new_v4().to_string(),
                    &chat.id,
                    "assistant",
                    text,
                    None,
                );
                notify_crate::send("Nebo", text);
                ToolResult::ok(format!(
                    "Messaged the owner ({} chars)",
                    text.chars().count()
                ))
            }
            Err(e) => ToolResult::error(format!(
                "Failed to message the owner: {e}. Do not retry — this is a database error."
            )),
        }
    }

    /// A persisted notification (the bell) plus the OS notification; an
    /// urgent one is the alert: a `notification` broadcast the desktop turns
    /// into the auto-dismissing HUD. Never a modal.
    fn notify(&self, input: &Value, ctx: &ToolContext) -> ToolResult {
        let text = input["message"].as_str().unwrap_or("");
        let title = input["title"]
            .as_str()
            .filter(|t| !t.is_empty())
            .unwrap_or("Nebo");
        let urgent = input["urgent"].as_bool().unwrap_or(false);
        // Attributed to the employee that sent it.
        let agent_id =
            Some(types::keyparser::extract_agent_id(&ctx.session_key)).filter(|s| !s.is_empty());
        let id = uuid::Uuid::new_v4().to_string();
        let n = crate::owner_notify::OwnerNotification {
            id: &id,
            kind: if urgent { "warning" } else { "info" },
            title,
            body: Some(text),
            action_url: None,
            agent_id: agent_id.as_deref(),
            loud: urgent,
        };
        if urgent {
            let notify = self.notify_fn.read().ok().and_then(|g| g.clone());
            match notify {
                Some(f) => {
                    crate::owner_notify::emit(&self.store, Some(&|ev, payload| f(ev, payload)), &n)
                }
                None => crate::owner_notify::emit(&self.store, None, &n),
            }
            return ToolResult::ok(format!("Alerted the owner: {title}"));
        }
        crate::owner_notify::emit(&self.store, None, &n);
        notify_crate::send(title, text);
        ToolResult::ok(format!(
            "Notification sent ({} chars, title '{title}')",
            text.chars().count()
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnerOp {
    Message,
    Notify,
    CheckDnd,
}

struct OwnerTool {
    op: OwnerOp,
    owner: Arc<Owner>,
}

impl DynTool for OwnerTool {
    fn name(&self) -> &str {
        match self.op {
            OwnerOp::Message => "message_owner",
            OwnerOp::Notify => "push_notification",
            OwnerOp::CheckDnd => "check_dnd",
        }
    }

    fn description(&self) -> String {
        match self.op {
            OwnerOp::Message => "Sends the owner a message in their chat with you, with a notification. For work nobody is watching (a schedule, a workflow); in a conversation, just reply.".to_string(),
            OwnerOp::Notify => "Sends the owner a notification on this computer and in the app's bell.\n\
                 - `urgent: true` raises the alert: for something that needs them now.\n\
                 - check_dnd first if it can wait."
                .to_string(),
            OwnerOp::CheckDnd => "Reads whether Do Not Disturb (Focus) is on for the owner, and which modes.".to_string(),
        }
    }

    fn schema(&self) -> Value {
        match self.op {
            OwnerOp::Message => json!({
                "type": "object",
                "properties": { "message": { "type": "string", "description": "What to tell the owner." } },
                "required": ["message"]
            }),
            OwnerOp::Notify => json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string", "description": "The notification text." },
                    "title": { "type": "string", "description": "Short title (default Nebo)." },
                    "urgent": { "type": "boolean", "description": "Raise the alert: it needs the owner now." }
                },
                "required": ["message"]
            }),
            OwnerOp::CheckDnd => json!({ "type": "object", "properties": {} }),
        }
    }

    fn search_hint(&self) -> &str {
        match self.op {
            OwnerOp::Message => "message the owner in their chat",
            OwnerOp::Notify => "notify or alert the owner",
            OwnerOp::CheckDnd => "is do not disturb on",
        }
    }

    fn read_only(&self, _input: &Value) -> bool {
        self.op == OwnerOp::CheckDnd
    }

    /// Telling the owner is not an outside effect.
    fn effects(&self, _input: &Value) -> types::permissions::CallEffects {
        types::permissions::CallEffects::none()
    }

    fn validate_input(&self, input: &Value) -> Result<(), String> {
        if self.op != OwnerOp::CheckDnd
            && input["message"]
                .as_str()
                .is_none_or(|m| m.trim().is_empty())
        {
            return Err("message can't be empty.".to_string());
        }
        Ok(())
    }

    fn activity(&self, _input: &Value) -> String {
        match self.op {
            OwnerOp::Message => "messaging you".to_string(),
            OwnerOp::Notify => "notifying you".to_string(),
            OwnerOp::CheckDnd => "checking Do Not Disturb".to_string(),
        }
    }

    fn outcome(&self, _input: &Value) -> String {
        match self.op {
            OwnerOp::Message => "Messaged you".to_string(),
            OwnerOp::Notify => "Notified you".to_string(),
            OwnerOp::CheckDnd => "Checked Do Not Disturb".to_string(),
        }
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            match self.op {
                OwnerOp::Message => self.owner.message(&input),
                OwnerOp::Notify => self.owner.notify(&input, ctx),
                OwnerOp::CheckDnd => handle_dnd_status().await,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Execution raises a real OS notification, so only the interface is
    /// exercised here.
    #[test]
    fn the_owner_tools_need_a_message() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("o.db").to_string_lossy()).unwrap());
        let tools = Owner::new(store, Arc::new(std::sync::RwLock::new(None))).tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(names, ["message_owner", "push_notification", "check_dnd"]);
        assert!(tools[0].validate_input(&json!({"message": ""})).is_err());
        assert!(tools[1].validate_input(&json!({"title": "x"})).is_err());
        assert!(tools[2].validate_input(&json!({})).is_ok() && tools[2].read_only(&json!({})));
    }
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
        let assertions =
            dirs::home_dir().map(|h| h.join("Library/DoNotDisturb/DB/Assertions.json"));
        let read = match assertions {
            Some(ref p) => tokio::fs::read_to_string(p)
                .await
                .map_err(|e| e.to_string()),
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
        // Focus Assist keeps its state in an undocumented binary blob; the
        // only honest reading is whether the key exists and how big it is.
        let script = r#"try { $val = Get-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\CloudStore\Store\DefaultAccount\Current\default$windows.data.notifications.quiethourssettings\windows.data.notifications.quiethourssettings' -ErrorAction Stop; Write-Output ("" + $val.Data.Length) } catch { Write-Output 'unavailable' }"#;
        let r = run_powershell(script).await;
        if r.is_error {
            return r;
        }
        let note = match r.content.trim().parse::<usize>() {
            Ok(n) => format!(
                "DND state unknown: Focus Assist stores its state as an undocumented {} byte binary blob under HKCU ...quiethourssettings, which Nebo does not decode",
                n
            ),
            Err(_) => "DND state unknown: the Focus Assist registry key (HKCU ...quiethourssettings) is not present".to_string(),
        };
        return ToolResult::ok(
            serde_json::json!({
                "dnd_enabled": null,
                "note": note,
            })
            .to_string(),
        );
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Do Not Disturb status is not available on this platform. Do not retry.")
}

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
            ToolResult::error(format!(
                "PowerShell error: {}. Do not retry — this is a system error.",
                stderr
            ))
        }
        Err(e) => ToolResult::error(format!(
            "Failed to run PowerShell: {}. Do not retry — this is a system error.",
            e
        )),
    }
}
