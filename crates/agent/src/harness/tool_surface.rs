//! The loop side of tools: the set declared on each step, the deferred
//! tools `find_tools` loaded, and the names-only listing of the rest.
//!
//! Declared = the core, in name order, then each loaded tool in the order it
//! was loaded, with the definition it was loaded with. The core is the same
//! for every run on every bot (every employee, helper type, workflow
//! activity, mode and channel) and a load only appends, so the tools array,
//! the head of the cached prefix, only ever grows at its end. A tool a run
//! may not call stays declared and the one permission check refuses the
//! call, as Claude Code's fork path does: the child sends the parent's exact
//! tool array for a cache-identical prefix and a disallowed call is refused
//! when it is made (2.1.280 m0480 @26732 `useExactTools`, and the
//! `subagent_recursive_fork` refusal; `src/tools/AgentTool/forkSubagent.ts`).
//!
//! Everything else is deferred: listed by name, narrowed to what the run may
//! use (an employee's job names its tools in its session context,
//! `prompt::inputs::job_tools`), and loaded with `find_tools`. A tool
//! arriving or leaving (a plugin connecting, an MCP server going away) and a
//! loaded tool whose definition changed (an operation tool gaining a second
//! provider) are told in the listing delta, never by changing the declared
//! array: Claude Code 2.1.280 re-announces a surfaced tool whose definition
//! changed as a replacement in its `deferred_tools_delta` (m0342 @3120000,
//! `replacedNames`).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use ai::ToolDefinition;
use db::models::ChatMessage;
use tools::find_tools::{definition_of, function_entry, functions_block};

/// Past this many names the external families collapse to one line per
/// server or app.
const GROUP_PAST: usize = 30;

/// The key a compaction boundary row carries the loaded tools under: their
/// definitions as declared.
pub const LOADED_TOOLS_KEY: &str = "loadedTools";

/// The key a `tools_available` row carries its replaced definitions under.
const REPLACED_KEY: &str = "replaced";

/// A deferred tool the conversation loaded.
#[derive(Debug, Clone)]
pub struct LoadedTool {
    /// What every request declares for it: the definition it was first
    /// loaded with. It never changes, so the tools array before it stays
    /// cached.
    pub declared: ToolDefinition,
    /// The definition the conversation was last shown, as its entry: a later
    /// load, or a replacement the listing told.
    pub told: serde_json::Value,
}

/// The deferred tools `find_tools` loaded in this conversation, in the
/// order they were first loaded, re-derived from its stored rows on every
/// step. `messages` is the conversation as stored, not the compacted window,
/// so a tool stays loaded after the result that loaded it is trimmed or
/// evicted; a compaction boundary carries the tools loaded before it.
pub fn loaded(messages: &[ChatMessage]) -> Vec<LoadedTool> {
    let mut find_calls: HashSet<String> = HashSet::new();
    let mut out: Vec<LoadedTool> = Vec::new();
    let show = |out: &mut Vec<LoadedTool>, def: ToolDefinition| {
        let entry = function_entry(&def);
        match out.iter_mut().find(|t| t.declared.name == def.name) {
            Some(t) => t.told = entry,
            None => out.push(LoadedTool { declared: def, told: entry }),
        }
    };
    for msg in messages {
        let metadata = msg
            .metadata
            .as_deref()
            .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok());
        if let Some(carried) = metadata.as_ref().and_then(|m| m.get(LOADED_TOOLS_KEY)?.as_array()) {
            for def in carried.iter().filter_map(definition_of) {
                show(&mut out, def);
            }
        }
        if let Some(fields) = super::reminders::attachment_fields(msg)
            && fields.get("kind").and_then(|k| k.as_str()) == Some("tools_available")
            && let Some(replaced) = fields.get(REPLACED_KEY).and_then(|r| r.as_object())
        {
            for (name, entry) in replaced {
                if let Some(t) = out.iter_mut().find(|t| &t.declared.name == name) {
                    t.told = entry.clone();
                }
            }
        }
        match msg.role.as_str() {
            "assistant" => {
                let Some(calls) = msg
                    .tool_calls
                    .as_deref()
                    .and_then(|j| serde_json::from_str::<Vec<serde_json::Value>>(j).ok())
                else {
                    continue;
                };
                for call in calls {
                    if call.get("name").and_then(|v| v.as_str()) == Some(tools::find_tools::FIND_TOOLS)
                        && let Some(id) = call.get("id").and_then(|v| v.as_str())
                    {
                        find_calls.insert(id.to_string());
                    }
                }
            }
            "tool" => {
                let Some(results) = msg
                    .tool_results
                    .as_deref()
                    .and_then(|j| serde_json::from_str::<Vec<serde_json::Value>>(j).ok())
                else {
                    continue;
                };
                for result in results {
                    let from_find = result
                        .get("tool_call_id")
                        .and_then(|v| v.as_str())
                        .is_some_and(|id| find_calls.contains(id));
                    if !from_find {
                        continue;
                    }
                    let content = result.get("content").and_then(|v| v.as_str()).unwrap_or("");
                    for def in tools::find_tools::loaded_in_result(content) {
                        show(&mut out, def);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The definitions sent this step: the core (every tool that is not
/// deferred) in name order, then the loaded tools in load order as they were
/// loaded. `all` is the registry's list, in name order.
pub fn declared(all: &[ToolDefinition], deferred: &HashSet<String>, loaded: &[LoadedTool]) -> Vec<ToolDefinition> {
    let mut out: Vec<ToolDefinition> = all.iter().filter(|d| !deferred.contains(&d.name)).cloned().collect();
    let core: HashSet<String> = out.iter().map(|d| d.name.clone()).collect();
    out.extend(loaded.iter().filter(|t| !core.contains(&t.declared.name)).map(|t| t.declared.clone()));
    out
}

/// The loaded tools whose current definition differs from the one the
/// conversation was last shown: name → the current definition's entry. A
/// tool no longer registered has none; the listing says it left.
pub fn replaced(all: &[ToolDefinition], loaded: &[LoadedTool]) -> BTreeMap<String, serde_json::Value> {
    let live: HashMap<&str, &ToolDefinition> = all.iter().map(|d| (d.name.as_str(), d)).collect();
    loaded
        .iter()
        .filter_map(|t| {
            let entry = function_entry(live.get(t.declared.name.as_str())?);
            (entry != t.told).then(|| (t.declared.name.clone(), entry))
        })
        .collect()
}

/// A change in the listing: names that arrived and left, and loaded tools
/// whose definition changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListingDelta {
    pub added: BTreeSet<String>,
    pub removed: BTreeSet<String>,
    /// Loaded tools whose definition changed: name → the current entry.
    pub replaced: BTreeMap<String, serde_json::Value>,
}

impl ListingDelta {
    /// The whole set, announced from nothing.
    pub fn all(listed: BTreeSet<String>) -> Self {
        Self {
            added: listed,
            ..Self::default()
        }
    }

    /// What changed between what was announced and what is listed now, and
    /// the definitions `replaced`; `None` when nothing did.
    pub fn between(
        announced: &BTreeSet<String>,
        now: &BTreeSet<String>,
        replaced: BTreeMap<String, serde_json::Value>,
    ) -> Option<Self> {
        let delta = Self {
            added: now.difference(announced).cloned().collect(),
            removed: announced.difference(now).cloned().collect(),
            replaced,
        };
        (!delta.added.is_empty() || !delta.removed.is_empty() || !delta.replaced.is_empty()).then_some(delta)
    }

    /// The row's data: the replaced definitions, so the next step knows
    /// what the conversation was shown.
    pub fn replaced_data(&self) -> Option<(String, serde_json::Value)> {
        (!self.replaced.is_empty()).then(|| {
            (
                REPLACED_KEY.to_string(),
                serde_json::Value::Object(self.replaced.clone().into_iter().collect()),
            )
        })
    }
}

/// The listing's text: one name per line, the external families grouped as
/// `mcp__<server>__* (N)` / `app__<app>__* (N)` once there are more than
/// [`GROUP_PAST`] names.
pub fn render_listing(delta: &ListingDelta) -> String {
    let mut out = String::new();
    if !delta.added.is_empty() {
        out.push_str(&format!(
            "The following deferred tools are available through {find}. Their definitions aren't loaded — load them with {find}(\"select:<name>[,<name>…]\") before calling. One name per line:\n{}",
            names_block(&delta.added),
            find = tools::find_tools::FIND_TOOLS,
        ));
    }
    if !delta.removed.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&format!(
            "The following deferred tools are no longer available:\n{}",
            names_block(&delta.removed)
        ));
    }
    if !delta.replaced.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        let definitions: Vec<ToolDefinition> = delta.replaced.values().filter_map(definition_of).collect();
        out.push_str(&format!(
            "These loaded tools changed. Their definitions below replace the ones loaded earlier:\n{}",
            functions_block(&definitions)
        ));
    }
    out
}

fn names_block(names: &BTreeSet<String>) -> String {
    if names.len() <= GROUP_PAST {
        return names.iter().cloned().collect::<Vec<_>>().join("\n");
    }
    let mut lines: Vec<String> = Vec::new();
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    for name in names {
        match family_prefix(name) {
            Some(prefix) => *groups.entry(prefix).or_default() += 1,
            None => lines.push(name.clone()),
        }
    }
    lines.extend(groups.into_iter().map(|(prefix, n)| format!("{prefix}* ({n})")));
    lines.sort();
    lines.join("\n")
}

/// `mcp__github__` for `mcp__github__create_issue`; same for `app__`.
fn family_prefix(name: &str) -> Option<String> {
    for family in ["mcp__", "app__"] {
        if let Some(rest) = name.strip_prefix(family)
            && let Some((owner, _)) = rest.split_once("__")
        {
            return Some(format!("{family}{owner}__"));
        }
    }
    None
}

/// What a step's surface is built from: the seat's side of it. It narrows
/// the listing only; the declared tools are the same for every run.
pub struct SurfaceInputs<'a> {
    pub agent_id: &'a str,
    /// A restricted run's allowlist: only what it admits is listed.
    pub allowlist: Option<&'a HashSet<String>>,
    /// A workflow activity lists its own tools: its declaration, deferred
    /// tools included, with the `exit` primitive.
    pub workflow: Option<&'a crate::harness::WorkflowMode>,
    /// A helper's kind and depth, and whether the run is a workflow
    /// activity, decide what it is offered (`delegation::on_surface`).
    pub mode: &'a crate::harness::TurnMode,
    /// The tools the run may not use: the employee's own tools the active
    /// tool scope leaves out ([`scope_withheld`]) and a sealed seat's company
    /// Memory (`seat::company_memory_tools`); never listed.
    pub withheld: &'a HashSet<String>,
}

impl SurfaceInputs<'_> {
    /// Whether the run is offered the deferred tool `name` in its listing.
    fn offers(&self, name: &str) -> bool {
        crate::harness::delegation::on_surface(self.mode, name)
            && self.workflow.is_none_or(|m| m.advertised_tools.contains(name))
            && self.allowlist.is_none_or(|a| allowlist_admits(a, name))
            && !self.withheld.contains(name)
    }
}

/// The employee's own tools a tool scope leaves out. A scope that lists
/// tools (`scopes.<name>.tools`) keeps only those of the employee's own
/// tools for its conversations: the rest may not be loaded or called there.
/// The runtime's tools are not the scope's to narrow. Empty when the run
/// has no scope or the scope lists no tools.
pub async fn scope_withheld(
    agent: &tools::ActiveAgent,
    tool_scope: Option<&str>,
    registry: &tools::Registry,
) -> HashSet<String> {
    let Some(scope) = tool_scope.and_then(|s| agent.config.as_ref()?.scopes.get(s)) else {
        return HashSet::new();
    };
    if scope.tools.is_empty() {
        return HashSet::new();
    }
    registry
        .agent_tool_names(&agent.agent_id)
        .await
        .into_iter()
        .filter(|n| !scope.tools.contains(n))
        .collect()
}

/// One step's tool surface.
pub struct Surface {
    /// The definitions the request carries: the core, then the loaded tools
    /// in load order.
    pub declared: Vec<ToolDefinition>,
    /// The change in the deferred listing since the conversation was last
    /// told; `None` when it is current.
    pub listing: Option<ListingDelta>,
}

/// The surface of the next call and the listing delta the step writes
/// before its call. `conversation` is the conversation as stored since the
/// last checkpoint.
pub async fn surface(
    tools: &tools::Registry,
    conversation: &[ChatMessage],
    seat: &SurfaceInputs<'_>,
) -> Surface {
    let deferred = tools.get_deferred_names().await;
    let all = tools.list().await;
    let loaded = loaded(conversation);
    let declared = declared(&all, &deferred, &loaded);
    let listed: BTreeSet<String> = deferred.into_iter().filter(|n| seat.offers(n)).collect();
    let announced: BTreeSet<String> =
        crate::harness::events::announced("tools_available", conversation).into_keys().collect();
    let listing = ListingDelta::between(&announced, &listed, replaced(&all, &loaded));
    Surface { declared, listing }
}

/// Whether a restricted run's allowlist names this tool: by name, as the
/// tool of a `tool:resource` entry, or by a `prefix*` family.
fn allowlist_admits(allowlist: &HashSet<String>, name: &str) -> bool {
    allowlist.contains(name)
        || allowlist.iter().any(|e| {
            e.split_once(':').is_some_and(|(tool, _)| tool == name)
                || e.strip_suffix('*')
                    .is_some_and(|prefix| !prefix.is_empty() && name.starts_with(prefix))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: String::new(),
            input_schema: serde_json::json!({}),
        }
    }

    fn msg(role: &str, tool_calls: Option<&str>, tool_results: Option<&str>) -> ChatMessage {
        ChatMessage {
            id: String::new(),
            chat_id: String::new(),
            role: role.to_string(),
            content: String::new(),
            metadata: None,
            created_at: 0,
            day_marker: None,
            tool_calls: tool_calls.map(str::to_string),
            tool_results: tool_results.map(str::to_string),
            token_estimate: None,
            html: None,
        }
    }

    fn set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    fn find_result(id: &str, names: &[&str]) -> ChatMessage {
        let mut content = String::from("<functions>\n");
        for n in names {
            content.push_str(&format!("<function>{}</function>\n", serde_json::json!({"name": n, "description": "", "parameters": {}})));
        }
        content.push_str("</functions>\nLoaded.");
        msg("tool", None, Some(&serde_json::json!([{"tool_call_id": id, "content": content}]).to_string()))
    }

    fn names(loaded: &[LoadedTool]) -> Vec<&str> {
        loaded.iter().map(|t| t.declared.name.as_str()).collect()
    }

    #[test]
    fn a_find_tools_result_loads_its_tools_in_load_order_and_nothing_else_does() {
        let messages = vec![
            msg("assistant", Some(r#"[{"id":"c1","name":"find_tools","input":{"query":"select:vm,code"}}]"#), None),
            find_result("c1", &["vm", "code"]),
            // A direct call to a deferred tool doesn't load it.
            msg("assistant", Some(r#"[{"id":"c2","name":"mcp__gh__issue","input":{}}]"#), None),
            // A <functions> block from any other tool loads nothing.
            msg("assistant", Some(r#"[{"id":"c3","name":"web","input":{}}]"#), None),
            find_result("c3", &["mcp__gh__issue"]),
            msg("assistant", Some(r#"[{"id":"c4","name":"find_tools","input":{"query":"select:authority"}}]"#), None),
            find_result("c4", &["authority"]),
        ];
        assert_eq!(names(&loaded(&messages)), ["vm", "code", "authority"], "the order they were loaded in");
    }

    #[test]
    fn a_compaction_boundary_carries_the_loaded_definitions() {
        let mut boundary = msg("user", None, None);
        let carried = ToolDefinition { name: "vm".into(), description: "Runs a VM.".into(), input_schema: serde_json::json!({"type": "object"}) };
        boundary.metadata = Some(serde_json::json!({ LOADED_TOOLS_KEY: [function_entry(&carried)] }).to_string());
        let loaded = loaded(&[boundary]);
        assert_eq!(names(&loaded), ["vm"]);
        assert_eq!(loaded[0].declared.description, "Runs a VM.", "the definition as it was declared");
    }

    /// The core first in name order, then each loaded tool in load order: a
    /// load appends and never moves a tool already sent.
    #[test]
    fn the_declared_set_is_the_core_then_the_loaded_tools_in_load_order() {
        let all = vec![def("authority"), def("code"), def("os"), def("vm"), def("web")];
        let deferred = set(&["vm", "authority", "code"]);
        let messages = vec![
            msg("assistant", Some(r#"[{"id":"c1","name":"find_tools","input":{"query":"select:vm"}}]"#), None),
            find_result("c1", &["vm"]),
            msg("assistant", Some(r#"[{"id":"c2","name":"find_tools","input":{"query":"select:code"}}]"#), None),
            find_result("c2", &["code"]),
        ];
        let first: Vec<String> = declared(&all, &deferred, &loaded(&messages[..2])).into_iter().map(|d| d.name).collect();
        let then: Vec<String> = declared(&all, &deferred, &loaded(&messages)).into_iter().map(|d| d.name).collect();
        assert_eq!(first, ["os", "web", "vm"]);
        assert_eq!(then, ["os", "web", "vm", "code"], "code is appended, not sorted in before vm");
    }

    /// A loaded tool keeps the definition it was loaded with; when its
    /// definition changes (a second provider connects) the change is a
    /// replacement for the listing, told once.
    #[test]
    fn a_changed_definition_is_a_replacement_not_a_new_declaration() {
        let deferred = set(&["pay"]);
        let messages = vec![
            msg("assistant", Some(r#"[{"id":"c1","name":"find_tools","input":{"query":"select:pay"}}]"#), None),
            find_result("c1", &["pay"]),
        ];
        let then = def("pay");
        let now = ToolDefinition { description: "Pays through one of: a, b.".into(), ..def("pay") };
        let loaded_now = loaded(&messages);
        assert!(replaced(std::slice::from_ref(&then), &loaded_now).is_empty(), "unchanged");
        let changed = replaced(std::slice::from_ref(&now), &loaded_now);
        assert_eq!(changed.get("pay"), Some(&function_entry(&now)));
        let declared_now: Vec<ToolDefinition> = declared(std::slice::from_ref(&now), &deferred, &loaded_now);
        assert_eq!(declared_now[0].description, "", "the declared definition is the one loaded");

        let delta = ListingDelta::between(&BTreeSet::new(), &BTreeSet::new(), changed).unwrap();
        let text = render_listing(&delta);
        assert!(text.starts_with("These loaded tools changed.") && text.contains("Pays through one of: a, b."), "{text}");
        let mut told = msg("user", None, None);
        told.metadata = Some(serde_json::json!({"attachment": {"kind": "tools_available", "replaced": {"pay": function_entry(&now)}}}).to_string());
        let mut after = messages.clone();
        after.push(told);
        assert!(replaced(std::slice::from_ref(&now), &loaded(&after)).is_empty(), "told once");
    }

    #[test]
    fn the_listing_is_names_only_one_per_line() {
        let text = render_listing(&ListingDelta::all(["vm".to_string(), "code".to_string()].into()));
        assert!(text.starts_with("The following deferred tools are available through find_tools."), "{text}");
        assert!(text.ends_with(":\ncode\nvm"), "{text}");
    }

    #[test]
    fn a_delta_says_what_arrived_and_what_left_and_nothing_when_unchanged() {
        let before: BTreeSet<String> = ["a".into(), "b".into()].into();
        let after: BTreeSet<String> = ["b".into(), "c".into()].into();
        let delta = ListingDelta::between(&before, &after, BTreeMap::new()).unwrap();
        let text = render_listing(&delta);
        assert!(text.contains("available through find_tools") && text.contains(":\nc"), "{text}");
        assert!(text.ends_with("no longer available:\na"), "{text}");
        assert_eq!(ListingDelta::between(&after, &after, BTreeMap::new()), None);
    }

    #[test]
    fn past_thirty_names_mcp_and_app_tools_group_by_server() {
        let mut names: BTreeSet<String> = (0..40).map(|i| format!("mcp__github__tool_{i}")).collect();
        names.insert("app__crm__lookup".into());
        names.insert("app__crm__update".into());
        names.insert("vm".into());
        let text = render_listing(&ListingDelta::all(names));
        let lines: Vec<&str> = text.lines().skip(1).collect();
        assert_eq!(lines, ["app__crm__* (2)", "mcp__github__* (40)", "vm"]);
    }
}
