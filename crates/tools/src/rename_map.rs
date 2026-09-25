//! Old tool call shapes → the current tool set, for the one-time upgrade
//! migration that rewrites stored workflows, employee manifests, installed
//! skills, grants and scheduled jobs. Never consulted at runtime: nothing
//! aliases an old name to a running tool. Each tool package adds the rows
//! for the shapes it retires.

/// One retired call shape and the tool that does its job now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rename {
    pub tool: &'static str,
    /// The old `resource`, when the shape had one.
    pub resource: Option<&'static str>,
    /// The old `action`, when the shape had one.
    pub action: Option<&'static str>,
    pub to: &'static str,
    /// Parameters renamed on the way: (old, new).
    pub params: &'static [(&'static str, &'static str)],
}

pub const RENAMES: &[Rename] = &[
    Rename {
        tool: "tool_search",
        resource: None,
        action: None,
        to: crate::find_tools::FIND_TOOLS,
        params: &[],
    },
    // os: files and shell (Tools WP1).
    Rename { tool: "os", resource: Some("file"), action: Some("read"), to: "read_file", params: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("write"), to: "write_file", params: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("edit"), to: "edit_file", params: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("glob"), to: "run_command", params: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("grep"), to: "run_command", params: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("list"), to: "run_command", params: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("share"), to: "share_file", params: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("present"), to: "share_file", params: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("send"), to: "share_file", params: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("convert"), to: "convert_file", params: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("checkpoint"), to: "checkpoint_files", params: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("checkpoints"), to: "list_checkpoints", params: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("restore"), to: "restore_checkpoint", params: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("plan"), to: "write_plan", params: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("plan_check"), to: "check_plan", params: &[] },
    Rename { tool: "os", resource: Some("shell"), action: Some("exec"), to: "run_command", params: &[] },
    Rename { tool: "os", resource: Some("shell"), action: Some("poll"), to: "read_output", params: &[("session_id", "task_id")] },
    Rename { tool: "os", resource: Some("shell"), action: Some("log"), to: "read_output", params: &[("session_id", "task_id")] },
    Rename { tool: "os", resource: Some("shell"), action: Some("kill"), to: "stop_task", params: &[("session_id", "task_id")] },
    Rename { tool: "os", resource: Some("shell"), action: Some("list"), to: "list_processes", params: &[] },
    Rename { tool: "os", resource: Some("shell"), action: Some("info"), to: "list_processes", params: &[] },
    Rename { tool: "os", resource: Some("shell"), action: Some("write"), to: "send_input", params: &[("session_id", "task_id"), ("data", "text")] },
    // The flat names the runtime used to alias to os file/shell calls.
    Rename { tool: "file_read", resource: None, action: None, to: "read_file", params: &[] },
    Rename { tool: "fileread", resource: None, action: None, to: "read_file", params: &[] },
    Rename { tool: "read", resource: None, action: None, to: "read_file", params: &[] },
    Rename { tool: "file_write", resource: None, action: None, to: "write_file", params: &[] },
    Rename { tool: "filewrite", resource: None, action: None, to: "write_file", params: &[] },
    Rename { tool: "file_edit", resource: None, action: None, to: "edit_file", params: &[] },
    Rename { tool: "fileedit", resource: None, action: None, to: "edit_file", params: &[] },
    Rename { tool: "edit", resource: None, action: None, to: "edit_file", params: &[] },
    Rename { tool: "grep", resource: None, action: None, to: "run_command", params: &[] },
    Rename { tool: "grep_tool", resource: None, action: None, to: "run_command", params: &[] },
    Rename { tool: "greptool", resource: None, action: None, to: "run_command", params: &[] },
    Rename { tool: "file_grep", resource: None, action: None, to: "run_command", params: &[] },
    Rename { tool: "glob", resource: None, action: None, to: "run_command", params: &[] },
    Rename { tool: "glob_tool", resource: None, action: None, to: "run_command", params: &[] },
    Rename { tool: "globtool", resource: None, action: None, to: "run_command", params: &[] },
    Rename { tool: "file_glob", resource: None, action: None, to: "run_command", params: &[] },
    Rename { tool: "bash", resource: None, action: None, to: "run_command", params: &[] },
    Rename { tool: "shell", resource: None, action: None, to: "run_command", params: &[] },
    Rename { tool: "bash_tool", resource: None, action: None, to: "run_command", params: &[] },
    Rename { tool: "bashtool", resource: None, action: None, to: "run_command", params: &[] },
    Rename { tool: "exec", resource: None, action: None, to: "run_command", params: &[] },
    // The `web` tool (tools WP5): one tool per resource and action.
    web("search", "search", "search_web", &[]),
    web("http", "fetch", "fetch_url", &[]),
    web("http", "get", "fetch_url", &[]),
    web("http", "sanitize", "fetch_url", &[]),
    web("http", "head", "http_request", &[]),
    web("http", "post", "http_request", &[]),
    web("http", "put", "http_request", &[]),
    web("http", "patch", "http_request", &[]),
    web("http", "delete", "http_request", &[]),
    web("browser", "navigate", "browser_open", &[]),
    web("browser", "read_page", "browser_read", &[("maxChars", "max_chars"), ("refId", "ref_id")]),
    web("browser", "find", "browser_find", &[]),
    web("browser", "click", "browser_act", &[]),
    web("browser", "hover", "browser_act", &[]),
    web("browser", "type", "browser_act", &[]),
    web("browser", "press", "browser_act", &[]),
    web("browser", "scroll", "browser_act", &[]),
    web("browser", "drag", "browser_act", &[]),
    web("browser", "select", "browser_act", &[]),
    web("browser", "wait", "browser_act", &[]),
    web("browser", "screenshot", "browser_act", &[]),
    web("browser", "fill", "browser_fill_form", &[]),
    web("browser", "fill_form", "browser_fill_form", &[]),
    web("browser", "evaluate", "browser_run_js", &[]),
    web("browser", "list_tabs", "browser_list_tabs", &[]),
    web("browser", "new_tab", "browser_new_tab", &[]),
    web("browser", "close_tab", "browser_close_tab", &[("tabId", "tab_id")]),
    web("browser", "read_console_messages", "browser_console", &[("onlyErrors", "only_errors")]),
    web("devtools", "console", "browser_console", &[("filter", "pattern"), ("onlyErrors", "only_errors")]),
    web("browser", "read_network_requests", "browser_network", &[("urlPattern", "url_pattern")]),
    web("browser", "file_upload", "browser_upload", &[]),
    web("browser", "resize_window", "browser_resize", &[]),
    web("browser", "history", "browser_history", &[]),
    web("browser", "status", "browser_status", &[]),
    web("browser", "browser_batch", "browser_batch", &[("actions", "steps")]),
    web("browser", "webmcp_list", "browser_page_tools", &[]),
    web("browser", "webmcp_call", "browser_call_page_tool", &[]),
    // The `event` tool (tools WP9): one tool per scheduling job. A pause and
    // a resume both become set_schedule_paused (`paused: true` / `false`).
    flat("event", "create", "create_schedule", &[("schedule", "cron")]),
    flat("event", "list", "list_schedules", &[]),
    flat("event", "delete", "delete_schedule", &[]),
    flat("event", "pause", "set_schedule_paused", &[]),
    flat("event", "resume", "set_schedule_paused", &[]),
    flat("event", "run", "run_schedule_now", &[]),
    flat("event", "history", "schedule_history", &[]),
    // The `team` tool (tools WP9). A team post is send_message to the team.
    flat("team", "create", "create_team", &[("agents", "members")]),
    flat("team", "update", "update_team", &[("agents", "members")]),
    flat("team", "edit", "update_team", &[("agents", "members")]),
    flat("team", "list", "list_teams", &[]),
    flat("team", "send", "send_message", TEAM_POST),
    flat("team", "post", "send_message", TEAM_POST),
    flat("team", "messages", "team_messages", &[]),
    flat("team", "history", "team_messages", &[]),
    flat("team", "members", "team_members", &[]),
    // The `loop` tool (tools WP9): the NeboAI hub, one tool per job. A
    // direct message and a channel post are one send_loop_message; the old
    // `group` resource is a loop; a `workroom` is a team.
    hub("dm", "send", "send_loop_message", &[]),
    hub("dm", "share", "share_to_loop", &[]),
    hub("channel", "send", "send_loop_message", &[]),
    hub("channel", "share", "share_to_loop", &[]),
    hub("channel", "ensure", "ensure_loop_channel", &[]),
    hub("channel", "list", "list_loop_channels", &[]),
    hub("channel", "messages", "read_loop_channel", &[]),
    hub("channel", "members", "loop_channel_members", &[]),
    hub("loop", "list", "list_loops", &[]),
    hub("loop", "get", "get_loop", &[]),
    hub("loop", "members", "loop_members", &[]),
    hub("group", "list", "list_loops", &[]),
    hub("group", "get", "get_loop", &[]),
    hub("group", "members", "loop_members", &[]),
    hub("topic", "subscribe", "subscribe_topic", &[]),
    hub("topic", "unsubscribe", "unsubscribe_topic", &[]),
    hub("topic", "status", "topic_status", &[]),
    hub("workroom", "create", "create_team", &[("agents", "members")]),
    hub("workroom", "ensure", "create_team", &[("agents", "members")]),
    hub("workroom", "list", "list_teams", &[]),
    hub("workroom", "send", "send_message", TEAM_POST),
    hub("workroom", "messages", "team_messages", &[]),
    hub("workroom", "members", "team_members", &[]),
    // The `work` tool (tools WP9). Its lifecycle actions took no resource;
    // the calls on one workflow named it in `resource`, which is now the
    // `workflow` parameter. `agent` is now `employee`. Enabling is a state
    // (`enabled: true` / `false`), not a toggle.
    flat("work", "list", "list_workflows", EMPLOYEE),
    flat("work", "install", "install_workflow", &[]),
    flat("work", "uninstall", "uninstall_workflow", &[]),
    flat("work", "create", "create_workflow", EMPLOYEE),
    flat("work", "update", "update_workflow", EMPLOYEE),
    flat("work", "edit", "update_workflow", EMPLOYEE),
    flat("work", "delete", "delete_workflow", EMPLOYEE),
    flat("work", "cancel", "stop_task", &[("id", "task_id")]),
    flat("work", "run", "run_workflow", ON_WORKFLOW),
    flat("work", "status", "workflow_status", ON_WORKFLOW),
    flat("work", "runs", "list_workflow_runs", ON_WORKFLOW),
    flat("work", "toggle", "set_workflow_enabled", ON_WORKFLOW),
    // The `emit` tool (tools WP9).
    Rename { tool: "emit", resource: None, action: None, to: "emit_event", params: &[] },
];

/// A team post's parameters as send_message takes them.
const TEAM_POST: &[(&str, &str)] = &[("team", "to"), ("text", "message")];
/// The employee whose workflows a call manages.
const EMPLOYEE: &[(&str, &str)] = &[("agent", "employee")];
/// A call on one workflow: the workflow and the employee it belongs to.
const ON_WORKFLOW: &[(&str, &str)] = &[("resource", "workflow"), ("agent", "employee")];

/// A retired `tool(action)` shape of a tool with no resources.
const fn flat(
    tool: &'static str,
    action: &'static str,
    to: &'static str,
    params: &'static [(&'static str, &'static str)],
) -> Rename {
    Rename { tool, resource: None, action: Some(action), to, params }
}

/// A retired `loop(resource, action)` shape.
const fn hub(
    resource: &'static str,
    action: &'static str,
    to: &'static str,
    params: &'static [(&'static str, &'static str)],
) -> Rename {
    Rename { tool: "loop", resource: Some(resource), action: Some(action), to, params }
}

/// A retired `web(resource, action)` shape.
const fn web(
    resource: &'static str,
    action: &'static str,
    to: &'static str,
    params: &'static [(&'static str, &'static str)],
) -> Rename {
    Rename { tool: "web", resource: Some(resource), action: Some(action), to, params }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_points_at_a_current_name_and_old_names_are_unique_per_shape() {
        for (i, r) in RENAMES.iter().enumerate() {
            assert!(crate::registry::is_tool_name(r.to), "{} is not a current tool name", r.to);
            assert!(
                RENAMES[..i]
                    .iter()
                    .all(|o| (o.tool, o.resource, o.action) != (r.tool, r.resource, r.action)),
                "duplicate shape {r:?}"
            );
        }
    }
}
