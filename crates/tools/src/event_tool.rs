use std::sync::Arc;

use crate::domain::DomainInput;
use crate::errors;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};
use chrono::Local;
use db::Store;

/// EventTool manages scheduled tasks and cron jobs.
/// Flat domain (no resources, actions map directly).
pub struct EventTool {
    store: Arc<Store>,
    runner: Option<Arc<dyn crate::bot_tool::AdvisorDeliberator>>,
}

impl EventTool {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            runner: None,
        }
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
        "Scheduling & reminders — one-time and recurring time-based triggers.\n\
         USE THIS when: user mentions \"every\", \"remind me\", \"daily\", \"weekly\", \"in X minutes\", or any time-based trigger.\n\
         NOT for a created/named agent's recurring duties — those belong on the agent itself via agent(resource: \"registry\", ..., automations/add_automations). Use event only for YOUR OWN reminders and standalone tasks.\n\
         Prefer task_type: \"agent\" — this means YOU execute the task when it fires, with full access to all your tools and memory.\n\n\
         One-time reminders (use \"at\" with relative time):\n\
         - event(action: \"create\", name: \"call-kristi\", at: \"in 10 minutes\", task_type: \"agent\", prompt: \"Remind user to call Kristi\")\n\n\
         Recurring tasks (use \"cron\" expression: second minute hour day month weekday):\n\
         - event(action: \"create\", name: \"morning-brief\", cron: \"0 0 8 * * 1-5\", task_type: \"agent\", prompt: \"Check today's calendar and send a summary\")\n\n\
         Management:\n\
         - event(action: \"list\") — List all reminders\n\
         - event(action: \"delete\", name: \"...\") — Remove a reminder\n\
         - event(action: \"pause\", name: \"...\") / event(action: \"resume\", name: \"...\") — Pause or resume\n\
         - event(action: \"run\", name: \"...\") — Trigger immediately\n\
         - event(action: \"history\", name: \"...\") — View execution history\n\n\
         Common cron patterns: \"0 0 9 * * 1-5\" (9am weekdays), \"0 30 8 * * *\" (8:30am daily), \"0 0 */2 * * *\" (every 2h)"
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
                "cron": { "type": "string", "description": "Cron expression: second minute hour day month weekday [year] (e.g. \"0 30 9 * * *\" = 9:30 AM daily, \"0 30 9 * * * *\" with year wildcard)" },
                "at": { "type": "string", "description": "Relative time for one-shot tasks (e.g. \"in 5 minutes\", \"in 1 hour\"). Converted to a cron expression automatically." },
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
        ctx: &'a ToolContext,
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
                    let cron_val = input["cron"]
                        .as_str()
                        .filter(|v| !v.is_empty())
                        // `schedule` is the natural synonym models reach for — accept it.
                        .or_else(|| input["schedule"].as_str())
                        .unwrap_or("");
                    let at_val = input["at"].as_str().unwrap_or("");
                    let task_type = input["task_type"].as_str().unwrap_or("bash");
                    let command = input["command"].as_str().unwrap_or("");
                    let prompt = input["prompt"].as_str().unwrap_or("");

                    if let Some(text) = reminder_shape_error(&input) {
                        return ToolResult::error(text);
                    }
                    if name.is_empty() {
                        return ToolResult::error(errors::missing_param(
                            "create",
                            "name",
                            "event(action: \"create\", name: \"daily-report\", cron: \"0 30 9 * * *\", command: \"echo hello\")",
                        ));
                    }

                    // Resolve schedule: prefer `cron`, fall back to `at` (relative time)
                    let mut fires_at: Option<String> = None;
                    let schedule = if !cron_val.is_empty() {
                        cron_val.to_string()
                    } else if !at_val.is_empty() {
                        match parse_relative_time(at_val) {
                            Some((s, target)) => {
                                fires_at = Some(target.format("%Y-%m-%d %H:%M:%S %Z").to_string());
                                s
                            }
                            None => {
                                return ToolResult::error(format!(
                                    "Could not parse '{}'. Use format like 'in 5 minutes', 'in 1 hour', 'in 30 seconds'.",
                                    at_val
                                ));
                            }
                        }
                    } else {
                        return ToolResult::error(
                            "Either 'cron' or 'at' is required. Use cron: \"0 30 9 * * *\" or at: \"in 5 minutes\".",
                        );
                    };

                    let (cmd, msg) = if task_type == "agent" {
                        ("", Some(prompt))
                    } else {
                        if command.is_empty() {
                            return ToolResult::error(errors::missing_param(
                                "create",
                                "command",
                                "Missing 'command' for a bash task. For an agent task pass task_type: \"agent\", prompt: \"...\". Example bash: command: \"echo hello\"",
                            ));
                        }
                        // Cron commands execute later without going through the
                        // interactive shell pipeline — run the same unconditional
                        // safeguard here at creation time so a scheduled job can't
                        // smuggle a command the shell tool would refuse.
                        if let Some(block) = crate::safeguard::check_safeguard(
                            "shell",
                            &serde_json::json!({
                                "resource": "bash",
                                "action": "exec",
                                "command": command,
                            }),
                        ) {
                            return ToolResult::error(format!(
                                "Refusing to schedule this command: {}",
                                block
                            ));
                        }
                        (command, None::<&str>)
                    };

                    // Capture the originating agent + channel context so the
                    // scheduler can route the response back to the same place
                    // (e.g. timer set in a Slack thread → alert in the same
                    // thread). agent_id is parsed from session_key, channel
                    // is read from ctx.channel — both NULL when the task was
                    // created outside an agent-bound channel conversation.
                    let agent_id = Some(types::keyparser::extract_agent_id(&ctx.session_key))
                        .filter(|id| !id.is_empty());
                    let channel_ctx_json = ctx.channel.as_ref().map(|ch| {
                        serde_json::json!({
                            "kind": ch.kind,
                            "channel_id": ch.channel_id,
                            "thread_ts": ch.thread_ts,
                        })
                        .to_string()
                    });

                    match self.store.create_cron_job(
                        name,
                        &schedule,
                        cmd,
                        task_type,
                        msg,
                        None,
                        None,
                        true,
                        agent_id.as_deref(),
                        channel_ctx_json.as_deref(),
                    ) {
                        Ok(job) => ToolResult::ok(format!(
                            "Created scheduled task '{}' (id={}): {} ({}){}",
                            name,
                            job.id,
                            schedule,
                            task_type,
                            fires_at
                                .map(|t| format!("; fires at {t}"))
                                .unwrap_or_default()
                        )),
                        Err(e) => ToolResult::error(format!("Failed to create task: {}", e)),
                    }
                }
                "list" => match self.store.list_cron_jobs(LIST_CAP, 0) {
                    Ok(jobs) => {
                        if jobs.is_empty() {
                            ToolResult::ok("No scheduled tasks.")
                        } else {
                            let total = self
                                .store
                                .count_cron_jobs()
                                .map(|n| n.max(jobs.len() as i64) as usize)
                                .unwrap_or(jobs.len());
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
                                "{}\n{}",
                                list_header(jobs.len(), total),
                                lines.join("\n")
                            ))
                        }
                    }
                    Err(e) => ToolResult::error(format!("Failed to list tasks: {}", e)),
                },
                "delete" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error(errors::missing_param(
                            "delete",
                            "name",
                            "event(action: \"delete\", name: \"daily-report\")",
                        ));
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
                        return ToolResult::error(errors::missing_param(
                            "pause",
                            "name",
                            "event(action: \"pause\", name: \"daily-report\")",
                        ));
                    }
                    match self.store.get_cron_job_by_name(name) {
                        Ok(None) => return ToolResult::error(format!("Task '{}' not found", name)),
                        Err(e) => return ToolResult::error(format!("Failed to find task: {}", e)),
                        Ok(Some(_)) => {}
                    }
                    match self.store.disable_cron_job_by_name(name) {
                        Ok(_) => ToolResult::ok(format!("Paused task: {}", name)),
                        Err(e) => ToolResult::error(format!("Failed to pause: {}", e)),
                    }
                }
                "resume" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error(errors::missing_param(
                            "resume",
                            "name",
                            "event(action: \"resume\", name: \"daily-report\")",
                        ));
                    }
                    match self.store.get_cron_job_by_name(name) {
                        Ok(None) => return ToolResult::error(format!("Task '{}' not found", name)),
                        Err(e) => return ToolResult::error(format!("Failed to find task: {}", e)),
                        Ok(Some(_)) => {}
                    }
                    match self.store.enable_cron_job_by_name(name) {
                        Ok(_) => ToolResult::ok(format!("Resumed task: {}", name)),
                        Err(e) => ToolResult::error(format!("Failed to resume: {}", e)),
                    }
                }
                "run" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error(errors::missing_param(
                            "run",
                            "name",
                            "event(action: \"run\", name: \"daily-report\")",
                        ));
                    }
                    match self.store.get_cron_job_by_name(name) {
                        Ok(Some(job)) => {
                            // Create history entry
                            let history = match self.store.create_cron_history(job.id) {
                                Ok(h) => h,
                                Err(e) => {
                                    return ToolResult::error(format!(
                                        "Failed to create history: {}",
                                        e
                                    ));
                                }
                            };
                            let _ = self.store.update_cron_job_last_run(job.id, None);

                            // Execute based on task type
                            let (success, output) = match job.task_type.as_str() {
                                "bash" => {
                                    match tokio::time::timeout(
                                        std::time::Duration::from_secs(120),
                                        tokio::process::Command::new("bash")
                                            .arg("-c")
                                            .arg(&job.command)
                                            .output(),
                                    )
                                    .await
                                    {
                                        Err(_) => (
                                            false,
                                            "Command timed out after 120s".to_string(),
                                        ),
                                        Ok(Ok(result)) => {
                                            let stdout =
                                                String::from_utf8_lossy(&result.stdout).to_string();
                                            let stderr =
                                                String::from_utf8_lossy(&result.stderr).to_string();
                                            let out = if stderr.is_empty() {
                                                stdout
                                            } else {
                                                format!("{}\n[stderr] {}", stdout, stderr)
                                            };
                                            (result.status.success(), out)
                                        }
                                        Ok(Err(e)) => (false, format!("Failed to execute: {}", e)),
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
                                        (
                                            false,
                                            format!(
                                                "Agent task '{}' cannot be run on demand in this context; it will run at its scheduled time ({}).",
                                                name, job.schedule
                                            ),
                                        )
                                    }
                                }
                                other => (false, format!("Unknown task type: {}", other)),
                            };

                            let (out, err) = if success {
                                (Some(output.as_str()), None)
                            } else {
                                (None, Some(output.as_str()))
                            };
                            let _ = self
                                .store
                                .update_cron_history(history.id, success, out, err);
                            let _ = self.store.update_cron_job_last_run(job.id, Some(&output));

                            if success {
                                ToolResult::ok(format!(
                                    "Task '{}' executed successfully:\n{}",
                                    name, output
                                ))
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
                        return ToolResult::error(errors::missing_param(
                            "history",
                            "name",
                            "event(action: \"history\", name: \"daily-report\")",
                        ));
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
                                            format!(
                                                "- [{}] {}",
                                                status,
                                                h.output.as_deref().unwrap_or("-")
                                            )
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

/// Parse relative time strings like "in 5 minutes" into a one-shot cron expression.
/// Returns a cron string like "0 25 18 14 3 *" (specific second/minute/hour/day/month).
///
/// Cron expressions are emitted in **local time** because Nebo is a desktop
/// AI companion — the machine's local timezone IS the user's wall clock, and
/// agents author schedules in those terms (e.g. "morning briefing at 7 AM"
/// means 7 AM local). The scheduler (`crates/server/src/scheduler.rs::tick`)
/// reads `Local::now()` and evaluates `schedule.after()` with a local-time
/// `last_run`, so this side must match.
/// Cap on the `list` action; the header says "showing N of M" when it applies.
const LIST_CAP: i64 = 100;

/// Header for the `list` action: "N scheduled tasks" when the list is complete,
/// "showing N of M scheduled tasks" when the cap cut it.
fn list_header(shown: usize, total: usize) -> String {
    if total > shown {
        format!("showing {shown} of {total} scheduled tasks (list cap {LIST_CAP}):")
    } else {
        format!("{shown} scheduled tasks:")
    }
}

/// The fields a scheduled task needs, named together. A call shaped like the
/// reminder an early system prompt taught (`title`, `when`, nothing the tool
/// reads) used to earn three errors in a row: name, then cron/at, then
/// command. One error, all three fields, one valid call.
fn reminder_shape_error(input: &serde_json::Value) -> Option<String> {
    let has = |k: &str| input[k].as_str().is_some_and(|v| !v.trim().is_empty());
    let reminder_shaped = has("title") || has("when");
    let has_a_required_field = ["name", "cron", "schedule", "at", "command"].iter().any(|k| has(k));
    if !reminder_shaped || has_a_required_field {
        return None;
    }
    let title = input["title"].as_str().map(str::trim).filter(|t| !t.is_empty()).unwrap_or("reminder");
    let name = Some(comm::handle::slugify(title)).filter(|n| !n.is_empty()).unwrap_or_else(|| "reminder".to_string());
    let when = input["when"].as_str().map(str::trim).unwrap_or("");
    let (at, when_note) = if !when.is_empty() && parse_relative_time(when).is_some() {
        (when.to_string(), String::new())
    } else if when.is_empty() {
        ("in 3 hours".to_string(), String::new())
    } else {
        (
            "in 3 hours".to_string(),
            format!(" `at` takes a relative time, not \"{when}\": say how long from now (\"in 2 hours\"), or give a cron for a clock time."),
        )
    };
    Some(format!(
        "A scheduled task needs three fields, and `title`/`when` are not fields: name (a unique id), \
         a time (at: \"in 3 hours\" or cron: \"0 0 15 * * *\"), and the work (task_type: \"agent\", prompt: \"...\", \
         or command: \"...\" for a shell command). Example: event(action: \"create\", name: \"{name}\", at: \"{at}\", \
         task_type: \"agent\", prompt: \"Remind the user: {title}\").{when_note}"
    ))
}

/// Parse "in 5 minutes" style input into a 7-field cron expression plus the
/// local time it resolves to, so the result can say when the task fires.
fn parse_relative_time(input: &str) -> Option<(String, chrono::DateTime<Local>)> {
    let s = input.trim().to_lowercase();
    let s = s.strip_prefix("in ").unwrap_or(&s);

    // Extract number and unit
    let mut parts = s.split_whitespace();
    let num_str = parts.next()?;
    let num: i64 = num_str.parse().ok()?;
    let unit = parts.next().unwrap_or("");

    let duration = if unit.starts_with("second") || unit == "s" || unit == "sec" {
        chrono::Duration::seconds(num)
    } else if unit.starts_with("minute") || unit == "m" || unit == "min" {
        chrono::Duration::minutes(num)
    } else if unit.starts_with("hour") || unit == "h" || unit == "hr" {
        chrono::Duration::hours(num)
    } else {
        return None;
    };

    let target = Local::now() + duration;
    // Cron format: second minute hour day-of-month month day-of-week year (7 fields)
    let cron = format!(
        "{} {} {} {} {} * {}",
        target.format("%-S"),
        target.format("%-M"),
        target.format("%-H"),
        target.format("%-d"),
        target.format("%-m"),
        target.format("%Y"),
    );
    Some((cron, target))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_header_says_showing_n_of_m_only_when_capped() {
        assert_eq!(list_header(3, 3), "3 scheduled tasks:");
        assert_eq!(
            list_header(100, 240),
            "showing 100 of 240 scheduled tasks (list cap 100):"
        );
    }

    #[test]
    fn a_reminder_shaped_call_gets_one_error_naming_all_three_fields() {
        use serde_json::json;
        let call = json!({"action": "create", "resource": "reminder", "title": "Call back", "when": "3pm"});
        let text = reminder_shape_error(&call).expect("refused");
        for needle in ["name", "at:", "cron:", "prompt:", "command:", "name: \"call-back\"", "not \"3pm\""] {
            assert!(text.contains(needle), "{needle}: {text}");
        }
        let relative = json!({"action": "create", "title": "Call back", "when": "in 2 hours"});
        let text = reminder_shape_error(&relative).expect("refused");
        assert!(text.contains("at: \"in 2 hours\""), "{text}");
        assert!(!text.contains("relative time, not"), "{text}");
        let well_formed = json!({"action": "create", "name": "call-back", "at": "in 3 hours", "task_type": "agent", "prompt": "x", "title": "Call back"});
        assert!(reminder_shape_error(&well_formed).is_none());
        assert!(reminder_shape_error(&json!({"action": "create"})).is_none());
        assert!(reminder_shape_error(&json!({"action": "create", "when": "3pm", "command": "echo hi"})).is_none());
    }

    /// The refusal fires before the name check, and the corrected call lands.
    #[tokio::test]
    async fn create_refuses_the_reminder_shape_before_asking_for_a_name() {
        use crate::registry::DynTool;
        let path = std::env::temp_dir().join(format!("nebo-event-reminder-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = Arc::new(Store::new(&path.to_string_lossy()).unwrap());
        let tool = EventTool::new(store);
        let ctx = ToolContext::default();
        let refused = tool
            .execute_dyn(&ctx, serde_json::json!({"action": "create", "title": "Call back", "when": "3pm"}))
            .await;
        assert!(refused.is_error, "{}", refused.content);
        assert!(refused.content.contains("three fields"), "{}", refused.content);
        assert!(!refused.content.contains("Missing required parameter"), "{}", refused.content);
        let created = tool
            .execute_dyn(
                &ctx,
                serde_json::json!({"action": "create", "name": "call-back", "at": "in 5 minutes", "task_type": "agent", "prompt": "Remind the user: call back"}),
            )
            .await;
        assert!(!created.is_error, "{}", created.content);
        assert!(created.content.contains("Created scheduled task 'call-back'"), "{}", created.content);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn relative_time_yields_cron_and_the_moment_it_fires() {
        let before = Local::now();
        let (cron, target) = parse_relative_time("in 5 minutes").expect("parses");
        assert_eq!(cron.split_whitespace().count(), 7, "{cron}");
        let delta = target - before;
        assert!(delta >= chrono::Duration::minutes(5) - chrono::Duration::seconds(1));
        assert!(delta <= chrono::Duration::minutes(5) + chrono::Duration::seconds(5));
        assert!(parse_relative_time("next tuesday").is_none());
    }
}
