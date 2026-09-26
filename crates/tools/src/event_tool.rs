//! The schedule tools: create, list, delete, pause, run now and history of
//! scheduled work. One purpose per
//! tool over the one cron store.

use std::sync::Arc;

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};
use chrono::Local;
use db::Store;

/// One tool of the schedule family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Create,
    List,
    Delete,
    SetPaused,
    RunNow,
    History,
}

const KINDS: &[Kind] = &[Kind::Create, Kind::List, Kind::Delete, Kind::SetPaused, Kind::RunNow, Kind::History];

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Create => "create_schedule",
            Kind::List => "list_schedules",
            Kind::Delete => "delete_schedule",
            Kind::SetPaused => "set_schedule_paused",
            Kind::RunNow => "run_schedule_now",
            Kind::History => "schedule_history",
        }
    }

    fn search_hint(self) -> &'static str {
        match self {
            Kind::Create => "schedule a reminder or recurring job",
            Kind::List => "list scheduled reminders and jobs",
            Kind::Delete => "delete a scheduled reminder or job",
            Kind::SetPaused => "pause or resume a schedule",
            Kind::RunNow => "run a scheduled job right now",
            Kind::History => "past runs of a scheduled job",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Kind::Create => "Schedules a reminder or other work to run later, once or on a repeat.\n\
                - `at` runs it once, a relative time from now (\"in 20 minutes\", \"in 3 hours\"). `cron` sets a clock time or a repeat, six fields starting with seconds: \"0 0 9 * * 1-5\" is 9am on weekdays, \"0 30 8 * * *\" is 8:30 every morning.\n\
                - `prompt` is what you do when it fires, with your tools and memory; `command` runs a shell command instead.\n\
                - It runs with your own permissions: scheduling grants nothing new.\n\
                - Not for checking on work in progress: helpers report back when they finish, and a run's outcome lands in its history. Never schedule a check on a run.",
            Kind::List => "Lists the scheduled reminders and jobs with their timing and whether each is paused.",
            Kind::Delete => "Deletes a scheduled reminder or job by name. It never fires again.",
            Kind::SetPaused => "Pauses a schedule (`paused: true`) so it stops firing, or resumes it (`paused: false`). The schedule is kept.",
            Kind::RunNow => "Runs a scheduled job once, now, and waits for its outcome. Its regular schedule is unchanged.",
            Kind::History => "Shows the recent runs of a scheduled job and whether each succeeded.",
        }
    }

    fn schema(self) -> serde_json::Value {
        let name = serde_json::json!({ "type": "string", "description": "The schedule's name, as list_schedules shows it." });
        match self {
            Kind::Create => serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "A short unique name, e.g. \"call-back-kristi\"." },
                    "at": { "type": "string", "description": "Run once, this long from now: \"in 5 minutes\", \"in 2 hours\"." },
                    "cron": { "type": "string", "description": "Run at a clock time or on a repeat: second minute hour day month weekday." },
                    "prompt": { "type": "string", "description": "What to do when it fires; you run it with your tools and memory." },
                    "command": { "type": "string", "description": "A shell command to run instead of a prompt." },
                    "overlap": {
                        "type": "string",
                        "enum": ["skip", "buffer_one", "allow_all"],
                        "description": "When it comes due while its last run is still going: skip that time (the default), buffer_one (run once when the last run ends), or allow_all (run anyway)."
                    }
                },
                "required": ["name"]
            }),
            Kind::List => serde_json::json!({ "type": "object", "properties": {} }),
            Kind::SetPaused => serde_json::json!({
                "type": "object",
                "properties": {
                    "name": name,
                    "paused": { "type": "boolean", "description": "true pauses it; false resumes it." }
                },
                "required": ["name", "paused"]
            }),
            Kind::Delete | Kind::RunNow | Kind::History => serde_json::json!({
                "type": "object",
                "properties": { "name": name },
                "required": ["name"]
            }),
        }
    }

    fn read_only(self) -> bool {
        matches!(self, Kind::List | Kind::History)
    }

    fn labels(self, input: &serde_json::Value) -> (String, String) {
        let name = input["name"].as_str().unwrap_or("").trim();
        let paused = input["paused"].as_bool().unwrap_or(true);
        match self {
            Kind::Create => (format!("scheduling {name}"), format!("Scheduled {name}")),
            Kind::List => ("checking the schedule".into(), "Checked the schedule".into()),
            Kind::Delete => (format!("removing the {name} schedule"), format!("Removed the {name} schedule")),
            Kind::SetPaused if paused => (format!("pausing {name}"), format!("Paused {name}")),
            Kind::SetPaused => (format!("resuming {name}"), format!("Resumed {name}")),
            Kind::RunNow => (format!("running {name} now"), format!("Ran {name}")),
            Kind::History => (format!("checking the runs of {name}"), format!("Checked the runs of {name}")),
        }
    }
}

fn str_field<'a>(input: &'a serde_json::Value, key: &str) -> &'a str {
    input.get(key).and_then(|v| v.as_str()).map(str::trim).unwrap_or("")
}

/// The time field of a create: `cron`, or `schedule` for it.
fn cron_field(input: &serde_json::Value) -> &str {
    Some(str_field(input, "cron")).filter(|c| !c.is_empty()).unwrap_or_else(|| str_field(input, "schedule"))
}

/// One tool of the schedule family, over the cron store.
pub struct ScheduleTool {
    store: Arc<Store>,
    kind: Kind,
}

/// Every schedule tool, sharing one store.
pub fn tools(store: Arc<Store>) -> Vec<ScheduleTool> {
    KINDS.iter().map(|&kind| ScheduleTool { store: store.clone(), kind }).collect()
}

impl DynTool for ScheduleTool {
    fn name(&self) -> &str {
        self.kind.name()
    }

    fn description(&self) -> String {
        self.kind.description().to_string()
    }

    fn schema(&self) -> serde_json::Value {
        self.kind.schema()
    }

    fn search_hint(&self) -> &str {
        self.kind.search_hint()
    }

    /// create_schedule is always loaded: deferred, "Remind me in 3 hours"
    /// never found it in the proof runs of 2026-09-26 (3/3 → 0/3, and one
    /// run told the owner there was no scheduling tool). The rest of the
    /// family is deferred.
    fn should_defer(&self) -> bool {
        self.kind != Kind::Create
    }

    fn read_only(&self, _input: &serde_json::Value) -> bool {
        self.kind.read_only()
    }

    /// A scheduled command is a shell command run later: the same shell
    /// rules decide it now, so scheduling grants nothing new.
    fn rule_field(&self, input: &serde_json::Value) -> Option<types::permissions::RuleField> {
        let command = str_field(input, "command");
        (self.kind == Kind::Create && !command.is_empty())
            .then(|| types::permissions::RuleField::CommandPrefix(command.to_string()))
    }

    fn capability(&self, input: &serde_json::Value) -> Option<&'static str> {
        (self.kind == Kind::Create && !str_field(input, "command").is_empty()).then_some("shell")
    }

    /// A schedule is the employee's own work; a scheduled prompt runs later
    /// with the employee's own permissions. What a shell command does can't
    /// be known from its text.
    fn effects(&self, input: &serde_json::Value) -> types::permissions::CallEffects {
        if self.kind == Kind::Create && !str_field(input, "command").is_empty() {
            types::permissions::CallEffects::unknown()
        } else {
            types::permissions::CallEffects::none()
        }
    }

    fn validate_input(&self, input: &serde_json::Value) -> Result<(), String> {
        if self.kind != Kind::Create {
            return Ok(());
        }
        let (at, cron) = (str_field(input, "at"), cron_field(input));
        if at.is_empty() == cron.is_empty() {
            return Err("Give exactly one of `at` (once, e.g. \"in 3 hours\") or `cron` (a clock time or repeat).".into());
        }
        let (prompt, command) = (str_field(input, "prompt"), str_field(input, "command"));
        if prompt.is_empty() == command.is_empty() {
            return Err("Give exactly one of `prompt` (what you do when it fires) or `command` (a shell command).".into());
        }
        if !at.is_empty() && parse_relative_time(at).is_none() {
            return Err(format!(
                "Could not read at: \"{at}\". Use a time from now like \"in 5 minutes\" or \"in 2 hours\"; for a clock time use `cron`."
            ));
        }
        Ok(())
    }

    fn activity(&self, input: &serde_json::Value) -> String {
        self.kind.labels(input).0
    }

    fn outcome(&self, input: &serde_json::Value) -> String {
        self.kind.labels(input).1
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let name = str_field(&input, "name");
            match self.kind {
                Kind::Create => self.create(ctx, &input).await,
                Kind::List => self.list(),
                Kind::Delete => match self.store.delete_cron_job_by_name(name) {
                    Ok(count) if count > 0 => ToolResult::ok(format!("Deleted schedule: {name}")),
                    Ok(_) => not_found(name),
                    Err(e) => ToolResult::error(format!("Failed to delete: {e}")),
                },
                Kind::SetPaused => {
                    match self.store.get_cron_job_by_name(name) {
                        Ok(None) => return not_found(name),
                        Err(e) => return ToolResult::error(format!("Failed to find schedule: {e}")),
                        Ok(Some(_)) => {}
                    }
                    if input["paused"].as_bool().unwrap_or(true) {
                        match self.store.disable_cron_job_by_name(name) {
                            Ok(_) => ToolResult::ok(format!("Paused schedule: {name}")),
                            Err(e) => ToolResult::error(format!("Failed to pause: {e}")),
                        }
                    } else {
                        match self.store.enable_cron_job_by_name(name) {
                            Ok(_) => ToolResult::ok(format!("Resumed schedule: {name}")),
                            Err(e) => ToolResult::error(format!("Failed to resume: {e}")),
                        }
                    }
                }
                Kind::RunNow => self.run_now(name).await,
                Kind::History => self.history(name),
            }
        })
    }
}

fn not_found(name: &str) -> ToolResult {
    ToolResult::error(format!("No schedule named '{name}'. list_schedules shows the names."))
}

impl ScheduleTool {
    async fn create(&self, ctx: &ToolContext, input: &serde_json::Value) -> ToolResult {
        let name = str_field(input, "name");
        let cron_val = cron_field(input);
        let command = str_field(input, "command");
        let prompt = str_field(input, "prompt");
        let overlap = match input["overlap"].as_str().map(db::models::OverlapPolicy::parse).transpose() {
            Ok(o) => o,
            Err(e) => return ToolResult::error(e.to_string()),
        };

        let mut fires_at: Option<String> = None;
        let schedule = if !cron_val.is_empty() {
            cron_val.to_string()
        } else {
            match parse_relative_time(str_field(input, "at")) {
                Some((s, target)) => {
                    fires_at = Some(target.format("%Y-%m-%d %H:%M:%S %Z").to_string());
                    s
                }
                None => return ToolResult::error("Could not read `at`; use a time from now like \"in 5 minutes\"."),
            }
        };

        let (task_type, cmd, msg) = if command.is_empty() {
            ("agent", "", Some(prompt))
        } else {
            // Cron commands execute later without going through the
            // interactive shell pipeline — run the same unconditional
            // safeguard here at creation time so a scheduled job can't
            // smuggle a command the shell tool would refuse.
            if let Some(block) =
                crate::safeguard::check_safeguard("run_command", &serde_json::json!({ "command": command }))
            {
                return ToolResult::error(format!("Refusing to schedule this command: {block}"));
            }
            ("bash", command, None::<&str>)
        };

        // Capture the originating agent + channel context so the
        // scheduler can route the response back to the same place
        // (e.g. timer set in a Slack thread → alert in the same
        // thread). agent_id is parsed from session_key, channel
        // is read from ctx.channel — both NULL when the task was
        // created outside an agent-bound channel conversation.
        let agent_id = Some(types::keyparser::extract_agent_id(&ctx.session_key)).filter(|id| !id.is_empty());
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
            overlap,
        ) {
            Ok(job) => ToolResult::ok(format!(
                "Created schedule '{}' (id={}): {} ({}){}",
                name,
                job.id,
                schedule,
                if command.is_empty() { "prompt" } else { "command" },
                fires_at.map(|t| format!("; fires at {t}")).unwrap_or_default()
            )),
            Err(e) if e.to_string().contains("UNIQUE constraint failed: cron_jobs.name") => ToolResult::error(format!(
                "A schedule named '{name}' already exists. Delete it first with delete_schedule or pick another name."
            )),
            Err(e) => ToolResult::error(format!("Failed to create schedule: {e}")),
        }
    }

    fn list(&self) -> ToolResult {
        match self.store.list_cron_jobs(LIST_CAP, 0) {
            Ok(jobs) if jobs.is_empty() => ToolResult::ok("No schedules."),
            Ok(jobs) => {
                let total = self
                    .store
                    .count_cron_jobs()
                    .map(|n| n.max(jobs.len() as i64) as usize)
                    .unwrap_or(jobs.len());
                let lines: Vec<String> = jobs
                    .iter()
                    .map(|j| {
                        let state = if j.enabled.unwrap_or(0) != 0 { "active" } else { "paused" };
                        let work = if j.task_type == "agent" { "prompt" } else { "command" };
                        format!("- {} [{}] ({}) — {}", j.name, state, work, j.schedule)
                    })
                    .collect();
                ToolResult::ok(format!("{}\n{}", list_header(jobs.len(), total), lines.join("\n")))
            }
            Err(e) => ToolResult::error(format!("Failed to list schedules: {e}")),
        }
    }

    async fn run_now(&self, name: &str) -> ToolResult {
        let job = match self.store.get_cron_job_by_name(name) {
            Ok(Some(job)) => job,
            Ok(None) => return not_found(name),
            Err(e) => return ToolResult::error(format!("Failed to find schedule: {e}")),
        };
        // One fire, queued to the engine — the same way a scheduled fire
        // runs. Wait for it to settle so the caller gets the outcome, not a
        // promise.
        let run_id = match self.store.queue_cron_run(&job, true, false) {
            Ok(id) => id,
            Err(e) => return ToolResult::error(format!("Failed to queue the run: {e}")),
        };
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(RUN_NOW_WAIT_SECS);
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            match self.store.engine_get_run(&run_id) {
                Ok(Some(run)) if run.state == "done" => {
                    return ToolResult::ok(format!("'{}' ran successfully:\n{}", name, run.result.unwrap_or_default()));
                }
                Ok(Some(run)) if matches!(run.state.as_str(), "failed" | "cancelled") => {
                    return ToolResult::error(format!(
                        "'{}' failed:\n{}",
                        name,
                        run.error.or(run.result).unwrap_or_default()
                    ));
                }
                Ok(Some(_)) => {}
                Ok(None) => return ToolResult::error(format!("The run record of '{name}' disappeared")),
                Err(e) => return ToolResult::error(format!("Failed to read the run: {e}")),
            }
            if tokio::time::Instant::now() >= deadline {
                return ToolResult::ok(format!(
                    "'{name}' is still running after {RUN_NOW_WAIT_SECS}s. It carries on by itself and its outcome lands in its history; go on with other work."
                ));
            }
        }
    }

    fn history(&self, name: &str) -> ToolResult {
        let job = match self.store.get_cron_job_by_name(name) {
            Ok(Some(job)) => job,
            Ok(None) => return not_found(name),
            Err(e) => return ToolResult::error(format!("Failed to find schedule: {e}")),
        };
        match self.store.get_recent_cron_history(job.id) {
            Ok(history) if history.is_empty() => ToolResult::ok(format!("'{name}' has not run yet.")),
            Ok(history) => {
                let lines: Vec<String> = history
                    .iter()
                    .map(|h| {
                        let status = if h.success.unwrap_or(0) != 0 { "OK" } else { "FAIL" };
                        format!("- [{}] {}", status, h.output.as_deref().unwrap_or("-"))
                    })
                    .collect();
                ToolResult::ok(format!("Runs of '{}':\n{}", name, lines.join("\n")))
            }
            Err(e) => ToolResult::error(format!("Failed to get history: {e}")),
        }
    }
}

/// Cap on `list_schedules`; the header says "showing N of M" when it applies.
const LIST_CAP: i64 = 100;

/// How long `run_schedule_now` waits for the engine to settle a run.
const RUN_NOW_WAIT_SECS: u64 = 600;

/// Header for `list_schedules`: "N schedules" when the list is complete,
/// "showing N of M schedules" when the cap cut it.
fn list_header(shown: usize, total: usize) -> String {
    if total > shown {
        format!("showing {shown} of {total} schedules (list cap {LIST_CAP}):")
    } else {
        format!("{shown} schedules:")
    }
}

/// Parse relative time strings like "in 5 minutes" into a one-shot cron
/// expression plus the local time it resolves to, so the result can say when
/// the schedule fires.
///
/// Cron expressions are emitted in **local time**: the machine's local
/// timezone IS the owner's wall clock, and employees author schedules in
/// those terms (e.g. "morning briefing at 7 AM" means 7 AM local). The
/// scheduler (`crates/server/src/scheduler.rs::tick`) reads `Local::now()`
/// and evaluates `schedule.after()` with a local-time `last_run`, so this
/// side must match.
fn parse_relative_time(input: &str) -> Option<(String, chrono::DateTime<Local>)> {
    let s = input.trim().to_lowercase();
    let s = s.strip_prefix("in ").unwrap_or(&s);

    let mut parts = s.split_whitespace();
    let num: i64 = parts.next()?.parse().ok()?;
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
    use serde_json::json;

    fn store() -> (Arc<Store>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("s.db").to_string_lossy()).unwrap());
        (store, dir)
    }

    fn tool(store: &Arc<Store>, name: &str) -> ScheduleTool {
        tools(store.clone()).into_iter().find(|t| t.name() == name).unwrap()
    }

    #[test]
    fn list_header_says_showing_n_of_m_only_when_capped() {
        assert_eq!(list_header(3, 3), "3 schedules:");
        assert_eq!(list_header(100, 240), "showing 100 of 240 schedules (list cap 100):");
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

    /// A create needs exactly one time and exactly one kind of work, and a
    /// relative time it can read; each refusal says the call to make.
    #[test]
    fn a_create_names_one_time_and_one_kind_of_work() {
        let (s, _d) = store();
        let create = tool(&s, "create_schedule");
        let ok = json!({"name": "call-back", "at": "in 3 hours", "prompt": "Remind the owner to call back"});
        assert!(create.validate_input(&ok).is_ok());
        assert!(create.validate_input(&json!({"name": "x", "cron": "0 0 9 * * 1-5", "command": "echo hi"})).is_ok());
        let no_time = create.validate_input(&json!({"name": "x", "prompt": "p"})).unwrap_err();
        assert!(no_time.contains("`at`") && no_time.contains("`cron`"), "{no_time}");
        assert!(create.validate_input(&json!({"name": "x", "at": "in 1 hour", "cron": "0 0 9 * * *", "prompt": "p"})).is_err());
        let no_work = create.validate_input(&json!({"name": "x", "at": "in 1 hour"})).unwrap_err();
        assert!(no_work.contains("`prompt`") && no_work.contains("`command`"), "{no_work}");
        let clock = create.validate_input(&json!({"name": "x", "at": "3pm", "prompt": "p"})).unwrap_err();
        assert!(clock.contains("\"3pm\"") && clock.contains("`cron`"), "{clock}");
    }

    /// Create, list, pause, resume and delete, each through its own tool.
    #[tokio::test]
    async fn each_tool_does_its_one_job() {
        let (s, _d) = store();
        let ctx = ToolContext::default();
        let r = tool(&s, "create_schedule")
            .execute_dyn(&ctx, json!({"name": "call-back", "at": "in 5 minutes", "prompt": "Remind the owner: call back"}))
            .await;
        assert!(!r.is_error && r.content.contains("Created schedule 'call-back'") && r.content.contains("fires at"), "{}", r.content);
        let listed = tool(&s, "list_schedules").execute_dyn(&ctx, json!({})).await;
        assert!(listed.content.contains("call-back [active] (prompt)"), "{}", listed.content);
        let paused = tool(&s, "set_schedule_paused").execute_dyn(&ctx, json!({"name": "call-back", "paused": true})).await;
        assert!(!paused.is_error, "{}", paused.content);
        assert!(tool(&s, "list_schedules").execute_dyn(&ctx, json!({})).await.content.contains("[paused]"));
        tool(&s, "set_schedule_paused").execute_dyn(&ctx, json!({"name": "call-back", "paused": false})).await;
        assert!(tool(&s, "list_schedules").execute_dyn(&ctx, json!({})).await.content.contains("[active]"));
        let history = tool(&s, "schedule_history").execute_dyn(&ctx, json!({"name": "call-back"})).await;
        assert!(history.content.contains("has not run yet"), "{}", history.content);
        let deleted = tool(&s, "delete_schedule").execute_dyn(&ctx, json!({"name": "call-back"})).await;
        assert!(!deleted.is_error, "{}", deleted.content);
        let gone = tool(&s, "delete_schedule").execute_dyn(&ctx, json!({"name": "call-back"})).await;
        assert!(gone.is_error && gone.content.contains("list_schedules"), "{}", gone.content);
    }

    /// A scheduled command meets the shell's rules at creation: the shell
    /// capability and its command prefix, and the shell safeguard. A
    /// scheduled prompt is the employee's own work.
    #[test]
    fn a_scheduled_command_is_decided_as_a_command() {
        let (s, _d) = store();
        let create = tool(&s, "create_schedule");
        let cmd = json!({"name": "x", "cron": "0 0 9 * * *", "command": "rm -rf /tmp/cache"});
        assert_eq!(create.capability(&cmd), Some("shell"));
        assert_eq!(
            create.rule_field(&cmd),
            Some(types::permissions::RuleField::CommandPrefix("rm -rf /tmp/cache".into()))
        );
        assert_eq!(create.effects(&cmd), types::permissions::CallEffects::unknown());
        let prompt = json!({"name": "x", "at": "in 1 hour", "prompt": "check the calendar"});
        assert_eq!(create.capability(&prompt), None);
        assert_eq!(create.rule_field(&prompt), None);
        assert_eq!(create.effects(&prompt), types::permissions::CallEffects::none());
        assert!(tool(&s, "list_schedules").read_only(&json!({})));
        assert!(!tool(&s, "delete_schedule").read_only(&json!({"name": "x"})));
    }
}
