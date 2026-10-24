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

/// [`cap`] for a title the owner reads: a cut ends at the last whole word
/// that fits. Text with no space to cut at (one long word, a language
/// written without spaces) is cut by grapheme.
pub fn cap_words(text: &str, max: usize) -> String {
    let capped = cap(text, max);
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    // Uncut (a text that ends in "…" of its own included).
    if capped == one_line {
        return capped;
    }
    let Some(kept) = capped.strip_suffix('…') else { return capped };
    // A cut that already falls between two words keeps every word.
    let whole = if kept.ends_with(' ') || one_line[kept.len()..].starts_with(' ') {
        kept.trim_end()
    } else {
        match kept.rfind(' ') {
            Some(end) => &kept[..end],
            None => return capped,
        }
    };
    format!("{}…", whole.trim_end_matches([',', ';', ':', '-', '–', '—']))
}

/// The longest a helper's title runs, in graphemes, its number included:
/// two lines of a card.
pub const TITLE_CAP: usize = 80;

/// What a title body leaves for its number (" · 99").
const NUMBER_ROOM: usize = 5;

/// What makes two titles the same: their words, in one Unicode form, case
/// aside.
pub fn title_key(text: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    text.split_whitespace().collect::<Vec<_>>().join(" ").nfc().collect::<String>().to_lowercase()
}

/// `text` without the marks [`shown_titles`] adds at its end (" · 2",
/// " · #4"), however many: a description that already reads like a marked
/// title is taken as its plain words.
pub fn unmarked(text: &str) -> &str {
    let mut text = text.trim_end();
    while let Some((head, mark)) = text.rsplit_once(" · ") {
        let digits = mark.strip_prefix('#').unwrap_or(mark);
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
            break;
        }
        text = head.trim_end();
    }
    text
}

/// What makes two descriptions the same, for siblings told apart and for
/// the numbers helpers are given: their [`title_key`], marks aside
/// ([`unmarked`]).
pub fn description_key(description: &str) -> String {
    title_key(unmarked(description))
}

/// `base` within [`TITLE_CAP`], with ` · <mark>` after it when `mark` is
/// not empty: the base is cut, never the mark.
pub fn numbered(base: &str, mark: &str) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    let suffix = if mark.is_empty() { String::new() } else { format!(" · {mark}") };
    let room = TITLE_CAP - suffix.chars().count();
    let body = if base.graphemes(true).count() <= room {
        base.to_string()
    } else {
        // Its "…" takes one of the room's graphemes.
        cap_words(base, room - 1)
    };
    format!("{body}{suffix}")
}

/// What the owner's screen shows for running siblings, given each one's
/// base title ([`sibling_titles`]), the number it was given as it started
/// (one per description, never reused in its conversation; 0 when none
/// could be given) and its place in the conversation's start order. A
/// title is its base without any mark it already carries ([`unmarked`]);
/// titles that would read the same show their numbers ("scan logs · 1",
/// "scan logs · 3"), and any still alike their start place instead ("·
/// #4"). That last mark is reached only when two different descriptions cut
/// to the same words with the same number, or a number couldn't be given;
/// it carries no words, so it reads the same in every language. No two
/// titles ever read the same, and no number is ever changed.
pub fn shown_titles(siblings: &[(&str, u32, u64)]) -> Vec<String> {
    let alike = |titles: &[String]| {
        let mut count: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for t in titles {
            *count.entry(title_key(t)).or_default() += 1;
        }
        titles.iter().map(|t| count[&title_key(t)] > 1).collect::<Vec<_>>()
    };
    let bases: Vec<&str> = siblings.iter().map(|(base, _, _)| unmarked(base)).collect();
    let mut titles: Vec<String> = bases.iter().map(|base| numbered(base, "")).collect();
    let clash = alike(&titles);
    for (k, (_, number, _)) in siblings.iter().enumerate() {
        if clash[k] && *number > 0 {
            titles[k] = numbered(bases[k], &number.to_string());
        }
    }
    // Checked against every title, the untouched ones too.
    let clash = alike(&titles);
    for (k, (_, _, seq)) in siblings.iter().enumerate() {
        if clash[k] {
            titles[k] = numbered(bases[k], &format!("#{}", seq + 1));
        }
    }
    debug_assert!(!alike(&titles).contains(&true), "titles still alike: {titles:?}");
    titles
}

/// One base title per helper started from one conversation, given as
/// (description, brief): what the owner's screen calls each before
/// [`shown_titles`] tells apart any that still read the same, in order. It is the helper's own description,
/// unless a sibling's reads the same ([`title_key`]): eight research runs
/// whose questions open alike all read "research: What concrete dated
/// evidence…". Then it is told apart by the words of its brief, in one
/// pass:
/// - from the opening every such sibling shares when the word where it
///   parts from the sibling most like it fits in the title from there,
///   else from that word;
/// - a brief another one opens with shows its own last words ("…rents fell
///   in Phoenix"), never a cut that ends on a dangling word;
/// - a description that is a cut of its brief (its first words after a
///   `kind:`, or cut short with "…") gives way to those words, after its
///   `kind:` ("research: …Nvidia revenue grew in Q3?"); any other is kept,
///   and they follow it ("map rivals — …Ohio");
/// - a word longer than the title is cut with "…".
///
/// Siblings asked the very same keep their description; their numbers tell
/// them apart ([`shown_titles`]).
pub fn sibling_titles(siblings: &[(&str, &str)]) -> Vec<String> {
    use unicode_normalization::UnicodeNormalization;
    let labels: Vec<String> = siblings.iter().map(|(d, _)| d.split_whitespace().collect::<Vec<_>>().join(" ")).collect();
    let mut sets: Vec<Vec<usize>> = Vec::new();
    let mut set_of: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, label) in labels.iter().enumerate() {
        let set = *set_of.entry(description_key(label)).or_insert_with(|| {
            sets.push(Vec::new());
            sets.len() - 1
        });
        sets[set].push(i);
    }
    let mut titles = labels.clone();
    for members in sets.iter().filter(|m| m.len() > 1) {
        let briefs: Vec<Vec<String>> = members
            .iter()
            .map(|&i| siblings[i].1.nfc().collect::<String>().split_whitespace().map(str::to_string).collect())
            .collect();
        let shared = |a: &[String], b: &[String]| a.iter().zip(b).take_while(|(x, y)| x == y).count();
        // The opening all of them share, and how far each runs with the
        // one most like it: next to it in word order.
        let mut order: Vec<usize> = (0..members.len()).collect();
        order.sort_by(|&x, &y| briefs[x].cmp(&briefs[y]));
        let mut closest = vec![0; members.len()];
        let mut twin = vec![false; members.len()];
        let mut opening = usize::MAX;
        for w in order.windows(2) {
            let n = shared(&briefs[w[0]], &briefs[w[1]]);
            closest[w[0]] = closest[w[0]].max(n);
            closest[w[1]] = closest[w[1]].max(n);
            opening = opening.min(n);
            if briefs[w[0]] == briefs[w[1]] {
                (twin[w[0]], twin[w[1]]) = (true, true);
            }
        }
        for (k, &i) in members.iter().enumerate() {
            if !twin[k] {
                titles[i] = told_apart(&labels[i], &briefs[k], opening, closest[k]);
            }
        }
    }
    titles
}

/// The base title of a helper whose description `label` a sibling shares,
/// from its brief's words `own` (in one Unicode form): `opening` is how
/// many words every alike sibling's brief shares, `parts` how many it
/// shares with the one most like it ([`sibling_titles`]).
fn told_apart(label: &str, own: &[String], opening: usize, parts: usize) -> String {
    use unicode_normalization::UnicodeNormalization;
    if own.is_empty() {
        return label.to_string();
    }
    let (kind, cut) = match label.split_once(' ') {
        Some((kind, cut)) if kind.len() > 1 && kind.ends_with(':') => (Some(kind), cut),
        _ => (None, label),
    };
    // A cut of its brief: a `kind:` before its first words, or its first
    // words cut short.
    let is_cut = kind.is_some() || cut.ends_with('…');
    let cut: String = cut.trim_end_matches('…').trim_end().nfc().collect();
    let gives_way = is_cut && !cut.is_empty() && own.join(" ").starts_with(&cut);
    // What the brief's words may run to, beside what leads them and the
    // number's room.
    let lead_room = match (gives_way, kind) {
        (true, Some(kind)) => kind.chars().count() + 2,
        (true, None) => 1,
        (false, _) => label.chars().count() + 4,
    };
    let room = TITLE_CAP.saturating_sub(NUMBER_ROOM + lead_room).max(12);
    let runs = |from: usize, to: usize| own[from..to].join(" ").chars().count();
    let last = own.len();
    let from = if parts >= last {
        // Another brief opens with all of it: its end shows, from as early
        // as still fits.
        let mut from = last - 1;
        let mut length = own[from].chars().count();
        while from > 0 && length + 1 + own[from - 1].chars().count() <= room {
            from -= 1;
            length += 1 + own[from].chars().count();
        }
        from
    } else if runs(opening, parts + 1) <= room {
        opening
    } else {
        parts
    };
    let lead = if from > 0 { "…" } else { "" };
    let tail = cap_words(&own[from..].join(" "), room);
    match (gives_way, kind) {
        (true, Some(kind)) => format!("{kind} {lead}{tail}"),
        (true, None) => format!("{lead}{tail}"),
        (false, _) => format!("{label} — {lead}{tail}"),
    }
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
        "ask_owner" => {
            let first = input["questions"][0]["question"].as_str().map(|q| q.trim().to_string()).unwrap_or_else(|| s("question"));
            return Call::new("ask", &[("question", cap(&first, 200))]);
        }
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


/// What a call would do, as the owner's decision card says it after "wants
/// to": "**Bookkeeper** wants to **send a text to +13855550100**". A
/// lowercase verb phrase in Nebo's own words is worded by a message key
/// (`decision.wants.<key>` in the apps) with the call's values, so it
/// reads in the owner's language; `text` is its English. A call that
/// carries the model's own line for the owner (`display`) says that line,
/// with no key: those words are the model's.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Intent {
    /// `decision.wants.<key>`, when the words are Nebo's own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub values: std::collections::BTreeMap<String, String>,
    /// The phrase in English: what an app without the key shows.
    pub text: String,
}

/// Nebo's own phrases for a call, by key, in English. The apps hold the
/// same words under `decision.wants.<key>` in every language.
pub const INTENT_PHRASES: &[(&str, &str)] = &[
    ("textTo", "send a text to {phone}"),
    ("text", "send a text"),
    ("searchTextsFor", "search your texts for {query}"),
    ("searchTexts", "search your texts"),
    ("listTexts", "list your text conversations"),
    ("readTextsWith", "read your texts with {phone}"),
    ("readTexts", "read your texts"),
    ("useTexts", "use your texts"),
    ("emailToAbout", "send an email to {to} about {subject}"),
    ("emailTo", "send an email to {to}"),
    ("emailAbout", "send an email about {subject}"),
    ("email", "send an email"),
    ("readEmail", "read your email"),
    ("searchEmailFor", "search your email for {query}"),
    ("searchEmail", "search your email"),
    ("addEventTitled", "add {title} to your calendar"),
    ("addEvent", "add an event to your calendar"),
    ("changeEvent", "change an event on your calendar"),
    ("cancelEvent", "cancel an event on your calendar"),
    ("readCalendar", "read your calendar"),
    ("addReminderTitled", "add the reminder {title}"),
    ("addReminder", "add a reminder"),
    ("changeReminders", "change your reminders"),
    ("readReminders", "read your reminders"),
    ("lookUpContact", "look up {query} in your contacts"),
    ("lookInContacts", "look in your contacts"),
    ("click", "click {target}"),
    ("clickIn", "click {target} in {app}"),
    ("captureScreen", "capture your screen"),
    ("captureApp", "capture {app}"),
    ("captureRegion", "capture part of your screen"),
    ("openApp", "open {app}"),
    ("seeScreen", "see your screen"),
    ("hearMicrophone", "record from your microphone"),
    ("useCamera", "take a picture with your camera"),
    ("controlApp", "control {app}"),
    ("controlApps", "control your apps"),
    ("controlInput", "control your mouse and keyboard"),
    ("runCommand", "run {command}"),
    ("readFile", "read {file}"),
    ("writeFile", "write {file}"),
    ("editFile", "edit {file}"),
    ("writePlan", "write the plan {file}"),
    ("convertFile", "convert {file}"),
    ("shareFile", "share {file}"),
    ("shareFiles", "share {count} files"),
    ("searchWeb", "search the web for {query}"),
    ("research", "research {query}"),
    ("openUrl", "open {url}"),
    ("requestMethod", "send a {method} request to {host}"),
    ("request", "send a request to {host}"),
    ("useBrowserOn", "use the browser on {url}"),
    ("useBrowser", "use the browser"),
    ("useComputer", "use your computer"),
    ("startHelper", "start a helper"),
    ("createTask", "create the task {subject}"),
    ("askYou", "ask you a question"),
    ("agreeGoal", "agree on a goal for this work"),
    ("useServiceTo", "use {service} to {tool}"),
    ("create", "create {noun}"),
    ("read", "read {noun}"),
    ("list", "list {noun}"),
    ("search", "search {noun}"),
    ("update", "update {noun}"),
    ("delete", "delete {noun}"),
    ("send", "send {noun}"),
    ("run", "run {noun}"),
    ("write", "write {noun}"),
    ("download", "download {noun}"),
    ("upload", "upload {noun}"),
    ("open", "open {noun}"),
    ("stop", "stop {noun}"),
    ("check", "check {noun}"),
    ("notify", "notify {noun}"),
    ("otherAction", "{action} {noun}"),
    ("useTool", "use {name}"),
];

impl Intent {
    /// Nebo's phrase `key` with `values`.
    fn worded(key: &str, values: &[(&str, String)]) -> Self {
        let template = INTENT_PHRASES.iter().find(|(k, _)| *k == key).map(|(_, t)| *t).unwrap_or_default();
        let values: std::collections::BTreeMap<String, String> = values.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        let mut text = template.to_string();
        for (k, v) in &values {
            text = text.replace(&format!("{{{k}}}"), v);
        }
        if key == "shareFiles" && values.get("count").map(String::as_str) == Some("1") {
            text = "share 1 file".to_string();
        }
        Intent { key: Some(format!("decision.wants.{key}")), values, text }
    }

    /// The model's own line for the owner, as it reads after "wants to".
    fn said(line: &str) -> Self {
        Intent { key: None, values: Default::default(), text: lower_first(line.trim().trim_end_matches('.')) }
    }
}

/// "Send the invoice" → "send the invoice"; a word in capitals ("ACME",
/// "iPhone") stays as written.
fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(first), Some(second)) if first.is_uppercase() && !second.is_uppercase() => {
            first.to_lowercase().collect::<String>() + &text[first.len_utf8()..]
        }
        _ => text.to_string(),
    }
}

/// What `tool_name` called with `input` would do: see [`Intent`].
pub fn intent(tool_name: &str, input: &serde_json::Value) -> Intent {
    let s = |key: &str| input.get(key).and_then(|v| v.as_str()).map(str::trim).unwrap_or("").to_string();
    let list = |key: &str| match input.get(key) {
        Some(serde_json::Value::Array(items)) => items.iter().filter_map(|v| v.as_str()).map(str::trim).collect::<Vec<_>>().join(", "),
        _ => s(key),
    };
    let quoted = |v: String| format!("\u{201c}{}\u{201d}", cap(&v, DESCRIPTION_CAP));
    let w = Intent::worded;
    // The model's own line for the owner says it.
    let display = s("display");
    if !display.is_empty() {
        return Intent::said(&cap(&display, DESCRIPTION_CAP));
    }
    if tool_name == "suggest_goal" {
        return w("agreeGoal", &[]);
    }
    if tool_name == "message" {
        return match (crate::message_tool::rule_key(input), s("phone"), s("query")) {
            ("sms_message_send", phone, _) if phone.is_empty() => w("text", &[]),
            ("sms_message_send", phone, _) => w("textTo", &[("phone", phone)]),
            ("sms_search", _, query) if query.is_empty() => w("searchTexts", &[]),
            ("sms_search", _, query) => w("searchTextsFor", &[("query", quoted(query))]),
            ("sms_conversations", ..) => w("listTexts", &[]),
            ("sms_read", phone, _) if phone.is_empty() => w("readTexts", &[]),
            ("sms_read", phone, _) => w("readTextsWith", &[("phone", phone)]),
            _ => w("useTexts", &[]),
        };
    }
    if tool_name == "os" {
        let (action, app) = (s("action"), s("app"));
        match action.as_str() {
            "click" | "double_click" | "right_click" => {
                let target = call(tool_name, input).params.get("target").cloned().unwrap_or_default();
                return if app.is_empty() { w("click", &[("target", target)]) } else { w("clickIn", &[("target", target), ("app", app)]) };
            }
            "screenshot" | "see" | "capture" => {
                return if !app.is_empty() {
                    w("captureApp", &[("app", app)])
                } else if !s("region").is_empty() {
                    w("captureRegion", &[])
                } else {
                    w("captureScreen", &[])
                };
            }
            "activate" | "launch" if !app.is_empty() => return w("openApp", &[("app", app)]),
            _ => {}
        }
        let worded = match (crate::os_tool::OsTool::resolved_resource(input), action.as_str()) {
            ("mail", "send") => Some(match (list("to"), s("subject")) {
                (to, subject) if to.is_empty() && subject.is_empty() => w("email", &[]),
                (to, subject) if to.is_empty() => w("emailAbout", &[("subject", quoted(subject))]),
                (to, subject) if subject.is_empty() => w("emailTo", &[("to", to)]),
                (to, subject) => w("emailToAbout", &[("to", to), ("subject", quoted(subject))]),
            }),
            ("mail", "unread" | "read") => Some(w("readEmail", &[])),
            ("mail", "search") => Some(match s("query") {
                q if q.is_empty() => w("searchEmail", &[]),
                q => w("searchEmailFor", &[("query", quoted(q))]),
            }),
            ("calendar", "create") => Some(match s("title") {
                t if t.is_empty() => w("addEvent", &[]),
                t => w("addEventTitled", &[("title", quoted(t))]),
            }),
            ("calendar", "update") => Some(w("changeEvent", &[])),
            ("calendar", "delete") => Some(w("cancelEvent", &[])),
            ("calendar", _) => Some(w("readCalendar", &[])),
            ("reminders", "create") => Some(match s("title") {
                t if t.is_empty() => w("addReminder", &[]),
                t => w("addReminderTitled", &[("title", quoted(t))]),
            }),
            ("reminders", "complete" | "delete") => Some(w("changeReminders", &[])),
            ("reminders", _) => Some(w("readReminders", &[])),
            ("contacts", _) => Some(match s("query") {
                q if q.is_empty() => w("lookInContacts", &[]),
                q => w("lookUpContact", &[("query", quoted(q))]),
            }),
            _ => None,
        };
        if let Some(worded) = worded {
            return worded;
        }
    }
    let c = call(tool_name, input);
    let p = |key: &str| c.params.get(key).cloned().unwrap_or_default();
    let file = || {
        let path = p("path");
        std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(path)
    };
    let host = || {
        let url = p("url");
        url.split("://").nth(1).unwrap_or(&url).split(['/', '?', '#']).next().unwrap_or_default().to_string()
    };
    match c.kind {
        "read" => w("readFile", &[("file", file())]),
        "write" => w("writeFile", &[("file", file())]),
        "edit" => w("editFile", &[("file", file())]),
        "plan" => w("writePlan", &[("file", file())]),
        "convert" => w("convertFile", &[("file", file())]),
        "share" if c.params.contains_key("path") => w("shareFile", &[("file", file())]),
        "share" => w("shareFiles", &[("count", p("count"))]),
        "command" => match p("reach").as_str() {
            "screen" => w("seeScreen", &[]),
            "microphone" => w("hearMicrophone", &[]),
            "camera" => w("useCamera", &[]),
            "input" => w("controlInput", &[]),
            "app" if !p("app").is_empty() => w("controlApp", &[("app", p("app"))]),
            "app" => w("controlApps", &[]),
            _ => w("runCommand", &[("command", cap(&p("command").split_whitespace().collect::<Vec<_>>().join(" "), 72))]),
        },
        "search" => w("searchWeb", &[("query", quoted(p("query")))]),
        "research" => w("research", &[("query", quoted(p("query")))]),
        "fetch" => w("openUrl", &[("url", p("url"))]),
        "request" if !p("method").is_empty() => w("requestMethod", &[("method", p("method").to_uppercase()), ("host", host())]),
        "request" => w("request", &[("host", host())]),
        "browser" if !p("url").is_empty() => w("useBrowserOn", &[("url", p("url"))]),
        "browser" => w("useBrowser", &[]),
        "computer" => w("useComputer", &[]),
        "helper" => w("startHelper", &[]),
        "task" => w("createTask", &[("subject", quoted(p("subject")))]),
        "ask" => w("askYou", &[]),
        "mcp" => w("useServiceTo", &[("service", p("service")), ("tool", p("tool"))]),
        "action" if !p("verb").is_empty() => w(&p("verb"), &[("noun", p("noun"))]),
        "action" => w("otherAction", &[("action", p("action")), ("noun", p("noun"))]),
        _ => w("useTool", &[("name", p("name"))]),
    }
}

#[cfg(test)]
mod tests {
    use super::{call, cap, cap_words, description_key, numbered, sentence, shown_titles, sibling_titles, title_key, unmarked, DESCRIPTION_CAP, TITLE_CAP, service_name as humanize_slug, strap_verb};
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
        let c = call("ask_owner", &json!({"questions": [{"header": "Vendor", "question": "Which vendor?"}, {"header": "Start", "question": "When?"}]}));
        assert_eq!((c.kind, params(&c)), ("ask", vec![("question", "Which vendor?")]));
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

    /// A title is cut between words, never inside one.
    #[test]
    fn titles_are_cut_between_words() {
        assert_eq!(cap_words("What concrete dated evidence exists between July 22, 2026 and now", 47), "What concrete dated evidence exists between…");
        assert_eq!(cap_words("between July 22, 2026 and now", 13), "between July…", "a cut at a space keeps the word before it");
        assert_eq!(cap_words("between July 22, 2026 and now", 12), "between July…");
        assert_eq!(cap_words("between July 22, 2026 and now", 16), "between July 22…", "no comma before the ellipsis");
        assert_eq!(cap_words("Phoenix rents 2026", 60), "Phoenix rents 2026");
        assert_eq!(cap_words("請求書を確認", 3), "請求書…");
        assert_eq!(cap_words("ends cut short…", 60), "ends cut short…", "a text's own ellipsis is no cut");
    }

    /// A research run started the way `deep_research` starts one.
    fn research(question: &str) -> (String, String) {
        (format!("research: {}", cap_words(question, DESCRIPTION_CAP)), question.to_string())
    }

    /// The titles of siblings started together, in order, as the helper
    /// registry gives them: each numbered as it starts, the least number no
    /// earlier one with its description has.
    fn titles(siblings: &[(String, String)]) -> Vec<String> {
        let bases = sibling_titles(&siblings.iter().map(|(d, b)| (d.as_str(), b.as_str())).collect::<Vec<_>>());
        let mut used: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        let numbers: Vec<u32> = siblings
            .iter()
            .map(|(d, _)| {
                let n = used.entry(description_key(d)).or_default();
                *n += 1;
                *n
            })
            .collect();
        let shown: Vec<(&str, u32, u64)> = bases.iter().zip(&numbers).enumerate().map(|(k, (b, n))| (b.as_str(), *n, k as u64)).collect();
        shown_titles(&shown)
    }

    const OPENING: &str = "What concrete dated evidence exists between July 22, 2026 and October 1, 2026 th";

    /// Eight research runs whose questions share their first 80 characters
    /// each get a title of their own, from where the question parts from
    /// the others, cut between words.
    #[test]
    fn eight_alike_research_runs_get_eight_titles() {
        assert_eq!(OPENING.chars().count(), 80);
        let subjects = [
            "at Nvidia's data-center revenue kept growing?",
            "at Apple shipped a foldable iPhone?",
            "at mortgage rates fell below six percent?",
            "at the Fed cut rates twice?",
            "at EU AI Act enforcement began?",
            "at Phoenix rents went down?",
            "at remote work grew among lawyers?",
            "at solar installs outpaced gas?",
        ];
        let runs: Vec<_> = subjects.iter().map(|s| research(&format!("{OPENING}{s}"))).collect();
        assert!(runs.windows(2).all(|w| w[0].0 == w[1].0), "the descriptions alone are all alike");
        let t = titles(&runs);
        assert_eq!(t.iter().collect::<std::collections::HashSet<_>>().len(), 8, "{t:#?}");
        assert_eq!(t[0], "research: …Nvidia's data-center revenue kept growing?");
        assert!(t.iter().all(|t| t.chars().count() <= TITLE_CAP), "{t:#?}");
    }

    /// A lone run keeps its natural title, cut between words.
    #[test]
    fn a_lone_run_keeps_its_natural_title() {
        let q = "What concrete dated evidence exists between July 22, 2026 and October 1, 2026 that rents fell?";
        assert_eq!(titles(&[research(q)]), ["research: What concrete dated evidence exists between July 22, 2026…"]);
    }

    /// A third sibling never moves the others: each title starts after the
    /// opening every alike sibling shares, so "Nvidia" stays in A's.
    #[test]
    fn a_third_sibling_never_drops_the_subject() {
        let a = research(&format!("{OPENING}at Nvidia revenue grew in Q3?"));
        let b = research(&format!("{OPENING}at Apple shipped a foldable?"));
        let c = research(&format!("{OPENING}at Nvidia revenue fell in Q4?"));
        let two = titles(&[a.clone(), b.clone()]);
        let three = titles(&[a, b, c]);
        assert_eq!(two[0], "research: …Nvidia revenue grew in Q3?");
        assert_eq!(three[0], two[0], "A keeps its title as C starts");
        assert_eq!(three[2], "research: …Nvidia revenue fell in Q4?");
    }

    /// When the sibling it was told apart from ends, a run reads as it did
    /// alone again.
    #[test]
    fn a_title_reverts_when_its_sibling_ends() {
        let a = research(&format!("{OPENING}at Nvidia revenue grew in Q3?"));
        let b = research(&format!("{OPENING}at Apple shipped a foldable?"));
        assert_ne!(titles(&[a.clone(), b])[0], a.0);
        assert_eq!(titles(&[a.clone()])[0], a.0);
    }

    /// A description the model wrote is kept, and the part of the brief
    /// that differs follows it.
    #[test]
    fn a_written_description_is_kept() {
        let ohio = ("map rivals".to_string(), "List rivals in Ohio".to_string());
        let iowa = ("map rivals".to_string(), "List rivals in Iowa".to_string());
        assert_eq!(titles(&[ohio, iowa]), ["map rivals — …Ohio", "map rivals — …Iowa"]);
        let apart = [("map rival 1".to_string(), "x".to_string()), ("map rival 2".to_string(), "x".to_string())];
        assert_eq!(titles(&apart), ["map rival 1", "map rival 2"], "descriptions that differ are kept as they are");
    }

    /// `titles` that must finish within a few seconds: a title that never
    /// settles fails the test rather than stalling the run.
    fn titles_in_time(siblings: Vec<(String, String)>) -> Vec<String> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(titles(&siblings));
        });
        rx.recv_timeout(std::time::Duration::from_secs(5)).expect("titles in time")
    }

    fn assert_distinct(t: &[String]) {
        let set: std::collections::HashSet<_> = t.iter().collect();
        assert_eq!(set.len(), t.len(), "{t:#?}");
    }

    /// Two questions that share 150 characters and a third that parts
    /// early: the two read apart from the word they part at, and the third
    /// keeps its subject.
    #[test]
    fn a_long_shared_opening_never_makes_two_titles_alike() {
        let long = "at Nvidia's data-center revenue in the third fiscal quarter of the year grew faster than analysts expected according to filings from";
        let a = research(&format!("{OPENING}{long} the company itself?"));
        let b = research(&format!("{OPENING}{long} its largest customers?"));
        let c = research(&format!("{OPENING}at Apple shipped a foldable?"));
        assert!(format!("{OPENING}{long}").chars().count() > 150);
        let t = titles_in_time(vec![a, b, c]);
        assert_eq!(t[0], "research: …the company itself?");
        assert_eq!(t[1], "research: …its largest customers?");
        assert_eq!(t[2], "research: …Apple shipped a foldable?");
    }

    /// The reviewer's cases that once never settled: a word too long for
    /// the room left beside alike siblings, under a written description and
    /// under a research one.
    #[test]
    fn a_word_too_long_for_the_title_always_settles() {
        let url = format!("https://example.com/{}", "q".repeat(65));
        assert_eq!(url.chars().count(), 85);
        let cases = [
            vec![format!("Summarize {url}"), format!("Summarize the {url} risks"), format!("Summarize the {url}")],
            vec![format!("Summarize the {url} risks"), format!("Summarize the {url} costs"), format!("Summarize the {url}")],
            vec![format!("{url} risks"), format!("{url}"), format!("{url} costs and risks")],
        ];
        for briefs in cases {
            for research_kind in [false, true] {
                let set: Vec<(String, String)> = briefs.iter().map(|b| if research_kind { research(b) } else { ("read report".to_string(), b.clone()) }).collect();
                let t = titles_in_time(set);
                assert_distinct(&t);
                assert!(t.iter().all(|t| t.chars().count() <= TITLE_CAP), "{t:#?}");
            }
        }
    }

    /// A single word of 70 or 90 characters, under both kinds of
    /// description, is cut with "…" and the titles still differ.
    #[test]
    fn single_long_words_are_cut_and_told_apart() {
        for len in [70, 90] {
            let a = format!("{}a", "w".repeat(len - 1));
            let b = format!("{}b", "w".repeat(len - 1));
            for research_kind in [false, true] {
                let set: Vec<(String, String)> = [&a, &b].iter().map(|q| if research_kind { research(q) } else { ("read words".to_string(), q.to_string()) }).collect();
                let t = titles_in_time(set);
                assert_distinct(&t);
            }
        }
    }

    /// Text in any script is cut on character boundaries; a question in two
    /// Unicode forms is the same question.
    #[test]
    fn cjk_emoji_and_combining_characters_are_safe() {
        let cjk = [("調べる".to_string(), "東京の家賃は下がったか".to_string()), ("調べる".to_string(), "大阪の家賃は下がったか".to_string())];
        assert_distinct(&titles_in_time(cjk.to_vec()));
        let emoji = [("plan".to_string(), "plan the 🎉👨‍👩‍👧‍👦 party".to_string()), ("plan".to_string(), "plan the 🎂👨‍👩‍👧‍👦 party".to_string())];
        let t = titles_in_time(emoji.to_vec());
        assert_distinct(&t);
        assert_eq!(t[0], "plan — …🎉👨‍👩‍👧‍👦 party");
        let composed = ("check café".to_string(), "Is the caf\u{e9} open".to_string());
        let decomposed = ("check cafe\u{301}".to_string(), "Is the cafe\u{301} open".to_string());
        let t = titles_in_time(vec![composed, decomposed]);
        assert_eq!(t, ["check café · 1", "check cafe\u{301} · 2"], "one question in two forms; each keeps its own description");
    }

    /// Descriptions that differ only in case are alike; each title keeps
    /// its own description's case.
    #[test]
    fn descriptions_alike_but_for_case_are_siblings() {
        let t = titles_in_time(vec![("Map Rivals".to_string(), "List rivals in Ohio".to_string()), ("map rivals".to_string(), "List rivals in Iowa".to_string())]);
        assert_eq!(t, ["Map Rivals — …Ohio", "map rivals — …Iowa"]);
    }

    /// A brief another one opens with shows its own last words, never a
    /// dangling cut.
    #[test]
    fn a_brief_another_opens_with_shows_its_end() {
        let t = titles_in_time(vec![
            ("check rents".to_string(), "Find out whether rents fell in Phoenix".to_string()),
            ("check rents".to_string(), "Find out whether rents fell in Phoenix and Tucson".to_string()),
        ]);
        assert_eq!(t, ["check rents — Find out whether rents fell in Phoenix", "check rents — …and Tucson"]);
    }

    /// Up to ten siblings with long random words, 2,000 times over, under
    /// both kinds of description: every set settles in time and no two
    /// titles in it match.
    #[test]
    fn titles_are_distinct_and_settle_for_any_siblings() {
        let mut seed = 7u64;
        let mut next = move || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as usize
        };
        let started = std::time::Instant::now();
        for round in 0..2000 {
            let vocab: Vec<String> = (0..6).map(|_| format!("{}{}", "x".repeat(1 + next() % 100), next() % 3)).collect();
            let n = 2 + next() % 9;
            let base: Vec<String> = (0..12).map(|_| vocab[next() % vocab.len()].clone()).collect();
            let briefs: Vec<String> = (0..n)
                .map(|_| {
                    let mut q = base.clone();
                    for _ in 0..next() % 3 {
                        let at = next() % q.len();
                        q[at] = vocab[next() % vocab.len()].clone();
                    }
                    q.truncate(1 + next() % 12);
                    q.join(" ")
                })
                .collect();
            let research_kind = round % 2 == 0;
            let set: Vec<(String, String)> = briefs.iter().map(|b| if research_kind { research(b) } else { ("map rents".to_string(), b.clone()) }).collect();
            assert_distinct(&titles_in_time(set));
        }
        // A generous ceiling for a loaded machine; a hang trips the
        // per-set timeout first.
        assert!(started.elapsed() < std::time::Duration::from_secs(120), "{:?}", started.elapsed());
    }

    /// Siblings asked the very same thing are numbered after the first:
    /// no two titles ever match.
    #[test]
    fn identical_questions_are_numbered() {
        let q = research(&format!("{OPENING}at rents fell?"));
        assert_eq!(titles(&[q.clone(), q.clone(), q.clone()]), [format!("{} · 1", q.0), format!("{} · 2", q.0), format!("{} · 3", q.0)]);
    }

    /// Descriptions that differ only in case, asked the very same, are told
    /// apart by their numbers.
    #[test]
    fn case_only_twins_are_numbered() {
        let t = titles(&[("Map Rivals".to_string(), "List rivals in Ohio".to_string()), ("map rivals".to_string(), "List rivals in Ohio".to_string())]);
        assert_eq!(t, ["Map Rivals · 1", "map rivals · 2"]);
    }

    /// The whole title, number included, stays within the cap: the base is
    /// cut, never the number; a text that ends in "(2)" itself reads plainly.
    #[test]
    fn the_whole_title_stays_within_the_cap() {
        let long = "w ".repeat(120);
        for n in [1, 2, 9, 10, 123, 4567] {
            let t = numbered(&long, &n.to_string());
            assert!(t.chars().count() <= TITLE_CAP, "{n}: {t}");
            assert!(t.ends_with(&format!(" · {n}")), "{t}");
        }
        let odd = [("check (2)".to_string(), "Check the rents (2)".to_string()), ("check (2)".to_string(), "Check the rents (2)".to_string())];
        assert_eq!(titles(&odd), ["check (2) · 1", "check (2) · 2"]);
        let odd = [("check".to_string(), "Check the rents (2)".to_string()), ("check".to_string(), "Check the rents (3)".to_string())];
        assert_eq!(titles(&odd), ["check — …(2)", "check — …(3)"]);
        let t = titles(&[research(&format!("{OPENING}{}", "at the rents fell ".repeat(10))), research(&format!("{OPENING}{}x", "at the rents fell ".repeat(10)))]);
        assert!(t.iter().all(|t| t.chars().count() <= TITLE_CAP), "{t:#?}");
    }

    /// Numbering two thousand siblings asked the same is quick.
    #[test]
    fn numbering_many_twins_is_quick() {
        let q = ("check".to_string(), "Check rents".to_string());
        let started = std::time::Instant::now();
        let t = titles(&vec![q; 2000]);
        assert_eq!(t[1999], "check · 2000");
        // A generous ceiling: a quadratic numbering took seconds here.
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "{:?}", started.elapsed());
    }

    /// A description that already reads like a marked title is taken as its
    /// plain words, so no mark added to another can read the same as it.
    #[test]
    fn descriptions_that_look_marked_never_collide() {
        assert_eq!(unmarked("x · 1"), "x");
        assert_eq!(unmarked("x · 2 · #3 "), "x");
        assert_eq!(unmarked("x · v2"), "x · v2");
        assert_eq!(unmarked("· 2"), "· 2");
        let probe = ["x · 1", "x", "x", "x · #2"];
        let shown = shown_titles(&probe.iter().enumerate().map(|(k, d)| (*d, 1 + (k as u32 == 2) as u32, k as u64)).collect::<Vec<_>>());
        let keys: std::collections::HashSet<String> = shown.iter().map(|t| title_key(t)).collect();
        assert_eq!(keys.len(), shown.len(), "{shown:#?}");
        let t = titles(&probe.iter().map(|d| (d.to_string(), "Scan the logs".to_string())).collect::<Vec<_>>());
        assert_eq!(t, ["x · 1", "x · 2", "x · 3", "x · 4"], "one description, four numbers");
        assert_eq!(shown_titles(&[("x", 0, 0), ("x", 0, 1)]), ["x · #1", "x · #2"], "no number could be given");
    }

    /// Helpers start, end and resume in any order under one conversation,
    /// numbered the way the registry numbers them (as each first starts,
    /// the least number its description never had, kept for good): at
    /// every step the running ones' titles all differ, and no helper's
    /// number ever changes.
    #[test]
    fn titles_stay_distinct_through_starts_ends_and_resumes() {
        let mut seed = 11u64;
        let mut next = move || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as usize
        };
        let descriptions = ["scan logs", "Scan Logs", "scan logs · 1", "scan logs · #2", "scan logs · 2 · #3", "research: rents", "research: What concrete dated evidence exists…"];
        let long = "x ".repeat(60);
        let briefs = ["p", "q", "What concrete dated evidence exists for rents", "What concrete dated evidence exists for wages", long.as_str()];
        for round in 0..300 {
            // (description, brief, number, seq, running)
            let mut helpers: Vec<(String, String, u32, u64, bool)> = Vec::new();
            let mut given: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
            let mut numbers_seen: std::collections::HashMap<usize, u32> = std::collections::HashMap::new();
            for step in 0..40 {
                match next() % 3 {
                    0 => {
                        let d = descriptions[next() % descriptions.len()].to_string();
                        let b = briefs[next() % briefs.len()].to_string();
                        let n = given.entry(description_key(&d)).or_default();
                        *n += 1;
                        let seq = helpers.len() as u64;
                        helpers.push((d, b, *n, seq, true));
                    }
                    1 if !helpers.is_empty() => {
                        let k = next() % helpers.len();
                        helpers[k].4 = false;
                    }
                    _ if !helpers.is_empty() => {
                        let k = next() % helpers.len();
                        helpers[k].4 = true;
                    }
                    _ => {}
                }
                let running: Vec<usize> = (0..helpers.len()).filter(|&k| helpers[k].4).collect();
                let bases = sibling_titles(&running.iter().map(|&k| (helpers[k].0.as_str(), helpers[k].1.as_str())).collect::<Vec<_>>());
                let shown = shown_titles(&running.iter().zip(&bases).map(|(&k, b)| (b.as_str(), helpers[k].2, helpers[k].3)).collect::<Vec<_>>());
                let keys: std::collections::HashSet<String> = shown.iter().map(|t| title_key(t)).collect();
                assert_eq!(keys.len(), shown.len(), "round {round} step {step}: {shown:#?}");
                assert!(shown.iter().all(|t| t.chars().count() <= TITLE_CAP), "{shown:#?}");
                for &k in &running {
                    assert_eq!(*numbers_seen.entry(k).or_insert(helpers[k].2), helpers[k].2);
                }
            }
        }
    }
    /// 1b (owner, 10-24): the decision card says one sentence, "Bookkeeper
    /// wants to send a text to +13855550100", the key words bold. What a
    /// call would do reads after "wants to", lowercase, in Nebo's words by a
    /// key the apps hold in every language, or in the model's own line.
    #[test]
    fn a_call_says_what_it_would_do_after_wants_to() {
        let said = |tool: &str, input: serde_json::Value| {
            let i = super::intent(tool, &input);
            (i.key.unwrap_or_default(), i.text)
        };
        let cases = [
            (said("message", json!({"resource": "sms", "action": "send", "phone": "+13855550100", "text": "On my way"})), ("decision.wants.textTo", "send a text to +13855550100")),
            (said("message", json!({"action": "search", "query": "invoice"})), ("decision.wants.searchTextsFor", "search your texts for \u{201c}invoice\u{201d}")),
            (
                said("os", json!({"resource": "mail", "action": "send", "to": ["dana@acme.com"], "subject": "Q3 quote"})),
                ("decision.wants.emailToAbout", "send an email to dana@acme.com about \u{201c}Q3 quote\u{201d}"),
            ),
            (said("os", json!({"resource": "calendar", "action": "create", "title": "Site visit"})), ("decision.wants.addEventTitled", "add \u{201c}Site visit\u{201d} to your calendar")),
            (said("os", json!({"action": "screenshot"})), ("decision.wants.captureScreen", "capture your screen")),
            (said("run_command", json!({"command": "rm -rf ~/old", "description": "Removes the old folder"})), ("decision.wants.runCommand", "run rm -rf ~/old")),
            (said("run_command", json!({"command": "screencapture /tmp/s.png"})), ("decision.wants.seeScreen", "see your screen")),
            (said("write_file", json!({"path": "/Users/me/notes.md", "content": "x"})), ("decision.wants.writeFile", "write notes.md")),
            (said("http_request", json!({"method": "post", "url": "https://api.example.com/v1?x=1"})), ("decision.wants.requestMethod", "send a POST request to api.example.com")),
            (said("mcp__github__create_issue", json!({"title": "Login fails"})), ("decision.wants.useServiceTo", "use Github to create issue")),
            (said("os", json!({"resource": "notes", "action": "delete", "name": "Old"})), ("decision.wants.delete", "delete notes")),
            (said("send_invoice", json!({})), ("decision.wants.useTool", "use send invoice")),
            (said("share_file", json!({"paths": ["/a.md"]})), ("decision.wants.shareFile", "share a.md")),
            // The model's own line for the owner: its words, no key.
            (said("quickbooks", json!({"command": "invoice send", "display": "Send invoice 1041 to ACME."})), ("", "send invoice 1041 to ACME")),
        ];
        for ((key, text), (want_key, want_text)) in &cases {
            assert_eq!((key.as_str(), text.as_str()), (*want_key, *want_text));
        }
        assert_eq!(super::intent("share_file", &json!({"paths": ["/a", "/b"]})).text, "share 2 files");
        assert_eq!(super::intent("x", &json!({"display": "ACME gets paid"})).text, "ACME gets paid", "a word in capitals stays");
    }

    /// Every phrase is in the apps' English under its key, in the same
    /// words, so the owner's language and the English fallback agree.
    #[test]
    fn the_apps_hold_every_phrase() {
        let en: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../app/src/lib/i18n/locales/en.json")).unwrap()).unwrap();
        for (key, english) in super::INTENT_PHRASES {
            let held = en["decision"]["wants"][key].as_str().unwrap_or_else(|| panic!("decision.wants.{key} is missing"));
            if *key == "shareFiles" {
                assert!(held.contains("plural"), "{held}");
            } else {
                assert_eq!(held, *english, "decision.wants.{key}");
            }
        }
    }
}
