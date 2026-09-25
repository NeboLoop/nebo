//! Hard limits: checked before any rule or mode, never asked, never lifted
//! by Full Access. The safeguard (destructive commands, protected paths),
//! the origin limits (what a run reached from outside may do), the run's
//! allowlist, and credentials leaving in an outbound call.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use tools::Origin;
use types::permissions::{Decision, Door, Target, Why};

use super::CheckCx;

/// Running commands on this machine, and controlling the background shell
/// sessions they leave: reading their output, stopping them, writing to
/// them. Helper status and cancel (`read_output`, `stop_task`) are not shell
/// control and are not in this set.
const SHELL_KEYS: &[&str] = &[
    "run_command",
    "list_processes",
    "send_input",
    "read_command_output",
    "stop_command",
];
/// Reading and changing files on this machine.
const FILE_KEYS: &[&str] = &[
    "read_file",
    "write_file",
    "edit_file",
    "share_file",
    "convert_file",
    "checkpoint_files",
    "list_checkpoints",
    "restore_checkpoint",
    "write_plan",
    "check_plan",
    "exit_plan_mode",
    "edit_notebook",
];
/// Looking at the screen.
const CAPTURE_KEYS: &[&str] = &["desktop_screenshot", "desktop_see"];

/// Nothing that touches the machine, the mailbox, money, other people, or
/// the roster is reachable from outside, and no owner rule or Full Access
/// can put it back. Enablable per channel (deliberately absent): recall,
/// message_owner, scheduling, the organizer's calendar and reminders,
/// skills.
const OUTSIDE_KEYS: &[&str] = &[
    // Mail and contacts.
    "mail_*",
    "contacts_*",
    // The web and the browser.
    "search_web",
    "fetch_url",
    "http_request",
    "browser_*",
    // Code, scripts, machines, publishing.
    "run_skill_script",
    "vm_*",
    "publish_app",
    "list_publications",
    "publication_status",
    "code_intel",
    "search_computer",
    // Desktop control, apps, settings, secrets.
    "desktop_*",
    "window_*",
    "clipboard_*",
    "ui_*",
    "menu_*",
    "dialog_*",
    "space_*",
    "shortcut_*",
    "dock_*",
    "app_*",
    "speak",
    "system_settings",
    "music_control",
    "keychain_*",
    // Plugins, MCP, NeboAI loops. A catalog operation a connected account
    // performs is refused by `denied_operation_for_origin`.
    "plugin__*",
    "find_plugins",
    "read_plugin_events",
    "loop_*",
    "send_loop_message",
    "ensure_loop_channel",
    "list_loop_channels",
    "read_loop_channel",
    "list_loops",
    "get_loop",
    "subscribe_topic",
    "unsubscribe_topic",
    "topic_status",
    "share_to_loop",
    // The roster, helpers, tasks, sessions, the profile, the advisors, runs.
    "list_employees",
    "get_employee",
    "find_employees",
    "hire_employee",
    "create_employee",
    "update_employee",
    "delete_employee",
    "set_employee_active",
    "setup_employee",
    "repair_employee",
    "reload_employee",
    "employee_stats",
    "delegate",
    "send_message",
    "read_output",
    "stop_task",
    "create_task",
    "update_task",
    "get_task",
    "list_tasks",
    "assign_task",
    "list_assignments",
    "search_history",
    "read_session",
    "list_sessions",
    "get_profile",
    "update_profile",
    "open_billing",
    "consult_advisors",
    "list_advisors",
    "list_runs",
    // SMS and notifications.
    "sms_*",
    "push_notification",
    "check_dnd",
];

/// The rule keys each origin may never use. An entry ending in `*` denies
/// every key with that prefix.
fn origin_limits() -> &'static HashMap<Origin, HashSet<&'static str>> {
    static LIMITS: OnceLock<HashMap<Origin, HashSet<&'static str>>> = OnceLock::new();
    LIMITS.get_or_init(|| {
        let keys = |groups: &[&[&'static str]]| -> HashSet<&'static str> {
            groups.iter().flat_map(|g| g.iter().copied()).collect()
        };
        let mut limits = HashMap::new();
        // A peer Nebo, a loop, an agent space: another program's words. Shell
        // was always off the table; files join it (2026-09-05, the QR
        // file-share incident), the screen with them.
        limits.insert(Origin::Comm, keys(&[SHELL_KEYS, FILE_KEYS, CAPTURE_KEYS]));
        limits.insert(Origin::App, keys(&[SHELL_KEYS]));
        limits.insert(Origin::Skill, keys(&[SHELL_KEYS]));
        // External MCP clients: at most comm-level trust. An authenticated
        // client is still another program injecting prompts from outside.
        limits.insert(Origin::Mcp, keys(&[SHELL_KEYS]));
        // Outside origins, a phone caller or a visitor from a QR scan or an
        // embedded chat, are strangers. The allowlist on their run is the
        // real fence (deny-by-default: the seat's restrict_outside_origin);
        // this set is the backstop that holds even if an allowlist is ever
        // mis-built.
        let outside = keys(&[SHELL_KEYS, FILE_KEYS, CAPTURE_KEYS, OUTSIDE_KEYS]);
        limits.insert(Origin::Caller, outside.clone());
        limits.insert(Origin::Visitor, outside);
        limits
    })
}

/// The catalog capabilities whose operations the owner may enable on a
/// channel a stranger reaches (a caller booking a time). Every other
/// operation performed through a connected account stays out of reach.
const OUTSIDE_OPERATION_CAPABILITIES: &[&str] = &["calendar"];

/// Whether a catalog operation is refused for the origin: outside origins
/// never perform one, except the capabilities an owner may enable.
fn denied_operation_for_origin(origin: Origin, t: &Target) -> bool {
    matches!(origin, Origin::Caller | Origin::Visitor)
        && t.operation.as_deref().is_some_and(|op| {
            !op.split('.').next().is_some_and(|c| OUTSIDE_OPERATION_CAPABILITIES.contains(&c))
        })
}

/// Whether a call with this rule key is refused for the origin.
pub fn denied_for_origin(origin: Origin, rule_key: &str) -> bool {
    origin_limits().get(&origin).is_some_and(|denied| {
        denied.contains(rule_key)
            || denied.iter().any(|e| {
                e.strip_suffix('*')
                    .is_some_and(|prefix| !prefix.is_empty() && rule_key.starts_with(prefix))
            })
    })
}

/// Plain words for where a call came from. The model reads this, so it names
/// the caller the way the owner would.
pub fn origin_label(origin: Origin) -> &'static str {
    match origin {
        Origin::User => "the app",
        Origin::Comm => "a chat channel",
        Origin::App => "an external app",
        Origin::Skill => "a skill template",
        Origin::System => "a scheduled system task",
        Origin::Mcp => "an external MCP client",
        Origin::Workflow => "an unattended workflow run",
        Origin::Caller => "a phone caller",
        Origin::Visitor => "a visitor",
    }
}

/// A call the run's origin never makes, refused in the one wording for
/// what happened: another employee asked (limit `coworker`; a coworker's
/// request can only be answered), or the call came from where the origin
/// says (limit `origin`). The activity page words each limit for the owner.
fn origin_refusal(cx: &CheckCx<'_>, t: &Target) -> Decision {
    match cx.ctx.door {
        Door::Coworker { .. } => deny(
            "coworker",
            format!(
                "'{}' is not permitted: a coworker asked for this, and a coworker's request can only be \
                 answered with a reply. Say in your reply what you would need; do not retry.",
                t.key
            ),
        ),
        _ => deny(
            "origin",
            format!(
                "'{}' is not permitted when called from {}. Tell the user what you needed it for; do not retry.",
                t.key,
                origin_label(cx.ctx.origin)
            ),
        ),
    }
}

fn deny(limit: &str, reason: String) -> Decision {
    Decision::Deny { reason, why: Why::HardLimit { limit: limit.to_string() } }
}

/// The hard limits, in order: the safeguard, the origin limits, the run's
/// allowlist, the tool scope's narrowing, credentials in an outbound call.
/// `None`: none applies.
pub fn hard_limits(cx: &CheckCx<'_>, t: &Target) -> Option<Decision> {
    if let Some(err) = tools::safeguard::check_safeguard(&t.key, cx.input) {
        return Some(deny("safeguard", err));
    }
    if denied_for_origin(cx.ctx.origin, &t.key) || denied_operation_for_origin(cx.ctx.origin, t) {
        return Some(origin_refusal(cx, t));
    }
    // The restricted run's allowlist (the review fork, phone callers).
    if !cx.ctx.whitelist_allows(t) {
        return Some(deny(
            "allowlist",
            match &cx.ctx.whitelist_denial_hint {
                Some(hint) => format!("'{}' is not available in this run. {hint}", t.tool),
                None => format!(
                    "Tool '{}' is not available in this restricted run. Use one of the tools you \
                     were given, or say plainly that you can't do that.",
                    t.tool
                ),
            },
        ));
    }
    // What the run may not use: the tool scope's narrowing, and company
    // Memory for an isolated seat with no matter, however the call was
    // reached (a name from find_tools, a guess).
    if cx
        .ctx
        .withheld_tools
        .as_ref()
        .is_some_and(|w| w.contains(&t.tool))
    {
        return Some(deny(
            "scope",
            format!(
                "'{}' isn't one of the tools for this conversation. Use the tools you were given, or say \
                 plainly that you can't do that here.",
                t.tool
            ),
        ));
    }
    if let Some(d) = credentials(cx, t) {
        return Some(d);
    }
    None
}

/// Rule keys that carry content out of this machine to someone else's
/// server, whatever their recipients say.
const OUTBOUND_KEYS: &[&str] = &["http_request"];

/// A call that would send a secret somewhere: an outbound message, a
/// publish, a request carrying one. The owner's secrets never leave in a
/// tool call's input.
fn credentials(cx: &CheckCx<'_>, t: &Target) -> Option<Decision> {
    let outbound = !t.read_only
        && (!t.effects.recipients.is_empty()
            || t.effects.publishes == types::permissions::Knowable::Yes
            || OUTBOUND_KEYS.contains(&t.key.as_str()));
    if !outbound {
        return None;
    }
    let text = cx.input.to_string();
    let kind = tools::memory_guard::detect_secret(&text)?;
    Some(deny(
        "credentials",
        format!(
            "This would send a secret ({kind}) out of this machine. Secrets are never sent in a \
             message or a request. Leave it out, or ask the owner how they want it shared."
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_session_control_is_shell_and_helper_control_is_not() {
        for origin in [Origin::Comm, Origin::App, Origin::Skill, Origin::Mcp] {
            for key in ["read_command_output", "stop_command", "send_input", "list_processes"] {
                assert!(denied_for_origin(origin, key), "{origin:?} must not use {key}");
            }
            for key in ["read_output", "stop_task"] {
                assert!(!denied_for_origin(origin, key), "{origin:?} keeps {key}");
            }
        }
        for origin in [Origin::User, Origin::System, Origin::Workflow] {
            assert!(!denied_for_origin(origin, "stop_command"), "{origin:?}");
        }
    }

    #[test]
    fn origin_limits_match_rule_keys_and_prefixes() {
        for origin in [Origin::Comm, Origin::App, Origin::Skill, Origin::Mcp] {
            assert!(denied_for_origin(origin, "run_command"), "{origin:?}");
        }
        assert!(!denied_for_origin(Origin::User, "run_command"));
        assert!(!denied_for_origin(Origin::System, "run_command"));
        assert!(!denied_for_origin(Origin::App, "read_file"));
        assert!(denied_for_origin(Origin::Visitor, "desktop_click"), "prefix entry");
        assert!(!denied_for_origin(Origin::Visitor, "desktop"), "a prefix entry needs its prefix");
    }

    /// The outside hard-deny: what no allowlist, no owner rule and no Full
    /// Access can ever hand to a stranger. Visitors and callers share it.
    #[test]
    fn outside_origins_hard_deny_the_machine_the_mailbox_and_the_roster() {
        for origin in [Origin::Visitor, Origin::Caller] {
            for key in [
                "read_file", "write_file", "run_command", "desktop_screenshot", "mail_message_send",
                "contacts_search", "fetch_url", "search_web", "browser_open", "run_skill_script",
                "vm_run", "publish_app", "code_intel", "desktop_click", "keychain_get",
                "system_settings", "list_employees", "delegate", "search_history", "get_profile",
                "plugin__gws", "app_open", "send_loop_message", "sms_message_send",
                "edit_notebook", "search_computer", "find_plugins", "push_notification",
            ] {
                assert!(denied_for_origin(origin, key), "{origin:?} must deny {key}");
            }
            // Enablable by the owner per channel — never on the hard list.
            for key in ["recall", "message_owner", "create_schedule", "use_skill", "calendar_event_list"] {
                assert!(!denied_for_origin(origin, key), "{origin:?} must leave {key} to the allowlist");
            }
        }
        // Another program's words (a peer Nebo, a loop) keep shell, files and the screen off the table.
        for key in ["read_file", "write_file", "desktop_see", "run_command"] {
            assert!(denied_for_origin(Origin::Comm, key), "{key}");
        }
    }

    /// An operation tool reaches a connected account: outside origins never
    /// perform one, except the calendar an owner may open to callers; every
    /// other origin is left to the rules.
    #[test]
    fn outside_origins_never_perform_an_operation_through_a_connected_account() {
        let op = |operation: &str| {
            let tool = operation.replace('.', "_");
            Target {
                tool: tool.clone(),
                key: tool,
                operation: Some(operation.to_string()),
                capability: None,
                field: None,
                subject: None,
                read_only: false,
                effects: types::permissions::CallEffects::unknown(),
            }
        };
        for origin in [Origin::Visitor, Origin::Caller] {
            assert!(denied_operation_for_origin(origin, &op("ledger.bill.create")), "{origin:?}");
            assert!(denied_operation_for_origin(origin, &op("crm.contact.get")), "{origin:?}");
            assert!(!denied_operation_for_origin(origin, &op("calendar.event.list")), "{origin:?}");
        }
        for origin in [Origin::User, Origin::Comm, Origin::Workflow] {
            assert!(!denied_operation_for_origin(origin, &op("ledger.bill.create")), "{origin:?}");
        }
        let mut plain = op("ledger.bill.create");
        plain.operation = None;
        assert!(!denied_operation_for_origin(Origin::Caller, &plain), "no operation, nothing to refuse here");
    }

    /// The origin refusal names the caller in plain words, never the enum
    /// variant.
    #[test]
    fn origin_labels_are_plain_words() {
        assert_eq!(origin_label(Origin::Mcp), "an external MCP client");
        assert_eq!(origin_label(Origin::Comm), "a chat channel");
        assert_eq!(origin_label(Origin::User), "the app");
    }
}
