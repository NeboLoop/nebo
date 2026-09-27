//! THE canonical capability list — single source of truth for permissions.
//!
//! Before this module there were three drifted vocabularies for "what the agent
//! can do": the Settings → Permissions toggles (frontend), the persisted keys
//! (`user_profiles.tool_permissions` / `entity_config.permissions`), and the
//! backend gate's `tool_category()`. They only overlapped on `web` and
//! `desktop`, so `file`/`shell`/`system`/`media`/`contacts` toggles gated
//! nothing and the whole `os` tool was wrongly blocked behind `desktop`. This
//! module is the one place that defines the capability set; each tool's spec
//! (`DynTool::capability`) names the capability a call belongs to. The frontend renders
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

/// What every refusal caused by web access being off says, to the employee
/// and to a helper working for it alike, whichever way the call reached for
/// the web (the browser, `fetch_url`, a search, a command's network): `what`
/// is what happened to this call, `whose` whose setting it is. It names the
/// setting and closes every other route: a refusal that said only "beyond
/// what the employee may do" sent a helper through the browser, `fetch_url`
/// and `curl` in turn, 9 attempts in 3 runs (2026-09-27 release-fix proof,
/// helper-cannot-exceed-parent). And it keeps the report to the owner
/// honest: in 2 of those runs the employee gave the page's title from
/// memory instead of saying the page couldn't be opened.
pub fn web_off(what: &str, whose: &str) -> String {
    format!(
        "{what} The \"{label}\" permission is off for {whose}, so nothing on the web can be reached from this work: \
         not the browser, fetch_url, a web search, or a command such as curl (commands here have no \
         network). No other tool, helper or coworker gets around it, so don't try another way. Tell the owner \
         plainly that web access is off and what you needed it for; it is turned on in the employee's settings. \
         Nothing from the web was seen, so don't give what it would have said from memory.",
        label = capability_label("web"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_keys_are_unique_and_labeled() {
        for c in CAPABILITIES {
            assert_eq!(capability_label(c.key), c.label);
        }
    }
}
