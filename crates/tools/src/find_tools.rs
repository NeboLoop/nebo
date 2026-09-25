//! `find_tools`: loads deferred tools. A deferred tool is listed by name
//! until this tool returns its definition; from the next model call that
//! definition is in the request and it is called like any other tool. The
//! loaded set is re-derived from history each step ([`loaded_in_result`]
//! reads the definitions back out of a result).

use std::sync::Arc;

use serde_json::json;

use crate::origin::ToolContext;
use crate::registry::{DynTool, Registry, ToolResult};

pub const FIND_TOOLS: &str = "find_tools";

/// Score weights (Claude Code's): a query word equal to a word of the name,
/// part of one, a word of the search hint, found in the description.
const NAME_PART_EXACT: i32 = 10;
const NAME_PART_PARTIAL: i32 = 5;
const HINT_WORD: i32 = 4;
const DESCRIPTION_HIT: i32 = 2;

const DEFAULT_MAX_RESULTS: usize = 5;

/// A deferred tool as the search sees it.
#[derive(Debug, Clone)]
pub struct DeferredEntry {
    pub definition: ai::ToolDefinition,
    pub search_hint: String,
}

pub struct FindToolsTool {
    registry: Arc<Registry>,
}

impl FindToolsTool {
    pub fn new(registry: Arc<Registry>) -> Self {
        Self { registry }
    }
}

impl DynTool for FindToolsTool {
    fn name(&self) -> &str {
        FIND_TOOLS
    }

    fn description(&self) -> String {
        "Loads the full definitions of deferred tools so they can be called.\n\
         Deferred tools are listed by name in reminders; until loaded, only the name is known.\n\
         When a reminder, instruction or another tool's description names a deferred tool, load it with \"select:<name>\" first.\n\
         Query forms: \"select:mail_message_send,calendar_event_create\" loads exact names · \"invoice create\" searches by keywords · \"+shopify order\" requires \"shopify\" in the name."
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "\"select:name1,name2\" to load by exact name, or keywords to search."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Most tools a keyword search returns.",
                    "default": DEFAULT_MAX_RESULTS
                }
            },
            "required": ["query"]
        })
    }

    fn search_hint(&self) -> &str {
        "load deferred tools by name or keyword"
    }

    fn should_defer(&self) -> bool {
        false
    }

    fn read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn activity(&self, _input: &serde_json::Value) -> String {
        "loading tools".to_string()
    }

    fn outcome(&self, _input: &serde_json::Value) -> String {
        "Loaded tools".to_string()
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let query = input.get("query").and_then(|v| v.as_str()).unwrap_or("").trim();
            let max_results = input
                .get("max_results")
                .and_then(|v| v.as_u64())
                .map_or(DEFAULT_MAX_RESULTS, |n| n.max(1) as usize);
            let mut catalog = self.registry.deferred_entries().await;
            // A tool the run's tool scope leaves out can't be loaded.
            if let Some(withheld) = &ctx.withheld_tools {
                catalog.retain(|e| !withheld.contains(&e.definition.name));
            }
            ToolResult::ok(answer(&catalog, query, max_results))
        })
    }
}

/// The result for `query` over `catalog`: the matched definitions in a
/// `<functions>` block (the encoding of the tools array), then the names
/// loaded. Nothing matched: what to try instead.
pub fn answer(catalog: &[DeferredEntry], query: &str, max_results: usize) -> String {
    let (found, missing) = search(catalog, query, max_results);
    if found.is_empty() {
        return format!(
            "No deferred tool matches \"{query}\". Deferred tools are listed by name in \
             reminders: load one with \"select:<name>\", or try other keywords."
        );
    }
    let mut out = functions_block(found.iter().map(|e| &e.definition));
    out.push('\n');
    let names: Vec<&str> = found.iter().map(|e| e.definition.name.as_str()).collect();
    out.push_str(&format!("Loaded: {}. Call them directly.", names.join(", ")));
    if !missing.is_empty() {
        out.push_str(&format!(" Not found: {}.", missing.join(", ")));
    }
    out
}

/// The entries `query` selects, best first, and the `select:` names that
/// match nothing.
fn search<'a>(
    catalog: &'a [DeferredEntry],
    query: &str,
    max_results: usize,
) -> (Vec<&'a DeferredEntry>, Vec<String>) {
    if let Some(names) = query.strip_prefix("select:") {
        let mut found = Vec::new();
        let mut missing = Vec::new();
        for want in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            match catalog
                .iter()
                .find(|e| e.definition.name.eq_ignore_ascii_case(want))
            {
                Some(e) if !found.iter().any(|f: &&DeferredEntry| std::ptr::eq(*f, e)) => {
                    found.push(e)
                }
                Some(_) => {}
                None => missing.push(want.to_string()),
            }
        }
        return (found, missing);
    }
    // A bare tool name is that tool: a query naming one exactly loads it,
    // not the tools whose descriptions mention it.
    if let Some(e) = catalog
        .iter()
        .find(|e| e.definition.name.eq_ignore_ascii_case(query))
    {
        return (vec![e], Vec::new());
    }
    let mut words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let required: Vec<String> = words
        .iter()
        .filter_map(|w| w.strip_prefix('+').map(str::to_string))
        .filter(|w| !w.is_empty())
        .collect();
    words.retain(|w| !w.starts_with('+'));
    let mut scored: Vec<(i32, &DeferredEntry)> = catalog
        .iter()
        .filter(|e| {
            let name = e.definition.name.to_lowercase();
            required.iter().all(|r| name.contains(r.as_str()))
        })
        .map(|e| (score(e, &words), e))
        .filter(|(s, _)| *s > 0 || (!required.is_empty() && words.is_empty()))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.definition.name.cmp(&b.1.definition.name)));
    (
        scored.into_iter().take(max_results).map(|(_, e)| e).collect(),
        Vec::new(),
    )
}

fn score(e: &DeferredEntry, words: &[String]) -> i32 {
    let parts = name_parts(&e.definition.name);
    let hint: Vec<String> = e
        .search_hint
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    let description = e.definition.description.to_lowercase();
    let mut total = 0;
    for w in words {
        if parts.iter().any(|p| p == w) {
            total += NAME_PART_EXACT;
        } else if parts.iter().any(|p| p.contains(w.as_str())) {
            total += NAME_PART_PARTIAL;
        }
        if hint.iter().any(|h| h == w) {
            total += HINT_WORD;
        }
        if description.contains(w.as_str()) {
            total += DESCRIPTION_HIT;
        }
    }
    total
}

/// A tool name's words: split on `_`, `-` and lower-to-upper case changes.
fn name_parts(name: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if c == '_' || c == '-' {
            if !cur.is_empty() {
                parts.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower && !cur.is_empty() {
            parts.push(std::mem::take(&mut cur));
        }
        prev_lower = c.is_lowercase();
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
}

/// A definition as the model reads it in text: the encoding of the tools
/// array (`description`, `name`, `parameters`).
pub fn function_entry(def: &ai::ToolDefinition) -> serde_json::Value {
    json!({
        "description": def.description,
        "name": def.name,
        "parameters": def.input_schema,
    })
}

/// The definition a [`function_entry`] carries; `None` without a name.
pub fn definition_of(entry: &serde_json::Value) -> Option<ai::ToolDefinition> {
    Some(ai::ToolDefinition {
        name: entry.get("name")?.as_str()?.to_string(),
        description: entry.get("description").and_then(|d| d.as_str()).unwrap_or_default().to_string(),
        input_schema: entry.get("parameters").cloned().unwrap_or_else(|| json!({})),
    })
}

/// `definitions` in a `<functions>` block, one `<function>` line each.
pub fn functions_block<'a>(definitions: impl IntoIterator<Item = &'a ai::ToolDefinition>) -> String {
    let mut out = String::from("<functions>\n");
    for def in definitions {
        out.push_str(&format!("<function>{}</function>\n", function_entry(def)));
    }
    out.push_str("</functions>");
    out
}

/// The definitions a `find_tools` result loaded (its `<function>` entries),
/// as the result showed them.
pub fn loaded_in_result(content: &str) -> Vec<ai::ToolDefinition> {
    content
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("<function>")?
                .strip_suffix("</function>")
        })
        .filter_map(|body| serde_json::from_str::<serde_json::Value>(body).ok())
        .filter_map(|v| definition_of(&v))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, description: &str, hint: &str) -> DeferredEntry {
        DeferredEntry {
            definition: ai::ToolDefinition {
                name: name.to_string(),
                description: description.to_string(),
                input_schema: json!({"type": "object", "properties": {}}),
            },
            search_hint: hint.to_string(),
        }
    }

    fn catalog() -> Vec<DeferredEntry> {
        vec![
            entry("mail_message_send", "Sends an email.", "send email to someone"),
            entry("calendar_event_create", "Creates a calendar event.", "schedule a meeting"),
            entry("plugin__shopify", "Runs the Shopify plugin: orders, products.", "shopify store orders"),
            entry("mcp__github__create_issue", "Create an issue.", ""),
        ]
    }

    fn names(result: &str) -> Vec<String> {
        loaded_in_result(result).into_iter().map(|d| d.name).collect()
    }

    #[test]
    fn select_loads_exact_names_case_insensitively_and_says_what_is_missing() {
        let out = answer(&catalog(), "select:Mail_Message_Send, calendar_event_create,nope", 5);
        assert_eq!(names(&out), ["mail_message_send", "calendar_event_create"]);
        assert!(out.starts_with("<functions>\n<function>{"), "{out}");
        assert!(out.contains("\"parameters\":{"), "definitions carry the schema: {out}");
        assert!(out.ends_with("Loaded: mail_message_send, calendar_event_create. Call them directly. Not found: nope."));
    }

    /// What a result loaded reads back byte for byte: the request declares
    /// the definition the model was shown.
    #[test]
    fn a_loaded_definition_reads_back_as_it_was_shown() {
        let schema = json!({"type": "object", "properties": {"to": {"type": "string"}}, "required": ["to"]});
        let catalog = vec![DeferredEntry {
            definition: ai::ToolDefinition { name: "mail_message_send".into(), description: "Sends an email.".into(), input_schema: schema },
            search_hint: String::new(),
        }];
        let loaded = loaded_in_result(&answer(&catalog, "select:mail_message_send", 5));
        assert_eq!(serde_json::to_string(&loaded).unwrap(), serde_json::to_string(&[&catalog[0].definition]).unwrap());
    }

    #[test]
    fn keywords_score_name_parts_then_hints_then_descriptions() {
        // "event" is a whole word of one name: +10 beats a description hit.
        assert_eq!(names(&answer(&catalog(), "event", 5))[0], "calendar_event_create");
        // A hint word finds a tool whose name doesn't say it.
        assert_eq!(names(&answer(&catalog(), "meeting", 5)), ["calendar_event_create"]);
        // Part of a name word.
        assert_eq!(names(&answer(&catalog(), "calend", 5)), ["calendar_event_create"]);
        assert_eq!(names(&answer(&catalog(), "issue", 5)), ["mcp__github__create_issue"]);
    }

    /// Gate run 36099651233: find_tools("fetch_url") loaded browser_open,
    /// http_request and search_web — the tools whose descriptions mention
    /// fetch_url — and never fetch_url itself.
    #[test]
    fn a_bare_tool_name_loads_that_tool_alone() {
        let mut catalog = catalog();
        catalog.push(entry("fetch_url", "Fetches a URL and returns its content as text.", "read a web page"));
        catalog.push(entry("browser_open", "Opens a URL; to just read a page, fetch_url is faster.", ""));
        catalog.push(entry("http_request", "Sends an HTTP request. To just read, use fetch_url.", ""));
        assert_eq!(names(&answer(&catalog, "fetch_url", 5)), ["fetch_url"]);
        assert_eq!(names(&answer(&catalog, "Fetch_URL", 5)), ["fetch_url"]);
    }

    #[test]
    fn a_plus_word_must_be_in_the_name() {
        assert_eq!(names(&answer(&catalog(), "+shopify orders", 5)), ["plugin__shopify"]);
        assert!(names(&answer(&catalog(), "+shopify meeting", 5)).is_empty());
        assert_eq!(names(&answer(&catalog(), "+github", 5)), ["mcp__github__create_issue"]);
    }

    #[test]
    fn no_match_says_how_to_find_one_without_naming_other_tools() {
        let out = answer(&catalog(), "teleport", 5);
        assert!(names(&out).is_empty());
        assert!(out.contains("select:<name>"));
    }

    #[test]
    fn max_results_bounds_a_keyword_search() {
        let out = answer(&catalog(), "create send shopify issue", 2);
        assert_eq!(names(&out).len(), 2);
    }

    #[test]
    fn name_parts_split_snake_kebab_and_camel_case() {
        assert_eq!(name_parts("mcp__github__createIssue"), ["mcp", "github", "create", "issue"]);
        assert_eq!(name_parts("gws-gmail"), ["gws", "gmail"]);
    }
}
