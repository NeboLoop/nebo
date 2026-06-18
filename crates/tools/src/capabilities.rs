//! THE canonical capability list — single source of truth for permissions.
//!
//! Before this module there were three drifted vocabularies for "what the agent
//! can do": the Settings → Permissions toggles (frontend), the persisted keys
//! (`user_profiles.tool_permissions` / `entity_config.permissions`), and the
//! backend gate's `tool_category()`. They only overlapped on `web` and
//! `desktop`, so `file`/`shell`/`system`/`media`/`contacts` toggles gated
//! nothing and the whole `os` tool was wrongly blocked behind `desktop`. This
//! module is the one place that defines the capability set and maps a tool call
//! to the capability that gates it (CODE_AUDITOR Rule 8). The frontend renders
//! its toggles from `CAPABILITIES` (served via the API) instead of hardcoding
//! them, so the lists cannot drift again.
//!
//! **What is gated vs. not.** The toggles gate the agent's *ambient built-in
//! powers* — the broad, always-present abilities a user should be able to switch
//! off (read/write files, run shell, browse, control the screen, read system
//! info, camera/mic, contacts). *Installed extensions* — plugins, MCP servers,
//! apps, skills, and sub-agents — are **not** gated here: installing one is
//! itself an explicit, HIL-approved grant of its functionality (plugins also
//! gate per-account at connect time). Re-gating them behind a coarse toggle
//! would second-guess a decision the user already made at install.

use serde_json::Value;

use crate::os_tool::OsTool;

/// One user-facing capability toggle.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct Capability {
    /// Stable key — matches the persisted permission key and the gate.
    pub key: &'static str,
    /// Human label shown in Settings → Permissions.
    pub label: &'static str,
    /// One-line description shown under the label.
    pub desc: &'static str,
}

/// The canonical capability set. Order is the display order in Settings.
pub const CAPABILITIES: &[Capability] = &[
    Capability {
        key: "chat",
        label: "Chat",
        desc: "Respond to messages and conversations",
    },
    Capability {
        key: "file",
        label: "File Access",
        desc: "Read and write files on your system",
    },
    Capability {
        key: "shell",
        label: "Shell Commands",
        desc: "Execute terminal commands",
    },
    Capability {
        key: "web",
        label: "Web Access",
        desc: "Make HTTP requests and browse the web",
    },
    Capability {
        key: "contacts",
        label: "Contacts",
        desc: "Access your contacts and address book",
    },
    Capability {
        key: "desktop",
        label: "Desktop",
        desc: "Control mouse, keyboard, and windows",
    },
    Capability {
        key: "media",
        label: "Media",
        desc: "Access camera, microphone, and screen",
    },
    Capability {
        key: "system",
        label: "System",
        desc: "Access system information and settings",
    },
];

/// User-facing label for a capability key (for denial messages that tell the
/// user exactly which switch to flip). Falls back to the key itself.
pub fn capability_label(key: &str) -> &str {
    CAPABILITIES
        .iter()
        .find(|c| c.key == key)
        .map(|c| c.label)
        .unwrap_or(key)
}

/// The capability that gates a specific tool call, or `None` if the call is
/// ungated. `input` is the tool's arguments (used to resolve the `os` resource).
///
/// `None` is a deliberate "not behind a coarse toggle" — see the module doc:
/// installed extensions (plugin/mcp/app/skill/agent) are granted at install
/// time, and a few built-in tools (message/event) aren't ambient powers.
pub fn gating_capability(tool: &str, input: &Value) -> Option<&'static str> {
    match tool {
        "web" | "loop" => Some("web"),
        // A file-management verb (move/copy/delete with file args) is redirected
        // to a shell correction by the os tool — it never reaches a desktop
        // resource, so don't gate it on Desktop here (asking/denying the wrong
        // capability would block a file move). Ungated: the agent's shell retry
        // gets the correct (Shell) ask.
        "os" if OsTool::is_file_mgmt_redirect(input) => None,
        "os" => Some(os_capability(input)),
        "organizer" => match input.get("resource").and_then(|v| v.as_str()) {
            Some("contacts") => Some("contacts"),
            // mail / calendar / reminders: not behind a coarse toggle
            _ => None,
        },
        // Installed extensions — installation is the grant. Built-in
        // message/event are not ambient powers. All ungated.
        _ => None,
    }
}

/// The single capability gating a *whole* tool, for toolset list filtering
/// (deciding whether a tool appears in the model's toolset at all).
///
/// Only pure single-capability tools are hidden when their capability is off.
/// Meta-tools (`os`, which spans file/shell/system/desktop/media) and mixed
/// tools (`organizer`, which has ungated mail/calendar paths) are always listed
/// — their individual calls are gated per-resource at execution time by
/// [`gating_capability`]. Returning `None` keeps a tool always-visible.
pub fn whole_tool_capability(tool: &str) -> Option<&'static str> {
    match tool {
        "web" | "loop" => Some("web"),
        _ => None,
    }
}

/// Map an `os` call to its capability. `os` is a meta-tool spanning file, shell,
/// system, desktop control and screen/media — gating it all behind one
/// capability (the original `desktop` bug) blocks file/shell when only Desktop
/// is off. Resolve the specific capability from the resource, inferring it from
/// the action when the model omits `resource` (reusing the tool's own
/// inference so the gate and the dispatch agree).
fn os_capability(input: &Value) -> &'static str {
    let resource = input
        .get("resource")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
            let inferred = OsTool::infer_resource(action);
            if inferred.is_empty() {
                OsTool::infer_resource_from_context(input).to_string()
            } else {
                inferred.to_string()
            }
        });

    match resource.as_str() {
        "file" => "file",
        "shell" => "shell",
        // System information & settings.
        "settings" | "keychain" | "platform" | "system" => "system",
        // Screen / camera / microphone capture.
        "capture" | "screenshot" | "see" => "media",
        // Everything else is desktop control: input, window, ui, menu, dialog,
        // clipboard, space, shortcut, dock, tts, music, app, notification.
        _ => "desktop",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn os_file_ops_gate_on_file_not_desktop() {
        // The bug this module fixes: file ops were gated behind `desktop`.
        assert_eq!(
            gating_capability("os", &json!({"resource": "file", "action": "read"})),
            Some("file")
        );
        // resource omitted → inferred from action (write → file).
        assert_eq!(
            gating_capability("os", &json!({"action": "write", "path": "/tmp/x"})),
            Some("file")
        );
    }

    #[test]
    fn os_shell_gates_on_shell() {
        assert_eq!(
            gating_capability("os", &json!({"resource": "shell", "action": "exec"})),
            Some("shell")
        );
        // inferred from action.
        assert_eq!(
            gating_capability("os", &json!({"action": "exec", "command": "ls"})),
            Some("shell")
        );
    }

    #[test]
    fn os_desktop_and_media_and_system() {
        assert_eq!(
            gating_capability("os", &json!({"resource": "input", "action": "click"})),
            Some("desktop")
        );
        assert_eq!(
            gating_capability("os", &json!({"action": "screenshot"})),
            Some("media")
        );
        assert_eq!(
            gating_capability("os", &json!({"resource": "settings"})),
            Some("system")
        );
        assert_eq!(
            gating_capability("os", &json!({"resource": "system", "action": "info"})),
            Some("system")
        );
    }

    #[test]
    fn installed_extensions_are_ungated() {
        // Installation is the grant — these are never blocked by a toggle.
        for tool in ["plugin", "mcp", "app", "skill", "agent"] {
            assert_eq!(gating_capability(tool, &json!({})), None, "{tool} should be ungated");
        }
    }

    #[test]
    fn organizer_contacts_gated_mail_ungated() {
        assert_eq!(
            gating_capability("organizer", &json!({"resource": "contacts"})),
            Some("contacts")
        );
        assert_eq!(
            gating_capability("organizer", &json!({"resource": "mail"})),
            None
        );
    }

    #[test]
    fn web_gates_on_web() {
        assert_eq!(gating_capability("web", &json!({})), Some("web"));
    }

    #[test]
    fn capability_keys_are_unique_and_labeled() {
        for c in CAPABILITIES {
            assert_eq!(capability_label(c.key), c.label);
        }
    }
}
