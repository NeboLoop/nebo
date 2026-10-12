//! How a tool call is named. Two audiences, one source each:
//!
//! - The owner's screens name a call by [`call`]: a `kind` every client words
//!   in the owner's language, and the call's own values verbatim. It is the
//!   only name a thread, a live chip, a helper line or a team row shows.
//! - The model, and the owner's approval card, read what a call does as a
//!   sentence: [`crate::registry::DynTool::describe`], whose default is
//!   [`sentence`] ("reading reminders", "using send invoice").

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

/// A STRAP action's canonical verb (the key clients word it under) and its
/// gerund for a sentence: "get" → ("read", "reading").
pub fn strap_verb(action: &str) -> Option<(&'static str, &'static str)> {
    Some(match action {
        "create" | "add" | "insert" => ("create", "creating"),
        "read" | "get" | "view" | "fetch" => ("read", "reading"),
        "list" | "ls" => ("list", "listing"),
        "search" | "find" | "query" | "glob" | "grep" => ("search", "searching"),
        "update" | "edit" | "set" | "patch" | "rename" | "move" => ("update", "updating"),
        "delete" | "remove" | "clear" => ("delete", "deleting"),
        "send" | "post" | "reply" | "dm" => ("send", "sending"),
        "run" | "exec" | "execute" | "shell" => ("run", "running"),
        "write" | "save" => ("write", "writing"),
        "download" => ("download", "downloading"),
        "upload" => ("upload", "uploading"),
        "open" | "launch" | "start" => ("open", "opening"),
        "stop" | "close" | "kill" => ("stop", "stopping"),
        "check" | "status" | "verify" => ("check", "checking"),
        "notify" | "alert" => ("notify", "notifying"),
        _ => return None,
    })
}

/// The generic sentence for what a call does, the default of every tool's
/// `DynTool::describe` (a tool with better words says them itself): MCP
/// tools (`mcp__slug__tool`) read from their slug and tool name, a
/// `resource`/`action` signature reads as verb + noun, anything else names
/// the tool as it is.
pub fn sentence(tool_name: &str, input: &serde_json::Value) -> String {
    // MCP: mcp__github__create_issue → "using github (create issue)".
    if let Some((slug, tool)) = tool_name.strip_prefix("mcp__").and_then(|rest| rest.split_once("__")) {
        return format!("using {slug} ({})", tool.replace('_', " "));
    }
    // STRAP: toolName(resource, action, …).
    let resource = input.get("resource").and_then(|v| v.as_str());
    let action = input.get("action").and_then(|v| v.as_str());
    if let (Some(resource), Some(action)) = (resource, action) {
        let noun = resource.replace('_', " ");
        return match strap_verb(action) {
            Some((_, gerund)) => format!("{gerund} {noun}"),
            // Unknown verb: the signature as it is, never a guess.
            None => format!("running {action} on {noun}"),
        };
    }
    format!("using {}", tool_name.replace('_', " "))
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
/// {path}; `share` {path} or {count}; `command` {command, desc?, reach?,
/// app?} where `reach` is what of the owner's it touches (`screen`,
/// `microphone`, `camera`, `input`, `app`); `search`
/// {query}; `fetch` {url}; `request` {url, method?}; `browser` {step,
/// url?}; `computer` {step, app?, target?} for a click or a look at the
/// screen; `research` {query}; `helper` {desc}; `task` {subject}; `ask`
/// {question} for a question to the owner; `mcp`
/// {service, tool}; `action` {verb, noun} where `verb` is one of
/// [`strap_verb`]'s keys, or {action, noun} for any other action; `tool`
/// {name}. `action`, `mcp` and `tool` also carry `target` when the call
/// names a path, URL, query or name, and `desc` when the model wrote the
/// call's own line (`display`, a plugin's or an operation's).
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
    let desc = || cap(&s("display"), DESCRIPTION_CAP);
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
            // A command that watches the owner or drives his apps says so,
            // whatever the model wrote for it.
            use types::permissions::OwnerReach;
            let (reach, app) = match crate::policy::owner_reach(&s("command")) {
                Some(OwnerReach::Screen) => ("screen", String::new()),
                Some(OwnerReach::Microphone) => ("microphone", String::new()),
                Some(OwnerReach::Camera) => ("camera", String::new()),
                Some(OwnerReach::Input) => ("input", String::new()),
                Some(OwnerReach::App { app }) => ("app", app.unwrap_or_default()),
                None => ("", String::new()),
            };
            return Call::new(
                "command",
                &[
                    ("command", s("command")),
                    ("desc", cap(&s("description"), DESCRIPTION_CAP)),
                    ("reach", reach.to_string()),
                    ("app", app),
                ],
            );
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
        // A question to the owner reads as one ("Waiting for your answer",
        // then "Asked · the question"), never as the tool's name (live
        // 2026-10-11: "working with ask_owner…" over his own question).
        "ask_owner" => return Call::new("ask", &[("question", cap(&s("question"), 200))]),
        _ => {}
    }
    if tool_name == "os" {
        let step = s("action");
        let point = input
            .get("coordinate")
            .and_then(|v| v.as_array())
            .filter(|a| a.len() == 2)
            .map(|a| format!("({},{})", a[0], a[1]))
            .or_else(|| Some(format!("({},{})", input.get("x")?.as_i64()?, input.get("y")?.as_i64()?)));
        // A click names what it clicked, a look names what it looked at:
        // seven taps on one point read as seven, never as "Checked ×7".
        let target = match step.as_str() {
            "click" | "double_click" | "right_click" => Some(if s("ref").is_empty() { point.unwrap_or_default() } else { s("ref") }),
            "screenshot" | "see" | "capture" => Some(s("region")),
            "activate" | "launch" if !s("app").is_empty() => Some(String::new()),
            _ => None,
        };
        if let Some(target) = target {
            return Call::new("computer", &[("step", step.replace('_', " ")), ("app", s("app")), ("target", target)]);
        }
    }
    if let Some(step) = tool_name.strip_prefix("browser_") {
        let step = match (step, s("action")) {
            ("act", action) if !action.is_empty() => action,
            _ => step.to_string(),
        };
        return Call::new("browser", &[("step", step), ("url", s("url"))]);
    }
    if let Some((slug, tool)) = tool_name.strip_prefix("mcp__").and_then(|rest| rest.split_once("__")) {
        return Call::new("mcp", &[("service", service_name(slug)), ("tool", tool.replace('_', " ")), ("target", target()), ("desc", desc())]);
    }
    let (resource, action) = (s("resource"), s("action"));
    if !resource.is_empty() && !action.is_empty() {
        let noun = resource.replace('_', " ");
        return match strap_verb(&action).map(|(key, _)| key) {
            Some(verb) => Call::new("action", &[("verb", verb.to_string()), ("noun", noun), ("target", target()), ("desc", desc())]),
            None => Call::new("action", &[("action", action.replace('_', " ")), ("noun", noun), ("target", target()), ("desc", desc())]),
        };
    }
    Call::new("tool", &[("name", tool_name.replace('_', " ")), ("target", target()), ("desc", desc())])
}


#[cfg(test)]
mod tests {
    use super::{call, cap, sentence, service_name as humanize_slug, strap_verb};
    use serde_json::json;

    /// A click or a look at the screen names what it touched: the point or
    /// the ref, the app, the region. "Checked the workspace ×17" hid a model
    /// tapping the same point seven times and never looking (2026-09-19).
    #[test]
    fn os_calls_say_what_they_touched() {
        let c = call("os", &json!({"action":"click","app":"Simulator","coordinate":[223,900]}));
        assert_eq!((c.kind, params(&c)), ("computer", vec![("app", "Simulator"), ("step", "click"), ("target", "(223,900)")]));
        let c = call("os", &json!({"action":"click","ref":"B2"}));
        assert_eq!((c.kind, params(&c)), ("computer", vec![("step", "click"), ("target", "B2")]));
        let c = call("os", &json!({"resource":"capture","action":"screenshot","app":"Simulator"}));
        assert_eq!((c.kind, params(&c)), ("computer", vec![("app", "Simulator"), ("step", "screenshot")]));
        let c = call("os", &json!({"action":"launch","app":"Notes"}));
        assert_eq!((c.kind, params(&c)), ("computer", vec![("app", "Notes"), ("step", "launch")]));
        // Anything else is its STRAP signature.
        let c = call("os", &json!({"resource":"app","action":"list"}));
        assert_eq!((c.kind, params(&c)), ("action", vec![("noun", "app"), ("verb", "list")]));
    }

    /// The line the model wrote for a plugin's or an operation's call rides
    /// with it, capped like any description.
    #[test]
    fn a_calls_own_line_rides_as_its_desc() {
        let c = call("quickbooks", &json!({"command": "invoice send", "display": "Send invoice 1041 to ACME"}));
        assert_eq!((c.kind, params(&c)), ("tool", vec![("desc", "Send invoice 1041 to ACME"), ("name", "quickbooks"), ("target", "invoice send")]));
    }

    /// Service slugs render as the service's own name — dashes/underscores
    /// become spaces, each word capitalized, empty segments dropped.
    #[test]
    fn slugs_render_as_service_names() {
        assert_eq!(humanize_slug("google-search-console"), "Google Search Console");
        assert_eq!(humanize_slug("gws_calendar"), "Gws Calendar");
        assert_eq!(humanize_slug("a--b"), "A B");
    }

    /// STRAP verbs map to (key, gerund); an unknown verb yields None so the
    /// caller can show the raw signature honestly instead of guessing.
    #[test]
    fn strap_verbs_cover_known_and_refuse_unknown() {
        assert_eq!(strap_verb("get"), Some(("read", "reading")));
        assert_eq!(strap_verb("delete"), Some(("delete", "deleting")));
        assert_eq!(strap_verb("frobnicate"), None);
    }

    /// The sentence the model and the approval card read: MCP tools from
    /// slug and tool name, STRAP signatures as verb + noun (an unknown verb
    /// as the signature itself), anything else named as it is.
    #[test]
    fn a_call_reads_as_a_sentence() {
        assert_eq!(sentence("mcp__github__create_issue", &json!({})), "using github (create issue)");
        assert_eq!(sentence("os", &json!({"resource": "reminders", "action": "read"})), "reading reminders");
        assert_eq!(sentence("os", &json!({"resource": "reminders", "action": "frobnicate"})), "running frobnicate on reminders");
        assert_eq!(sentence("send_invoice", &json!({})), "using send invoice");
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
        // A command that watches the owner says so, whatever its description.
        let c = call("run_command", &json!({"command": "screencapture -x /tmp/a.png", "description": "List files"}));
        assert_eq!((c.kind, params(&c)), ("command", vec![("command", "screencapture -x /tmp/a.png"), ("desc", "List files"), ("reach", "screen")]));
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

    /// Live 2026-10-11: the owner's phone read "working with ask_owner…"
    /// over the question it was asking him. A question to the owner is the
    /// `ask` kind with the question as its value; the tool's name is never
    /// a word on his screen.
    #[test]
    fn a_question_to_the_owner_is_an_ask_never_the_tools_name() {
        let c = call("ask_owner", &json!({"question": "Are you flying from Salt Lake City?"}));
        assert_eq!((c.kind, params(&c)), ("ask", vec![("question", "Are you flying from Salt Lake City?")]));
        let c = call("ask_owner", &json!({"question": "Which one?", "options": ["A", "B"]}));
        assert_eq!((c.kind, params(&c)), ("ask", vec![("question", "Which one?")]));
        assert!(!serde_json::to_string(&c).unwrap().contains("ask_owner"));
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
