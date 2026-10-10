//! The tools a workflow step a builder writes is given. A declared step
//! (`tools` present) runs with only its tools (`workflow::engine::enforced_tools`),
//! and a call outside them pauses the run for the owner. So when an
//! employee or a workflow is saved from a conversation, Nebo completes each
//! declared step from what it already knows: the tools the step's own words
//! name, and the employee's memory, which is how a run keeps state for the
//! next one. A call the step's words write with a parameter its tool
//! doesn't take is refused before anything is saved.
//!
//! Bake-off 2026-10-10: 66 of 69 runs of employees built in chat paused for
//! the owner on a tool the builder left out (find_tools, recall, remember);
//! steps without memory hunted for their state in made-up files and Nebo's
//! own database; and a step that said `recall(key '…')` failed 51 recalls in
//! one run, because recall takes `query`.

use std::collections::HashMap;

use ai::ToolDefinition;
use serde_json::Value;

/// What every declared step a builder writes is given beyond its list: the
/// employee's own memory, to keep what later runs need.
pub const STATE_TOOLS: [&str; 2] = ["recall", "remember"];

/// Whether a `tools` entry names tool `n`: its exact name, a dotted prefix
/// (`"odoo"` covers every `odoo.*` tool) or a `prefix*` family
/// (`"mcp__github__*"` covers that server's tools).
pub fn names_tool(entry: &str, n: &str) -> bool {
    n == entry
        || n.strip_prefix(entry).is_some_and(|r| r.starts_with('.'))
        || entry.strip_suffix('*').is_some_and(|p| !p.is_empty() && n.starts_with(p))
}

/// Complete a declared activity's `tools` and check the calls its words
/// write. An activity without `tools` is left as it is: it isn't limited.
///
/// Added, unless an entry already names them: [`STATE_TOOLS`], and every
/// tool in `defs` the activity's intent or steps name, as a call
/// (`name(`) or, for a name no prose word could be (one with an
/// underscore), as a word. Returns the names added, in order.
///
/// Refused: a call in the words, `name(param: …)`, whose parameter tool
/// `name` doesn't take. The error says the step, the call and the
/// parameters the tool does take.
pub fn complete(activity: &mut Value, defs: &HashMap<String, ToolDefinition>) -> Result<Vec<String>, String> {
    let Some(declared) = activity.get("tools").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut entries: Vec<String> = declared.iter().filter_map(|t| t.as_str().map(str::to_string)).collect();
    let words = activity_words(activity);
    for (place, text) in &words {
        check_calls(place, text, defs)?;
    }
    let mut wanted: Vec<&str> = STATE_TOOLS.to_vec();
    for (_, text) in &words {
        wanted.extend(named_tools(text, defs));
    }
    let mut added = Vec::new();
    for name in wanted {
        if !entries.iter().any(|e| names_tool(e, name)) {
            entries.push(name.to_string());
            added.push(name.to_string());
        }
    }
    activity["tools"] = Value::from(entries);
    Ok(added)
}

/// The activity's own words, each with where it is: its intent and each
/// of its steps.
fn activity_words(activity: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(intent) = activity.get("intent").and_then(Value::as_str) {
        out.push(("its task".to_string(), intent.to_string()));
    }
    for (i, step) in activity.get("steps").and_then(Value::as_array).into_iter().flatten().enumerate() {
        if let Some(text) = step.as_str() {
            out.push((format!("step {}", i + 1), text.to_string()));
        }
    }
    out
}

/// The tools in `defs` that `text` names: as a call, or as a word when the
/// name has an underscore.
fn named_tools<'d>(text: &str, defs: &'d HashMap<String, ToolDefinition>) -> Vec<&'d str> {
    let mut out: Vec<&'d str> = Vec::new();
    for (at, word) in words_of(text) {
        let Some((name, _)) = defs.get_key_value(word) else { continue };
        let called = text[at + word.len()..].starts_with('(');
        if (called || word.contains('_')) && !out.contains(&name.as_str()) {
            out.push(name.as_str());
        }
    }
    out
}

/// Each run of tool-name characters (lowercase letters, digits, `_`) in
/// `text`, with where it starts.
fn words_of(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let is_name = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_';
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in text.char_indices() {
        match (is_name(c), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                out.push((s, &text[s..i]));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((s, &text[s..]));
    }
    out.into_iter()
}

/// Refuse a call in `text` to a tool in `defs` with a parameter it doesn't
/// take. A call's parameters are the words before `:`, `=` or a quote at
/// the start of each of its arguments (`recall(key: "x")`,
/// `recall(key 'x')`); prose inside the parentheses names none.
fn check_calls(place: &str, text: &str, defs: &HashMap<String, ToolDefinition>) -> Result<(), String> {
    for (at, word) in words_of(text) {
        let Some(def) = defs.get(word) else { continue };
        let Some(args) = text[at + word.len()..].strip_prefix('(') else { continue };
        let Some(params) = def.input_schema.get("properties").and_then(Value::as_object) else { continue };
        let args = args.split(')').next().unwrap_or("");
        for arg in args.split(',') {
            let arg = arg.trim_start();
            let name_len = arg.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(arg.len());
            let (param, rest) = arg.split_at(name_len);
            let rest = rest.trim_start();
            let names_param = !param.is_empty()
                && rest.starts_with([':', '=', '"', '\'', '\u{2018}', '\u{201c}']);
            if names_param && !params.contains_key(param) {
                let mut takes: Vec<&str> = params.keys().map(String::as_str).collect();
                takes.sort_unstable();
                return Err(format!(
                    "{place} writes {word}({param} …), but {word} has no parameter `{param}`; it takes: {}. \
                     Write the call with its own parameters.",
                    takes.join(", ")
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn def(name: &str, params: &[&str]) -> (String, ToolDefinition) {
        let properties: serde_json::Map<String, Value> =
            params.iter().map(|p| (p.to_string(), json!({"type": "string"}))).collect();
        (
            name.to_string(),
            ToolDefinition {
                name: name.to_string(),
                description: String::new(),
                input_schema: json!({"type": "object", "properties": properties}),
            },
        )
    }

    fn defs() -> HashMap<String, ToolDefinition> {
        [
            def("recall", &["query"]),
            def("remember", &["key", "value", "scope"]),
            def("write_file", &["path", "content"]),
            def("message", &["resource", "action", "text"]),
            def("message_owner", &["text"]),
            def("mcp__hubspot__hubspot_search_deals", &["query", "limit"]),
            def("mcp__hubspot__hubspot_get_deal", &["deal_id"]),
        ]
        .into_iter()
        .collect()
    }

    /// Bake-off 2026-10-10: the builders wrote steps without the memory
    /// tools, and runs paused for the owner on recall and remember.
    #[test]
    fn a_declared_step_gets_its_memory() {
        let mut activity = json!({
            "id": "run",
            "intent": "Evening wrap",
            "steps": ["Pull today's deals with mcp__hubspot__hubspot_search_deals.", "Send the wrap with message_owner."],
            "tools": ["mcp__hubspot__hubspot_search_deals", "message_owner"]
        });
        let added = complete(&mut activity, &defs()).unwrap();
        assert_eq!(added, ["recall", "remember"]);
        assert_eq!(
            activity["tools"],
            json!(["mcp__hubspot__hubspot_search_deals", "message_owner", "recall", "remember"])
        );
    }

    /// The tools a step's words name are its tools: as a call, or as a word
    /// no prose has. "Message the owner" names no tool.
    #[test]
    fn the_tools_a_step_names_are_added() {
        let mut activity = json!({
            "id": "run",
            "intent": "Message the owner about stalled deals.",
            "steps": [
                "Read each stalled deal with mcp__hubspot__hubspot_get_deal(deal_id: \"…\").",
                "Save the list to a file with write_file, then message_owner."
            ],
            "tools": ["mcp__hubspot__*"]
        });
        let added = complete(&mut activity, &defs()).unwrap();
        assert_eq!(added, ["recall", "remember", "write_file", "message_owner"], "the family covers the HubSpot call; prose `message` adds nothing");
    }

    /// An entry that already names a tool (exactly, dotted or a family) is
    /// enough: nothing is added twice.
    #[test]
    fn nothing_is_added_twice() {
        let mut activity = json!({"id": "a", "intent": "recall(query: \"watchlist\")", "tools": ["recall", "remember"]});
        assert!(complete(&mut activity, &defs()).unwrap().is_empty());
        assert_eq!(activity["tools"], json!(["recall", "remember"]));
    }

    /// An undeclared activity isn't limited, so nothing is added to it.
    #[test]
    fn an_undeclared_step_is_left_alone() {
        let mut activity = json!({"id": "a", "intent": "Use write_file."});
        assert!(complete(&mut activity, &defs()).unwrap().is_empty());
        assert!(activity.get("tools").is_none());
    }

    /// Bake-off 2026-10-10 (B13): a step said `recall(key '…')`; recall
    /// takes `query`, and the run failed 51 recalls. The save is refused
    /// with what the tool takes.
    #[test]
    fn a_call_with_a_parameter_its_tool_lacks_is_refused() {
        for written in ["recall(key 'watchlist/accounts')", "recall(key: \"watchlist\")", "recall(key=\"x\")"] {
            let mut activity = json!({"id": "a", "intent": "Watch the list.", "steps": [format!("Read the list with {written}.")], "tools": []});
            let err = complete(&mut activity, &defs()).unwrap_err();
            assert_eq!(
                err,
                "step 1 writes recall(key …), but recall has no parameter `key`; it takes: query. Write the call with its own parameters."
            );
        }
        let mut fine = json!({"id": "a", "intent": "x", "steps": ["recall(query: \"watchlist\") and search deals (limit 100) with mcp__hubspot__hubspot_search_deals(limit: 100)"], "tools": []});
        assert!(complete(&mut fine, &defs()).is_ok());
        let mut prose = json!({"id": "a", "intent": "x", "steps": ["message_owner(the summary, in two lines)"], "tools": []});
        assert!(complete(&mut prose, &defs()).is_ok(), "prose in the parentheses names no parameter");
    }

    #[test]
    fn a_family_or_dotted_entry_names_its_tools() {
        assert!(names_tool("mcp__github__*", "mcp__github__create_issue"));
        assert!(names_tool("odoo", "odoo.invoice.create"));
        assert!(!names_tool("odoo", "odoo_extra"));
        assert!(!names_tool("*", "anything"));
    }
}
