//! The tools a workflow step a builder writes is given. A declared step
//! (`tools` present) runs with its tools (`workflow::engine::enforced_tools`)
//! and the set every step has ([`ALWAYS_ALLOWED`]). So when an employee or
//! a workflow is saved from a conversation, Nebo completes each declared
//! step from the tools the step's own words name
//! (`napp::workflow_check::named_tools`, the audit's reading of them). A
//! call the step's words write with a parameter its tool doesn't take is
//! refused before anything is saved.
//!
//! Bake-off 2026-10-10: 66 of 69 runs of employees built in chat paused for
//! the owner on a tool the builder left out (find_tools, recall, remember);
//! steps without memory hunted for their state in made-up files and Nebo's
//! own database; and a step that said `recall(key '…')` failed 51 recalls in
//! one run, because recall takes `query`.

use std::collections::HashMap;

use ai::ToolDefinition;
use napp::workflow_check::{name_words, named_tools, names_tool};
use serde_json::Value;

/// What every workflow step may call without declaring it: looking tools
/// up, the employee's memory, and Nebo's own read tools. A run never pauses
/// for one of these, and the audit never needs to add them.
pub const ALWAYS_ALLOWED: [&str; 8] = [
    "find_tools",
    "recall",
    "remember",
    "read_file",
    "read_session",
    "search_history",
    "read_skill_file",
    "use_skill",
];

/// Complete a declared activity's `tools` and check the calls its words
/// write. An activity without `tools` is left as it is: it isn't limited;
/// nor is one with `tools: []`, a judging step that calls nothing.
///
/// Added, unless an entry already names them: every tool in `defs` the
/// activity's intent or steps name (`napp::workflow_check::named_tools`).
/// Returns the names added, in order.
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
    // `tools: []` is a judging step: one call, no tools, safe to run many at
    // once (`workflows::authoring`). It stays exactly that.
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    let known = |n: &str| defs.contains_key(n);
    let mut added: Vec<String> = Vec::new();
    for (_, text) in &words {
        for name in named_tools(text, &known) {
            if !entries.iter().any(|e| names_tool(e, &name)) {
                entries.push(name.clone());
                added.push(name);
            }
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

/// Refuse a call in `text` to a tool in `defs` with a parameter it doesn't
/// take. A call's parameters are the words before `:`, `=` or a quote at
/// the start of each of its arguments (`recall(key: "x")`,
/// `recall(key 'x')`); prose inside the parentheses names none.
fn check_calls(place: &str, text: &str, defs: &HashMap<String, ToolDefinition>) -> Result<(), String> {
    for (at, word) in name_words(text) {
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
    /// tools, and runs paused for the owner on recall and remember. Memory
    /// is always allowed now, so a declaration gains nothing for it.
    #[test]
    fn memory_is_always_allowed_not_declared() {
        let mut activity = json!({
            "id": "run",
            "intent": "Evening wrap",
            "steps": ["Pull today's deals with mcp__hubspot__hubspot_search_deals.", "Send the wrap with message_owner."],
            "tools": ["mcp__hubspot__hubspot_search_deals", "message_owner"]
        });
        assert!(complete(&mut activity, &defs()).unwrap().is_empty());
        assert!(ALWAYS_ALLOWED.contains(&"recall") && ALWAYS_ALLOWED.contains(&"remember") && ALWAYS_ALLOWED.contains(&"find_tools"));
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
        assert_eq!(added, ["write_file", "message_owner"], "the family covers the HubSpot call; prose `message` adds nothing");
    }

    /// An entry that already names a tool (exactly, dotted or a family) is
    /// enough: nothing is added twice.
    #[test]
    fn nothing_is_added_twice() {
        let mut activity = json!({"id": "a", "intent": "recall(query: \"watchlist\")", "tools": ["recall", "remember"]});
        assert!(complete(&mut activity, &defs()).unwrap().is_empty());
        assert_eq!(activity["tools"], json!(["recall", "remember"]));
    }

    /// `tools: []` is a judging step (one call, nothing to call): it gets
    /// no tools, memory included.
    #[test]
    fn a_judging_step_stays_without_tools() {
        let mut activity = json!({"id": "judge", "intent": "Decide which deals are stalled.", "steps": ["Save nothing; write_file is for later steps."], "tools": []});
        assert!(complete(&mut activity, &defs()).unwrap().is_empty());
        assert_eq!(activity["tools"], json!([]));
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
