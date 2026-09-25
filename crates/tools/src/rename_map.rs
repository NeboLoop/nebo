//! Old tool call shapes → the current tool set, for the one-time upgrade
//! migration that rewrites stored workflows, employee manifests, installed
//! skills, grants and scheduled jobs (`server::stored_tool_names`). Never
//! consulted at runtime: nothing aliases an old name to a running tool.
//! Each tool package adds the rows for the shapes it retires.
//!
//! The rewriting itself lives here too, beside the table it reads: a call
//! written in text (`plugin(resource: "rentcast", action: "exec", …)`), a
//! stored call (`{name, input}`), a list of tool names and a tool allowlist
//! entry (`agent:memory`) each become the current shape.

use serde_json::Value;

/// One retired call shape and the tool that does its job now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rename {
    pub tool: &'static str,
    /// The old `resource`, when the shape had one. `None` matches any
    /// resource: it is carried by a placeholder or a renamed parameter, or
    /// it is dropped.
    pub resource: Option<&'static str>,
    /// The old `action`, when the shape had one. `None` matches a call that
    /// names no action (or any action, when the call moves onto a tool that
    /// still takes one).
    pub action: Option<&'static str>,
    /// The tool that does the job now. A family written per call carries a
    /// placeholder the migration fills from the old call: `{resource}` is
    /// its `resource` (`plugin__{resource}`), and `{operation}` is its
    /// `operation` written as a tool name, its `input` fields becoming the
    /// call's own.
    pub to: &'static str,
    /// Parameters renamed on the way: (old, new).
    pub params: &'static [(&'static str, &'static str)],
    /// Parameters the new call carries that the old shape said with its
    /// action: (name, JSON value). A value may name the old call's own
    /// parameters, `{name}` or `{name:default}`, which it then takes over.
    pub sets: &'static [(&'static str, &'static str)],
}

impl Rename {
    const fn sets(mut self, sets: &'static [(&'static str, &'static str)]) -> Self {
        self.sets = sets;
        self
    }
}

pub const RENAMES: &[Rename] = &[
    Rename {
        tool: "tool_search",
        resource: None,
        action: None,
        to: crate::find_tools::FIND_TOOLS,
        params: &[],
        sets: &[],
    },
    // The plugin tool: `list` and the `mcp` tool have no successor — the
    // deferred-tool listing names every installed plugin and MCP tool.
    Rename {
        tool: "plugin",
        resource: None,
        action: Some("discover"),
        to: crate::plugin_tools::FIND_PLUGINS,
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "plugin",
        resource: None,
        action: Some("events"),
        to: crate::plugin_tools::READ_PLUGIN_EVENTS,
        params: &[("resource", "plugin")],
        sets: &[],
    },
    Rename {
        tool: "plugin",
        resource: None,
        action: Some("exec"),
        to: "plugin__{resource}",
        params: &[],
        sets: &[],
    },
    // A plugin's usage: its binary prints it (`<service> --help`), as its
    // skills say.
    Rename {
        tool: "plugin",
        resource: None,
        action: Some("help"),
        to: "plugin__{resource}",
        params: &[],
        sets: &[("command", "\"{command} --help\"")],
    },
    // A typed port call (`operation` + `input`) names no action.
    Rename {
        tool: "plugin",
        resource: None,
        action: None,
        to: "{operation}",
        params: &[],
        sets: &[],
    },
    // Tools WP2: helpers, memory, asking and reaching the owner. A task
    // delete is `update_task` with status "deleted"; a notify alert is
    // `push_notification` with `urgent: true`.
    Rename {
        tool: "agent",
        resource: Some("memory"),
        action: Some("store"),
        to: "remember",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("memory"),
        action: Some("save"),
        to: "remember",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("memory"),
        action: Some("recall"),
        to: "recall",
        params: &[("key", "query")],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("memory"),
        action: Some("search"),
        to: "recall",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("memory"),
        action: Some("list"),
        to: "recall",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("memory"),
        action: Some("delete"),
        to: "forget",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("spawn"),
        to: "delegate",
        params: &[("agent_type", "helper_type")],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("spawn_parallel"),
        to: "delegate",
        params: &[("agent_type", "helper_type"), ("isolate", "isolation")],
        sets: &[],
    },
    // The old orchestrate took one prompt for a whole job. Fan-out is several
    // delegate calls in one response; a stored call becomes one helper
    // given the job, which starts its own helpers for independent parts.
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("orchestrate"),
        to: "delegate",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("send"),
        to: "send_message",
        params: &[("task_id", "to")],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("status"),
        to: "read_output",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("cancel"),
        to: "stop_task",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("create"),
        to: "create_task",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("update"),
        to: "update_task",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("delete"),
        to: "update_task",
        params: &[],
        sets: &[("status", "\"deleted\"")],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("get"),
        to: "get_task",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("list"),
        to: "list_tasks",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("assign"),
        to: "assign_task",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("assignments"),
        to: "list_assignments",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("ask"),
        action: Some("prompt"),
        to: "ask_owner",
        params: &[("text", "question")],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("ask"),
        action: Some("confirm"),
        to: "ask_owner",
        params: &[("text", "question")],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("ask"),
        action: Some("select"),
        to: "ask_owner",
        params: &[("text", "question")],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("runs"),
        action: Some("list"),
        to: "list_runs",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("runs"),
        action: Some("cancel"),
        to: "stop_task",
        params: &[("run_id", "task_id")],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("session"),
        action: Some("query"),
        to: "search_history",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("session"),
        action: Some("history"),
        to: "read_session",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("session"),
        action: Some("list"),
        to: "list_sessions",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("advisors"),
        action: Some("deliberate"),
        to: "consult_advisors",
        params: &[("task", "question")],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("advisors"),
        action: Some("list"),
        to: "list_advisors",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("research"),
        action: Some("deep_research"),
        to: "deep_research",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("research"),
        action: Some("research"),
        to: "quick_research",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("research"),
        action: Some("submit_findings"),
        to: "submit_findings",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("profile"),
        action: Some("get"),
        to: "get_profile",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("profile"),
        action: Some("update"),
        to: "update_profile",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("profile"),
        action: Some("open_billing"),
        to: "open_billing",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "message",
        resource: Some("owner"),
        action: Some("notify"),
        to: "message_owner",
        params: &[("text", "message")],
        sets: &[],
    },
    Rename {
        tool: "message",
        resource: Some("notify"),
        action: Some("send"),
        to: "push_notification",
        params: &[("text", "message")],
        sets: &[],
    },
    Rename {
        tool: "message",
        resource: Some("notify"),
        action: Some("alert"),
        to: "push_notification",
        params: &[("text", "message")],
        sets: &[("urgent", "true")],
    },
    Rename {
        tool: "message",
        resource: Some("notify"),
        action: Some("dnd_status"),
        to: "check_dnd",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "os",
        resource: Some("notification"),
        action: Some("send"),
        to: "push_notification",
        params: &[],
        sets: &[],
    },
    Rename {
        tool: "os",
        resource: Some("notification"),
        action: Some("alert"),
        to: "push_notification",
        params: &[],
        sets: &[("urgent", "true")],
    },
    // os: files and shell (Tools WP1).
    Rename { tool: "os", resource: Some("file"), action: Some("read"), to: "read_file", params: &[], sets: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("write"), to: "write_file", params: &[], sets: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("edit"), to: "edit_file", params: &[], sets: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("glob"), to: "run_command", params: &[], sets: &[("command", "\"find {path:.} -name '{pattern}'\"")] },
    Rename { tool: "os", resource: Some("file"), action: Some("grep"), to: "run_command", params: &[], sets: &[("command", "\"grep -rn '{pattern}' {path:.}\"")] },
    Rename { tool: "os", resource: Some("file"), action: Some("list"), to: "run_command", params: &[], sets: &[("command", "\"ls {path:.}\"")] },
    Rename { tool: "os", resource: Some("file"), action: Some("share"), to: "share_file", params: &[], sets: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("present"), to: "share_file", params: &[], sets: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("send"), to: "share_file", params: &[], sets: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("convert"), to: "convert_file", params: &[], sets: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("checkpoint"), to: "checkpoint_files", params: &[], sets: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("checkpoints"), to: "list_checkpoints", params: &[], sets: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("restore"), to: "restore_checkpoint", params: &[], sets: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("plan"), to: "write_plan", params: &[], sets: &[] },
    Rename { tool: "os", resource: Some("file"), action: Some("plan_check"), to: "check_plan", params: &[], sets: &[] },
    Rename { tool: "os", resource: Some("shell"), action: Some("exec"), to: "run_command", params: &[], sets: &[] },
    Rename { tool: "os", resource: Some("shell"), action: Some("poll"), to: "read_output", params: &[("session_id", "task_id")], sets: &[] },
    Rename { tool: "os", resource: Some("shell"), action: Some("log"), to: "read_output", params: &[("session_id", "task_id")], sets: &[] },
    Rename { tool: "os", resource: Some("shell"), action: Some("kill"), to: "stop_task", params: &[("session_id", "task_id")], sets: &[] },
    Rename { tool: "os", resource: Some("shell"), action: Some("list"), to: "list_processes", params: &[], sets: &[] },
    Rename { tool: "os", resource: Some("shell"), action: Some("info"), to: "list_processes", params: &[], sets: &[] },
    Rename { tool: "os", resource: Some("shell"), action: Some("write"), to: "send_input", params: &[("session_id", "task_id"), ("data", "text")], sets: &[] },
    // The flat names the runtime used to alias to os file/shell calls.
    Rename { tool: "file_read", resource: None, action: None, to: "read_file", params: &[], sets: &[] },
    Rename { tool: "fileread", resource: None, action: None, to: "read_file", params: &[], sets: &[] },
    Rename { tool: "read", resource: None, action: None, to: "read_file", params: &[], sets: &[] },
    Rename { tool: "file_write", resource: None, action: None, to: "write_file", params: &[], sets: &[] },
    Rename { tool: "filewrite", resource: None, action: None, to: "write_file", params: &[], sets: &[] },
    Rename { tool: "file_edit", resource: None, action: None, to: "edit_file", params: &[], sets: &[] },
    Rename { tool: "fileedit", resource: None, action: None, to: "edit_file", params: &[], sets: &[] },
    Rename { tool: "edit", resource: None, action: None, to: "edit_file", params: &[], sets: &[] },
    Rename { tool: "grep", resource: None, action: None, to: "run_command", params: &[], sets: &[("command", "\"grep -rn '{pattern}' {path:.}\"")] },
    Rename { tool: "grep_tool", resource: None, action: None, to: "run_command", params: &[], sets: &[("command", "\"grep -rn '{pattern}' {path:.}\"")] },
    Rename { tool: "greptool", resource: None, action: None, to: "run_command", params: &[], sets: &[("command", "\"grep -rn '{pattern}' {path:.}\"")] },
    Rename { tool: "file_grep", resource: None, action: None, to: "run_command", params: &[], sets: &[("command", "\"grep -rn '{pattern}' {path:.}\"")] },
    Rename { tool: "glob", resource: None, action: None, to: "run_command", params: &[], sets: &[("command", "\"find {path:.} -name '{pattern}'\"")] },
    Rename { tool: "glob_tool", resource: None, action: None, to: "run_command", params: &[], sets: &[("command", "\"find {path:.} -name '{pattern}'\"")] },
    Rename { tool: "globtool", resource: None, action: None, to: "run_command", params: &[], sets: &[("command", "\"find {path:.} -name '{pattern}'\"")] },
    Rename { tool: "file_glob", resource: None, action: None, to: "run_command", params: &[], sets: &[("command", "\"find {path:.} -name '{pattern}'\"")] },
    Rename { tool: "bash", resource: None, action: None, to: "run_command", params: &[], sets: &[] },
    Rename { tool: "shell", resource: None, action: None, to: "run_command", params: &[], sets: &[] },
    Rename { tool: "bash_tool", resource: None, action: None, to: "run_command", params: &[], sets: &[] },
    Rename { tool: "bashtool", resource: None, action: None, to: "run_command", params: &[], sets: &[] },
    Rename { tool: "exec", resource: None, action: None, to: "run_command", params: &[], sets: &[] },
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
    web("browser", "go_back", "browser_history", &[]).sets(&[("direction", "\"back\"")]),
    web("browser", "go_forward", "browser_history", &[]).sets(&[("direction", "\"forward\"")]),
    web("browser", "close", "browser_close_tab", &[("tabId", "tab_id")]),
    web("browser", "status", "browser_status", &[]),
    web("browser", "browser_batch", "browser_batch", &[("actions", "steps")]),
    web("browser", "webmcp_list", "browser_page_tools", &[]),
    web("browser", "webmcp_call", "browser_call_page_tool", &[]),
    // The `agent` tool's employee registry (tools WP3): one tool per action.
    // activate and deactivate both become set_employee_active (active: true
    // or false).
    employee("list", "list_employees", &[]),
    employee("info", "get_employee", &[]),
    employee("discover", "find_employees", &[]),
    employee("install", "hire_employee", &[]),
    employee("create", "create_employee", &[]),
    employee("update", "update_employee", &[("prompt", "instructions")]),
    employee("delete", "delete_employee", &[]),
    employee("activate", "set_employee_active", &[]).sets(&[("active", "true")]),
    employee("deactivate", "set_employee_active", &[]).sets(&[("active", "false")]),
    employee("setup", "setup_employee", &[]),
    employee("repair", "repair_employee", &[]),
    employee("reload", "reload_employee", &[]),
    employee("stats", "employee_stats", &[]),
    // The `skill` tool (tools WP4): `skill(name)` with no action was a load.
    // `unload` has no successor: switching a skill off is the owner's, in
    // the app.
    skill(None, crate::skill_tool::USE_SKILL),
    skill(Some("load"), crate::skill_tool::USE_SKILL),
    skill(Some("list"), "find_skills"),
    skill(Some("discover"), "find_skills"),
    skill(Some("browse"), "read_skill_file"),
    skill(Some("read_resource"), "read_skill_file"),
    skill(Some("create"), "save_skill"),
    skill(Some("update"), "save_skill"),
    skill(Some("delete"), "delete_skill"),
    skill(Some("install"), "install_skill"),
    skill(Some("configure"), "configure_skill"),
    skill(Some("secrets"), "configure_skill"),
    skill(Some("rate"), "rate_skill"),
    skill(Some("reviews"), "read_skill_reviews"),
    // The `event` tool (tools WP9): one tool per scheduling job. A pause and
    // a resume both become set_schedule_paused (`paused: true` / `false`).
    flat("event", "create", "create_schedule", &[("schedule", "cron")]),
    flat("event", "list", "list_schedules", &[]),
    flat("event", "delete", "delete_schedule", &[]),
    flat("event", "pause", "set_schedule_paused", &[]).sets(&[("paused", "true")]),
    flat("event", "resume", "set_schedule_paused", &[]).sets(&[("paused", "false")]),
    flat("event", "run", "run_schedule_now", &[]),
    flat("event", "history", "schedule_history", &[]),
    // A coworker message (tools WP9): send_message to the employee.
    Rename { tool: "message", resource: Some("coworker"), action: Some("send"), to: "send_message", params: &[("text", "message")], sets: &[] },
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
    Rename { tool: "emit", resource: None, action: None, to: "emit_event", params: &[], sets: &[] },
    // Shapes older than the agent tool that published content still writes:
    // an event it emits, and a message it sends.
    Rename { tool: "agent", resource: Some("event"), action: Some("emit"), to: "emit_event", params: &[], sets: &[] },
    Rename { tool: "agent", resource: Some("message"), action: Some("send"), to: "send_message", params: &[("text", "message")], sets: &[] },
    // The pre-STRAP names the registry used to alias at call time. Their
    // calls are `os` calls; the single-purpose ones name their resource.
    Rename { tool: "organizer", resource: None, action: None, to: "os", params: &[], sets: &[] },
    Rename { tool: "desktop", resource: None, action: None, to: "os", params: &[], sets: &[] },
    Rename { tool: "system", resource: None, action: None, to: "os", params: &[], sets: &[] },
    Rename { tool: "app", resource: None, action: None, to: "os", params: &[], sets: &[("resource", "\"app\"")] },
    Rename { tool: "settings", resource: None, action: None, to: "os", params: &[], sets: &[("resource", "\"settings\"")] },
    Rename { tool: "music", resource: None, action: None, to: "os", params: &[], sets: &[("resource", "\"music\"")] },
    Rename { tool: "keychain", resource: None, action: None, to: "os", params: &[], sets: &[("resource", "\"keychain\"")] },
    Rename { tool: "spotlight", resource: None, action: None, to: "os", params: &[], sets: &[("resource", "\"search\"")] },
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
    Rename { tool, resource: None, action: Some(action), to, params, sets: &[] }
}

/// A retired `loop(resource, action)` shape.
const fn hub(
    resource: &'static str,
    action: &'static str,
    to: &'static str,
    params: &'static [(&'static str, &'static str)],
) -> Rename {
    Rename { tool: "loop", resource: Some(resource), action: Some(action), to, params, sets: &[] }
}

/// A retired `skill(action)` shape.
const fn skill(action: Option<&'static str>, to: &'static str) -> Rename {
    Rename { tool: "skill", resource: None, action, to, params: &[], sets: &[] }
}

/// A retired `web(resource, action)` shape.
const fn web(
    resource: &'static str,
    action: &'static str,
    to: &'static str,
    params: &'static [(&'static str, &'static str)],
) -> Rename {
    Rename { tool: "web", resource: Some(resource), action: Some(action), to, params, sets: &[] }
}

/// A retired `agent(resource: "registry", action)` shape.
const fn employee(
    action: &'static str,
    to: &'static str,
    params: &'static [(&'static str, &'static str)],
) -> Rename {
    Rename { tool: "agent", resource: Some("registry"), action: Some(action), to, params, sets: &[] }
}

/// Old tools whose name is still a tool, with fewer jobs: a call or a grant
/// no row covers keeps its name.
pub const KEPT: &[&str] = &["os", "message"];

/// Current tools that still take `resource` and `action`: a call moved onto
/// one keeps them.
const DISPATCHING: &[&str] = &["os"];

/// The action an old tool ran when a call named none (and no operation).
const DEFAULT_ACTIONS: &[(&str, &str)] = &[("plugin", "exec")];

/// Old tools that worked out a missing `resource` from the action: a call
/// with none moves by its action alone, when that action names one job.
const INFERRED_RESOURCE: &[&str] = &["web"];

/// Every tool name a row retires or narrows, once each.
pub fn old_tools() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Vec::new();
    for r in RENAMES {
        if !names.contains(&r.tool) {
            names.push(r.tool);
        }
    }
    names
}

/// The old names that are no tool now.
pub fn retired_tools() -> Vec<&'static str> {
    old_tools().into_iter().filter(|t| !KEPT.contains(t)).collect()
}

/// The row an old call moves by: the most specific one its tool, resource,
/// action and operation match.
fn row_for(
    tool: &str,
    resource: Option<&str>,
    action: Option<&str>,
    operation: bool,
) -> Option<&'static Rename> {
    let default = || DEFAULT_ACTIONS.iter().find(|(t, _)| *t == tool).map(|(_, a)| *a);
    let action = action.or_else(|| if operation { None } else { default() });
    RENAMES
        .iter()
        .filter(|r| {
            r.tool == tool
                && r.resource.is_none_or(|x| Some(x) == resource)
                && match r.action {
                    Some(a) => Some(a) == action,
                    None => action.is_none() || DISPATCHING.contains(&r.to),
                }
                && (r.to != "{operation}" || operation)
                && (!r.to.contains("{resource}") || resource.is_some())
        })
        .max_by_key(|r| u8::from(r.resource.is_some()) * 2 + u8::from(r.action.is_some()))
        .or_else(|| {
            let (None, Some(action)) = (resource, action) else { return None };
            if !INFERRED_RESOURCE.contains(&tool) {
                return None;
            }
            let jobs: Vec<&'static Rename> =
                RENAMES.iter().filter(|r| r.tool == tool && r.action == Some(action)).collect();
            let one_job = jobs.windows(2).all(|w| w[0].to == w[1].to && w[0].params == w[1].params && w[0].sets == w[1].sets);
            jobs.first().copied().filter(|_| one_job)
        })
}

/// A catalog operation written where a plugin command goes
/// (`marketing.social-media-manager.social.queue.get`): dotted segments,
/// no spaces, at least three.
fn is_operation(command: &str) -> bool {
    let parts: Vec<&str> = command.split('.').collect();
    parts.len() >= 3
        && parts.iter().all(|p| {
            !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
}

/// The tool an old port call names: its operation's catalog suffix, as the
/// operation tools are named.
fn operation_tool(operation: &str) -> String {
    crate::operation_tools::operation_tool_name(&crate::plugin_tool::port_suffix(operation))
}

/// A call argument's value, in the form it is stored.
pub trait ArgValue: Clone {
    /// The value as a string, when it is one.
    fn text(&self) -> Option<String>;
    /// The fields of an object value.
    fn fields(&self) -> Option<Vec<Arg<Self>>>;
    /// A value written from its JSON literal.
    fn literal(json: &str) -> Self;
}

/// One argument of a call. A positional argument has an empty key.
#[derive(Debug, Clone, PartialEq)]
pub struct Arg<V> {
    pub key: String,
    pub value: V,
}

/// Where an old call goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Moved<V> {
    /// The current call.
    To { name: String, args: Vec<Arg<V>> },
    /// Not an old shape: a kept tool's call no row covers, or a name that
    /// was never retired.
    Kept,
    /// A retired shape with no successor.
    NoSuccessor,
}

fn position<V>(args: &[Arg<V>], key: &str) -> Option<usize> {
    args.iter().position(|a| a.key == key)
}

fn text_of<V: ArgValue>(args: &[Arg<V>], key: &str) -> Option<String> {
    position(args, key).and_then(|i| args[i].value.text())
}

/// Move one old call onto the current tool set.
pub fn move_call<V: ArgValue>(tool: &str, mut args: Vec<Arg<V>>) -> Moved<V> {
    if !old_tools().contains(&tool) {
        return Moved::Kept;
    }
    let resource = text_of(&args, "resource");
    let action = text_of(&args, "action");
    // A plugin call whose command is a catalog operation calls that
    // operation.
    let command_is_operation = tool == "plugin"
        && action.is_none()
        && text_of(&args, "operation").is_none()
        && text_of(&args, "command").is_some_and(|c| is_operation(c.trim()));
    if command_is_operation && let Some(i) = position(&args, "command") {
        args[i].key = "operation".to_string();
    }
    let operation = text_of(&args, "operation");
    let Some(row) = row_for(tool, resource.as_deref(), action.as_deref(), operation.is_some()) else {
        return if KEPT.contains(&tool) { Moved::Kept } else { Moved::NoSuccessor };
    };
    for arg in args.iter_mut() {
        if let Some((_, new)) = row.params.iter().find(|(old, _)| *old == arg.key) {
            arg.key = new.to_string();
        }
    }
    let mut name = row.to.to_string();
    if name.contains("{resource}") {
        name = name.replace("{resource}", resource.as_deref().unwrap_or_default());
        args.retain(|a| a.key != "resource");
    }
    if name == "{operation}" {
        name = operation_tool(operation.as_deref().unwrap_or_default().trim());
        args.retain(|a| a.key != "operation");
        if let Some(i) = position(&args, "input")
            && let Some(fields) = args[i].value.fields()
        {
            args.splice(i..=i, fields);
        }
    }
    if !DISPATCHING.contains(&name.as_str()) {
        args.retain(|a| a.key != "resource" && a.key != "action");
    }
    for (key, template) in row.sets {
        let Some((json, taken)) = fill(template, &args) else { continue };
        args.retain(|a| !taken.contains(&a.key));
        if position(&args, key).is_none() {
            // An os call names its resource first.
            let at = if *key == "resource" { 0 } else { args.len() };
            args.insert(at, Arg { key: key.to_string(), value: V::literal(&json) });
        }
    }
    Moved::To { name, args }
}

/// A `sets` value with its `{name}` / `{name:default}` parts filled from the
/// call, and the parameters it took. `None` when a part has no value.
fn fill<V: ArgValue>(template: &str, args: &[Arg<V>]) -> Option<(String, Vec<String>)> {
    let mut out = String::new();
    let mut taken = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let end = start + rest[start..].find('}')?;
        out.push_str(&rest[..start]);
        let part = &rest[start + 1..end];
        let (key, default) = match part.split_once(':') {
            Some((k, d)) => (k, Some(d)),
            None => (part, None),
        };
        let value = match text_of(args, key) {
            Some(v) => {
                taken.push(key.to_string());
                v
            }
            None => default?.to_string(),
        };
        // The value sits inside a JSON string literal.
        let escaped = serde_json::to_string(&value).unwrap_or_default();
        out.push_str(&escaped[1..escaped.len() - 1]);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Some((out, taken))
}

impl ArgValue for Value {
    fn text(&self) -> Option<String> {
        self.as_str().map(str::to_string)
    }

    fn fields(&self) -> Option<Vec<Arg<Self>>> {
        self.as_object()
            .map(|o| o.iter().map(|(k, v)| Arg { key: k.clone(), value: v.clone() }).collect())
    }

    fn literal(json: &str) -> Self {
        serde_json::from_str(json).unwrap_or(Value::Null)
    }
}

/// A stored call (`{name, input}`) on the current tool set. `None` when it
/// is not an old shape, or has no successor.
pub fn move_stored_call(name: &str, input: &Value) -> Option<(String, Value)> {
    let args = input.fields().unwrap_or_default();
    match move_call(name, args) {
        Moved::To { name, args } => {
            let input: serde_json::Map<String, Value> = args.into_iter().map(|a| (a.key, a.value)).collect();
            Some((name, Value::Object(input)))
        }
        Moved::Kept | Moved::NoSuccessor => None,
    }
}

/// An argument as written in text: the source of its value.
#[derive(Debug, Clone, PartialEq)]
pub struct Source(pub String);

impl ArgValue for Source {
    fn text(&self) -> Option<String> {
        let s = self.0.trim();
        let quoted = |q: char| s.len() >= 2 && s.starts_with(q) && s.ends_with(q);
        if quoted('"') {
            return Some(serde_json::from_str::<String>(s).unwrap_or_else(|_| s[1..s.len() - 1].to_string()));
        }
        if quoted('\'') {
            return Some(s[1..s.len() - 1].replace("\\'", "'"));
        }
        let bare = !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || "_-.:/<>*".contains(c));
        bare.then(|| s.to_string())
    }

    fn fields(&self) -> Option<Vec<Arg<Self>>> {
        let s = self.0.trim();
        let inner = s.strip_prefix('{')?.strip_suffix('}')?;
        let args = parse_args(inner);
        args.iter().all(|a| !a.key.is_empty()).then_some(args)
    }

    fn literal(json: &str) -> Self {
        Source(json.to_string())
    }
}

/// What a text rewrite changed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextRewrite {
    pub text: String,
    /// Each call it moved: (old, new).
    pub moved: Vec<(String, String)>,
    /// Old calls with no successor, left as written.
    pub unmoved: Vec<String>,
}

/// A tool call written in text: where it starts and ends, and its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Written<'a> {
    pub name: &'a str,
    /// The whole call, `name(…)`.
    pub call: &'a str,
    start: usize,
    open: usize,
    close: usize,
}

/// Every tool call written in `text`: a name, not part of a longer name, a
/// method or a definition (`fn read(path: &str)`), followed by an argument
/// list whose every argument is named, as a tool call is written
/// (`search_web(query: "x")`, `workflow_status(workflow="w")`).
pub fn written_calls(text: &str) -> Vec<Written<'_>> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let starts_name = (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_')
            && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || b"_.-$".contains(&bytes[i - 1])));
        if !starts_name {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
            j += 1;
        }
        let close = (bytes.get(j) == Some(&b'('))
            .then(|| closing_paren(text, j).or_else(|| closing_paren_on_line(text, j)))
            .flatten();
        let Some(close) = close else {
            i = j;
            continue;
        };
        let args = parse_args(&text[j + 1..close]);
        let defined = ["fn ", "def ", "function ", "func "].iter().any(|kw| text[..i].ends_with(kw));
        if args.is_empty() || args.iter().any(|a| a.key.is_empty()) || defined {
            i = j;
            continue;
        }
        out.push(Written { name: &text[i..j], call: &text[i..=close], start: i, open: j, close });
        i = close + 1;
    }
    out
}

/// Rewrite every old call written in `text` (`web(resource: "search",
/// action: "search", query: "x")` → `search_web(query: "x")`), as
/// [`written_calls`] finds them. Everything else is left exactly as
/// written, so a second pass changes nothing.
pub fn rewrite_text(text: &str) -> TextRewrite {
    let old = old_tools();
    let mut out = TextRewrite::default();
    let mut copied = 0;
    for w in written_calls(text).into_iter().filter(|w| old.contains(&w.name)) {
        let src = &text[w.open + 1..w.close];
        match move_call(w.name, parse_args(src)) {
            Moved::To { name, args } => {
                let new = format!("{name}({})", write_args(&args, key_style(src)));
                out.text.push_str(&text[copied..w.start]);
                out.text.push_str(&new);
                out.moved.push((w.call.to_string(), new));
                copied = w.close + 1;
            }
            Moved::NoSuccessor => out.unmoved.push(w.call.to_string()),
            Moved::Kept => {}
        }
    }
    out.text.push_str(&text[copied..]);
    out
}

/// The index of the `)` closing the `(` at `open`, skipping quoted text and
/// nested brackets. `None` when it never closes.
fn closing_paren(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (off, c) in text[open..].char_indices() {
        if let Some(q) = quote {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                _ if c == q => quote = None,
                _ => {}
            }
            continue;
        }
        match c {
            '"' | '\'' | '`' => quote = Some(c),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return (c == ')').then_some(open + off);
                }
            }
            '\n' if depth == 1 && text[open..open + off].ends_with('\n') => return None,
            _ => {}
        }
    }
    None
}

/// The `)` closing the `(` at `open` on its own line, quotes not counted:
/// a call whose quote was never closed (`command: "a b`)`) still ends
/// where its brackets do.
fn closing_paren_on_line(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (off, c) in text[open..].char_indices() {
        match c {
            '\n' => return None,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return (c == ')').then_some(open + off);
                }
            }
            _ => {}
        }
    }
    None
}

/// Split an argument list at its top-level commas. `key: value`,
/// `key=value` and `"key": value` are named; anything else is positional.
fn parse_args(src: &str) -> Vec<Arg<Source>> {
    let mut pieces = Vec::new();
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut start = 0;
    for (i, c) in src.char_indices() {
        if let Some(q) = quote {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                _ if c == q => quote = None,
                _ => {}
            }
            continue;
        }
        match c {
            '"' | '\'' | '`' => quote = Some(c),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                pieces.push(&src[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    pieces.push(&src[start..]);
    pieces
        .into_iter()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| match named(p) {
            Some((key, value)) => Arg { key: key.to_string(), value: Source(value.trim().to_string()) },
            None => Arg { key: String::new(), value: Source(p.to_string()) },
        })
        .collect()
}

/// `key: value`, `key=value` or `"key": value`, as (key, value).
fn named(piece: &str) -> Option<(&str, &str)> {
    let (key, rest) = match piece.strip_prefix('"') {
        Some(q) => {
            let end = q.find('"')?;
            (&q[..end], q[end + 1..].trim_start())
        }
        None => {
            let end = piece.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))?;
            (&piece[..end], piece[end..].trim_start())
        }
    };
    let starts_ok = key.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    let value = rest.strip_prefix(':').or_else(|| rest.strip_prefix('=').filter(|v| !v.starts_with('=')))?;
    starts_ok.then_some((key, value))
}

/// How a call wrote its names: `key: value` or `key=value`.
#[derive(Debug, Clone, Copy, PartialEq)]
enum KeyStyle {
    Colon,
    Equals,
}

fn key_style(src: &str) -> KeyStyle {
    let first_named = parse_args(src).into_iter().find(|a| !a.key.is_empty());
    match first_named {
        Some(a) if src.contains(&format!("{}=", a.key)) => KeyStyle::Equals,
        _ => KeyStyle::Colon,
    }
}

fn write_args(args: &[Arg<Source>], style: KeyStyle) -> String {
    args.iter()
        .map(|a| match (a.key.is_empty(), style) {
            (true, _) => a.value.0.clone(),
            (false, KeyStyle::Colon) => format!("{}: {}", a.key, a.value.0),
            (false, KeyStyle::Equals) => format!("{}={}", a.key, a.value.0),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// What an entry of a tool list or a tool allowlist names now: `None` when
/// it is not an old name, else its successors. A bare old name stands for
/// every tool that took over one of its jobs (and itself, when it is still a
/// tool). `tool:resource` stands for the jobs of that resource: a scoped
/// successor keeps the scope (`work:weekly` → `run_workflow:weekly`), and a
/// per-plugin family names the plugin (`plugin:shopify` →
/// `plugin__shopify`). A family written per call (`plugin__*`) ends in `*`.
pub fn successor_names(entry: &str) -> Option<Vec<String>> {
    let (tool, resource) = match entry.split_once(':') {
        Some((t, r)) => (t, Some(r)),
        None => (entry, None),
    };
    if !old_tools().contains(&tool) {
        return None;
    }
    let mut names: Vec<String> = Vec::new();
    let mut push = |n: String| {
        if !names.contains(&n) {
            names.push(n);
        }
    };
    let rows = RENAMES.iter().filter(|r| r.tool == tool);
    match resource {
        None => {
            if KEPT.contains(&tool) {
                push(tool.to_string());
            }
            for r in rows.filter(|r| r.to != "{operation}") {
                // A single-purpose name stood for one resource of its tool.
                match r.sets.iter().find(|(k, _)| *k == "resource") {
                    Some((_, v)) => push(format!("{}:{}", r.to, v.trim_matches('"'))),
                    None => push(r.to.replace("{resource}", "*")),
                }
            }
        }
        Some(res) => {
            let specific: Vec<&Rename> = rows.clone().filter(|r| r.resource == Some(res)).collect();
            if specific.is_empty() && KEPT.contains(&tool) {
                return None;
            }
            for r in specific {
                push(r.to.to_string());
            }
            for r in rows.filter(|r| r.resource.is_none() && r.to != "{operation}") {
                if r.to.contains("{resource}") {
                    push(r.to.replace("{resource}", res));
                } else if r.params.iter().any(|(old, _)| *old == "resource") || DISPATCHING.contains(&r.to) {
                    push(format!("{}:{res}", r.to));
                }
            }
        }
    }
    Some(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_points_at_a_current_name_and_old_names_are_unique_per_shape() {
        for (i, r) in RENAMES.iter().enumerate() {
            let to = r.to.replace("{resource}", "quickbooks").replace("{operation}", "ledger_bill_create");
            assert!(crate::registry::is_tool_name(&to), "{} is not a current tool name", r.to);
            assert!(
                RENAMES[..i]
                    .iter()
                    .all(|o| (o.tool, o.resource, o.action) != (r.tool, r.resource, r.action)),
                "duplicate shape {r:?}"
            );
        }
    }

    fn moved(text: &str) -> String {
        rewrite_text(text).text
    }

    /// The owner's stored shapes, and the ones the table's placeholders and
    /// fixed parameters exist for.
    #[test]
    fn old_calls_in_text_become_current_calls() {
        let cases = [
            (
                "plugin(resource: 'shopify', command: 'products list --store go-store-007.myshopify.com') to get all products",
                "plugin__shopify(command: 'products list --store go-store-007.myshopify.com') to get all products",
            ),
            (
                r#"Call it via plugin(resource: "workflow", command: "marketing.social-media-manager.social.queue.get")"#,
                "Call it via social_queue_get()",
            ),
            (
                r#"`plugin(resource: "rentcast", action: "exec", command: "properties search --address \"1 Main St\" --radius 1")`"#,
                r#"`plugin__rentcast(command: "properties search --address \"1 Main St\" --radius 1")`"#,
            ),
            (
                r#"using work(action="status", resource="engagement-desk", agent="Social Media Manager"). Report"#,
                r#"using workflow_status(workflow="engagement-desk", employee="Social Media Manager"). Report"#,
            ),
            (
                r#"skill(action: "browse", name: "neighbor-mail-blasts", path: "reference/actions.md")"#,
                r#"read_skill_file(name: "neighbor-mail-blasts", path: "reference/actions.md")"#,
            ),
            (r#"plugin(resource: "gmail", action: "help", command: "drafts")"#, r#"plugin__gmail(command: "drafts --help")"#),
            (r#"web(resource: "search", action: "search", query: "rates")"#, r#"search_web(query: "rates")"#),
            (r#"agent(resource: "task", action: "delete", task_id: "3")"#, r#"update_task(task_id: "3", status: "deleted")"#),
            (r#"agent(resource: "registry", action: "deactivate", name: "Pat")"#, r#"set_employee_active(name: "Pat", active: false)"#),
            (
                r#"organizer(resource: "mail", action: "send", to: ["pat@example.com"], subject: "Hi")"#,
                r#"os(resource: "mail", action: "send", to: ["pat@example.com"], subject: "Hi")"#,
            ),
            (r#"music(action: "play")"#, r#"os(resource: "music", action: "play")"#),
            (r#"os(resource: "file", action: "grep", pattern: "TODO", path: "src")"#, r#"run_command(command: "grep -rn 'TODO' src")"#),
            (r#"os(resource: "file", action: "read", path: "/tmp/a.txt")"#, r#"read_file(path: "/tmp/a.txt")"#),
            (r#"team(action: "post", team: "ops", text: "done")"#, r#"send_message(to: "ops", message: "done")"#),
            // Published content: a web call that named only its action, and
            // shapes older than the agent tool.
            (r#"web(action: "navigate", url: "https://example.com")"#, r#"browser_open(url: "https://example.com")"#),
            (r#"web(action: 'go_back')"#, r#"browser_history(direction: "back")"#),
            (r#"agent(resource: "event", action: "emit", name: "done")"#, r#"emit_event(name: "done")"#),
            // A quote the author never closed: the call still ends at its bracket.
            (
                r#"`plugin(resource: "rentcast", action: "exec", command: "properties search --limit 100`)"#,
                r#"`plugin__rentcast(command: "properties search --limit 100`)"#,
            ),
        ];
        for (old, new) in cases {
            assert_eq!(moved(old), new, "{old}");
            assert_eq!(moved(new), new, "a second pass changes nothing: {new}");
        }
    }

    #[test]
    fn text_that_is_not_an_old_call_is_left_as_written() {
        for text in [
            r#"os(resource: "calendar", action: "today")"#,
            r#"message(resource: "sms", action: "send", to: "+15550100")"#,
            "nebo.emit('ready', {a: 1})",
            "exec(cmd)",
            "exec(code, globals=g)",
            "fn read(answer: Option<&str>) -> Self",
            "def edit(path: str):",
            "a web (resource) of links",
            r#"my_plugin(resource: "x")"#,
            r#"emit_event(name: "x")"#,
            "plugin(",
        ] {
            let r = rewrite_text(text);
            assert_eq!(r.text, text);
            assert!(r.moved.is_empty() && r.unmoved.is_empty(), "{text}");
        }
    }

    #[test]
    fn a_shape_with_no_successor_is_reported_and_left() {
        let r = rewrite_text(r#"First plugin(action: "list"), then skill(action: "unload", name: "x")."#);
        assert_eq!(r.text, r#"First plugin(action: "list"), then skill(action: "unload", name: "x")."#);
        assert_eq!(r.unmoved, [r#"plugin(action: "list")"#, r#"skill(action: "unload", name: "x")"#]);
    }

    /// A stored call (a parked approval's pending call) moves with its
    /// typed input becoming the operation tool's own.
    #[test]
    fn a_stored_port_call_becomes_its_operation_tool() {
        let input = serde_json::json!({
            "operation": "mail.message.send",
            "display": "Send the alert",
            "input": {"to": "pat@example.com", "subject": "Low stock"},
        });
        let (name, input) = move_stored_call("plugin", &input).unwrap();
        assert_eq!(name, "mail_message_send");
        assert_eq!(input, serde_json::json!({"display": "Send the alert", "to": "pat@example.com", "subject": "Low stock"}));
        assert_eq!(move_stored_call("read_file", &serde_json::json!({"path": "/x"})), None);
    }

    #[test]
    fn grant_entries_name_their_successors_and_keep_their_scope() {
        let names = |e: &str| successor_names(e).unwrap();
        assert_eq!(names("agent:memory"), ["remember", "recall", "forget"]);
        assert_eq!(
            names("work:weekly"),
            ["run_workflow:weekly", "workflow_status:weekly", "list_workflow_runs:weekly", "set_workflow_enabled:weekly"]
        );
        assert_eq!(names("plugin:shopify"), ["read_plugin_events:shopify", "plugin__shopify"]);
        assert_eq!(names("organizer:mail"), ["os:mail"]);
        assert!(names("web").iter().any(|n| n == "search_web") && names("web").iter().any(|n| n == "fetch_url"));
        assert!(names("os").starts_with(&["os".to_string()]) && names("os").iter().any(|n| n == "read_file"));
        assert!(names("plugin").iter().any(|n| n == "plugin__*"));
        assert_eq!(names("spotlight"), ["os:search"]);
        assert_eq!(successor_names("os:calendar"), None, "os still does the calendar");
        assert_eq!(successor_names("read_file"), None);
        assert_eq!(successor_names("mcp__monument__*"), None);
    }


    /// Every tool this build registers: the full roster, with the workflow
    /// tools, the NeboAI loop tools and the script runner the server adds.
    async fn every_tool() -> (std::sync::Arc<crate::Registry>, tempfile::TempDir) {
        let (registry, dir) = crate::registry::tests::full_registry().await;
        registry.register_workflows(std::sync::Arc::new(crate::workflows::TestManager::default())).await;
        let comm: std::sync::Arc<dyn comm::CommPlugin> = std::sync::Arc::new(comm::LoopbackPlugin::new());
        for tool in crate::loop_tool::tools(crate::loop_tool::LoopCore::new(comm, None)) {
            registry.register(Box::new(tool)).await;
        }
        let loader = std::sync::Arc::new(crate::skills::Loader::new(dir.path().join("s1"), dir.path().join("s2")));
        let tier = std::sync::Arc::new(tokio::sync::RwLock::new("free".to_string()));
        registry.register(Box::new(crate::execute_tool::ExecuteTool::new(loader, tier, None))).await;
        (registry, dir)
    }

    /// The old tools a sentence can call "the X tool": the ones that took a
    /// resource or an action.
    fn retired_domain_tools() -> Vec<&'static str> {
        retired_tools()
            .into_iter()
            .filter(|t| RENAMES.iter().any(|r| r.tool == *t && (r.resource.is_some() || r.action.is_some())))
            .collect()
    }

    /// What `text` says that names a tool this build doesn't have: an old
    /// call, a retired tool called by name, and (for text the model reads
    /// as instructions) a call to any name that is not a registered tool.
    fn stale_names(text: &str, tools: &std::collections::HashSet<String>, every_call: bool) -> Vec<String> {
        let r = rewrite_text(text);
        let mut found: Vec<String> = r.moved.into_iter().map(|(old, _)| old).chain(r.unmoved).collect();
        static CALLED_BY_NAME: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            let names = retired_domain_tools().join("|");
            regex::Regex::new(&format!(r"(?:\bthe `?(?:{names})`? tool\b|`(?:{names})` tool\b)(?:[^s]|$)")).unwrap()
        });
        found.extend(CALLED_BY_NAME.find_iter(text).map(|m| m.as_str().to_string()));
        if every_call {
            found.extend(
                written_calls(text)
                    .into_iter()
                    .filter(|w| !tools.contains(w.name))
                    .map(|w| w.call.to_string()),
            );
        }
        found
    }

    /// The string literals of Rust source, comments and char literals
    /// skipped.
    fn rust_strings(src: &str) -> Vec<String> {
        let b = src.as_bytes();
        let (mut out, mut i) = (Vec::new(), 0);
        while i < b.len() {
            match b[i] {
                b'/' if b.get(i + 1) == Some(&b'/') => i = src[i..].find('\n').map_or(b.len(), |n| i + n),
                b'/' if b.get(i + 1) == Some(&b'*') => i = src[i + 2..].find("*/").map_or(b.len(), |n| i + n + 4),
                b'\'' => {
                    // A char literal ('x', '\n', '"'); a lifetime has no closing quote.
                    let close = if b.get(i + 1) == Some(&b'\\') { src[i + 2..].find('\'').map(|n| i + 2 + n) } else { (b.get(i + 2) == Some(&b'\'')).then_some(i + 2) };
                    i = close.map_or(i + 1, |c| c + 1);
                }
                b'r' if (b.get(i + 1) == Some(&b'"') || b.get(i + 1) == Some(&b'#')) && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) => {
                    let hashes = src[i + 1..].bytes().take_while(|c| *c == b'#').count();
                    if b.get(i + 1 + hashes) != Some(&b'"') {
                        i += 1;
                        continue;
                    }
                    let open = i + 2 + hashes;
                    let end = format!("\"{}", "#".repeat(hashes));
                    let close = src[open..].find(&end).map_or(b.len(), |n| open + n);
                    out.push(src[open..close].to_string());
                    i = close + end.len();
                }
                b'"' => {
                    let mut j = i + 1;
                    while j < b.len() && b[j] != b'"' {
                        j += if b[j] == b'\\' { 2 } else { 1 };
                    }
                    let lit = &src[i + 1..j.min(b.len())];
                    // Escapes as the reader sees them; a line continuation
                    // joins its lines.
                    static CONTINUATION: std::sync::LazyLock<regex::Regex> =
                        std::sync::LazyLock::new(|| regex::Regex::new(r"\\\n\s*").unwrap());
                    let text = CONTINUATION.replace_all(lit, "");
                    out.push(text.replace("\\\"", "\"").replace("\\n", "\n"));
                    i = j + 1;
                }
                _ => i += 1,
            }
        }
        out
    }

    /// The string literals of TypeScript and Svelte source.
    fn js_strings(src: &str) -> Vec<String> {
        let b = src.as_bytes();
        let (mut out, mut i) = (Vec::new(), 0);
        while i < b.len() {
            match b[i] {
                b'/' if b.get(i + 1) == Some(&b'/') => i = src[i..].find('\n').map_or(b.len(), |n| i + n),
                b'/' if b.get(i + 1) == Some(&b'*') => i = src[i + 2..].find("*/").map_or(b.len(), |n| i + n + 4),
                q @ (b'"' | b'\'' | b'`') => {
                    let mut j = i + 1;
                    while j < b.len() && b[j] != q && (q == b'`' || b[j] != b'\n') {
                        j += if b[j] == b'\\' { 2 } else { 1 };
                    }
                    out.push(src[i + 1..j.min(b.len())].to_string());
                    i = j + 1;
                }
                _ => i += 1,
            }
        }
        out
    }

    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push(path);
            }
        }
    }

    /// Nothing the model, the owner or the phone reads names a tool this
    /// build doesn't have: no tool description or schema, bundled skill,
    /// prompt, hint, error or server message, and no frontend string. The
    /// descriptions and the bundled skills are instructions, so every call
    /// they write must name a registered tool. Test code, the rename table
    /// and the upgrade that reads it are the only places old names live.
    #[tokio::test]
    async fn nothing_anyone_reads_names_a_tool_this_build_does_not_have() {
        let (registry, _dir) = every_tool().await;
        let tools: std::collections::HashSet<String> = registry.get_tool_names().await.into_iter().collect();
        let mut found: Vec<String> = Vec::new();
        let mut check = |at: &str, text: &str, every_call: bool| {
            found.extend(stale_names(text, &tools, every_call).into_iter().map(|s| format!("{at}: {s}")));
        };

        for def in registry.list().await {
            check(&format!("tool {}", def.name), &format!("{}\n{}", def.description, def.input_schema), true);
        }
        for (name, text) in crate::skills::bundled::BUNDLED_SKILLS {
            check(&format!("bundled skill {name}"), text, true);
        }
        for (name, agent_md, agent_json, _) in crate::skills::bundled::BUNDLED_AGENTS {
            check(&format!("bundled employee {name}"), &format!("{agent_md}\n{agent_json}"), true);
        }

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut files = Vec::new();
        walk(&root.join("crates"), &mut files);
        walk(&root.join("app/src"), &mut files);
        for path in files {
            let at = path.strip_prefix(&root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            let test_code = at.contains("/tests/")
                || at.ends_with("/tests.rs")
                || at.contains("/staffed_proof/")
                || at.contains("/stored_tool_names/")
                || at.ends_with("/rename_map.rs")
                || at.contains(".test.")
                || at.contains("/target/");
            if test_code {
                continue;
            }
            let Ok(src) = std::fs::read_to_string(&path) else { continue };
            let texts: Vec<String> = match path.extension().and_then(|e| e.to_str()) {
                // Code after the tests marker is test code.
                Some("rs") => rust_strings(src.split("#[cfg(test)]").next().unwrap_or_default()),
                Some("ts" | "svelte") => js_strings(&src),
                Some("md" | "txt") if at.starts_with("crates/") && at.contains("/src/") => vec![src],
                Some("json") if at.contains("/i18n/") => vec![src],
                _ => continue,
            };
            for text in texts {
                check(&at, &text, false);
            }
        }
        assert!(found.is_empty(), "{} old tool names still reach a reader:\n{}", found.len(), found.join("\n"));
    }

}
