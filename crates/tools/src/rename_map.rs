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
    // Tools WP2: helpers, memory, asking and reaching the owner. A task
    // delete is `update_task` with status "deleted"; a notify alert is
    // `push_notification` with `urgent: true`.
    Rename {
        tool: "agent",
        resource: Some("memory"),
        action: Some("store"),
        to: "remember",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("memory"),
        action: Some("save"),
        to: "remember",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("memory"),
        action: Some("recall"),
        to: "recall",
        params: &[("key", "query")],
    },
    Rename {
        tool: "agent",
        resource: Some("memory"),
        action: Some("search"),
        to: "recall",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("memory"),
        action: Some("list"),
        to: "recall",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("memory"),
        action: Some("delete"),
        to: "forget",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("spawn"),
        to: "delegate",
        params: &[("agent_type", "helper_type")],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("spawn_parallel"),
        to: "delegate",
        params: &[("agent_type", "helper_type"), ("isolate", "isolation")],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("orchestrate"),
        to: "orchestrate",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("send"),
        to: "send_message",
        params: &[("task_id", "to")],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("status"),
        to: "read_output",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("cancel"),
        to: "stop_task",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("create"),
        to: "create_task",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("update"),
        to: "update_task",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("delete"),
        to: "update_task",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("get"),
        to: "get_task",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("list"),
        to: "list_tasks",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("assign"),
        to: "assign_task",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("task"),
        action: Some("assignments"),
        to: "list_assignments",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("ask"),
        action: Some("prompt"),
        to: "ask_owner",
        params: &[("text", "question")],
    },
    Rename {
        tool: "agent",
        resource: Some("ask"),
        action: Some("confirm"),
        to: "ask_owner",
        params: &[("text", "question")],
    },
    Rename {
        tool: "agent",
        resource: Some("ask"),
        action: Some("select"),
        to: "ask_owner",
        params: &[("text", "question")],
    },
    Rename {
        tool: "agent",
        resource: Some("runs"),
        action: Some("list"),
        to: "list_runs",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("runs"),
        action: Some("cancel"),
        to: "stop_task",
        params: &[("run_id", "task_id")],
    },
    Rename {
        tool: "agent",
        resource: Some("session"),
        action: Some("query"),
        to: "search_history",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("session"),
        action: Some("history"),
        to: "read_session",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("session"),
        action: Some("list"),
        to: "list_sessions",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("advisors"),
        action: Some("deliberate"),
        to: "consult_advisors",
        params: &[("task", "question")],
    },
    Rename {
        tool: "agent",
        resource: Some("advisors"),
        action: Some("list"),
        to: "list_advisors",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("research"),
        action: Some("deep_research"),
        to: "deep_research",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("research"),
        action: Some("research"),
        to: "quick_research",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("research"),
        action: Some("submit_findings"),
        to: "submit_findings",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("profile"),
        action: Some("get"),
        to: "get_profile",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("profile"),
        action: Some("update"),
        to: "update_profile",
        params: &[],
    },
    Rename {
        tool: "agent",
        resource: Some("profile"),
        action: Some("open_billing"),
        to: "open_billing",
        params: &[],
    },
    Rename {
        tool: "message",
        resource: Some("owner"),
        action: Some("notify"),
        to: "message_owner",
        params: &[("text", "message")],
    },
    Rename {
        tool: "message",
        resource: Some("notify"),
        action: Some("send"),
        to: "push_notification",
        params: &[("text", "message")],
    },
    Rename {
        tool: "message",
        resource: Some("notify"),
        action: Some("alert"),
        to: "push_notification",
        params: &[("text", "message")],
    },
    Rename {
        tool: "message",
        resource: Some("notify"),
        action: Some("dnd_status"),
        to: "check_dnd",
        params: &[],
    },
    Rename {
        tool: "os",
        resource: Some("notification"),
        action: Some("send"),
        to: "push_notification",
        params: &[],
    },
    Rename {
        tool: "os",
        resource: Some("notification"),
        action: Some("alert"),
        to: "push_notification",
        params: &[],
    },
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
];

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
