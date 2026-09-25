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
];

/// A retired `skill(action)` shape.
const fn skill(action: Option<&'static str>, to: &'static str) -> Rename {
    Rename { tool: "skill", resource: None, action, to, params: &[] }
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
