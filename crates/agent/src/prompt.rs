//! The system prompt's cache boundary, and the STRAP tool docs: the tools
//! doc owns those and retires them with the STRAP tools. The system prompt
//! itself is built by `harness::prompt`.

/// The shared, fixed part of the system prompt sits above this marker and
/// the employee's own section below it: the provider caches up to it for
/// every employee.
pub const CACHE_BOUNDARY: &str =
    "\n<!-- CACHE_BOUNDARY -->\n[--- cache boundary: content below changes per-turn ---]\n";

const STRAP_AGENT: &str = include_str!("strap/agent.txt");
const STRAP_CODE: &str = include_str!("strap/code.txt");
const STRAP_MESSAGE: &str = include_str!("strap/message.txt");
const STRAP_SKILL: &str = include_str!("strap/skill.txt");
const STRAP_EXECUTE: &str = include_str!("strap/execute.txt");
const STRAP_MCP: &str = include_str!("strap/mcp.txt");
const STRAP_PLUGIN: &str = include_str!("strap/plugin.txt");
const STRAP_VM: &str = include_str!("strap/vm.txt");
const STRAP_PUBLISHER: &str = include_str!("strap/publisher.txt");

// OS sub-context docs (keyword-activated, extend the OS tool)
#[cfg(target_os = "windows")]
const STRAP_DESKTOP: &str = concat!(include_str!("strap/desktop_shared.txt"), include_str!("strap/desktop_windows.txt"));
#[cfg(target_os = "linux")]
const STRAP_DESKTOP: &str = concat!(include_str!("strap/desktop_shared.txt"), include_str!("strap/desktop_linux.txt"));
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
const STRAP_DESKTOP: &str = concat!(include_str!("strap/desktop_shared.txt"), include_str!("strap/desktop_macos.txt"));

const STRAP_APP: &str = include_str!("strap/app.txt");
const STRAP_MUSIC: &str = include_str!("strap/music.txt");
const STRAP_KEYCHAIN: &str = include_str!("strap/keychain.txt");
const STRAP_SETTINGS: &str = include_str!("strap/settings.txt");
const STRAP_SPOTLIGHT: &str = include_str!("strap/spotlight.txt");
const STRAP_ORGANIZER: &str = include_str!("strap/organizer.txt");

/// Get STRAP doc for a core tool (injected when the tool is active).
pub fn strap_tool_doc(tool_name: &str) -> Option<&'static str> {
    match tool_name {
        "agent" => Some(STRAP_AGENT),
        "code" => Some(STRAP_CODE),
        "message" => Some(STRAP_MESSAGE),
        "skill" => Some(STRAP_SKILL),
        "execute" => Some(STRAP_EXECUTE),
        "mcp" => Some(STRAP_MCP),
        "plugin" => Some(STRAP_PLUGIN),
        "vm" => Some(STRAP_VM),
        "publisher" => Some(STRAP_PUBLISHER),
        _ => None,
    }
}

/// Get STRAP doc for an OS sub-context (activated by keyword matching).
pub fn strap_context_doc(context_name: &str) -> Option<&'static str> {
    match context_name {
        "desktop" => Some(STRAP_DESKTOP),
        "app" => Some(STRAP_APP),
        "music" => Some(STRAP_MUSIC),
        "keychain" => Some(STRAP_KEYCHAIN),
        "settings" => Some(STRAP_SETTINGS),
        "spotlight" => Some(STRAP_SPOTLIGHT),
        "organizer" => Some(STRAP_ORGANIZER),
        _ => None,
    }
}
