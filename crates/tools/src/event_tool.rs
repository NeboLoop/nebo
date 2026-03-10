use std::sync::Arc;

use db::Store;
use crate::domain::DomainInput;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// EventTool manages scheduled tasks and cron jobs.
/// Flat domain (no resources, actions map directly).
pub struct EventTool {
    store: Arc<Store>,
    runner: Option<Arc<dyn crate::bot_tool::AdvisorDeliberator>>,
}

impl EventTool {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store, runner: None }
    }

    pub fn with_runner(mut self, runner: Arc<dyn crate::bot_tool::AdvisorDeliberator>) -> Self {
        self.runner = Some(runner);
        self
    }
}

impl DynTool for EventTool {
    fn name(&self) -> &str {
        "event"
    }

    fn description(&self) -> String {
        "Schedule and manage recurring tasks — cron jobs, reminders, and routines.\n\n\
         Actions:\n\
         - create: Schedule a new task (cron expression)\n\
         - list: List all scheduled tasks\n\
         - delete: Remove a scheduled task by name\n\
         - pause: Disable a task (keeps it, won't fire)\n\
         - resume: Re-enable a paused task\n\
         - run: Immediately trigger a task\n\
         - history: Show execution history for a task\n\n\
         Examples:\n  \
         event(action: \"create\", name: \"daily-backup\", schedule: \"0 0 2 * * *\", task_type: \"bash\", command: \"./backup.sh\")\n  \
         event(action: \"create\", name: \"check-in\", schedule: \"0 30 9 * * *\", task_type: \"agent\", prompt: \"Check the server\")\n  \
         event(action: \"list\")\n  \
         event(action: \"run\", name: \"daily-backup\")"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "Action to perform",
                    "enum": ["create", "list", "delete", "pause", "resume", "run", "history"]
                },
                "name": { "type": "string", "description": "Task name (unique identifier)" },
                "schedule": { "type": "string", "description": "Cron expression (second minute hour day month weekday)" },
                "task_type": {
                    "type": "string",
                    "description": "Task type: bash (shell command) or agent (LLM prompt)",
                    "enum": ["bash", "agent"]
                },
                "command": { "type": "string", "description": "Shell command (for bash tasks)" },
                "prompt": { "type": "string", "description": "Agent prompt (for agent tasks)" }
            },
            "required": ["action"]
        })
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let domain_input: DomainInput = match serde_json::from_value(input.clone()) {
                Ok(v) => v,
                Err(e) => return ToolResult::error(format!("Failed to parse input: {}", e)),
            };

            match domain_input.action.as_str() {
                "create" => {
                    let name = input["name"].as_str().unwrap_or("");
                    let schedule = input["schedule"].as_str().unwrap_or("");
                    let task_type = input["task_type"].as_str().unwrap_or("bash");
                    let command = input["command"].as_str().unwrap_or("");
                    let prompt = input["prompt"].as_str().unwrap_or("");

                    if name.is_empty() {
                        return ToolResult::error("name is required");
                    }
                    if schedule.is_empty() {
                        return ToolResult::error("schedule (cron expression) is required");
                    }

                    let (cmd, msg) = if task_type == "agent" {
                        ("", Some(prompt))
                    } else {
                        if command.is_empty() {
                            return ToolResult::error("command is required for bash tasks");
                        }
                        (command, None::<&str>)
                    };

                    match self.store.create_cron_job(
                        name,
                        schedule,
                        cmd,
                        task_type,
                        msg,
                        None,
                        None,
                        true,
                    ) {
                        Ok(job) => ToolResult::ok(format!(
                            "Created scheduled task '{}' (id={}): {} ({})",
                            name, job.id, schedule, task_type
                        )),
                        Err(e) => ToolResult::error(format!("Failed to create task: {}", e)),
                    }
                }
                "list" => match self.store.list_cron_jobs(100, 0) {
                    Ok(jobs) => {
                        if jobs.is_empty() {
                            ToolResult::ok("No scheduled tasks.")
                        } else {
                            let lines: Vec<String> = jobs
                                .iter()
                                .map(|j| {
                                    let enabled = if j.enabled.unwrap_or(0) != 0 {
                                        "enabled"
                                    } else {
                                        "disabled"
                                    };
                                    format!(
                                        "- {} [{}] ({}) — {}",
                                        j.name, enabled, j.task_type, j.schedule
                                    )
                                })
                                .collect();
                            ToolResult::ok(format!(
                                "{} scheduled tasks:\n{}",
                                jobs.len(),
                                lines.join("\n")
                            ))
                        }
                    }
                    Err(e) => ToolResult::error(format!("Failed to list tasks: {}", e)),
                },
                "delete" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error("name is required");
                    }
                    match self.store.delete_cron_job_by_name(name) {
                        Ok(count) => {
                            if count > 0 {
                                ToolResult::ok(format!("Deleted task: {}", name))
                            } else {
                                ToolResult::error(format!("Task '{}' not found", name))
                            }
                        }
                        Err(e) => ToolResult::error(format!("Failed to delete: {}", e)),
                    }
                }
                "pause" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error("name is required");
                    }
                    match self.store.disable_cron_job_by_name(name) {
                        Ok(_) => ToolResult::ok(format!("Paused task: {}", name)),
                        Err(e) => ToolResult::error(format!("Failed to pause: {}", e)),
                    }
                }
                "resume" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error("name is required");
                    }
                    match self.store.enable_cron_job_by_name(name) {
                        Ok(_) => ToolResult::ok(format!("Resumed task: {}", name)),
                        Err(e) => ToolResult::error(format!("Failed to resume: {}", e)),
                    }
                }
                "run" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error("name is required");
                    }
                    match self.store.get_cron_job_by_name(name) {
                        Ok(Some(job)) => {
                            // Create history entry
                            let history = match self.store.create_cron_history(job.id) {
                                Ok(h) => h,
                                Err(e) => return ToolResult::error(format!("Failed to create history: {}", e)),
                            };
                            let _ = self.store.update_cron_job_last_run(job.id, None);

                            // Execute based on task type
                            let (success, output) = match job.task_type.as_str() {
                                "bash" => {
                                    match tokio::process::Command::new("bash")
                                        .arg("-c")
                                        .arg(&job.command)
                                        .output()
                                        .await
                                    {
                                        Ok(result) => {
                                            let stdout = String::from_utf8_lossy(&result.stdout).to_string();
                                            let stderr = String::from_utf8_lossy(&result.stderr).to_string();
                                            let out = if stderr.is_empty() {
                                                stdout
                                            } else {
                                                format!("{}\n[stderr] {}", stdout, stderr)
                                            };
                                            (result.status.success(), out)
                                        }
                                        Err(e) => (false, format!("Failed to execute: {}", e)),
                                    }
                                }
                                "agent" => {
                                    let prompt = job.message.as_deref().unwrap_or("");
                                    if prompt.is_empty() {
                                        (false, "No prompt configured for agent task".to_string())
                                    } else if let Some(ref runner) = self.runner {
                                        match runner.deliberate(prompt).await {
                                            Ok(result) => (true, result),
                                            Err(e) => (false, format!("Agent task failed: {}", e)),
                                        }
                                    } else {
                                        (false, format!("Agent task '{}' — runner not available. Run via the scheduler or API.", name))
                                    }
                                }
                                other => (false, format!("Unknown task type: {}", other)),
                            };

                            let (out, err) = if success {
                                (Some(output.as_str()), None)
                            } else {
                                (None, Some(output.as_str()))
                            };
                            let _ = self.store.update_cron_history(history.id, success, out, err);
                            let _ = self.store.update_cron_job_last_run(job.id, Some(&output));

                            if success {
                                ToolResult::ok(format!("Task '{}' executed successfully:\n{}", name, output))
                            } else {
                                ToolResult::error(format!("Task '{}' failed:\n{}", name, output))
                            }
                        }
                        Ok(None) => ToolResult::error(format!("Task '{}' not found", name)),
                        Err(e) => ToolResult::error(format!("Failed to find task: {}", e)),
                    }
                }
                "history" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error("name is required");
                    }
                    // Get the job by name to find its ID, then fetch history
                    match self.store.get_cron_job_by_name(name) {
                        Ok(Some(job)) => match self.store.get_recent_cron_history(job.id) {
                            Ok(history) => {
                                if history.is_empty() {
                                    ToolResult::ok(format!("No execution history for '{}'.", name))
                                } else {
                                    let lines: Vec<String> = history
                                        .iter()
                                        .map(|h| {
                                            let status = if h.success.unwrap_or(0) != 0 {
                                                "OK"
                                            } else {
                                                "FAIL"
                                            };
                                            format!("- [{}] {}", status, h.output.as_deref().unwrap_or("-"))
                                        })
                                        .collect();
                                    ToolResult::ok(format!(
                                        "History for '{}':\n{}",
                                        name,
                                        lines.join("\n")
                                    ))
                                }
                            }
                            Err(e) => ToolResult::error(format!("Failed to get history: {}", e)),
                        },
                        Ok(None) => ToolResult::error(format!("Task '{}' not found", name)),
                        Err(e) => ToolResult::error(format!("Failed to find task: {}", e)),
                    }
                }
                other => ToolResult::error(format!(
                    "Unknown action: {}. Available: create, list, delete, pause, resume, run, history",
                    other
                )),
            }
        })
    }
}
