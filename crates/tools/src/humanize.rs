//! Owner-facing names for tool calls. ONE source for the live activity chip,
//! the collapsed work line, the persisted tool result (so a reloaded thread
//! reads exactly like the live one), and every client that renders them.

/// Honest fallback for a tool we don't have nice copy for: name it as-is
/// ("using send invoice" / "Used send invoice") rather than vague filler.
pub fn raw_name(tool_name: &str) -> (String, String) {
    let n = tool_name.replace('_', " ");
    (format!("using {n}"), format!("Used {n}"))
}

/// "google-search-console" → "Google Search Console" — service slugs render
/// as the service's own name (the only vocabulary the owner should see).
pub fn service_name(slug: &str) -> String {
    slug.split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Verb forms for STRAP actions: (gerund for the live activity label,
/// past tense for the outcome label).
pub fn strap_verb(action: &str) -> Option<(&'static str, &'static str)> {
    Some(match action {
        "create" | "add" | "insert" => ("creating", "Created"),
        "read" | "get" | "view" | "fetch" => ("reading", "Read"),
        "list" | "ls" => ("listing", "Listed"),
        "search" | "find" | "query" | "glob" | "grep" => ("searching", "Searched"),
        "update" | "edit" | "set" | "patch" | "rename" | "move" => ("updating", "Updated"),
        "delete" | "remove" | "clear" => ("deleting", "Deleted"),
        "send" | "post" | "reply" | "dm" => ("sending", "Sent"),
        "run" | "exec" | "execute" | "shell" => ("running", "Ran"),
        "write" | "save" => ("writing", "Wrote"),
        "download" => ("downloading", "Downloaded"),
        "upload" => ("uploading", "Uploaded"),
        "open" | "launch" | "start" => ("opening", "Opened"),
        "stop" | "close" | "kill" => ("stopping", "Stopped"),
        "check" | "status" | "verify" => ("checking", "Checked"),
        "notify" | "alert" => ("notifying", "Notified"),
        _ => return None,
    })
}

/// The generic owner-facing lines for a call, the default of every tool's
/// `DynTool::activity`/`outcome` (a tool with better words says them
/// itself): MCP tools (`mcp__slug__tool`) read from their slug and tool
/// name, a `resource`/`action` signature reads as verb + noun, anything
/// else names the tool. Returns (activity gerund phrase, past-tense
/// outcome).
pub fn call_labels(tool_name: &str, input: &serde_json::Value) -> (String, String) {
    // MCP: mcp__github__create_issue → "using GitHub (create issue)".
    if let Some(rest) = tool_name.strip_prefix("mcp__") {
        if let Some((slug, tool)) = rest.split_once("__") {
            let tool_h = tool.replace('_', " ");
            return (
                format!("using {slug} ({tool_h})"),
                format!("Used {slug}: {tool_h}"),
            );
        }
    }
    // STRAP: toolName(resource, action, …).
    let resource = input.get("resource").and_then(|v| v.as_str());
    let action = input.get("action").and_then(|v| v.as_str());
    if let (Some(resource), Some(action)) = (resource, action) {
        let noun = resource.replace('_', " ");
        if let Some((gerund, past)) = strap_verb(action) {
            return (format!("{gerund} {noun}"), format!("{past} {noun}"));
        }
        // Unknown verb: show the signature honestly rather than guessing.
        return (
            format!("running {action} on {noun}"),
            format!("Ran {action} on {noun}"),
        );
    }
    // Otherwise name the tool honestly instead of "working" / "Did a step".
    raw_name(tool_name)
}


#[cfg(test)]
mod tests {
    use super::{call_labels, service_name as humanize_slug, strap_verb};
    use crate::os_tool::OsTool;
    use crate::plugin_tool::plugin_labels;
    use serde_json::json;

    #[test]
    fn os_calls_say_what_they_did() {
        let (_, p) = OsTool::labels(&json!({"action":"click","app":"Simulator","coordinate":[223,900]}));
        assert_eq!(p, "Clicked (223,900) in Simulator");
        let (_, p) = OsTool::labels(&json!({"action":"click","ref":"B2"}));
        assert_eq!(p, "Clicked B2");
        let (_, p) = OsTool::labels(&json!({"resource":"capture","action":"screenshot","app":"Simulator"}));
        assert_eq!(p, "Captured Simulator");
        let (_, p) = OsTool::labels(&json!({"action":"screenshot"}));
        assert_eq!(p, "Captured the screen");
        // Anything else keeps the STRAP signature wording.
        let (_, p) = OsTool::labels(&json!({"resource":"app","action":"list"}));
        assert!(p.contains("app"), "{p}");
    }

    /// Service slugs render as the service's own name — dashes/underscores
    /// become spaces, each word capitalized, empty segments dropped.
    #[test]
    fn slugs_render_as_service_names() {
        assert_eq!(humanize_slug("google-search-console"), "Google Search Console");
        assert_eq!(humanize_slug("gws_calendar"), "Gws Calendar");
        assert_eq!(humanize_slug("a--b"), "A B");
    }

    /// STRAP verbs map to (gerund, past); an unknown verb yields None so the
    /// caller can show the raw signature honestly instead of guessing.
    #[test]
    fn strap_verbs_cover_known_and_refuse_unknown() {
        assert_eq!(strap_verb("read"), Some(("reading", "Read")));
        assert_eq!(strap_verb("delete"), Some(("deleting", "Deleted")));
        assert_eq!(strap_verb("frobnicate"), None);
    }

    /// MCP tool names (`mcp__slug__tool`) humanize from slug + tool name —
    /// never leak the raw `mcp__` machinery into the owner's transcript.
    #[test]
    fn mcp_tools_humanize_from_slug_and_tool() {
        let (act, out) = call_labels("mcp__github__create_issue", &json!({}));
        assert_eq!(act, "using github (create issue)");
        assert_eq!(out, "Used github: create issue");
    }

    /// STRAP signatures (resource + action) read as verb+noun; an unknown
    /// verb shows the signature honestly rather than a guessed label.
    #[test]
    fn strap_signature_reads_as_verb_noun() {
        let (act, out) =
            OsTool::labels(&json!({"resource": "reminders", "action": "read"}));
        assert_eq!(act, "reading reminders");
        assert_eq!(out, "Read reminders");
        let (act, out) =
            OsTool::labels(&json!({"resource": "reminders", "action": "frobnicate"}));
        assert_eq!(act, "running frobnicate on reminders");
        assert_eq!(out, "Ran frobnicate on reminders");
    }

    /// Plugin calls must say the SERVICE ("using Gmail"), never the word
    /// "plugin" — the register the whole install flow protects.
    #[test]
    fn plugin_calls_name_the_service_never_the_word_plugin() {
        let (act, out) =
            plugin_labels(&json!({"resource": "google-search-console"}));
        assert_eq!(act, "using Google Search Console");
        assert_eq!(out, "Used Google Search Console");
        let (act, _) = plugin_labels(&json!({"action": "discover"}));
        assert_eq!(act, "browsing the marketplace");
    }

    /// A tool without words of its own is named as-is ("using send invoice")
    /// — honest, never vague filler.
    #[test]
    fn a_tool_without_better_words_is_named_as_is() {
        let (act, out) = call_labels("send_invoice", &json!({}));
        assert_eq!(act, "using send invoice");
        assert_eq!(out, "Used send invoice");
    }
}
