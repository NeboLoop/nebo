use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use cron::Schedule;
use tokio::process::Command;
use tracing::{error, info, warn};

use agent::{Runner, RunRequest};
use db::Store;
use tools::Origin;

use crate::handlers::ws::ClientHub;

/// Spawn the cron scheduler loop. Polls enabled cron_jobs every 60 seconds.
pub fn spawn(
    store: Arc<Store>,
    runner: Arc<Runner>,
    hub: Arc<ClientHub>,
    snapshot_store: Arc<browser::SnapshotStore>,
    workflow_manager: Arc<dyn tools::workflows::WorkflowManager>,
) {
    tokio::spawn(async move {
        // Initial delay to let the server boot
        tokio::time::sleep(Duration::from_secs(10)).await;

        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            if let Err(e) = tick(&store, &runner, &hub, &*workflow_manager).await {
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
) -> Result<(), String> {
    // Cleanup old completed/failed/cancelled tasks (7-day TTL)
    if let Err(e) = store.delete_completed_tasks() {
        warn!("failed to cleanup old tasks: {}", e);
    }

    let jobs = store
        .list_enabled_cron_jobs()
        .map_err(|e| e.to_string())?;

    let now = Utc::now();

    for job in &jobs {
        let schedule: Schedule = match job.schedule.parse() {
            Ok(s) => s,
            Err(e) => {
                warn!(job = job.name.as_str(), error = %e, "invalid cron expression");
                continue;
            }
        };

        // Check if job is due: find the most recent scheduled time and compare to last_run
        let last_run_ts = job.last_run.as_deref()
            .and_then(|s| s.parse::<i64>().ok()
                .or_else(|| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok()
                    .map(|dt| dt.and_utc().timestamp())))
            .unwrap_or(0);
        let last_run = chrono::DateTime::from_timestamp(last_run_ts, 0)
            .unwrap_or(chrono::DateTime::UNIX_EPOCH);

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
            "agent" => execute_agent(runner, hub, job).await,
            "workflow" => execute_workflow_task(workflow_manager, &job.command).await,
            other => (false, String::new(), Some(format!("unknown task type: {}", other))),
        };

        // Best-effort: update last_run timestamp (non-critical tracking)
        let _ = store.update_cron_job_last_run(job.id, err_msg.as_deref());

        // Best-effort: update history record (non-critical tracking)
        if let Some(h) = history {
            let _ = store.update_cron_history(
                h.id,
                success,
                if output.is_empty() { None } else { Some(&output) },
                err_msg.as_deref(),
            );
        }

        if success {
            info!(job = job.name.as_str(), "task completed");
            notify_crate::send("Nebo", &format!("{} completed", job.name));
        } else {
            let err = err_msg.as_deref().unwrap_or("unknown");
            warn!(job = job.name.as_str(), error = err, "task failed");
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
) -> (bool, String, Option<String>) {
    let prompt = job
        .message
        .as_deref()
        .unwrap_or(&job.command);

    let system = job.instructions.as_deref().unwrap_or("").to_string();

    let req = RunRequest {
        session_key: format!("cron-{}", job.name),
        prompt: prompt.to_string(),
        system,
        origin: Origin::System,
        channel: "cron".to_string(),
        ..Default::default()
    };

    match runner.run(req).await {
        Ok(mut rx) => {
            let mut full_text = String::new();
            while let Some(event) = rx.recv().await {
                match event.event_type {
                    ai::StreamEventType::Text => {
                        full_text.push_str(&event.text);
                        hub.broadcast(
                            "chat_stream",
                            serde_json::json!({
                                "session_id": format!("cron-{}", job.name),
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
            (true, full_text, None)
        }
        Err(e) => {
            error!(job = job.name.as_str(), error = %e, "agent run failed");
            (false, String::new(), Some(e.to_string()))
        }
    }
}

async fn execute_workflow_task(
    manager: &dyn tools::workflows::WorkflowManager,
    workflow_id: &str,
) -> (bool, String, Option<String>) {
    match manager.run(workflow_id, serde_json::Value::Null, "cron").await {
        Ok(run_id) => (true, format!("workflow run started: {}", run_id), None),
        Err(e) => (false, String::new(), Some(e)),
    }
}
