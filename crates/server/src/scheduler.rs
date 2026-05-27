use std::sync::Arc;
use std::time::Duration;

use chrono::{Local, TimeZone};
use cron::Schedule;
use tokio::process::Command;
use tracing::{error, info, warn};

use agent::{RunRequest, Runner};
use db::Store;
use tools::Origin;

use crate::handlers::ws::ClientHub;
use crate::run_registry::{RegisterParams, RunRegistry};
use crate::state::AppState;

/// Spawn the cron scheduler loop. Polls enabled cron_jobs every 60 seconds.
pub fn spawn(
    store: Arc<Store>,
    runner: Arc<Runner>,
    hub: Arc<ClientHub>,
    snapshot_store: Arc<browser::SnapshotStore>,
    workflow_manager: Arc<dyn tools::workflows::WorkflowManager>,
    run_registry: RunRegistry,
    state: AppState,
) {
    tokio::spawn(async move {
        // Initial delay to let the server boot
        tokio::time::sleep(Duration::from_secs(10)).await;

        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            if let Err(e) = tick(
                &store,
                &runner,
                &hub,
                &*workflow_manager,
                &run_registry,
                &state,
            )
            .await
            {
                warn!("scheduler tick error: {}", e);
            }
            // Cleanup expired snapshots
            snapshot_store.cleanup();
        }
    });
}

async fn tick(
    store: &Store,
    runner: &Runner,
    hub: &ClientHub,
    workflow_manager: &dyn tools::workflows::WorkflowManager,
    run_registry: &RunRegistry,
    state: &AppState,
) -> Result<(), String> {
    // Cleanup old completed/failed/cancelled tasks (7-day TTL)
    if let Err(e) = store.delete_completed_tasks() {
        warn!("failed to cleanup old tasks: {}", e);
    }

    let jobs = store.list_enabled_cron_jobs().map_err(|e| e.to_string())?;

    // Crons are evaluated in the machine's local timezone — Nebo is a desktop
    // AI companion, so the host's wall clock IS the user's wall clock. Agent
    // authors write schedules like "0 0 7 * * 1-5" meaning 7 AM local. If we
    // compared against `Utc::now()` here, that same cron would fire at 7 AM
    // UTC — e.g. 1 AM MDT for an MDT user.
    let now = Local::now();

    for job in &jobs {
        // Normalize schedule at read time — handles stale 5-field expressions in DB
        let normalized = tools::PersonaTool::normalize_cron(&job.schedule);
        let schedule: Schedule = match normalized.parse() {
            Ok(s) => s,
            Err(e) => {
                warn!(job = job.name.as_str(), schedule = %normalized, error = %e, "invalid cron expression");
                continue;
            }
        };

        // last_run is stored as SQLite `datetime('now')` (UTC) — parse as UTC
        // then convert to Local so the cron comparison stays in one timezone.
        //
        // When last_run is NULL (never fired), fall back to created_at — NOT
        // to `now`. With a year-pinned one-shot cron (e.g. an "in 1 minute"
        // timer with cron `47 37 19 26 5 * 2026`), defaulting to `now` means
        // `schedule.after(now)` returns the single moment when it's still in
        // the future, but on the very next tick (60s later) `now` has
        // advanced past it AND last_run has advanced with it — so
        // `schedule.after(new_now)` returns None (no future occurrences in
        // 2026 match) and the task silently never fires. Using
        // `created_at` as the floor guarantees `schedule.after(floor).next()`
        // always returns the cron's moment, which we then compare to `now`.
        let parse_db_ts = |s: &str| -> Option<i64> {
            s.parse::<i64>().ok().or_else(|| {
                chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                    .ok()
                    .map(|dt| dt.and_utc().timestamp())
            })
        };
        let last_run_ts = job
            .last_run
            .as_deref()
            .and_then(parse_db_ts)
            .or_else(|| job.created_at.as_deref().and_then(parse_db_ts))
            .unwrap_or(0);
        let last_run = chrono::Utc
            .timestamp_opt(last_run_ts, 0)
            .single()
            .map(|dt| dt.with_timezone(&Local))
            .unwrap_or(now);

        // Get the upcoming time from last_run — if it's before now, the job is due
        let next = match schedule.after(&last_run).next() {
            Some(t) => t,
            None => continue,
        };

        if next > now {
            continue; // not yet due
        }

        info!(job = job.name.as_str(), "executing scheduled task");

        // Record history start
        let history = store.create_cron_history(job.id).ok();

        let (success, output, err_msg) = match job.task_type.as_str() {
            "bash" | "shell" | "" => execute_shell(&job.command).await,
            "agent" => execute_agent(runner, hub, job, run_registry, state).await,
            "workflow" => execute_workflow_task(workflow_manager, &job.command).await,
            "agent_workflow" | "role_workflow" => {
                execute_agent_workflow_task(workflow_manager, &store, &job.command).await
            }
            other => (
                false,
                String::new(),
                Some(format!("unknown task type: {}", other)),
            ),
        };

        // Best-effort: update last_run timestamp (non-critical tracking)
        let _ = store.update_cron_job_last_run(job.id, err_msg.as_deref());

        // Best-effort: update history record (non-critical tracking)
        if let Some(h) = history {
            let _ = store.update_cron_history(
                h.id,
                success,
                if output.is_empty() {
                    None
                } else {
                    Some(&output)
                },
                err_msg.as_deref(),
            );
        }

        // Suppress the OS-level "Nebo" desktop popup when the job already
        // delivered its response to a channel (Slack/Discord/etc.). The
        // channel post IS the user-facing notification; firing an additional
        // desktop alert is duplicate noise that says "test-timer-live-2
        // completed" — meaningless to a user who just got the real message
        // in Slack. Non-channel-bound jobs (shell, system workflows) still
        // get the desktop notification because they have no other surface.
        let channel_bound = job.agent_id.as_deref().is_some_and(|s| !s.is_empty())
            && job
                .channel_ctx_json
                .as_deref()
                .is_some_and(|s| !s.is_empty());

        if success {
            info!(job = job.name.as_str(), "task completed");
            if !channel_bound {
                notify_crate::send("Nebo", &format!("{} completed", job.name));
            }
        } else {
            let err = err_msg.as_deref().unwrap_or("unknown");
            warn!(job = job.name.as_str(), error = err, "task failed");
            // Always surface failures — even for channel-bound jobs — because
            // the channel-side delivery itself may have failed and the user
            // needs to know something went wrong.
            notify_crate::send("Nebo", &format!("{} failed: {}", job.name, err));
        }
    }

    Ok(())
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

async fn execute_agent(
    runner: &Runner,
    hub: &ClientHub,
    job: &db::models::CronJob,
    run_registry: &RunRegistry,
    state: &AppState,
) -> (bool, String, Option<String>) {
    let prompt = job.message.as_deref().unwrap_or(&job.command);

    // If this job was created from an agent-bound channel conversation, route
    // the response back through that channel's bridge — same pathway inbound
    // replies use. Without this, the cron-fired agent run posts nothing
    // visible to the user (chat_stream broadcast doesn't reach Slack).
    if let (Some(agent_id), Some(ctx_json)) =
        (job.agent_id.as_deref(), job.channel_ctx_json.as_deref())
    {
        if !agent_id.is_empty() && !ctx_json.is_empty() {
            return execute_agent_channel_bound(runner, job, run_registry, state, agent_id, ctx_json, prompt).await;
        }
    }

    let system = job.instructions.as_deref().unwrap_or("").to_string();
    let session_key = format!("cron-{}", job.name);
    let cancel_token = tokio_util::sync::CancellationToken::new();

    // Register in the global RunRegistry so cron runs are visible and cancellable
    let run_handle = run_registry
        .register(RegisterParams {
            session_key: session_key.clone(),
            entity_id: "main".to_string(),
            entity_name: format!("Cron: {}", job.name),
            origin: "cron".to_string(),
            channel: "cron".to_string(),
            cancel_token: cancel_token.clone(),
            parent_run_id: None,
        })
        .await;

    let req = RunRequest {
        session_key: session_key.clone(),
        prompt: prompt.to_string(),
        system,
        origin: Origin::System,
        channel: "cron".to_string(),
        cancel_token: cancel_token,
        ..Default::default()
    };

    match runner.run(req).await {
        Ok(mut rx) => {
            let mut full_text = String::new();
            while let Some(event) = rx.recv().await {
                run_handle.touch();
                match event.event_type {
                    ai::StreamEventType::Text => {
                        full_text.push_str(&event.text);
                        hub.broadcast(
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
/// `event(create)` time. Runs the agent with the same `ChannelContext` the
/// inbound message would have carried, then writes the response to the
/// channel-plugin bridge as an `op: "post"` so it lands in the originating
/// thread.
async fn execute_agent_channel_bound(
    runner: &Runner,
    job: &db::models::CronJob,
    run_registry: &RunRegistry,
    state: &AppState,
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

    let run_handle = run_registry
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

    let system = job.instructions.as_deref().unwrap_or("").to_string();
    let req = RunRequest {
        session_key: session_key.clone(),
        prompt: prompt.to_string(),
        system,
        origin: Origin::System,
        channel: saved.kind.clone(),
        agent_id: agent_id.to_string(),
        cancel_token: cancel_token,
        channel_ctx: Some(channel_ctx.clone()),
        ..Default::default()
    };

    let mut full_text = String::new();
    match runner.run(req).await {
        Ok(mut rx) => {
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
    let key = tools::channel_bridge_key(agent_id, &saved.kind);
    let handle = match state.channel_bridges.read().await.get(&key).cloned() {
        Some(h) => h,
        None => {
            warn!(
                job = job.name.as_str(),
                agent = agent_id,
                plugin = saved.kind.as_str(),
                "scheduler: response generated but channel bridge `{key}` is not running; dropping"
            );
            return (
                false,
                full_text,
                Some(format!(
                    "channel bridge `{key}` not running — enable {} for agent {} in Settings → Channels",
                    saved.kind, agent_id
                )),
            );
        }
    };

    let mut op = serde_json::Map::new();
    op.insert("op".into(), serde_json::Value::String("post".into()));
    op.insert(
        "channel".into(),
        serde_json::Value::String(saved.channel_id.clone()),
    );
    if let Some(ts) = &saved.thread_ts {
        op.insert("thread_ts".into(), serde_json::Value::String(ts.clone()));
    }
    op.insert("text".into(), serde_json::Value::String(response));

    if let Err(e) = handle.stdin_tx.send(serde_json::Value::Object(op)).await {
        warn!(
            job = job.name.as_str(),
            agent = agent_id,
            plugin = saved.kind.as_str(),
            error = %e,
            "scheduler: failed to forward post to channel bridge"
        );
        return (false, full_text, Some(format!("bridge send: {e}")));
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
async fn execute_agent_workflow_task(
    manager: &dyn tools::workflows::WorkflowManager,
    store: &Store,
    command: &str,
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
    let emit_source = binding.emit.as_ref().map(|emit_name| {
        let slug = agent_rec.name.to_lowercase().replace(' ', "-");
        format!("{}.{}", slug, emit_name)
    });

    match manager
        .run_inline(
            def_json,
            inputs,
            "schedule",
            Some(binding_name.to_string()),
            agent_id,
            emit_source,
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
