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


/// The most a free-text description the model wrote (a command's
/// `description`, a helper's) shows in a label, in graphemes.
pub const DESCRIPTION_CAP: usize = 60;

/// `text` on one line, at most `max` graphemes (user-perceived characters:
/// an emoji or an accented letter is one), with "…" where it was cut.
pub fn cap(text: &str, max: usize) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut graphemes = one_line.graphemes(true);
    let kept: String = graphemes.by_ref().take(max).collect();
    if graphemes.next().is_some() { format!("{kept}…") } else { kept }
}

/// A call as the owner's thread names it: a `kind` every client words in
/// the owner's language, and the call's own values (`params`: a path, a
/// command, a query, a URL) shown verbatim. Derived from the call alone, so
/// a stored call gets the same one when its thread is read again.
///
/// Kinds and their params: `read` / `write` / `edit` / `plan` / `convert`
/// {path}; `share` {path} or {count}; `command` {command, desc?}; `search`
/// {query}; `fetch` {url}; `request` {url, method?}; `browser` {step,
/// url?}; `research` {query}; `helper` {desc}; `task` {subject};
/// `mcp` {service, tool}; `action` {verb, noun} where `verb` is one of
/// [`strap_verb`]'s keys, or {action, noun} for any other action; `tool`
/// {name}. `action`, `mcp` and `tool` also carry `target` when the call
/// names a path, URL, query or name.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Call {
    pub kind: &'static str,
    pub params: std::collections::BTreeMap<&'static str, String>,
}

impl Call {
    fn new(kind: &'static str, params: &[(&'static str, String)]) -> Self {
        Call {
            kind,
            params: params.iter().filter(|(_, v)| !v.trim().is_empty()).cloned().collect(),
        }
    }
}

/// The canonical verb of a STRAP action: the key [`strap_verb`] words it
/// under ("get" → "read").
fn verb_key(action: &str) -> Option<&'static str> {
    Some(match strap_verb(action)?.1 {
        "Created" => "create",
        "Read" => "read",
        "Listed" => "list",
        "Searched" => "search",
        "Updated" => "update",
        "Deleted" => "delete",
        "Sent" => "send",
        "Ran" => "run",
        "Wrote" => "write",
        "Downloaded" => "download",
        "Uploaded" => "upload",
        "Opened" => "open",
        "Stopped" => "stop",
        "Checked" => "check",
        "Notified" => "notify",
        _ => return None,
    })
}

/// The structured name of a call: see [`Call`].
pub fn call(tool_name: &str, input: &serde_json::Value) -> Call {
    let s = |key: &str| input.get(key).and_then(|v| v.as_str()).map(str::trim).unwrap_or("").to_string();
    let target = || {
        ["path", "url", "query", "command", "name", "title"]
            .into_iter()
            .map(|k| s(k))
            .find(|v| !v.is_empty())
            .map(|v| cap(&v, 200))
            .unwrap_or_default()
    };
    match tool_name {
        "read_file" => return Call::new("read", &[("path", s("path"))]),
        "write_file" => return Call::new("write", &[("path", s("path"))]),
        "edit_file" => return Call::new("edit", &[("path", s("path"))]),
        "write_plan" => return Call::new("plan", &[("path", s("path"))]),
        "convert_file" => return Call::new("convert", &[("path", s("path"))]),
        "share_file" => {
            let paths: Vec<String> = match input.get("paths").or_else(|| input.get("path")) {
                Some(serde_json::Value::Array(items)) => {
                    items.iter().filter_map(|v| v.as_str()).map(str::to_string).collect()
                }
                Some(serde_json::Value::String(one)) => vec![one.clone()],
                _ => Vec::new(),
            };
            return match paths.as_slice() {
                [one] => Call::new("share", &[("path", one.clone())]),
                many => Call::new("share", &[("count", many.len().to_string())]),
            };
        }
        "run_command" | "shell" => {
            return Call::new("command", &[("command", s("command")), ("desc", cap(&s("description"), DESCRIPTION_CAP))]);
        }
        "search_web" => {
            let queries: Vec<String> = match input.get("queries") {
                Some(serde_json::Value::Array(items)) => items
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(str::trim)
                    .filter(|q| !q.is_empty())
                    .map(str::to_string)
                    .collect(),
                _ => Vec::new(),
            };
            let query = if queries.is_empty() { s("query") } else { queries.join(" · ") };
            return Call::new("search", &[("query", query)]);
        }
        "fetch_url" => return Call::new("fetch", &[("url", s("url"))]),
        "http_request" => return Call::new("request", &[("url", s("url")), ("method", s("method"))]),
        "deep_research" | "quick_research" => return Call::new("research", &[("query", s("query"))]),
        "delegate" => return Call::new("helper", &[("desc", cap(&s("description"), DESCRIPTION_CAP))]),
        "create_task" => return Call::new("task", &[("subject", cap(&s("subject"), DESCRIPTION_CAP))]),
        _ => {}
    }
    if let Some(step) = tool_name.strip_prefix("browser_") {
        let step = match (step, s("action")) {
            ("act", action) if !action.is_empty() => action,
            _ => step.to_string(),
        };
        return Call::new("browser", &[("step", step), ("url", s("url"))]);
    }
    if let Some((slug, tool)) = tool_name.strip_prefix("mcp__").and_then(|rest| rest.split_once("__")) {
        return Call::new("mcp", &[("service", service_name(slug)), ("tool", tool.replace('_', " ")), ("target", target())]);
    }
    let (resource, action) = (s("resource"), s("action"));
    if !resource.is_empty() && !action.is_empty() {
        let noun = resource.replace('_', " ");
        return match verb_key(&action) {
            Some(verb) => Call::new("action", &[("verb", verb.to_string()), ("noun", noun), ("target", target())]),
            None => Call::new("action", &[("action", action.replace('_', " ")), ("noun", noun), ("target", target())]),
        };
    }
    Call::new("tool", &[("name", tool_name.replace('_', " ")), ("target", target())])
}


#[cfg(test)]
mod tests {
    use super::{call, call_labels, cap, service_name as humanize_slug, strap_verb};
    use crate::os_tool::OsTool;
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

    /// A tool without words of its own is named as-is ("using send invoice")
    /// — honest, never vague filler.
    #[test]
    fn a_tool_without_better_words_is_named_as_is() {
        let (act, out) = call_labels("send_invoice", &json!({}));
        assert_eq!(act, "using send invoice");
        assert_eq!(out, "Used send invoice");
    }

    /// The pairs of a call's params, for comparing.
    fn params(c: &super::Call) -> Vec<(&str, &str)> {
        c.params.iter().map(|(k, v)| (*k, v.as_str())).collect()
    }

    /// A call is named by kind and its own values, never by an English
    /// sentence: the clients word the kind, and show the values verbatim.
    #[test]
    fn calls_are_named_by_kind_and_their_values() {
        let c = call("read_file", &json!({"path": "/Users/me/Documents/請求書 2026.xlsx"}));
        assert_eq!((c.kind, params(&c)), ("read", vec![("path", "/Users/me/Documents/請求書 2026.xlsx")]));
        let c = call("run_command", &json!({"command": "ls -la ~/Desktop", "description": "List the desktop"}));
        assert_eq!((c.kind, params(&c)), ("command", vec![("command", "ls -la ~/Desktop"), ("desc", "List the desktop")]));
        let c = call("search_web", &json!({"queries": ["rust unicode", " ", "graphemes"]}));
        assert_eq!((c.kind, params(&c)), ("search", vec![("query", "rust unicode · graphemes")]));
        let c = call("fetch_url", &json!({"url": "https://example.com/a"}));
        assert_eq!((c.kind, params(&c)), ("fetch", vec![("url", "https://example.com/a")]));
        let c = call("share_file", &json!({"paths": ["/a/x.pdf", "/a/y.pdf"]}));
        assert_eq!((c.kind, params(&c)), ("share", vec![("count", "2")]));
        let c = call("browser_act", &json!({"action": "click", "ref": "B2"}));
        assert_eq!((c.kind, params(&c)), ("browser", vec![("step", "click")]));
        let c = call("mcp__google-drive__list_files", &json!({"query": "budget"}));
        assert_eq!((c.kind, params(&c)), ("mcp", vec![("service", "Google Drive"), ("target", "budget"), ("tool", "list files")]));
        let c = call("os", &json!({"resource": "reminders", "action": "get"}));
        assert_eq!((c.kind, params(&c)), ("action", vec![("noun", "reminders"), ("verb", "read")]));
        let c = call("os", &json!({"resource": "app", "action": "frobnicate"}));
        assert_eq!((c.kind, params(&c)), ("action", vec![("action", "frobnicate"), ("noun", "app")]));
        let c = call("send_invoice", &json!({"name": "ACME"}));
        assert_eq!((c.kind, params(&c)), ("tool", vec![("name", "send invoice"), ("target", "ACME")]));
    }

    /// A description the model wrote is cut at 60 graphemes, on one line;
    /// a grapheme is never split.
    #[test]
    fn free_text_is_capped_by_grapheme() {
        let long = "Lists every invoice in the March folder and then compares the totals";
        let c = call("run_command", &json!({"command": "ls", "description": long}));
        let desc = &c.params["desc"];
        assert!(desc.ends_with('…') && desc.chars().count() == 61, "{desc}");
        assert_eq!(cap("a\n  b", 10), "a b");
        assert_eq!(cap("👨‍👩‍👧‍👦👨‍👩‍👧‍👦", 1), "👨‍👩‍👧‍👦…");
        assert_eq!(cap("請求書を確認", 3), "請求書…");
    }
}
