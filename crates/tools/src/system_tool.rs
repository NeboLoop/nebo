use std::sync::Arc;

use crate::domain::DomainInput;
use crate::file_tool::FileTool;
use crate::origin::ToolContext;
use crate::policy::Policy;
use crate::process::ProcessRegistry;
use crate::registry::{DynTool, ToolResult};
use crate::shell_tool::ShellTool;

/// SystemTool consolidates OS-level operations into a single STRAP domain tool.
/// Core resources (always registered): file, shell
/// Platform resources can be added via register_resource().
pub struct SystemTool {
    file_tool: FileTool,
    shell_tool: ShellTool,
}

impl SystemTool {
    pub fn new(policy: Policy, process_registry: Arc<ProcessRegistry>) -> Self {
        Self {
            file_tool: FileTool::new(),
            shell_tool: ShellTool::new(policy, process_registry),
        }
    }

    /// Infer resource from action name when resource is omitted.
    fn infer_resource(&self, action: &str) -> &str {
        match action {
            // File-only actions
            "read" | "write" | "edit" | "glob" | "grep" => "file",
            // Shell-only actions
            "exec" | "poll" | "log" => "shell",
            _ => "",
        }
    }

    pub fn file_tool(&self) -> &FileTool {
        &self.file_tool
    }

    pub fn shell_tool(&self) -> &ShellTool {
        &self.shell_tool
    }
}

impl DynTool for SystemTool {
    fn name(&self) -> &str {
        "system"
    }

    fn description(&self) -> String {
        "OS operations — files, commands, apps, clipboard, settings.\n\n\
         Resources and Actions:\n\
         - file: read, write, edit, glob, grep (File operations)\n\
         - shell: exec, list, poll, log, write, kill, info (Shell operations)\n\n\
         Examples:\n  \
         system(resource: \"file\", action: \"read\", path: \"/path/to/file.txt\")\n  \
         system(resource: \"file\", action: \"write\", path: \"/path/to/file.txt\", content: \"hello\")\n  \
         system(resource: \"file\", action: \"edit\", path: \"/path/to/file.txt\", old_string: \"foo\", new_string: \"bar\")\n  \
         system(resource: \"file\", action: \"glob\", pattern: \"**/*.go\")\n  \
         system(resource: \"file\", action: \"grep\", pattern: \"TODO\", path: \"/project\")\n  \
         system(resource: \"shell\", action: \"exec\", command: \"ls -la\")"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "resource": {
                    "type": "string",
                    "description": "Resource type: file, shell",
                    "enum": ["file", "shell"]
                },
                "action": {
                    "type": "string",
                    "description": "Action to perform",
                    "enum": ["read", "write", "edit", "glob", "grep", "exec", "list", "kill", "info", "poll", "log"]
                },
                "path": {
                    "type": "string",
                    "description": "File path, directory path"
                },
                "content": {
                    "type": "string",
                    "description": "File content to write"
                },
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern or grep regex pattern"
                },
                "old_string": {
                    "type": "string",
                    "description": "String to find in file (for edit)"
                },
                "new_string": {
                    "type": "string",
                    "description": "Replacement string (for edit)"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences (for edit)"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line offset for reading"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max lines/results to return"
                },
                "append": {
                    "type": "boolean",
                    "description": "Append to file instead of overwrite"
                },
                "command": {
                    "type": "string",
                    "description": "Shell command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Command timeout in seconds"
                },
                "session_id": {
                    "type": "string",
                    "description": "Background session ID"
                },
                "pid": {
                    "type": "integer",
                    "description": "Process ID"
                },
                "signal": {
                    "type": "string",
                    "description": "Signal to send: SIGTERM, SIGKILL, SIGINT"
                },
                "regex": {
                    "type": "string",
                    "description": "Regular expression pattern (for grep)"
                },
                "glob": {
                    "type": "string",
                    "description": "File filter pattern for grep"
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "Case-insensitive search (for grep)"
                },
                "background": {
                    "type": "boolean",
                    "description": "Run command in background"
                }
            },
            "required": ["action"]
        })
    }

    fn requires_approval(&self) -> bool {
        false // Per-action check done by delegated tools
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let domain_input: DomainInput = match serde_json::from_value(input.clone()) {
                Ok(v) => v,
                Err(e) => {
                    return ToolResult::error(format!("Failed to parse input: {}", e));
                }
            };

            let resource = if domain_input.resource.is_empty() {
                self.infer_resource(&domain_input.action).to_string()
            } else {
                domain_input.resource
            };

            if resource.is_empty() {
                return ToolResult::error(
                    "Resource is required. Available: file, shell".to_string(),
                );
            }

            match resource.as_str() {
                "file" => self.file_tool.execute(ctx, input),
                "shell" => self.shell_tool.execute(ctx, input).await,
                other => ToolResult::error(format!(
                    "Resource {:?} not available. Available: file, shell",
                    other
                )),
            }
        })
    }
}
