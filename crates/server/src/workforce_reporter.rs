//! The workforce reporter — this bot telling the platform what it must do and
//! what it just did (accountability plan W2, bot half).
//!
//! The console used to learn about this bot's work by asking over the tunnel
//! when an owner happened to look; detection that waits for someone to look is
//! not detection. This pushes instead: an outbox drain over workflow_runs
//! (rows are marked reported only after the platform acks, and ingest dedups
//! on (bot, run_id), so a crash between send and mark re-sends harmlessly),
//! plus the duty roster whenever it changes, so the platform's overdue sweeper
//! knows each duty's own cadence.
//!
//! Reuses the one NeboAI client (build_api_client — Rule 8.1: no second HTTP
//! client), the one cron normalizer (PersonaTool::normalize_cron), the cron
//! crate already routing schedules, and timeutil's heartbeat-convention
//! duration parser. Nothing here re-implements an existing pathway.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tracing::{debug, warn};

use crate::AppState;

/// Outbox drain cadence. "Instant or close to it": a failed run reaches the
/// platform within this plus network time. Cheap — one indexed local read
/// when there is nothing to send.
const DRAIN_EVERY: Duration = Duration::from_secs(15);
/// When the bot is not connected to NeboAI there is nobody to report to;
/// probe slowly instead of hammering a client that cannot be built.
const OFFLINE_RETRY: Duration = Duration::from_secs(120);
/// Re-send the roster unchanged at this interval as a liveness floor, so a
/// missed change-detection can never go stale forever.
const ROSTER_REFRESH_CYCLES: u32 = 40; // × DRAIN_EVERY ≈ 10 minutes
const BATCH: i64 = 100;

pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut last_roster_hash: u64 = 0;
        let mut cycles_since_roster: u32 = ROSTER_REFRESH_CYCLES; // send on first pass
        loop {
            let api = match crate::codes::build_api_client(&state) {
                Ok(api) => api,
                Err(_) => {
                    tokio::time::sleep(OFFLINE_RETRY).await;
                    continue;
                }
            };

            // ── Duties: send when changed, and periodically as a floor ──
            cycles_since_roster += 1;
            let duties = collect_duties(&state);
            let hash = hash_duties(&duties);
            let send_roster =
                hash != last_roster_hash || cycles_since_roster >= ROSTER_REFRESH_CYCLES;

            // ── Runs: the outbox ──
            let runs = state.store.list_unreported_runs(BATCH).unwrap_or_default();
            let run_reports: Vec<serde_json::Value> =
                runs.iter().map(|r| run_report(&state, r)).collect();

            if send_roster || !run_reports.is_empty() {
                let mut body = json!({ "runs": run_reports });
                if send_roster {
                    body["duties"] = json!(duties);
                }
                // The review queue: runs parked waiting on a human. Sent with
                // every report (small, replace-on-arrival) so the console's
                // "waiting on you" is never stale by more than a drain.
                body["suspensions"] = json!(collect_suspensions(&state));
                match api.report_workforce(&body).await {
                    Ok(()) => {
                        if send_roster {
                            last_roster_hash = hash;
                            cycles_since_roster = 0;
                        }
                        if !runs.is_empty() {
                            let ids: Vec<String> = runs.iter().map(|r| r.id.clone()).collect();
                            if let Err(e) = state.store.mark_runs_reported(&ids) {
                                // The platform has the batch (it dedups); only
                                // the local cursor failed to advance.
                                warn!(error = %e, "workforce reporter: mark reported");
                            }
                            debug!(count = runs.len(), "workforce runs reported");
                        }
                    }
                    Err(e) => {
                        // Unacked: everything stays in the outbox for the next
                        // drain. Silence here is what the platform's own
                        // last-report state exists to notice.
                        debug!(error = %e, "workforce report failed; will retry");
                    }
                }
            }

            tokio::time::sleep(DRAIN_EVERY).await;
        }
    });
}

/// The duty roster: every active workflow binding of every agent, each with
/// its own cadence — the platform's overdue sweeper measures silence against
/// this, never against an assumed "daily".
fn collect_duties(state: &AppState) -> Vec<serde_json::Value> {
    let agents = state.store.list_agents(1000, 0).unwrap_or_default();
    let mut out = Vec::new();
    for agent in &agents {
        let bindings = state
            .store
            .list_agent_workflows(&agent.id)
            .unwrap_or_default();
        for b in bindings {
            let (trigger_type, period_hours) = match b.trigger_type.as_str() {
                "schedule" => ("schedule", cron_period_hours(&b.trigger_config)),
                // Heartbeats are schedules by another name: "30m|9-17" fires
                // on an interval, so silence past it is as much a finding.
                "heartbeat" => ("schedule", heartbeat_period_hours(&b.trigger_config)),
                "watch" => ("watch", 0.0),
                _ => continue, // manual/event bindings have no cadence to owe
            };
            out.push(json!({
                "agentId": agent.id,
                "agentName": agent.name,
                "workflowName": b.binding_name,
                // The owner-facing name and the sentence saying what the duty
                // does. binding_name is developer vocabulary ("chase") and
                // must never be the word a customer reads.
                "displayName": humanize(&b.binding_name),
                "description": b.description.clone().unwrap_or_default(),
                "triggerType": trigger_type,
                "cron": if b.trigger_type == "schedule" { b.trigger_config.clone() } else { String::new() },
                "periodHours": period_hours,
                "active": b.is_active != 0,
            }));
        }
    }
    out
}

/// A schedule's period, measured from the cron expression itself: normalize
/// with the ONE normalizer the scheduler uses, then diff the next two fire
/// times the cron crate reports. No re-implemented field heuristics — the
/// same library that decides when the duty runs decides how often it runs.
fn cron_period_hours(expr: &str) -> f64 {
    let normalized = tools::PersonaTool::normalize_cron(expr);
    let schedule: cron::Schedule = match normalized.parse() {
        Ok(s) => s,
        Err(_) => return 24.0, // unparseable: assume daily rather than exempt
    };
    let mut it = schedule.upcoming(chrono::Utc);
    match (it.next(), it.next()) {
        (Some(a), Some(b)) => ((b - a).num_seconds() as f64 / 3600.0).max(0.01),
        _ => 24.0,
    }
}

/// Heartbeat config is "interval" or "interval|window" (timeutil convention).
fn heartbeat_period_hours(config: &str) -> f64 {
    let interval = config.split('|').next().unwrap_or("");
    let d = types::timeutil::parse_duration(interval);
    if d.is_zero() {
        24.0
    } else {
        (d.as_secs_f64() / 3600.0).max(0.01)
    }
}

/// One run receipt. The run's agent comes from its workflow id (the
/// "agent:{id}" convention list_agent_runs already relies on); cost and
/// outcome join from run_usage when the receipt exists.
fn run_report(state: &AppState, run: &db::models::WorkflowRun) -> serde_json::Value {
    let agent_id = types::keyparser::agent_id_from_workflow_id(&run.workflow_id)
        .unwrap_or("")
        .to_string();
    let agent_name = if agent_id.is_empty() {
        String::new()
    } else {
        state
            .store
            .get_agent(&agent_id)
            .ok()
            .flatten()
            .map(|a| a.name)
            .unwrap_or_default()
    };
    let usage = state.store.usage_for_run(&run.id).ok().flatten();
    json!({
        "agentId": agent_id,
        "agentName": agent_name,
        "runId": run.id,
        "workflowName": run.trigger_detail.clone().unwrap_or_default(),
        "status": run.status,
        "outcome": usage.as_ref().and_then(|u| u.outcome.clone()),
        "error": run.error,
        "startedAt": chrono::DateTime::from_timestamp(run.started_at, 0),
        "finishedAt": run.completed_at.and_then(|t| chrono::DateTime::from_timestamp(t, 0)),
        "costMicrocents": usage.map(|u| u.cost_microcents).unwrap_or(0),
    })
}

fn hash_duties(duties: &[serde_json::Value]) -> u64 {
    let mut h = DefaultHasher::new();
    for d in duties {
        d.to_string().hash(&mut h);
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The period feeds the platform's overdue window; a wrong period is a
    // false alarm or a missed outage, so the derivation gets a check.
    #[test]
    fn cron_period_matches_the_schedule_itself() {
        let daily = cron_period_hours("0 7 * * *");
        assert!((23.0..=25.0).contains(&daily), "daily = {daily}");
        let hourly = cron_period_hours("0 * * * *");
        assert!((0.9..=1.1).contains(&hourly), "hourly = {hourly}");
        let weekly = cron_period_hours("0 9 * * 1");
        assert!((167.0..=169.0).contains(&weekly), "weekly = {weekly}");
        // Unparseable falls back to daily — surveilled, never exempt.
        assert_eq!(cron_period_hours("not a cron"), 24.0);
    }

    #[test]
    fn heartbeat_period_reads_the_interval() {
        assert!((0.49..=0.51).contains(&heartbeat_period_hours("30m|9-17")));
        assert!((1.9..=2.1).contains(&heartbeat_period_hours("2h")));
        assert_eq!(heartbeat_period_hours(""), 24.0);
    }
}

/// Open suspensions — the §07 review queue's raw material. Each is a run
/// parked on a question only the owner can answer; the bot must NEVER
/// self-answer one, which is why they are reported rather than healed.
fn collect_suspensions(state: &AppState) -> Vec<serde_json::Value> {
    let rows = state.store.list_workflow_suspensions().unwrap_or_default();
    rows.into_iter()
        .map(|(run_id, agent_id, binding_name, display, created_at)| {
            let agent_name = state
                .store
                .get_agent(&agent_id)
                .ok()
                .flatten()
                .map(|a| a.name)
                .unwrap_or_default();
            json!({
                "runId": run_id,
                "agentId": agent_id,
                "agentName": agent_name,
                "workflowName": binding_name,
                "displayName": humanize(&binding_name),
                "question": display,
                "since": chrono::DateTime::from_timestamp(created_at, 0),
            })
        })
        .collect()
}

/// humanize turns a developer binding name into words an owner reads:
/// "chase-overdue_invoices" → "Chase overdue invoices". Deliberately dumb —
/// a wrong-but-readable name beats a correct slug.
fn humanize(binding: &str) -> String {
    let words = binding.replace(['-', '_'], " ");
    let trimmed = words.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut c = trimmed.chars();
    let first = match c.next() {
        Some(ch) => ch.to_uppercase().collect::<String>(),
        None => return String::new(),
    };
    first + c.as_str()
}

#[cfg(test)]
mod humanize_tests {
    use super::humanize;

    // This string is what a customer reads on their NOC and in their daily
    // report — the whole point is that developer slugs never surface.
    #[test]
    fn humanize_makes_owner_words() {
        assert_eq!(humanize("chase-overdue_invoices"), "Chase overdue invoices");
        assert_eq!(humanize("inbox"), "Inbox");
        assert_eq!(humanize(""), "");
        assert_eq!(humanize("_-_"), "");
    }
}
