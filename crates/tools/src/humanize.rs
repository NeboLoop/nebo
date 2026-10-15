//! How a tool call is named. Two audiences, one source each:
//!
//! - The owner's screens name a call by [`call`]: a `kind` every client words
//!   in the owner's language, and the call's own values verbatim. It is the
//!   only name a thread, a live chip, a helper line or a team row shows.
//! - The model, and the owner's approval card, read what a call does as a
//!   sentence: [`crate::registry::DynTool::describe`], whose default is
//!   [`sentence`] ("reading reminders", "using send invoice for ACME").
//! - The approval card previews what a call would send or change by
//!   [`details`]: the call's own values under a `kind` every client words
//!   in the owner's language, as [`call`] does.

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
/// tools (`mcp__slug__tool`) read from their service and tool name, a
/// `resource`/`action` signature reads as verb + noun, anything else as the
/// tool's name in words. Each names what the call acts on ([`subject`])
/// when it says, so the owner reads more than a tool's name.
pub fn sentence(tool_name: &str, input: &serde_json::Value) -> String {
    let on = subject(input).map(|s| format!(": {s}")).unwrap_or_default();
    // MCP: mcp__github__create_issue → "using Github to create issue".
    if let Some((slug, tool)) = tool_name.strip_prefix("mcp__").and_then(|rest| rest.split_once("__")) {
        return format!("using {} to {}{on}", service_name(slug), tool.replace('_', " "));
    }
    // STRAP: toolName(resource, action, …).
    let resource = input.get("resource").and_then(|v| v.as_str());
    let action = input.get("action").and_then(|v| v.as_str());
    if let (Some(resource), Some(action)) = (resource, action) {
        let noun = resource.replace('_', " ");
        return match strap_verb(action) {
            Some((_, gerund)) => format!("{gerund} {noun}{on}"),
            // Unknown verb: the signature as it is, never a guess.
            None => format!("running {action} on {noun}{on}"),
        };
    }
    format!("using {}{on}", tool_name.replace('_', " "))
}

/// What a call acts on, from its most telling value (a recipient, a
/// subject, a path, a URL, a query, a name), quoted when it is free text.
fn subject(input: &serde_json::Value) -> Option<String> {
    const KEYS: [&str; 10] = ["to", "recipient", "phone", "channel", "subject", "title", "path", "url", "query", "name"];
    let value = KEYS.into_iter().find_map(|k| detail_text(input.get(k)?))?;
    Some(match value.contains(' ') {
        true => format!("\u{201c}{}\u{201d}", cap(&value, DESCRIPTION_CAP)),
        false => cap(&value, DESCRIPTION_CAP),
    })
}

/// One thing the owner's approval card previews about a call: what it
/// would send or change, under a `kind` every client words in the owner's
/// language. Kinds: `to` (who it goes to), `from` (the account or line it
/// goes out from), `subject`, `body` (the first lines of what it says),
/// `amount`, `when`, `link`, `file`, `search` (what it looks for),
/// `command`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Detail {
    pub kind: String,
    pub value: String,
}

/// The most a detail's value shows, in graphemes: the card is a preview,
/// never the whole message.
pub const DETAIL_CAP: usize = 280;

/// The lines of a body a detail shows.
const BODY_LINES: usize = 4;

/// What a call would send or change, from its own values: the default of
/// every tool's `DynTool::details`. Reads the arguments calls name these
/// by across Nebo's tools, plugins and connectors. Nothing secret shows:
/// every value goes through the one redactor ([`types::redact::redact`]).
pub fn details(input: &serde_json::Value) -> Vec<Detail> {
    const KINDS: [(&str, &[&str]); 10] = [
        ("to", &["to", "recipient", "recipients", "phone", "email", "channel", "cc"]),
        ("from", &["from", "sender", "account"]),
        ("subject", &["subject", "title"]),
        ("body", &["body", "text", "message", "content", "html"]),
        ("amount", &["amount", "total", "price"]),
        ("when", &["when", "at", "start", "date", "due_date"]),
        ("link", &["url"]),
        ("file", &["path", "file", "paths"]),
        ("search", &["query", "q"]),
        ("command", &["command"]),
    ];
    let Some(obj) = input.as_object() else { return Vec::new() };
    let currency = obj.get("currency").and_then(detail_text);
    KINDS
        .iter()
        .filter_map(|(kind, keys)| {
            let raw = keys.iter().find_map(|k| obj.get(*k).and_then(detail_text))?;
            let value = match *kind {
                "body" => first_lines(&raw),
                "amount" => match &currency {
                    Some(c) => format!("{raw} {c}"),
                    None => raw,
                },
                _ => cap(&raw, DETAIL_CAP),
            };
            (!value.is_empty()).then(|| Detail { kind: kind.to_string(), value: types::redact::redact(&value) })
        })
        .collect()
}

/// The line the employee wrote for the owner about why it makes a call:
/// a call's `reason` argument (plugins and operations take one beside
/// their `display`), when it wrote one.
pub fn stated_reason(input: &serde_json::Value) -> Option<String> {
    input.get("reason").and_then(detail_text).map(|r| types::redact::redact(&cap(&r, DETAIL_CAP)))
}

/// A value as a detail shows it: text as it is, a number as written, a
/// list of texts joined. Anything else is not shown.
fn detail_text(v: &serde_json::Value) -> Option<String> {
    let text = match v {
        serde_json::Value::String(s) => s.trim().to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Array(items) => {
            items.iter().filter_map(|i| i.as_str()).map(str::trim).filter(|i| !i.is_empty()).collect::<Vec<_>>().join(", ")
        }
        _ => return None,
    };
    (!text.is_empty()).then_some(text)
}

/// The first lines of a body, each on its own line, cut at [`DETAIL_CAP`].
fn first_lines(body: &str) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    let lines: Vec<&str> = body.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let mut shown = lines.iter().take(BODY_LINES).map(|l| cap(l, DETAIL_CAP)).collect::<Vec<_>>().join("\n");
    if lines.len() > BODY_LINES && !shown.ends_with('…') {
        shown.push('…');
    }
    match shown.grapheme_indices(true).nth(DETAIL_CAP) {
        Some((cut, _)) => format!("{}…", shown[..cut].trim_end()),
        None => shown,
    }
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
    /// their service and tool name, STRAP signatures as verb + noun (an
    /// unknown verb as the signature itself), anything else as its name in
    /// words; each with what it acts on when the call says.
    #[test]
    fn a_call_reads_as_a_sentence() {
        assert_eq!(sentence("mcp__github__create_issue", &json!({})), "using Github to create issue");
        assert_eq!(
            sentence("mcp__github__create_issue", &json!({"title": "Login fails on Safari"})),
            "using Github to create issue: \u{201c}Login fails on Safari\u{201d}"
        );
        assert_eq!(sentence("os", &json!({"resource": "reminders", "action": "read"})), "reading reminders");
        assert_eq!(sentence("os", &json!({"resource": "reminders", "action": "frobnicate"})), "running frobnicate on reminders");
        assert_eq!(sentence("send_invoice", &json!({})), "using send invoice");
        assert_eq!(sentence("send_invoice", &json!({"to": "ap@acme.com"})), "using send invoice: ap@acme.com");
    }

    /// Live 2026-10-15: the owner's card read "Using message". A call with
    /// no words of its own reads from its name in words and its most
    /// telling value: never a tool's id, and never empty.
    #[test]
    fn a_fallback_never_shows_a_tool_id() {
        for (name, input) in [
            ("mcp__google-drive__share_file", json!({"path": "/Q3/plan.pdf", "email": "pat@acme.com"})),
            ("use_skill", json!({"name": "quarterly-close"})),
            ("crm_contact_upsert", json!({"name": "Dana Lee"})),
        ] {
            let said = sentence(name, &input);
            assert!(!said.contains(name) && !said.contains('_'), "{name}: {said}");
            assert!(said.split(' ').count() >= 3, "{name}: {said}");
        }
    }

    /// The card's preview: who it goes to, the subject, the first lines of
    /// the body, the amount with its currency, under kinds the clients
    /// word; nothing secret, and never the whole message.
    #[test]
    fn details_preview_what_would_go_out_and_nothing_secret() {
        let body = (1..=8).map(|i| format!("Line {i}")).collect::<Vec<_>>().join("\n");
        let got = super::details(&json!({
            "to": ["dana@acme.com", "pat@acme.com"],
            "subject": "Q3 quote",
            "text": body,
            "amount": 420.5,
            "currency": "USD",
            "api_key": "sk-live-123",
            "url": "https://hooks.example.com/x?token=abc123",
        }));
        let kinds: Vec<(&str, &str)> = got.iter().map(|d| (d.kind.as_str(), d.value.as_str())).collect();
        assert_eq!(
            kinds,
            vec![
                ("to", "dana@acme.com, pat@acme.com"),
                ("subject", "Q3 quote"),
                ("body", "Line 1\nLine 2\nLine 3\nLine 4…"),
                ("amount", "420.5 USD"),
                ("link", "https://hooks.example.com/x?token=[redacted]"),
            ]
        );
        assert!(!format!("{got:?}").contains("sk-live"), "no secret: {got:?}");
        let long = "word ".repeat(200);
        let one = super::details(&json!({ "body": long }));
        assert!(one[0].value.chars().count() <= super::DETAIL_CAP + 1 && one[0].value.ends_with('…'));
        assert_eq!(super::stated_reason(&json!({"reason": "  The client asked for it today. "})).as_deref(), Some("The client asked for it today."));
        assert_eq!(super::stated_reason(&json!({})), None);
    }

    /// The calls that most often ask the owner (an email, a text, a
    /// calendar event, a web request, a file, the browser, a payment) say
    /// what they do with the values that matter, never a tool's name.
    #[tokio::test]
    async fn the_common_calls_say_what_they_do() {
        use crate::registry::DynTool;
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(db::Store::new(&dir.path().join("t.db").to_string_lossy()).unwrap());
        let message = crate::MessageTool::new(store);
        let web = |name: &str| crate::web_tool::tools(crate::web_tool::WebCore::new()).into_iter().find(|t| t.name() == name).unwrap();
        let machine = std::sync::Arc::new(crate::file_tools::Machine::new(std::sync::Arc::new(crate::ProcessRegistry::new()), None));
        let write = crate::file_tools::WriteFileTool(machine);
        let os = |input: serde_json::Value| crate::os_tool::OsTool::sentence(&input);
        let cases = [
            (
                os(json!({"resource": "mail", "action": "send", "to": "dana@acme.com", "subject": "Q3 quote", "text": "Hi Dana"})),
                "sending an email to dana@acme.com about \u{201c}Q3 quote\u{201d}",
            ),
            (message.describe(&json!({"resource": "sms", "action": "send", "phone": "+13855550100", "text": "On my way"})), "texting +13855550100"),
            (
                message.describe(&json!({"action": "search", "query": "question bookkeeper accounting decision needed"})),
                "searching your texts for \u{201c}question bookkeeper accounting decision needed\u{201d}",
            ),
            (os(json!({"resource": "calendar", "action": "create", "title": "Site visit"})), "adding \u{201c}Site visit\u{201d} to your calendar"),
            (os(json!({"resource": "calendar", "action": "today"})), "reading your calendar"),
            (web("http_request").describe(&json!({"method": "POST", "url": "https://api.example.com/v1"})), "sending POST to api.example.com"),
            (write.describe(&json!({"path": "/Users/me/notes.md", "content": "x"})), "writing notes.md"),
            (web("browser_open").describe(&json!({"url": "https://example.com/pricing"})), &web("browser_open").describe(&json!({"url": "https://example.com/pricing"}))),
        ];
        for (said, want) in &cases {
            assert_eq!(said, want);
            for id in ["message", "os", "http_request", "write_file", "browser_open", "mcp__"] {
                assert!(!said.contains(id), "{said} names {id}");
            }
        }
        assert!(cases[7].0.contains("example.com"), "{}", cases[7].0);
        // A payment says what the employee wrote for the owner, and its
        // details carry the amount.
        let pay = json!({"display": "Pay ACME invoice 1041", "amount": "420.00", "currency": "USD"});
        assert_eq!(super::details(&pay), vec![super::Detail { kind: "amount".into(), value: "420.00 USD".into() }]);
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
