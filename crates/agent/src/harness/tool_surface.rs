//! The loop side of tools: the set declared on each step, the deferred
//! tools `find_tools` loaded, and the names-only listing of the rest.
//!
//! Declared set = core ∪ always_load (the employee's `requires.tools`, its
//! own app tools) ∪ loaded. Every other registered tool is deferred and
//! listed by name; `find_tools` returns a deferred tool's definition and its
//! schema joins the request from the next step. There is no keyword
//! filter, context group or per-turn show question.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use ai::ToolDefinition;
use db::models::ChatMessage;

/// Past this many names the external families collapse to one line per
/// server or app.
const GROUP_PAST: usize = 30;

/// The key a compaction boundary row carries the loaded tool names under.
pub const LOADED_TOOLS_KEY: &str = "loadedTools";

/// The deferred tools `find_tools` loaded in this conversation, re-derived
/// from its stored rows on every step. `messages` is the conversation as
/// stored, not the compacted window, so a tool stays loaded after the
/// result that loaded it is trimmed or evicted; a compaction boundary
/// carries the names loaded before it.
pub fn loaded_tools(messages: &[ChatMessage], deferred: &HashSet<String>) -> BTreeSet<String> {
    loaded_names(messages)
        .into_iter()
        .filter(|n| deferred.contains(n))
        .collect()
}

/// Every tool name a `find_tools` result in `messages` loaded, and every
/// name a compaction boundary row carried forward.
pub fn loaded_names(messages: &[ChatMessage]) -> BTreeSet<String> {
    let mut find_calls: HashSet<String> = HashSet::new();
    let mut loaded = BTreeSet::new();
    for msg in messages {
        if let Some(carried) = msg
            .metadata
            .as_deref()
            .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
            .and_then(|m| m.get(LOADED_TOOLS_KEY).cloned())
            .and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
        {
            loaded.extend(carried);
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
                    loaded.extend(tools::find_tools::loaded_in_result(content));
                }
            }
            _ => {}
        }
    }
    loaded
}

/// The definitions sent this step: every non-deferred tool, plus the
/// deferred ones that are always loaded for this employee or were loaded,
/// in name order (the tools array is part of the cached prefix).
pub fn declared(
    all: Vec<ToolDefinition>,
    deferred: &HashSet<String>,
    always_load: &HashSet<String>,
    loaded: &BTreeSet<String>,
) -> Vec<ToolDefinition> {
    let mut out: Vec<ToolDefinition> = all
        .into_iter()
        .filter(|d| {
            !deferred.contains(&d.name) || always_load.contains(&d.name) || loaded.contains(&d.name)
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The deferred tools not declared this step: what the listing names.
pub fn listed(deferred: &HashSet<String>, declared: &[ToolDefinition]) -> BTreeSet<String> {
    let sent: HashSet<&str> = declared.iter().map(|d| d.name.as_str()).collect();
    deferred
        .iter()
        .filter(|n| !sent.contains(n.as_str()))
        .cloned()
        .collect()
}

/// A change in the listed set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListingDelta {
    pub added: BTreeSet<String>,
    pub removed: BTreeSet<String>,
}

impl ListingDelta {
    /// The whole set, announced from nothing.
    pub fn all(listed: BTreeSet<String>) -> Self {
        Self {
            added: listed,
            removed: BTreeSet::new(),
        }
    }

    /// What changed between what was announced and what is listed now;
    /// `None` when nothing did.
    pub fn between(announced: &BTreeSet<String>, now: &BTreeSet<String>) -> Option<Self> {
        let delta = Self {
            added: now.difference(announced).cloned().collect(),
            removed: announced.difference(now).cloned().collect(),
        };
        (!delta.added.is_empty() || !delta.removed.is_empty()).then_some(delta)
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

/// What a step's surface is built from: the seat's side of it.
pub struct SurfaceInputs<'a> {
    pub agent_id: &'a str,
    /// Declared on every step for this employee: its `requires.tools` and,
    /// when its job needs plugins, the plugin tool. Its own app tools join
    /// them from the registry.
    pub always_load: &'a HashSet<String>,
    /// A restricted run's allowlist: only what it admits is declared or
    /// listed.
    pub allowlist: Option<&'a HashSet<String>>,
    /// An isolated seat with no matter: company Memory is withheld.
    pub company_memory_sealed: bool,
    /// A workflow activity's scoped set: its declaration, deferred tools
    /// included, with the `exit` primitive.
    pub workflow: Option<&'a crate::runner::WorkflowMode>,
    /// A helper's kind and depth take the helper tools off its surface.
    pub mode: &'a crate::harness::TurnMode,
}

/// One step's tool surface.
pub struct Surface {
    /// The definitions the request carries, in name order.
    pub declared: Vec<ToolDefinition>,
    /// The deferred tools loaded in this conversation.
    pub loaded: BTreeSet<String>,
    /// The change in the deferred listing since the conversation was last
    /// told; `None` when it is current.
    pub listing: Option<ListingDelta>,
}

/// The surface of the next call: core ∪ always_load ∪ loaded, narrowed by
/// the seat, plus the listing delta the step writes before its call.
/// `conversation` is the conversation as stored since the last checkpoint.
pub async fn surface(
    tools: &tools::Registry,
    store: &db::Store,
    conversation: &[ChatMessage],
    seat: &SurfaceInputs<'_>,
) -> Surface {
    if let Some(m) = seat.workflow {
        let mut declared: Vec<ToolDefinition> =
            tools.list().await.into_iter().filter(|d| m.advertised_tools.contains(&d.name)).collect();
        if m.advertised_tools.contains("exit") && !declared.iter().any(|d| d.name == "exit") {
            let exit = tools::ExitTool::new();
            declared.push(ToolDefinition {
                name: "exit".into(),
                description: tools::registry::DynTool::description(&exit),
                input_schema: tools::registry::DynTool::schema(&exit),
            });
        }
        declared.sort_by(|a, b| a.name.cmp(&b.name));
        return Surface { declared, loaded: BTreeSet::new(), listing: None };
    }

    let deferred = tools.get_deferred_names().await;
    let loaded = loaded_tools(conversation, &deferred);
    let mut all = tools.list().await;
    let mut own = tools.agent_tool_names(seat.agent_id).await;
    if seat.company_memory_sealed {
        crate::harness::seat::seal_company_memory(store, tools, seat.agent_id, &mut all, &mut own).await;
    }
    let always: HashSet<String> = seat.always_load.iter().cloned().chain(own).collect();
    let mut declared = declared(all, &deferred, &always, &loaded);
    declared.retain(|d| crate::harness::delegation::on_surface(seat.mode, &d.name));
    let mut listed = listed(&deferred, &declared);
    listed.retain(|n| crate::harness::delegation::on_surface(seat.mode, n));
    if let Some(allowlist) = seat.allowlist {
        declared.retain(|d| crate::runner::allowlist_admits(allowlist, &d.name));
        listed.retain(|n| crate::runner::allowlist_admits(allowlist, n));
    }
    let announced: BTreeSet<String> =
        crate::harness::events::announced("tools_available", conversation).into_keys().collect();
    let listing = ListingDelta::between(&announced, &listed);
    Surface { declared, loaded, listing }
}

/// Remove company-Memory tools from a run's declared set (the per-employee
/// ethical wall). `memory_tool_names` is resolved by the caller from the MCP
/// proxy → integration mapping, so this stays pure and testable.
///
/// Returns the surviving definitions and how many were withheld; also prunes
/// `agent_tool_names`, or a withheld tool would still be callable by name.
pub fn withhold_memory_tools(
    all_tools: Vec<ToolDefinition>,
    agent_tool_names: &mut HashSet<String>,
    memory_tool_names: &HashSet<String>,
) -> (Vec<ToolDefinition>, usize) {
    if memory_tool_names.is_empty() {
        return (all_tools, 0);
    }
    let mut withheld = 0usize;
    let kept: Vec<ToolDefinition> = all_tools
        .into_iter()
        .filter(|d| {
            if memory_tool_names.contains(&d.name) {
                agent_tool_names.remove(&d.name);
                withheld += 1;
                false
            } else {
                true
            }
        })
        .collect();
    (kept, withheld)
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

    #[test]
    fn a_find_tools_result_loads_its_tools_and_nothing_else_does() {
        let deferred = set(&["vm", "code", "mcp__gh__issue"]);
        let messages = vec![
            msg("assistant", Some(r#"[{"id":"c1","name":"find_tools","input":{"query":"select:vm,code"}}]"#), None),
            find_result("c1", &["vm", "code", "not_deferred"]),
            // A direct call to a deferred tool doesn't load it.
            msg("assistant", Some(r#"[{"id":"c2","name":"mcp__gh__issue","input":{}}]"#), None),
            // A <functions> block from any other tool loads nothing.
            msg("assistant", Some(r#"[{"id":"c3","name":"web","input":{}}]"#), None),
            find_result("c3", &["mcp__gh__issue"]),
        ];
        let loaded = loaded_tools(&messages, &deferred);
        assert_eq!(loaded.into_iter().collect::<Vec<_>>(), ["code", "vm"]);
    }

    #[test]
    fn a_compaction_boundary_carries_the_loaded_tools() {
        let deferred = set(&["vm", "code"]);
        let mut boundary = msg("assistant", None, None);
        boundary.metadata = Some(serde_json::json!({ LOADED_TOOLS_KEY: ["vm", "gone_since"] }).to_string());
        let loaded = loaded_tools(&[boundary], &deferred);
        assert_eq!(loaded.into_iter().collect::<Vec<_>>(), ["vm"], "carried, and filtered to what is still deferred");
    }

    #[test]
    fn the_declared_set_is_core_always_load_and_loaded_in_name_order() {
        let all = vec![def("web"), def("vm"), def("os"), def("authority"), def("code")];
        let deferred = set(&["vm", "authority", "code"]);
        let always = set(&["authority"]);
        let loaded: BTreeSet<String> = ["code".to_string()].into();
        let names: Vec<String> = declared(all, &deferred, &always, &loaded).into_iter().map(|d| d.name).collect();
        assert_eq!(names, ["authority", "code", "os", "web"]);
    }

    #[test]
    fn the_listing_names_only_what_is_not_declared() {
        let deferred = set(&["vm", "code", "authority"]);
        let listed = listed(&deferred, &[def("code"), def("os")]);
        assert_eq!(listed.into_iter().collect::<Vec<_>>(), ["authority", "vm"]);
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
        let delta = ListingDelta::between(&before, &after).unwrap();
        let text = render_listing(&delta);
        assert!(text.contains("available through find_tools") && text.contains(":\nc"), "{text}");
        assert!(text.ends_with("no longer available:\na"), "{text}");
        assert_eq!(ListingDelta::between(&after, &after), None);
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

    #[test]
    fn isolated_employee_loses_memory_tools_only() {
        let all = vec![def("os"), def("mcp__nebo_kb__memory_search"), def("mcp__other__x")];
        let mut agent_tools = set(&["mcp__nebo_kb__memory_search"]);
        let (kept, withheld) = withhold_memory_tools(all, &mut agent_tools, &set(&["mcp__nebo_kb__memory_search"]));
        assert_eq!(withheld, 1);
        assert_eq!(kept.into_iter().map(|d| d.name).collect::<Vec<_>>(), ["os", "mcp__other__x"]);
        assert!(agent_tools.is_empty());
    }

    #[test]
    fn non_isolated_employee_keeps_everything() {
        let (kept, withheld) = withhold_memory_tools(vec![def("os")], &mut HashSet::new(), &HashSet::new());
        assert_eq!((kept.len(), withheld), (1, 0));
    }
}
