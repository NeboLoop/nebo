//! Housekeeping loop and the ONE way a scheduled job executes. Firing —
//! when a job is due, that it fires once, that a fire interrupted by a
//! restart is retried once — is the engine's (`crate::engine`); this module
//! runs the job the engine hands it and returns the outcome.

use std::sync::Arc;
use std::time::Duration;

use tokio::process::Command;
use tracing::{error, info, warn};

use db::Store;
use db::models::CronJob;
use tools::Origin;

use crate::run_registry::RegisterParams;
use crate::state::AppState;

/// Spawn the housekeeping loop: boot-time recovery for workflow runs and
/// session wakes, then a sweep every 60 seconds.
pub fn spawn(
    store: Arc<Store>,
    snapshot_store: Arc<browser::SnapshotStore>,
    workflow_manager: Arc<dyn tools::workflows::WorkflowManager>,
    state: AppState,
) {
    tokio::spawn(async move {
        // Initial delay to let the server boot
        tokio::time::sleep(Duration::from_secs(10)).await;

        // Resuming stranded runs advances workflows: not until this bot
        // holds its lease (a cloud bot started while another copy runs
        // must not resume that copy's work).
        comm::lease::process().until_unfrozen().await;

        // Workflow runs stranded by process death (WS4): stamp interrupted,
        // resume from the last completed activity via the snapshotted
        // definition, fail the unresumable with a narrated reason.
        state.workflow_manager.recover_interrupted_runs().await;

        // Session wakes persisted but not delivered before a crash (session
        // wake rail, R1): redeliver on boot.
        crate::wake::recover_pending_wakes(&state).await;

        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            if crate::DRAINING.load(std::sync::atomic::Ordering::Relaxed)
                || comm::lease::process().frozen()
            {
                continue;
            }
            sweep(&store, &workflow_manager);
            // Cleanup expired snapshots
            snapshot_store.cleanup();
            nightly_backup(&store, &state).await;
            permission_digest(&state);
            crate::backup_ship::commit_if_due(&store, &state).await;
        }
    });
}

fn sweep(store: &Arc<Store>, workflow_manager: &Arc<dyn tools::workflows::WorkflowManager>) {
    // Cleanup old completed/failed/cancelled tasks (7-day TTL)
    if let Err(e) = store.delete_completed_tasks() {
        warn!("failed to cleanup old tasks: {}", e);
    }

    // Weekly workflow tuning pass, checked once a day per process. The sweep
    // itself gates per agent: learning mode, 2+ recent failures, and at most
    // one proposal per agent per 7 days.
    {
        use std::sync::atomic::{AtomicI64, Ordering};
        static LAST_TUNING_CHECK: AtomicI64 = AtomicI64::new(0);
        let now = chrono::Utc::now().timestamp();
        let last = LAST_TUNING_CHECK.load(Ordering::Relaxed);
        if now - last > 24 * 3600
            && LAST_TUNING_CHECK
                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            let manager = workflow_manager.clone();
            tokio::spawn(async move {
                manager.tuning_sweep().await;
            });
        }
    }

    // Expire staged self-improvement writes past their 30-day TTL and clear
    // their Inbox cards (audited gap: approvals pending forever).
    match store.expire_pending_writes() {
        Ok(ids) => {
            if !ids.is_empty() {
                let user_id = store.ensure_local_user_id().unwrap_or_default();
                for pid in ids {
                    let _ = store.delete_notification(&format!("learn:{}", pid), &user_id);
                }
            }
        }
        Err(e) => warn!("failed to expire pending writes: {}", e),
    }
}

/// Execute one fire of a job. Returns (success, output, error).
pub(crate) async fn execute_job(state: &AppState, job: &CronJob) -> (bool, String, Option<String>) {
    match job.task_type.as_str() {
        "bash" | "shell" | "" => execute_shell(&job.command).await,
        "agent" => execute_agent(state, job).await,
        "workflow" => execute_workflow_task(&*state.workflow_manager, &job.command).await,
        "agent_workflow" | "role_workflow" => {
            execute_agent_workflow_task(&*state.workflow_manager, &state.store, &job.command, "schedule").await
        }
        other => (
            false,
            String::new(),
            Some(format!("unknown task type: {}", other)),
        ),
    }
}


async fn execute_shell(command: &str) -> (bool, String, Option<String>) {
    match Command::new("sh").arg("-c").arg(command).output().await {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            if output.status.success() {
                (true, stdout, None)
            } else {
                let err = if stderr.is_empty() {
                    format!("exit code: {}", output.status.code().unwrap_or(-1))
                } else {
                    stderr
                };
                (false, stdout, Some(err))
            }
        }
        Err(e) => (false, String::new(), Some(e.to_string())),
    }
}

/// The turn a scheduled job runs: the job's message, with its standing
/// instructions after it, written into the thread as the owner scheduled
/// it, run by its employee under the employee's own grant (scheduling grants
/// nothing new).
fn scheduled_turn(
    job: &CronJob,
    prompt: &str,
    session_key: &str,
    agent_id: &str,
    channel: &str,
    channel_ctx: Option<tools::ChannelContext>,
    cancel: tokio_util::sync::CancellationToken,
) -> agent::TurnRequest {
    use agent::harness::{Delivery, SeatRequest, TurnInput, TurnMode};

    let text = match job.instructions.as_deref().map(str::trim).filter(|i| !i.is_empty()) {
        Some(instructions) => format!("{prompt}\n\n{instructions}"),
        None => prompt.to_string(),
    };
    agent::TurnRequest {
        session_key: session_key.to_string(),
        input: TurnInput::Owner { text, images: Vec::new(), attachments: Vec::new() },
        seat: SeatRequest {
            agent_id: agent_id.to_string(),
            user_id: String::new(),
            origin: Origin::System,
            door: types::permissions::Door::Schedule,
            mode: None,
            ceiling: None,
            cwd: None,
            seed_taint: Vec::new(),
            audience: None,
            tool_allowlist: None,
            tool_denial_hint: None,
            handoff_depth: 0,
            model_override: String::new(),
            model_preference: None,
            personality_snippet: None,
            tool_scope: None,
        },
        mode: TurnMode::Chat,
        delivery: Delivery {
            channel: channel.to_string(),
            channel_ctx,
            mention_briefing: None,
        },
        cancel,
        progress: None,
    }
}

async fn execute_agent(state: &AppState, job: &CronJob) -> (bool, String, Option<String>) {
    let prompt = job.message.as_deref().unwrap_or(&job.command);

    // If this job was created from an agent-bound channel conversation, route
    // the response back through that channel's bridge — same pathway inbound
    // replies use. Without this, the cron-fired agent run posts nothing
    // visible to the user (chat_stream broadcast doesn't reach Slack).
    if let (Some(agent_id), Some(ctx_json)) =
        (job.agent_id.as_deref(), job.channel_ctx_json.as_deref())
    {
        if !agent_id.is_empty() && !ctx_json.is_empty() {
            return execute_agent_channel_bound(state, job, agent_id, ctx_json, prompt).await;
        }
    }

    // A job an employee scheduled runs AS that employee: its session key
    // carries the employee (tools scope to it — its workflows, its runs),
    // and its operation policy governs. Run as the owner's front desk, an
    // employee's check on its own workflow run found no such workflow and
    // said so every fire.
    let agent_id = job.agent_id.as_deref().filter(|a| !a.is_empty());
    let session_key = match agent_id {
        Some(id) => format!("agent:{}:cron:{}", id, job.name),
        None => format!("cron-{}", job.name),
    };
    let cancel_token = tokio_util::sync::CancellationToken::new();

    // Register in the global RunRegistry so cron runs are visible and cancellable
    let run_handle = state
        .run_registry
        .register(RegisterParams {
            session_key: session_key.clone(),
            entity_id: agent_id.unwrap_or("main").to_string(),
            entity_name: format!("Cron: {}", job.name),
            origin: "cron".to_string(),
            channel: "cron".to_string(),
            cancel_token: cancel_token.clone(),
            parent_run_id: None,
        })
        .await;

    let req = scheduled_turn(job, prompt, &session_key, agent_id.unwrap_or_default(), "cron", None, cancel_token);

    match state.harness.start_turn(req).await {
        Ok(handle) => {
            let mut rx = handle.events;
            let mut full_text = String::new();
            while let Some(event) = rx.recv().await {
                run_handle.touch();
                match event.event_type {
                    ai::StreamEventType::Text => {
                        full_text.push_str(&event.text);
                        state.hub.broadcast(
                            "chat_stream",
                            serde_json::json!({
                                "session_id": session_key,
                                "content": event.text,
                            }),
                        );
                    }
                    ai::StreamEventType::Error => {
                        let err = event.error.unwrap_or_default();
                        return (false, full_text, Some(err));
                    }
                    ai::StreamEventType::Done => break,
                    _ => {}
                }
            }
            drop(run_handle);
            (true, full_text, None)
        }
        Err(e) => {
            error!(job = job.name.as_str(), error = %e, "agent run failed");
            drop(run_handle);
            (false, String::new(), Some(e.to_string()))
        }
    }
}

/// Fire a cron job whose originating channel context was captured at
/// `create_schedule` time. Runs the agent with the same `ChannelContext` the
/// inbound message would have carried, then writes the response to the
/// channel-plugin bridge as an `op: "post"` so it lands in the originating
/// thread.
async fn execute_agent_channel_bound(
    state: &AppState,
    job: &CronJob,
    agent_id: &str,
    ctx_json: &str,
    prompt: &str,
) -> (bool, String, Option<String>) {
    #[derive(serde::Deserialize)]
    struct SavedCtx {
        kind: String,
        channel_id: String,
        #[serde(default)]
        thread_ts: Option<String>,
    }

    let saved: SavedCtx = match serde_json::from_str(ctx_json) {
        Ok(c) => c,
        Err(e) => {
            warn!(
                job = job.name.as_str(),
                error = %e,
                "scheduler: invalid channel_ctx_json on cron job; running without channel context"
            );
            return (false, String::new(), Some(format!("invalid channel_ctx_json: {e}")));
        }
    };

    let channel_ctx = tools::ChannelContext {
        kind: saved.kind.clone(),
        channel_id: saved.channel_id.clone(),
        thread_ts: saved.thread_ts.clone(),
    };

    // Use the same session_key format inbound messages use, so the agent
    // sees the same thread history.
    let session_key = format!(
        "agent:{}:{}:{}",
        agent_id, saved.kind, saved.channel_id
    );
    let cancel_token = tokio_util::sync::CancellationToken::new();

    let run_handle = state
        .run_registry
        .register(RegisterParams {
            session_key: session_key.clone(),
            entity_id: agent_id.to_string(),
            entity_name: format!("Cron: {}", job.name),
            origin: "cron".to_string(),
            channel: saved.kind.clone(),
            cancel_token: cancel_token.clone(),
            parent_run_id: None,
        })
        .await;

    let req = scheduled_turn(job, prompt, &session_key, agent_id, &saved.kind, Some(channel_ctx.clone()), cancel_token);

    let mut full_text = String::new();
    match state.harness.start_turn(req).await {
        Ok(handle) => {
            let mut rx = handle.events;
            while let Some(event) = rx.recv().await {
                run_handle.touch();
                match event.event_type {
                    ai::StreamEventType::Text => {
                        full_text.push_str(&event.text);
                    }
                    ai::StreamEventType::Error => {
                        let err = event.error.unwrap_or_default();
                        drop(run_handle);
                        return (false, full_text, Some(err));
                    }
                    ai::StreamEventType::Done => break,
                    _ => {}
                }
            }
            drop(run_handle);
        }
        Err(e) => {
            error!(
                job = job.name.as_str(),
                error = %e,
                "channel-bound agent run failed"
            );
            drop(run_handle);
            return (false, String::new(), Some(e.to_string()));
        }
    }

    let response = full_text.trim().to_string();
    if response.is_empty() {
        return (true, full_text, None);
    }

    // Route the response back through the channel bridge as op:"post"
    // (not "reply" — there's no inbound placeholder to update; the agent is
    // posting on its own initiative).
    if let Err(e) = crate::channel_dispatch::post_to_channel(state, agent_id, &channel_ctx, response).await {
        warn!(
            job = job.name.as_str(),
            agent = agent_id,
            plugin = saved.kind.as_str(),
            error = %e,
            "scheduler: channel-bound cron response not posted"
        );
        return (false, full_text, Some(e));
    }

    info!(
        job = job.name.as_str(),
        agent = agent_id,
        plugin = saved.kind.as_str(),
        channel = saved.channel_id.as_str(),
        "scheduler: posted channel-bound cron response via bridge"
    );

    (true, full_text, None)
}

async fn execute_workflow_task(
    manager: &dyn tools::workflows::WorkflowManager,
    workflow_id: &str,
) -> (bool, String, Option<String>) {
    match manager
        .run(workflow_id, serde_json::Value::Null, "cron")
        .await
    {
        Ok(run_id) => (true, format!("workflow run started: {}", run_id), None),
        Err(e) => (false, String::new(), Some(e)),
    }
}

/// Execute an agent's inline workflow. Command format: `agent:{agent_id}:{binding_name}`
pub(crate) async fn execute_agent_workflow_task(
    manager: &dyn tools::workflows::WorkflowManager,
    store: &Store,
    command: &str,
    trigger: &str,
) -> (bool, String, Option<String>) {
    let parts: Vec<&str> = command.splitn(3, ':').collect();
    if parts.len() != 3 || (parts[0] != "agent" && parts[0] != "role") {
        return (
            false,
            String::new(),
            Some(format!("invalid agent_workflow command: {}", command)),
        );
    }
    let agent_id = parts[1];
    let binding_name = parts[2];

    // Guard: skip if automation is disabled or agent is disabled
    match store.is_agent_workflow_active(agent_id, binding_name) {
        Ok(false) => {
            info!(agent_id, binding_name, "skipping disabled agent workflow");
            return (
                false,
                String::new(),
                Some("automation is disabled".to_string()),
            );
        }
        Err(e) => {
            warn!(agent_id, binding_name, error = %e, "failed to check agent workflow status");
            // Fail closed: don't execute if we can't verify it's active
            return (
                false,
                String::new(),
                Some(format!("failed to check active status: {}", e)),
            );
        }
        Ok(true) => {} // proceed
    }

    // Load agent config from DB
    let agent_rec = match store.get_agent(agent_id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                false,
                String::new(),
                Some(format!("agent not found: {}", agent_id)),
            );
        }
        Err(e) => return (false, String::new(), Some(format!("db error: {}", e))),
    };

    let config = match napp::agent::parse_agent_config(&agent_rec.frontmatter) {
        Ok(c) => c,
        Err(e) => {
            return (
                false,
                String::new(),
                Some(format!("parse agent config: {}", e)),
            );
        }
    };

    let binding = match config.workflows.get(binding_name) {
        Some(b) => b,
        None => {
            return (
                false,
                String::new(),
                Some(format!("binding '{}' not found in agent", binding_name)),
            );
        }
    };

    if !binding.has_activities() {
        return (
            false,
            String::new(),
            Some("binding has no activities".to_string()),
        );
    }

    let def_json = binding.to_workflow_json(binding_name);
    let inputs: serde_json::Value = serde_json::to_value(&binding.inputs).unwrap_or_default();
    let emit_sources: Vec<String> = binding
        .emit
        .iter()
        .map(|emit_name| workflow::events::emit_source_for(&agent_rec.name, emit_name))
        .collect();

    match manager
        .run_inline(
            def_json,
            inputs,
            trigger,
            Some(binding_name.to_string()),
            agent_id,
            emit_sources,
        )
        .await
    {
        Ok(run_id) => (
            true,
            format!("inline workflow run started: {}", run_id),
            None,
        ),
        Err(e) => (false, String::new(), Some(e)),
    }
}

/// One verified copy of the database a day, kept by the retention rule.
///
/// "Nightly" means at most once in 24 hours and as soon as a day has passed,
/// so a Nebo that starts for the first time has a backup within a minute and
/// one that was off for a week catches up on its first tick. A copy that
/// fails to verify is not kept, and the owner hears about it in the inbox —
/// a broken copy of a broken database is the moment they need to know.
async fn nightly_backup(store: &Arc<Store>, state: &AppState) {
    let due = match store.list_backups() {
        Ok(rows) => rows
            .iter()
            .filter(|b| b.reason == "nightly")
            .map(|b| b.taken_at)
            .max()
            .map(|t| now_secs() - t >= 24 * 3600)
            .unwrap_or(true),
        Err(_) => true,
    };
    if !due {
        return;
    }
    let s = store.clone();
    let result = tokio::task::spawn_blocking(move || {
        let taken = s.snapshot("nightly")?;
        let _ = s.retain_backups();
        Ok::<_, types::NeboError>(taken)
    })
    .await;
    let err = match result {
        Ok(Ok(_)) => return,
        Ok(Err(e)) => e.to_string(),
        Err(e) => e.to_string(),
    };
    tracing::error!(error = %err, "nightly database backup failed");
    let day = now_secs() / 86_400;
    tools::owner_notify::emit(
        store,
        Some(&|name: &str, payload: serde_json::Value| state.hub.broadcast(name, payload)),
        &tools::owner_notify::OwnerNotification {
            id: &format!("backup-failed:{day}"),
            kind: "error",
            title: "Tonight's backup failed",
            body: Some(&format!("The copy could not be verified: {err}. The last good backup still stands.")),
            action_url: None,
            agent_id: None,
            loud: true,
        },
    );
}

/// The doors of runs nobody watches as they happen: heartbeats, workflows,
/// schedules, helpers and coworker requests.
const UNATTENDED_DOORS: &[&str] = &["heartbeat", "workflow", "schedule", "helper", "coworker"];

/// Once a day, the owner's Inbox lists yesterday's actions from unattended
/// runs that ran unreviewed because the permission check couldn't run
/// (both judges down). One item per day under a stable id; none when there
/// is nothing to list.
fn permission_digest(state: &AppState) {
    use std::sync::atomic::{AtomicI64, Ordering};
    static LAST_DAY: AtomicI64 = AtomicI64::new(-1);
    let today = now_secs() / 86_400;
    let last = LAST_DAY.load(Ordering::Relaxed);
    if last == today || LAST_DAY.compare_exchange(last, today, Ordering::Relaxed, Ordering::Relaxed).is_err() {
        return;
    }
    if let Some(item) = permission_digest_item(&state.store, today - 1) {
        crate::codes::push_inbox(state, item);
    }
}

/// The digest item for one UTC day (days since the epoch), or `None` when
/// no unattended run acted unreviewed that day.
pub(crate) fn permission_digest_item(store: &Store, day: i64) -> Option<serde_json::Value> {
    let (start, end) = (day * 86_400, (day + 1) * 86_400);
    let rows: Vec<_> = match store.unreviewed_permission_activity(start, UNATTENDED_DOORS) {
        Ok(rows) => rows.into_iter().filter(|r| r.created_at < end).collect(),
        Err(e) => {
            warn!(error = %e, "permission digest: activity unreadable");
            return None;
        }
    };
    if rows.is_empty() {
        return None;
    }
    let name_of = |agent_id: &str| match store.get_agent(agent_id) {
        Ok(Some(a)) if !a.name.is_empty() => a.name,
        _ => "Your assistant".to_string(),
    };
    let lines: Vec<String> = rows
        .iter()
        .map(|r| format!("- {}: {} ({})", name_of(&r.agent_id), r.activity, r.door))
        .collect();
    let count = rows.len();
    Some(serde_json::json!({
        "id": format!("permission-digest:{day}"),
        "type": "permission_digest",
        "title": if count == 1 {
            "1 action ran without a permission check".to_string()
        } else {
            format!("{count} actions ran without a permission check")
        },
        "body": format!(
            "While no one was watching, {} because {}:\n{}",
            if count == 1 { "this action ran" } else { "these actions ran" },
            types::permissions::UNREVIEWED_REASON,
            lines.join("\n")
        ),
    }))
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(agent_id: &str, door: &str, activity: &str, unreviewed: bool, at: i64) -> db::PermissionActivityRow {
        db::PermissionActivityRow {
            agent_id: agent_id.into(),
            door: door.into(),
            tool: "browser".into(),
            rule_key: "browser_click".into(),
            activity: activity.into(),
            decision: "allow".into(),
            why: "{}".into(),
            unreviewed,
            created_at: at,
            ..Default::default()
        }
    }

    /// Yesterday's unreviewed actions from unattended runs, in one item;
    /// none when there are none. Chat actions and reviewed ones stay out.
    #[test]
    fn unreviewed_appears_in_the_daily_digest() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(&dir.path().join("d.db").to_string_lossy()).unwrap();
        let day = 20_000;
        let at = day * 86_400 + 3_600;
        assert!(permission_digest_item(&store, day).is_none(), "nothing to list: no item");
        for r in [
            row("", "heartbeat", "submitting the signup form", true, at),
            row("", "workflow", "posting the weekly update", true, at + 1),
            row("", "chat", "sending a reply", true, at + 2),
            row("", "heartbeat", "reading a page", false, at + 3),
            row("", "schedule", "the next day", true, at + 86_400),
        ] {
            store.record_permission_activity(&r).unwrap();
        }
        let item = permission_digest_item(&store, day).unwrap();
        assert_eq!(item["id"], format!("permission-digest:{day}"));
        assert_eq!(item["type"], "permission_digest");
        assert_eq!(item["title"], "2 actions ran without a permission check");
        let body = item["body"].as_str().unwrap();
        assert!(body.contains(types::permissions::UNREVIEWED_REASON), "{body}");
        assert!(body.contains("submitting the signup form (heartbeat)") && body.contains("posting the weekly update"));
        assert!(!body.contains("sending a reply") && !body.contains("reading a page") && !body.contains("the next day"));
    }
}
