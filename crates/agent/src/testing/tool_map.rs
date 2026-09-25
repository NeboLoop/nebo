//! One table: the old STRAP tool names and the rewrite's tool names, so one
//! fixture's program checks decide both arms of a comparison.
//!
//! Arm A (the old loop) calls `os(resource: "file", action: "read", path)`;
//! arm P (the rewrite) calls `read_file(path)`. A check written in either
//! vocabulary must reach the same verdict on either arm, or the comparison
//! measures the fixture instead of the harness. So each traced call is seen
//! as itself AND as the same call in the other vocabulary, and a check passes
//! a call when either view satisfies it. Nothing is loosened: a view is the
//! same call renamed, never a different call, and a view is translated once
//! (old → new or new → old), never round-tripped.
//!
//! The rows come from Appendix A of the tools design (old → new map). A row
//! is used old → new for every old action it lists, and new → old as its
//! first resource and first action. Where several old actions collapse into
//! one new tool (`glob`, `grep` and `exec` all become `run_command`), the new
//! call's old view is the first row for it (`os shell exec`); a check that
//! pins `action: grep` is a check on the old tool's shape, not the outcome,
//! and only arm A can pass it.
//!
//! Test-harness only: nothing here runs in the product, and nothing in the
//! product aliases names at runtime.

use serde_json::{Map, Value};

/// One old surface and the new tool it becomes.
struct Row {
    old_tool: &'static str,
    /// Old resources this row covers; the first is the new call's old view.
    /// Empty for old tools with no resource (`tool_search`, `notebook`).
    resources: &'static [&'static str],
    /// Old actions this row covers; the first is the new call's old view.
    /// Empty for old tools with no action.
    actions: &'static [&'static str],
    new_tool: &'static str,
    /// Argument renames, (old name, new name).
    renames: &'static [(&'static str, &'static str)],
}

const fn row(
    old_tool: &'static str,
    resources: &'static [&'static str],
    actions: &'static [&'static str],
    new_tool: &'static str,
    renames: &'static [(&'static str, &'static str)],
) -> Row {
    Row { old_tool, resources, actions, new_tool, renames }
}

/// Old → new, Appendix A of the tools design. Order matters twice: the first
/// row naming a new tool is its old view, and when an old call leaves out its
/// resource the first row with its action is the one the old tool infers
/// (`list` and `delete` are task actions unless a resource says memory).
const ROWS: &[Row] = &[
    // os: files and shell
    row("os", &["file"], &["read"], "read_file", &[]),
    row("os", &["file"], &["write"], "write_file", &[]),
    row("os", &["file"], &["edit"], "edit_file", &[]),
    row("os", &["shell"], &["exec"], "run_command", &[]),
    row("os", &["file"], &["glob", "grep", "list"], "run_command", &[]),
    row("os", &["shell"], &["list", "info"], "list_processes", &[]),
    row("os", &["shell"], &["poll", "log"], "read_output", &[("session_id", "task_id")]),
    row("os", &["shell"], &["kill"], "stop_task", &[("session_id", "task_id")]),
    row("os", &["shell"], &["write"], "send_input", &[("session_id", "task_id")]),
    row("os", &["file"], &["share", "present", "send"], "share_file", &[]),
    row("os", &["file"], &["convert"], "convert_file", &[]),
    row("os", &["file"], &["checkpoint"], "checkpoint_files", &[]),
    row("os", &["file"], &["checkpoints"], "list_checkpoints", &[]),
    row("os", &["file"], &["restore"], "restore_checkpoint", &[]),
    row("os", &["file"], &["plan"], "write_plan", &[]),
    row("os", &["file"], &["plan_check"], "check_plan", &[]),
    // agent: helpers and tasks
    row("agent", &["task"], &["spawn", "spawn_parallel"], "delegate", &[("agent_type", "helper_type")]),
    row("agent", &["task"], &["orchestrate"], "orchestrate", &[]),
    row("agent", &["task"], &["send"], "send_message", &[("task_id", "to"), ("text", "message")]),
    row("agent", &["task"], &["status"], "read_output", &[]),
    row("agent", &["task"], &["cancel"], "stop_task", &[]),
    row("agent", &["task"], &["create"], "create_task", &[]),
    row("agent", &["task"], &["update", "delete"], "update_task", &[]),
    row("agent", &["task"], &["get"], "get_task", &[]),
    row("agent", &["task"], &["list"], "list_tasks", &[]),
    row("agent", &["task"], &["assign"], "assign_task", &[]),
    row("agent", &["task"], &["assignments"], "list_assignments", &[]),
    // agent: memory
    row("agent", &["memory"], &["store", "save"], "remember", &[]),
    row("agent", &["memory"], &["recall", "search", "list"], "recall", &[]),
    row("agent", &["memory"], &["delete"], "forget", &[]),
    // agent: the rest
    row("agent", &["ask"], &["prompt", "confirm", "select"], "ask_owner", &[("text", "question")]),
    row("agent", &["session"], &["query"], "search_history", &[]),
    row("agent", &["session"], &["history"], "read_session", &[]),
    row("agent", &["session"], &["list"], "list_sessions", &[]),
    row("agent", &["runs"], &["list"], "list_runs", &[]),
    row("agent", &["research"], &["deep_research"], "deep_research", &[]),
    row("agent", &["research"], &["research"], "quick_research", &[]),
    row("agent", &["research"], &["submit_findings"], "submit_findings", &[]),
    // skills
    row("skill", &[], &["load"], "use_skill", &[]),
    row("skill", &[], &["discover", "browse"], "find_skills", &[]),
    row("skill", &[], &["read_resource"], "read_skill_file", &[]),
    row("tool_search", &[], &[], "find_tools", &[]),
    // web
    row("web", &["search"], &["search"], "search_web", &[]),
    row("web", &["http"], &["fetch", "get", "sanitize"], "fetch_url", &[]),
    row("web", &["http"], &["post", "put", "patch", "delete", "head"], "http_request", &[]),
    row("web", &["browser"], &["navigate"], "browser_open", &[]),
    row("web", &["browser"], &["read_page"], "browser_read", &[("maxChars", "max_chars"), ("refId", "ref_id")]),
    row("web", &["browser"], &["find"], "browser_find", &[]),
    row("web", &["browser"], BROWSER_ACT, "browser_act", &[]),
    row("web", &["browser"], &["fill_form", "fill"], "browser_fill_form", &[]),
    row("web", &["browser"], &["evaluate"], "browser_run_js", &[]),
    row("web", &["browser"], &["list_tabs"], "browser_list_tabs", &[]),
    row("web", &["browser"], &["new_tab"], "browser_new_tab", &[]),
    row("web", &["browser"], &["close_tab"], "browser_close_tab", &[("tabId", "tab_id")]),
    row("web", &["browser"], &["read_console_messages"], "browser_console", &[("onlyErrors", "only_errors")]),
    row("web", &["devtools"], &["console"], "browser_console", &[("onlyErrors", "only_errors")]),
    row("web", &["browser"], &["read_network_requests"], "browser_network", &[("urlPattern", "url_pattern")]),
    row("web", &["browser"], &["file_upload"], "browser_upload", &[]),
    row("web", &["browser"], &["resize_window"], "browser_resize", &[]),
    row("web", &["browser"], &["history"], "browser_history", &[]),
    row("web", &["browser"], &["status"], "browser_status", &[]),
    row("web", &["browser"], &["browser_batch"], "browser_batch", &[("actions", "steps")]),
    row("web", &["browser"], &["webmcp_list"], "browser_page_tools", &[]),
    row("web", &["browser"], &["webmcp_call"], "browser_call_page_tool", &[]),
    // messages
    row("message", &["coworker"], &["send"], "send_message", &[("text", "message")]),
    row("message", &["owner"], &["notify"], "message_owner", &[("text", "message")]),
    row("message", &["notify"], &["send", "alert"], "push_notification", &[]),
    row("message", &["notify"], &["dnd_status"], "check_dnd", &[]),
    row("message", &["sms"], &["send"], "sms_message_send", &[]),
    // plugins other than exec, events and operations (below)
    row("plugin", &[], &["discover"], "find_plugins", &[]),
    // scheduling and workflows
    row("event", &[], &["create"], "create_schedule", &[]),
    row("event", &[], &["list"], "list_schedules", &[]),
    row("event", &[], &["delete"], "delete_schedule", &[]),
    row("work", &[], &["list"], "list_workflows", &[]),
    row("work", &[], &["install"], "install_workflow", &[]),
    row("work", &[], &["uninstall"], "uninstall_workflow", &[]),
    row("work", &[], &["create"], "create_workflow", &[]),
    row("work", &[], &["update"], "update_workflow", &[]),
    row("work", &[], &["delete"], "delete_workflow", &[]),
    row("work", &[], &["run"], "run_workflow", &[]),
    row("work", &[], &["status"], "workflow_status", &[]),
    // remaining built-ins
    row("notebook", &[], &["edit"], "edit_notebook", &[]),
    row("exit", &[], &[], "end_activity", &[]),
];

/// The old browser actions `browser_act` takes as its own `action` argument
/// (the one enum surface, as Chrome's `computer` tool).
const BROWSER_ACT: &[&str] =
    &["click", "hover", "type", "press", "scroll", "drag", "select", "wait", "screenshot"];

/// The prefix of a new-vocabulary plugin tool: `plugin__<slug>` is the old
/// `plugin(resource: <slug>, action: "exec")`.
const PLUGIN_PREFIX: &str = "plugin__";
/// The old plugin tool's events action; the slug is the new tool's `plugin`.
const PLUGIN_EVENTS: &str = "read_plugin_events";
/// The old `code` tool takes `action`; `code_intel` takes `operation`.
const CODE_OLD: &str = "code";
const CODE_NEW: &str = "code_intel";

/// The same call in the other vocabulary: an old call's new view, or a new
/// call's old view. `None` when the table has no row for it.
pub fn translate(tool: &str, args: &Value) -> Option<(String, Value)> {
    old_to_new(tool, args).or_else(|| new_to_old(tool, args))
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn old_to_new(tool: &str, args: &Value) -> Option<(String, Value)> {
    let action = str_arg(args, "action");
    let resource = str_arg(args, "resource");

    if tool == "plugin" {
        // A typed port: `plugin(operation, input, display)` is the operation's
        // own tool with the input's fields flat.
        if let Some(op) = str_arg(args, "operation") {
            let mut flat = args.get("input").and_then(Value::as_object).cloned().unwrap_or_default();
            if let Some(d) = args.get("display") {
                flat.insert("display".into(), d.clone());
            }
            let name = tools::operation_tools::operation_tool_name(&tools::plugin_tool::port_suffix(op));
            return Some((name, Value::Object(flat)));
        }
        let slug = resource.or_else(|| str_arg(args, "plugin"));
        match (action, slug) {
            // exec is the old tool's default action
            (None | Some("exec"), Some(slug)) => {
                let rest = without(args, &["resource", "action", "plugin"]);
                return Some((format!("{PLUGIN_PREFIX}{slug}"), rest));
            }
            (Some("events"), Some(slug)) => {
                return Some((PLUGIN_EVENTS.to_string(), serde_json::json!({ "plugin": slug })));
            }
            _ => {}
        }
    }
    if tool == CODE_OLD {
        let mut rest = without(args, &["action"]);
        if let (Some(a), Some(obj)) = (action, rest.as_object_mut()) {
            obj.insert("operation".into(), Value::String(a.to_string()));
        }
        return Some((CODE_NEW.to_string(), rest));
    }

    let row = ROWS.iter().find(|r| {
        r.old_tool == tool
            && (r.actions.is_empty() || action.is_some_and(|a| r.actions.contains(&a)))
            && (r.resources.is_empty() || resource.is_none_or(|res| r.resources.contains(&res)))
    })?;
    let keep = if row.actions == BROWSER_ACT { &["resource"][..] } else { &["resource", "action"][..] };
    let mut rest = without(args, keep);
    rename(&mut rest, row.renames.iter().map(|(o, n)| (*o, *n)));
    Some((row.new_tool.to_string(), rest))
}

fn new_to_old(tool: &str, args: &Value) -> Option<(String, Value)> {
    if let Some(slug) = tool.strip_prefix(PLUGIN_PREFIX).filter(|s| !s.is_empty()) {
        let mut old = args.clone();
        let obj = old.as_object_mut()?;
        obj.insert("resource".into(), Value::String(slug.to_string()));
        obj.insert("action".into(), Value::String("exec".into()));
        return Some(("plugin".to_string(), old));
    }
    if tool == PLUGIN_EVENTS {
        let slug = str_arg(args, "plugin")?;
        return Some(("plugin".to_string(), serde_json::json!({ "resource": slug, "action": "events" })));
    }
    if let Some(op) = tools::interface_catalog::operation_named(tool) {
        let mut input = args.as_object().cloned().unwrap_or_default();
        let mut old = serde_json::Map::new();
        old.insert("operation".into(), Value::String(op.to_string()));
        if let Some(d) = input.remove("display") {
            old.insert("display".into(), d);
        }
        old.insert("input".into(), Value::Object(input));
        return Some(("plugin".to_string(), Value::Object(old)));
    }
    if tool == CODE_NEW {
        let mut old = without(args, &["operation"]);
        if let (Some(op), Some(obj)) = (str_arg(args, "operation"), old.as_object_mut()) {
            obj.insert("action".into(), Value::String(op.to_string()));
        }
        return Some((CODE_OLD.to_string(), old));
    }

    let row = ROWS.iter().find(|r| r.new_tool == tool)?;
    let mut old = args.clone();
    rename(&mut old, row.renames.iter().map(|(o, n)| (*n, *o)));
    let obj = old.as_object_mut()?;
    if let Some(res) = row.resources.first() {
        obj.insert("resource".into(), Value::String(res.to_string()));
    }
    if row.actions == BROWSER_ACT {
        // browser_act's own `action` is already the old action
    } else if let Some(act) = row.actions.first() {
        obj.insert("action".into(), Value::String(act.to_string()));
    }
    Some((row.old_tool.to_string(), old))
}

fn without(args: &Value, keys: &[&str]) -> Value {
    match args.as_object() {
        Some(obj) => Value::Object(
            obj.iter()
                .filter(|(k, _)| !keys.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Map<String, Value>>(),
        ),
        None => Value::Object(Map::new()),
    }
}

fn rename<'a>(args: &mut Value, pairs: impl Iterator<Item = (&'a str, &'a str)>) {
    let Some(obj) = args.as_object_mut() else { return };
    for (from, to) in pairs {
        if obj.contains_key(to) {
            continue;
        }
        if let Some(v) = obj.remove(from) {
            obj.insert(to.to_string(), v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_file_read_is_read_file_both_ways() {
        let (tool, args) = translate("os", &json!({"resource": "file", "action": "read", "path": "/a"})).unwrap();
        assert_eq!(tool, "read_file");
        assert_eq!(args, json!({"path": "/a"}));

        let (tool, args) = translate("read_file", &json!({"path": "/a"})).unwrap();
        assert_eq!(tool, "os");
        assert_eq!(args, json!({"path": "/a", "resource": "file", "action": "read"}));
    }

    #[test]
    fn an_old_call_without_its_resource_takes_the_row_the_old_tool_infers() {
        assert_eq!(translate("os", &json!({"action": "exec", "command": "ls"})).unwrap().0, "run_command");
        // list is a task action unless the resource says memory
        assert_eq!(translate("agent", &json!({"action": "list"})).unwrap().0, "list_tasks");
        assert_eq!(translate("agent", &json!({"resource": "memory", "action": "list"})).unwrap().0, "recall");
    }

    #[test]
    fn many_old_actions_to_one_new_tool_come_back_as_the_first() {
        assert_eq!(translate("os", &json!({"action": "grep", "pattern": "TODO"})).unwrap().0, "run_command");
        let (tool, args) = translate("run_command", &json!({"command": "grep -rn TODO ."})).unwrap();
        assert_eq!(tool, "os");
        assert_eq!(args["resource"], "shell");
        assert_eq!(args["action"], "exec", "never grep: that is the old tool's shape, not the call");
    }

    #[test]
    fn arguments_are_renamed_with_the_tool() {
        let (tool, args) = translate("agent", &json!({"action": "spawn", "agent_type": "explore", "prompt": "p"})).unwrap();
        assert_eq!(tool, "delegate");
        assert_eq!(args, json!({"helper_type": "explore", "prompt": "p"}));

        let (tool, args) = translate("send_message", &json!({"to": "chief", "message": "hi"})).unwrap();
        assert_eq!(tool, "agent", "the first row for send_message is the task send");
        assert_eq!(args["task_id"], "chief");
        assert_eq!(args["text"], "hi");

        let (tool, args) = translate("message", &json!({"resource": "coworker", "action": "send", "to": "chief", "text": "hi"})).unwrap();
        assert_eq!(tool, "send_message");
        assert_eq!(args, json!({"to": "chief", "message": "hi"}));
    }

    #[test]
    fn plugins_and_code_carry_their_name_in_an_argument() {
        let (tool, args) = translate("plugin", &json!({"resource": "quickbooks", "action": "exec", "command": "doctor"})).unwrap();
        assert_eq!(tool, "plugin__quickbooks");
        assert_eq!(args, json!({"command": "doctor"}));
        let (tool, args) = translate("plugin__quickbooks", &json!({"command": "doctor"})).unwrap();
        assert_eq!(tool, "plugin");
        assert_eq!(args, json!({"command": "doctor", "resource": "quickbooks", "action": "exec"}));
        // exec was the old tool's default action
        assert_eq!(translate("plugin", &json!({"resource": "quickbooks", "command": "doctor"})).unwrap().0, "plugin__quickbooks");
        let (tool, args) = translate("plugin", &json!({"resource": "quickbooks", "action": "events"})).unwrap();
        assert_eq!((tool.as_str(), args), ("read_plugin_events", json!({"plugin": "quickbooks"})));
        assert_eq!(translate("read_plugin_events", &json!({"plugin": "quickbooks"})).unwrap().1, json!({"resource": "quickbooks", "action": "events"}));

        let (tool, args) = translate("code", &json!({"action": "outline", "path": "main.rs"})).unwrap();
        assert_eq!(tool, "code_intel");
        assert_eq!(args, json!({"operation": "outline", "path": "main.rs"}));
    }

    /// A typed port call is the operation's own tool, its input flat; the
    /// department and role prefix of a seat's port is not part of the name.
    #[test]
    fn a_typed_port_is_its_operation_tool() {
        let old = json!({"operation": "accounting.ap.ledger.bill.create", "input": {"vendorId": "V7"}, "display": "Pay V7"});
        let (tool, args) = translate("plugin", &old).unwrap();
        assert_eq!(tool, "ledger_bill_create");
        assert_eq!(args, json!({"vendorId": "V7", "display": "Pay V7"}));
        let (tool, args) = translate("ledger_bill_create", &json!({"vendorId": "V7", "display": "Pay V7"})).unwrap();
        assert_eq!(tool, "plugin");
        assert_eq!(args, json!({"operation": "ledger.bill.create", "input": {"vendorId": "V7"}, "display": "Pay V7"}));
        assert!(translate("ledger_nothing_here", &json!({})).is_none());
    }

    #[test]
    fn browser_act_keeps_its_action_argument() {
        let (tool, args) = translate("web", &json!({"resource": "browser", "action": "click", "ref": "B3"})).unwrap();
        assert_eq!(tool, "browser_act");
        assert_eq!(args, json!({"action": "click", "ref": "B3"}));
        let (tool, args) = translate("browser_act", &json!({"action": "type", "text": "hi"})).unwrap();
        assert_eq!(tool, "web");
        assert_eq!(args, json!({"action": "type", "text": "hi", "resource": "browser"}));
    }

    #[test]
    fn a_name_in_neither_vocabulary_has_no_view() {
        assert!(translate("propose_goal", &json!({})).is_none());
        assert!(translate("os", &json!({"action": "screenshot"})).is_none());
    }

    #[test]
    fn every_row_round_trips_to_its_own_new_tool() {
        for r in ROWS {
            let mut args = json!({});
            if let Some(res) = r.resources.first() {
                args["resource"] = json!(res);
            }
            for act in r.actions.iter().map(Some).chain(r.actions.is_empty().then_some(None)) {
                if let Some(a) = act {
                    args["action"] = json!(a);
                }
                let (tool, _) = translate(r.old_tool, &args).unwrap();
                assert_eq!(tool, r.new_tool, "{} {:?} {:?}", r.old_tool, r.resources, act);
            }
            let new_args = if r.actions == BROWSER_ACT { json!({"action": r.actions[0]}) } else { json!({}) };
            let (old, back) = translate(r.new_tool, &new_args).unwrap();
            assert!(ROWS.iter().any(|x| x.old_tool == old), "{} → {old}", r.new_tool);
            assert_eq!(translate(&old, &back).unwrap().0, r.new_tool, "{} does not come back", r.new_tool);
        }
    }
}
