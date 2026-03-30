use std::sync::Arc;

use crate::app_tool::AppTool;
use crate::desktop_tool::DesktopTool;
use crate::domain::DomainInput;
use crate::file_tool::FileTool;
use crate::keychain_tool::KeychainTool;
use crate::music_tool::MusicTool;
use crate::organizer_tool::OrganizerTool;
use crate::origin::ToolContext;
use crate::policy::Policy;
use crate::process::ProcessRegistry;
use crate::registry::{DynTool, ResourceKind, ToolResult};
use crate::settings_tool::SettingsTool;
use crate::shell_tool::ShellTool;
use crate::spotlight_tool::SpotlightTool;

/// Unified OS tool — all local machine operations under one namespace.
///
/// 25 resources spanning file system, shell, desktop automation, apps, settings,
/// media, credentials, search, and personal information management.
/// Per-resource approval: safe resources auto-approve, sensitive ones require confirmation.
pub struct OsTool {
    file_tool: FileTool,
    shell_tool: ShellTool,
    desktop_tool: DesktopTool,
    app_tool: AppTool,
    settings_tool: SettingsTool,
    music_tool: MusicTool,
    keychain_tool: KeychainTool,
    spotlight_tool: SpotlightTool,
    organizer_tool: OrganizerTool,
}

/// Resources that auto-approve (no user confirmation needed).
const AUTO_APPROVE_RESOURCES: &[&str] = &[
    "file", "shell", "clipboard", "capture", "search",
    "notification", "tts", "dock",
];

impl OsTool {
    pub fn new(policy: Policy, process_registry: Arc<ProcessRegistry>) -> Self {
        Self {
            file_tool: FileTool::new(),
            shell_tool: ShellTool::new(policy, process_registry),
            desktop_tool: DesktopTool::new(),
            app_tool: AppTool::new(),
            settings_tool: SettingsTool::new(),
            music_tool: MusicTool::new(),
            keychain_tool: KeychainTool::new(),
            spotlight_tool: SpotlightTool::new(),
            organizer_tool: OrganizerTool::new(),
        }
    }

    pub fn with_plugin_store(mut self, ps: Arc<napp::plugin::PluginStore>) -> Self {
        self.shell_tool = self.shell_tool.with_plugin_store(ps);
        self
    }

    /// Infer resource from action name when resource field is omitted.
    fn infer_resource(action: &str) -> &str {
        match action {
            // File
            "read" | "write" | "edit" | "glob" | "grep" => "file",
            // Shell
            "exec" | "poll" | "log" => "shell",
            // Input
            "click" | "type" | "press" | "move" | "double_click" | "right_click"
            | "hotkey" | "scroll" | "drag" | "paste" => "input",
            // Capture
            "screenshot" | "see" => "capture",
            // Music
            "play" | "pause" | "next" | "previous" | "shuffle" | "playlists" => "music",
            // App
            "launch" | "quit" | "quit_all" | "activate" | "hide" | "frontmost" => "app",
            // TTS
            "speak" => "tts",
            // Organizer inferences
            "accounts" | "unread" | "send" => "mail",
            "today" | "upcoming" | "calendars" => "calendar",
            "groups" => "contacts",
            "lists" | "complete" => "reminders",
            _ => "",
        }
    }

    pub fn file_tool(&self) -> &FileTool {
        &self.file_tool
    }

    pub fn shell_tool(&self) -> &ShellTool {
        &self.shell_tool
    }
}

impl DynTool for OsTool {
    fn name(&self) -> &str {
        "os"
    }

    fn description(&self) -> String {
        "Local machine operations — files, shell, apps, desktop automation, settings, media, credentials, search, PIM.\n\n\
         Resources:\n\
         - file: read, write, edit, glob, grep\n\
         - shell: exec, list, poll, log, write, kill, info\n\
         - window: list, focus, minimize, maximize, resize, close, move\n\
         - input: click, double_click, right_click, type, press, hotkey, move, scroll, drag, paste\n\
         - clipboard: read, write, clear\n\
         - capture: screenshot, see\n\
         - notification: send, alert\n\
         - ui: tree, find, click, get_value, set_value, list_apps\n\
         - menu: list, menus, click, status, click_status\n\
         - dialog: detect, list, click, fill, dismiss\n\
         - space: list, switch, move_window\n\
         - shortcut: list, run\n\
         - tts: speak\n\
         - dock: badges, recent, is_running (macOS)\n\
         - app: list, launch, quit, quit_all, activate, hide, info, frontmost\n\
         - settings: volume, brightness, wifi, bluetooth, battery, darkmode, sleep, lock, info, mute, unmute\n\
         - music: play, pause, next, previous, status, search, volume, playlists, shuffle\n\
         - keychain: get, find, add, delete\n\
         - search: search (file search via OS index)\n\
         - mail: accounts, unread, read, send, search\n\
         - contacts: search, get, create, groups\n\
         - calendar: calendars, today, upcoming, create, list\n\
         - reminders: lists, list, create, complete, delete\n\n\
         Examples:\n  \
         os(resource: \"file\", action: \"read\", path: \"/path/to/file.txt\")\n  \
         os(resource: \"shell\", action: \"exec\", command: \"ls -la\")\n  \
         os(resource: \"app\", action: \"launch\", app: \"Safari\")\n  \
         os(resource: \"capture\", action: \"screenshot\")\n  \
         os(resource: \"music\", action: \"play\")\n  \
         os(resource: \"keychain\", action: \"get\", service: \"myapp\", account: \"user@example.com\")\n  \
         os(resource: \"mail\", action: \"unread\")"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        // Built programmatically to avoid serde_json::json! recursion limit
        let mut props = serde_json::Map::new();

        let prop = |t: &str, d: &str| -> serde_json::Value {
            serde_json::json!({"type": t, "description": d})
        };

        props.insert("resource".into(), serde_json::json!({
            "type": "string",
            "description": "OS resource",
            "enum": [
                "file", "shell",
                "window", "input", "clipboard", "capture", "notification",
                "ui", "menu", "dialog", "space", "shortcut", "tts", "dock",
                "app", "settings", "music", "keychain", "search",
                "mail", "contacts", "calendar", "reminders"
            ]
        }));
        props.insert("action".into(), prop("string", "Action to perform on the resource"));
        // File
        props.insert("path".into(), prop("string", "File or directory path"));
        props.insert("content".into(), prop("string", "File content to write"));
        props.insert("pattern".into(), prop("string", "Glob or grep pattern"));
        props.insert("old_string".into(), prop("string", "String to find (for edit)"));
        props.insert("new_string".into(), prop("string", "Replacement string (for edit)"));
        props.insert("replace_all".into(), prop("boolean", "Replace all occurrences"));
        props.insert("offset".into(), prop("integer", "Line offset for reading"));
        props.insert("limit".into(), prop("integer", "Max lines/results to return"));
        props.insert("append".into(), prop("boolean", "Append to file"));
        props.insert("regex".into(), prop("string", "Regular expression (for grep)"));
        props.insert("case_insensitive".into(), prop("boolean", "Case-insensitive search"));
        props.insert("glob".into(), prop("string", "File filter pattern for grep"));
        // Shell
        props.insert("command".into(), prop("string", "Shell command to execute"));
        props.insert("timeout".into(), prop("integer", "Command timeout in seconds"));
        props.insert("session_id".into(), prop("string", "Background session ID"));
        props.insert("pid".into(), prop("integer", "Process ID"));
        props.insert("signal".into(), prop("string", "Signal: SIGTERM, SIGKILL, SIGINT"));
        props.insert("background".into(), prop("boolean", "Run command in background"));
        // Desktop
        props.insert("app".into(), prop("string", "Application name"));
        props.insert("title".into(), prop("string", "Window or notification title"));
        props.insert("message".into(), prop("string", "Notification message"));
        props.insert("text".into(), prop("string", "Text to type/write/speak"));
        props.insert("key".into(), prop("string", "Key to press"));
        props.insert("keys".into(), prop("string", "Key combination for hotkey"));
        props.insert("x".into(), prop("integer", "X coordinate"));
        props.insert("y".into(), prop("integer", "Y coordinate"));
        props.insert("x2".into(), prop("integer", "End X coordinate (drag)"));
        props.insert("y2".into(), prop("integer", "End Y coordinate (drag)"));
        props.insert("dx".into(), prop("integer", "Scroll delta X"));
        props.insert("dy".into(), prop("integer", "Scroll delta Y"));
        props.insert("width".into(), prop("integer", "Width for resize/move"));
        props.insert("height".into(), prop("integer", "Height for resize/move"));
        props.insert("region".into(), prop("string", "Screenshot region: 'x,y,w,h'"));
        props.insert("name".into(), prop("string", "Name for shortcut/menu/contact/reminder"));
        props.insert("value".into(), prop("string", "Value to set"));
        props.insert("role".into(), prop("string", "UI element role filter"));
        props.insert("label".into(), prop("string", "UI element label"));
        props.insert("index".into(), prop("integer", "Index for space/menu"));
        props.insert("voice".into(), prop("string", "TTS voice name"));
        props.insert("rate".into(), prop("integer", "TTS speaking rate"));
        // Keychain
        props.insert("service".into(), prop("string", "Keychain service name"));
        props.insert("account".into(), prop("string", "Keychain account"));
        props.insert("password".into(), prop("string", "Password to store"));
        // Search
        props.insert("query".into(), prop("string", "Search query"));
        props.insert("dir".into(), prop("string", "Directory to search within"));
        // Organizer
        props.insert("email".into(), prop("string", "Email address"));
        props.insert("subject".into(), prop("string", "Email subject"));
        props.insert("body".into(), prop("string", "Email/event body"));
        props.insert("to".into(), serde_json::json!({
            "oneOf": [
                { "type": "string" },
                { "type": "array", "items": { "type": "string" } }
            ],
            "description": "Email recipient(s)"
        }));
        props.insert("cc".into(), serde_json::json!({
            "type": "array",
            "items": { "type": "string" },
            "description": "CC recipient(s)"
        }));
        props.insert("mailbox".into(), prop("string", "Mailbox name (e.g. 'INBOX', 'Sent')"));
        props.insert("calendar".into(), prop("string", "Calendar name"));
        props.insert("date".into(), prop("string", "Start date (e.g. '2025-03-15 10:00', 'tomorrow')"));
        props.insert("end_date".into(), prop("string", "End date (defaults to start + 1 hour)"));
        props.insert("location".into(), prop("string", "Event location"));
        props.insert("days".into(), prop("integer", "Number of days to look ahead (default: 7)"));
        props.insert("list".into(), prop("string", "Reminder list name"));
        props.insert("due_date".into(), prop("string", "Due date (e.g. '2025-03-15', 'tomorrow', 'in 3 days')"));
        props.insert("priority".into(), prop("integer", "Priority: 1-3=high, 4-6=medium, 7-9=low"));
        props.insert("phone".into(), prop("string", "Contact phone number"));
        props.insert("company".into(), prop("string", "Contact company/organization"));
        props.insert("notes".into(), prop("string", "Notes or description"));

        serde_json::json!({
            "type": "object",
            "properties": serde_json::Value::Object(props),
            "required": ["action"]
        })
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn requires_approval_for(&self, input: &serde_json::Value) -> bool {
        let resource = input.get("resource")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // If no resource, try to infer it
        let resource = if resource.is_empty() {
            let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
            Self::infer_resource(action)
        } else {
            resource
        };
        !AUTO_APPROVE_RESOURCES.contains(&resource)
    }

    fn resource_permit(&self, input: &serde_json::Value) -> Option<ResourceKind> {
        let resource = input.get("resource")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let resource = if resource.is_empty() {
            let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
            OsTool::infer_resource(action)
        } else {
            resource
        };
        match resource {
            // Physical screen resources — one mouse, one keyboard, one display
            "window" | "input" | "ui" | "menu" | "dialog"
            | "space" | "shortcut" | "capture" | "app" => Some(ResourceKind::Screen),
            // Parallelizable: clipboard, notification, tts, dock, file, shell,
            // settings, music, keychain, search, mail, contacts, calendar, reminders
            _ => None,
        }
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let domain_input: DomainInput = match serde_json::from_value(input.clone()) {
                Ok(v) => v,
                Err(e) => {
                    return ToolResult::error(format!("Failed to parse input: {}", e));
                }
            };

            let resource = if domain_input.resource.is_empty() {
                Self::infer_resource(&domain_input.action).to_string()
            } else {
                domain_input.resource
            };

            if resource.is_empty() {
                return ToolResult::error(
                    "Resource is required. Available: file, shell, window, input, clipboard, capture, \
                     notification, ui, menu, dialog, space, shortcut, tts, dock, app, settings, music, \
                     keychain, search, mail, contacts, calendar, reminders"
                );
            }

            // Ensure resource is present in input for downstream tools
            let mut input = input;
            if !input.get("resource").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty()) {
                input["resource"] = serde_json::Value::String(resource.clone());
            }

            match resource.as_str() {
                // File + Shell — delegate to inner tools
                "file" => self.file_tool.execute(ctx, input),
                "shell" => self.shell_tool.execute(ctx, input).await,

                // Desktop resources — delegate to DesktopTool
                "window" | "input" | "clipboard" | "capture" | "notification"
                | "ui" | "menu" | "dialog" | "space" | "shortcut" | "tts" | "dock" => {
                    self.desktop_tool.execute_dyn(ctx, input).await
                }

                // App lifecycle
                "app" => self.app_tool.execute_dyn(ctx, input).await,

                // Settings — already uses resource/action internally
                "settings" => {
                    // SettingsTool expects resource=volume/brightness/etc, action=get/set/etc.
                    // OsTool receives resource="settings", action="volume", value=50
                    // But SettingsTool's own schema uses resource for the setting type.
                    // We need to remap: os(resource: "settings", action: "volume", value: 50)
                    // → settings(resource: "volume", action: "get|set", value: 50)
                    //
                    // However the user's action IS the SettingsTool resource, and the value
                    // determines if it's get or set. Let SettingsTool handle this — just swap
                    // the resource field to the action, and set action to the appropriate op.
                    let action = input["action"].as_str().unwrap_or("");
                    let has_value = input.get("value").is_some();
                    let mut settings_input = input.clone();
                    // Map: os action → settings resource, infer settings action
                    let settings_action = match action {
                        "sleep" | "lock" | "mute" | "unmute" => "trigger",
                        _ if has_value => "set",
                        "status" => "status",
                        "toggle" => "toggle",
                        _ => "get",
                    };
                    settings_input["resource"] = serde_json::Value::String(action.to_string());
                    settings_input["action"] = serde_json::Value::String(settings_action.to_string());
                    self.settings_tool.execute_dyn(ctx, settings_input).await
                }

                // Music
                "music" => self.music_tool.execute_dyn(ctx, input).await,

                // Keychain
                "keychain" => self.keychain_tool.execute_dyn(ctx, input).await,

                // File search
                "search" => self.spotlight_tool.execute_dyn(ctx, input).await,

                // PIM — delegate to OrganizerTool (already routes by resource)
                "mail" | "contacts" | "calendar" | "reminders" => {
                    self.organizer_tool.execute_dyn(ctx, input).await
                }

                other => ToolResult::error(format!(
                    "Unknown resource '{}'. Available: file, shell, window, input, clipboard, capture, \
                     notification, ui, menu, dialog, space, shortcut, tts, dock, app, settings, music, \
                     keychain, search, mail, contacts, calendar, reminders",
                    other
                )),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_resource() {
        assert_eq!(OsTool::infer_resource("read"), "file");
        assert_eq!(OsTool::infer_resource("exec"), "shell");
        assert_eq!(OsTool::infer_resource("click"), "input");
        assert_eq!(OsTool::infer_resource("screenshot"), "capture");
        assert_eq!(OsTool::infer_resource("play"), "music");
        assert_eq!(OsTool::infer_resource("launch"), "app");
        assert_eq!(OsTool::infer_resource("speak"), "tts");
        assert_eq!(OsTool::infer_resource("unread"), "mail");
        assert_eq!(OsTool::infer_resource("today"), "calendar");
        assert_eq!(OsTool::infer_resource("unknown_action"), "");
    }

    #[test]
    fn test_approval_map() {
        // Auto-approve resources
        for resource in AUTO_APPROVE_RESOURCES {
            let input = serde_json::json!({"resource": resource, "action": "test"});
            let tool = OsTool::new(
                crate::policy::Policy::default(),
                Arc::new(crate::process::ProcessRegistry::new()),
            );
            assert!(
                !tool.requires_approval_for(&input),
                "{} should auto-approve",
                resource
            );
        }

        // Requires-approval resources
        let sensitive = ["input", "window", "ui", "menu", "dialog", "app",
                         "settings", "music", "keychain", "mail", "contacts",
                         "calendar", "reminders", "space", "shortcut"];
        for resource in &sensitive {
            let input = serde_json::json!({"resource": resource, "action": "test"});
            let tool = OsTool::new(
                crate::policy::Policy::default(),
                Arc::new(crate::process::ProcessRegistry::new()),
            );
            assert!(
                tool.requires_approval_for(&input),
                "{} should require approval",
                resource
            );
        }
    }

    #[test]
    fn test_infer_resource_approval() {
        let tool = OsTool::new(
            crate::policy::Policy::default(),
            Arc::new(crate::process::ProcessRegistry::new()),
        );
        // read → file → auto-approve
        let input = serde_json::json!({"action": "read", "path": "/tmp/test"});
        assert!(!tool.requires_approval_for(&input));

        // click → input → requires approval
        let input = serde_json::json!({"action": "click", "x": 100, "y": 200});
        assert!(tool.requires_approval_for(&input));
    }
}
