//! Owner-facing names for tool calls. ONE source for the live activity chip,
//! the collapsed work line, the persisted tool result (so a reloaded thread
//! reads exactly like the live one), and every client that renders them.

pub fn activity_label(tool_name: &str) -> Option<&'static str> {
    Some(match tool_name {
        "bash" => "running a command",
        "grep" => "searching files",
        "glob" => "finding files",
        "read" => "reading a file",
        "write" => "writing a file",
        "edit" => "editing a file",

        "web" => "searching the web",
        "browser" => "reading a page",
        "bot" => "thinking it through",
        "desktop" => "using the desktop",
        "event" => "checking the schedule",
        "loop" => "sending a message",

        "os" => "checking the workspace",
        _ => return None,
    })
}

/// Past-tense counterpart, sent on the result phase: collapsed work lines
/// report OUTCOMES ("Ran a command"), not in-progress activity. One source
/// for every client — web and mobile render these verbatim.
pub fn outcome_label(tool_name: &str) -> Option<&'static str> {
    Some(match tool_name {
        "bash" => "Ran a command",
        "grep" => "Searched files",
        "glob" => "Found files",
        "read" => "Read a file",
        "write" => "Wrote a file",
        "edit" => "Edited a file",

        "web" => "Searched the web",
        "browser" => "Read a page",
        "bot" => "Thought it through",
        "desktop" => "Used the desktop",
        "event" => "Checked the schedule",
        "loop" => "Sent a message",

        "os" => "Checked the workspace",
        _ => return None,
    })
}

/// Honest fallback for a tool we don't have nice copy for: name it as-is
/// ("using tool_search" / "Used tool_search") rather than vague filler.
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

/// Humanize a tool call from its STRAP signature — `os(resource: file,
/// action: read)` reads as "reading a file" / "Read a file", which says far
/// more than the bare domain-tool name ("os"). MCP tools (`mcp__slug__tool`)
/// humanize from their slug + tool name. Falls back to the name-only maps.
/// Returns (activity gerund phrase, past-tense outcome).
pub fn tool_call(tool_name: &str, input: &serde_json::Value) -> (String, String) {
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
    // The web tool takes an action without a resource, so it used to fall
    // through to the static "Searched the web" for EVERYTHING — a run that
    // fetched four pages read as search-only. Label by action (+ host when
    // there's a URL) so fetches and navigations are visible as page visits.
    if tool_name == "web" {
        let host = input
            .get("url")
            .and_then(|v| v.as_str())
            .and_then(|u| url::Url::parse(u).ok())
            .and_then(|u| {
                u.host_str()
                    .map(|h| h.trim_start_matches("www.").to_string())
            });
        let site = host.as_deref().unwrap_or("a page");
        let (gerund, past) = match action {
            Some("search") | None => ("searching the web".to_string(), "Searched the web".to_string()),
            Some("fetch") => (format!("reading {site}"), format!("Read {site}")),
            Some("navigate") => (format!("opening {site}"), format!("Opened {site}")),
            Some("read_page") => ("reading the page".to_string(), "Read the page".to_string()),
            Some(a) => {
                let a = a.replace('_', " ");
                (format!("{a} (web)"), format!("Web: {a}"))
            }
        };
        return (gerund, past);
    }
    // Plugin calls: the chip must say the SERVICE ("using Gmail"), never the
    // word "plugin" — the register the whole install flow protects. Exec
    // calls carry the service slug as `resource` with CLI args (no action),
    // so the generic STRAP branch never fired and these fell to the raw
    // tool-name fallback ("using plugin").
    if tool_name == "plugin" {
        if matches!(action, Some("discover")) {
            return (
                "browsing the marketplace".to_string(),
                "Browsed the marketplace".to_string(),
            );
        }
        if matches!(action, Some("list")) {
            return (
                "checking available tools".to_string(),
                "Checked available tools".to_string(),
            );
        }
        if let Some(slug) = resource {
            let svc = service_name(slug);
            return (format!("using {svc}"), format!("Used {svc}"));
        }
    }
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
    match (
        activity_label(tool_name),
        outcome_label(tool_name),
    ) {
        (Some(a), Some(o)) => (a.to_string(), o.to_string()),
        // Unknown tool (e.g. tool_search, a skill, a delegate) — name it
        // honestly instead of "working" / "Did a step".
        _ => raw_name(tool_name),
    }
}


#[cfg(test)]
mod tests {
    use super::{service_name as humanize_slug, tool_call as humanize_tool_call, strap_verb};
    use serde_json::json;

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
        let (act, out) = humanize_tool_call("mcp__github__create_issue", &json!({}));
        assert_eq!(act, "using github (create issue)");
        assert_eq!(out, "Used github: create issue");
    }

    /// STRAP signatures (resource + action) read as verb+noun; an unknown
    /// verb shows the signature honestly rather than a guessed label.
    #[test]
    fn strap_signature_reads_as_verb_noun() {
        let (act, out) =
            humanize_tool_call("os", &json!({"resource": "file", "action": "read"}));
        assert_eq!(act, "reading file");
        assert_eq!(out, "Read file");
        let (act, out) =
            humanize_tool_call("os", &json!({"resource": "file", "action": "frobnicate"}));
        assert_eq!(act, "running frobnicate on file");
        assert_eq!(out, "Ran frobnicate on file");
    }

    /// Web fetches/navigations label by host (www. stripped) so a run that
    /// read four pages doesn't collapse into "Searched the web" for all of it.
    #[test]
    fn web_actions_label_by_host_not_generic_search() {
        let (act, out) = humanize_tool_call(
            "web",
            &json!({"action": "fetch", "url": "https://www.example.com/page"}),
        );
        assert_eq!(act, "reading example.com");
        assert_eq!(out, "Read example.com");
        let (act, _) = humanize_tool_call(
            "web",
            &json!({"action": "navigate", "url": "https://docs.rs/x"}),
        );
        assert_eq!(act, "opening docs.rs");
        // No action = search — the default label.
        let (act, out) = humanize_tool_call("web", &json!({}));
        assert_eq!(act, "searching the web");
        assert_eq!(out, "Searched the web");
    }

    /// Plugin calls must say the SERVICE ("using Gmail"), never the word
    /// "plugin" — the register the whole install flow protects.
    #[test]
    fn plugin_calls_name_the_service_never_the_word_plugin() {
        let (act, out) =
            humanize_tool_call("plugin", &json!({"resource": "google-search-console"}));
        assert_eq!(act, "using Google Search Console");
        assert_eq!(out, "Used Google Search Console");
        let (act, _) = humanize_tool_call("plugin", &json!({"action": "discover"}));
        assert_eq!(act, "browsing the marketplace");
    }

    /// Known bare tool names use the curated copy; unknown tools are named
    /// as-is ("using tool search") — honest, never vague filler.
    #[test]
    fn bare_tools_use_curated_copy_and_honest_fallback() {
        let (act, out) = humanize_tool_call("bash", &json!({}));
        assert_eq!(act, "running a command");
        assert_eq!(out, "Ran a command");
        let (act, out) = humanize_tool_call("tool_search", &json!({}));
        assert_eq!(act, "using tool search");
        assert_eq!(out, "Used tool search");
    }
}
