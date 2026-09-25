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
    Rename { tool: "tool_search", resource: None, action: None, to: crate::find_tools::FIND_TOOLS, params: &[] },
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
];

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
