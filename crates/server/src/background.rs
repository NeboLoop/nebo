//! Background work: everything of the bot's that runs, waits or is set to
//! run without the owner watching it in a conversation. Helpers an employee
//! started, background commands, scheduled and messaged turns, workflow runs
//! (running or parked on an approval, an expert or a reply), timers and the
//! watches that start work when something happens.
//!
//! One list ([`collect`]) feeds the owner's "Running now", each chat's
//! background strip, and the short note every turn of an employee reads
//! about its own background work ([`note`]), so neither the owner nor the
//! model has to guess what is still going. A loop ([`spawn`]) tells the app
//! when the list changes and when an item ends, and records what ended for
//! the "finished" list.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde::Serialize;
use tracing::{info, warn};

use crate::state::AppState;

/// What a row is, for its icon: an employee or helper at work, a shell
/// command, a workflow run, a watch, a timer that fires again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Agent,
    Shell,
    Workflow,
    Monitor,
    Loop,
}

/// Where a row comes from: the registry its id belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Helper,
    Turn,
    Workflow,
    Timer,
    Watch,
    Heartbeat,
    Shell,
    /// Work one employee passed to another (`crate::handoff`).
    Handoff,
}

impl Source {
    fn prefix(self) -> &'static str {
        match self {
            Source::Helper => "helper",
            Source::Turn => "turn",
            Source::Workflow => "workflow",
            Source::Timer => "timer",
            Source::Watch => "watch",
            Source::Heartbeat => "heartbeat",
            Source::Shell => "shell",
            Source::Handoff => "handoff",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Working now.
    Running,
    /// Parked on someone: the owner's approval or answer, an expert, a reply.
    Waiting,
    /// A timer: nothing runs until it fires.
    Scheduled,
    /// A watch: nothing runs until what it watches happens.
    Watching,
    /// A watch that can't do its job (its account needs the owner).
    Degraded,
}

/// What the owner can do to a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// Stop work that is running.
    Stop,
    /// Cancel a run that is waiting.
    Cancel,
    /// Allow the call a waiting run is parked on, this once.
    Approve,
    /// Stop a timer firing; it is kept.
    Pause,
    /// Remove a timer.
    Delete,
    /// Turn a watch, or a workflow's schedule, off.
    Off,
}

/// One piece of background work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundTask {
    /// `<source>:<its own id>`, the id every action names.
    pub id: String,
    pub kind: Kind,
    pub source: Source,
    /// The employee it belongs to; empty for the main bot.
    pub agent_id: String,
    pub employee: String,
    /// What it is, one line.
    pub title: String,
    /// What it is doing or waiting for now, when that is known.
    pub detail: String,
    pub status: Status,
    /// What a waiting run waits for: `approval`, `answer`, `expert`, `reply`.
    pub wait: Option<String>,
    /// What starts a watch or timer: `event`, `watch`, `folder`,
    /// `heartbeat`, `schedule`.
    pub trigger: Option<String>,
    /// Unix seconds.
    pub started_at: Option<i64>,
    pub last_activity_at: Option<i64>,
    pub next_run_at: Option<i64>,
    /// Who set it going: owner, chat, workflow, schedule, system, watch,
    /// message, unknown.
    pub created_by: String,
    /// The run that set it going, when one did.
    pub source_run_id: Option<String>,
    /// The conversation it belongs to: where a helper or command reports,
    /// where a timer was made, a run's own session.
    pub session_key: Option<String>,
    /// For work one employee passed to another: who passed it, and from
    /// which conversation. It belongs to both employees' lists.
    pub from_agent_id: Option<String>,
    pub from_session_key: Option<String>,
    /// A workflow run's model turns used of its budget.
    pub turns_used: Option<u32>,
    pub turns_cap: Option<u32>,
    pub actions: Vec<Action>,
}

impl BackgroundTask {
    fn new(source: Source, native_id: &str, kind: Kind, status: Status) -> Self {
        Self {
            id: format!("{}:{native_id}", source.prefix()),
            kind,
            source,
            agent_id: String::new(),
            employee: String::new(),
            title: String::new(),
            detail: String::new(),
            status,
            wait: None,
            trigger: None,
            started_at: None,
            last_activity_at: None,
            next_run_at: None,
            created_by: "unknown".into(),
            source_run_id: None,
            session_key: None,
            from_agent_id: None,
            from_session_key: None,
            turns_used: None,
            turns_cap: None,
            actions: Vec::new(),
        }
    }

    /// Whether it belongs to employee `agent_id` ("" or "main": the main
    /// bot): its own, or passed on by it.
    pub fn belongs_to(&self, agent_id: &str) -> bool {
        let agent_id = employee_id(agent_id);
        self.agent_id == agent_id || self.from_agent_id.as_deref() == Some(agent_id)
    }
}

/// How it ended, for the finished list and the chat's notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Done,
    Failed,
    Stopped,
    /// A timer or watch that was switched off or deleted.
    Removed,
}

/// A piece of background work that ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FinishedTask {
    pub task: BackgroundTask,
    pub outcome: Outcome,
    pub ended_at: i64,
}

/// The main bot's employee id is empty here, whatever a registry calls it.
fn employee_id(agent_id: &str) -> &str {
    if agent_id == "main" { "" } else { agent_id }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

// ── the list ────────────────────────────────────────────────────────────

/// Every piece of background work on the bot, or employee `agent_id`'s
/// alone: running work first (oldest first), then timers and watches by
/// when they next fire.
pub async fn collect(state: &AppState, agent_id: Option<&str>) -> Vec<BackgroundTask> {
    let mut tasks = Vec::new();
    tasks.extend(helpers(state.helpers.list(None)));
    tasks.extend(shells(state.tools.process_registry().running_for(None).await));
    let asks: HashSet<String> = state.run_registry.pending_asks().await.into_iter().map(|(s, _)| s).collect();
    // A turn in a conversation a hand-off is worked in IS that hand-off:
    // listed once, as the hand-off.
    let handed = crate::handoff::receiver_sessions(state);
    let runs = state.run_registry.list_top_level().await.into_iter().filter(|r| !handed.contains(&r.session_key)).collect();
    tasks.extend(turns(runs, &asks, now()));
    tasks.extend(handoffs(crate::handoff::live(state)));
    match state.store.list_live_workflow_runs() {
        Ok(runs) => {
            for mut task in workflows(runs, |id| state.workflow_manager.turns_used(id)) {
                let native = task.id.trim_start_matches("workflow:");
                if task.status == Status::Waiting
                    && let Ok(Some(ask)) = state.permission_asks.for_run(native)
                    && ask.status == agent::harness::permissions::AskStatus::Open
                {
                    task.actions.insert(0, Action::Approve);
                }
                tasks.push(task);
            }
        }
        Err(e) => warn!(error = %e, "background: could not read workflow runs"),
    }
    tasks.extend(timers(&state.store));
    tasks.extend(watches(&state.store));
    match crate::heartbeat::enabled_entities(state).await {
        Ok(enabled) => tasks.extend(heartbeats(&state.store, &enabled)),
        Err(e) => warn!(error = %e, "background: could not read heartbeats"),
    }
    if let Some(agent_id) = agent_id {
        tasks.retain(|t| t.belongs_to(agent_id));
    }
    name_employees(state, &mut tasks).await;
    order(&mut tasks);
    tasks
}

/// Running and waiting work first, oldest first; then what fires next.
fn order(tasks: &mut [BackgroundTask]) {
    let rank = |t: &BackgroundTask| match t.status {
        Status::Running | Status::Waiting => 0,
        Status::Scheduled | Status::Watching | Status::Degraded => 1,
    };
    tasks.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then_with(|| match rank(a) {
                0 => a.started_at.unwrap_or(i64::MAX).cmp(&b.started_at.unwrap_or(i64::MAX)),
                _ => a.next_run_at.unwrap_or(i64::MAX).cmp(&b.next_run_at.unwrap_or(i64::MAX)),
            })
            .then_with(|| a.id.cmp(&b.id))
    });
}

async fn name_employees(state: &AppState, tasks: &mut [BackgroundTask]) {
    let registry = state.agent_registry.read().await;
    let mut names: HashMap<String, String> = HashMap::new();
    for task in tasks.iter_mut() {
        let name = names.entry(task.agent_id.clone()).or_insert_with(|| {
            if task.agent_id.is_empty() {
                return state.store.get_agent_profile().ok().flatten().map(|p| p.name).unwrap_or_else(|| "Nebo".into());
            }
            registry
                .get(&task.agent_id)
                .map(|a| a.name.clone())
                .or_else(|| state.store.get_agent(&task.agent_id).ok().flatten().map(|a| a.name))
                .unwrap_or_else(|| task.agent_id.clone())
        });
        task.employee = name.clone();
    }
}

/// Helpers at work.
pub(crate) fn helpers(list: Vec<agent::harness::delegation::HelperStatus>) -> Vec<BackgroundTask> {
    list.into_iter()
        .filter(|h| h.running)
        .map(|h| {
            let mut t = BackgroundTask::new(Source::Helper, &h.task_id, Kind::Agent, Status::Running);
            t.agent_id = employee_id(&h.agent_id).to_string();
            t.title = h.description;
            t.detail = h.activity;
            t.started_at = Some(h.started_at);
            t.created_by = "chat".into();
            t.session_key = Some(h.parent_key);
            t.actions = vec![Action::Stop];
            t
        })
        .collect()
}

/// Background commands still running.
pub(crate) fn shells(running: Vec<(std::sync::Arc<tools::process::BackgroundSession>, tools::process::Caller)>) -> Vec<BackgroundTask> {
    running
        .into_iter()
        .map(|(session, caller)| {
            let mut t = BackgroundTask::new(Source::Shell, &session.id, Kind::Shell, Status::Running);
            t.agent_id = employee_id(&types::keyparser::extract_agent_id(&caller.session_key)).to_string();
            t.title = if caller.description.is_empty() { session.command.clone() } else { caller.description.clone() };
            t.detail = session.command.clone();
            t.started_at = Some(session.started_at);
            t.created_by = "chat".into();
            t.session_key = Some(caller.session_key);
            t.actions = vec![Action::Stop];
            t
        })
        .collect()
}

/// Turns nobody started from a conversation they watch: a schedule's, a
/// heartbeat's, a message's from outside. The owner's own chat turns are
/// in front of him and are not listed.
pub(crate) fn turns(runs: Vec<crate::run_registry::RunSnapshot>, parked: &HashSet<String>, now: i64) -> Vec<BackgroundTask> {
    runs.into_iter()
        .filter(|r| r.origin != "user")
        .map(|r| {
            let waiting = parked.contains(&r.session_key);
            let mut t = BackgroundTask::new(
                Source::Turn,
                &r.run_id,
                Kind::Agent,
                if waiting { Status::Waiting } else { Status::Running },
            );
            t.agent_id = employee_id(&r.entity_id).to_string();
            t.title = r.entity_name.clone();
            t.detail = if r.activity.is_empty() { r.current_tool.clone() } else { r.activity.clone() };
            t.wait = waiting.then(|| "answer".to_string());
            t.started_at = Some(now - r.elapsed_secs as i64);
            t.last_activity_at = Some(now - r.idle_secs as i64);
            t.created_by = match r.origin.as_str() {
                "cron" => "schedule",
                "system" if r.session_key.starts_with("heartbeat") => "schedule",
                "comm" | "neboai" | "caller" => "message",
                "mcp" => "owner",
                _ => "system",
            }
            .to_string();
            t.session_key = Some(r.session_key);
            t.actions = vec![Action::Stop];
            t
        })
        .collect()
}

/// Work one employee passed to another, still going: the receiving
/// employee's row, passed on by the sender from its conversation. A
/// message hand-off can be stopped; an assignment is the assignee's own
/// case and closes from there.
pub(crate) fn handoffs(live: Vec<crate::handoff::HandoffView>) -> Vec<BackgroundTask> {
    live.into_iter()
        .map(|h| {
            let status = if h.status == "queued" { Status::Waiting } else { Status::Running };
            let mut t = BackgroundTask::new(Source::Handoff, &h.id, Kind::Agent, status);
            t.agent_id = employee_id(&h.to_agent_id).to_string();
            t.title = h.ask.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default().chars().take(200).collect();
            t.started_at = Some(h.started_at.unwrap_or(h.created_at));
            t.created_by = "message".into();
            t.source_run_id = h.sender_run_id.clone();
            t.session_key = Some(h.receiver_session.clone());
            t.from_agent_id = Some(employee_id(&h.from_agent_id).to_string());
            t.from_session_key = Some(h.sender_session.clone());
            t.actions = if h.kind == "message" { vec![Action::Stop] } else { Vec::new() };
            t
        })
        .collect()
}

/// Who started a workflow run, from how it was triggered.
fn run_starter(trigger_type: &str) -> &'static str {
    match trigger_type {
        "schedule" | "cron" | "heartbeat" => "schedule",
        "manual" => "owner",
        "agent" => "chat",
        "event" | "watch" | "folder" | "webhook" => "watch",
        "case" | "expert" | "approval" => "workflow",
        _ => "unknown",
    }
}

/// What a waiting run waits for, from its live wait's kind.
fn wait_of(wait_kind: Option<&str>) -> String {
    match wait_kind {
        Some("approval") => "approval",
        Some("answer") => "answer",
        Some("expert_reply") => "expert",
        Some("signal") => "reply",
        _ => "approval",
    }
    .to_string()
}

/// Workflow runs still going, running or parked. `turns` reads a running
/// run's turns used of its budget.
pub(crate) fn workflows(runs: Vec<db::models::LiveWorkflowRun>, turns: impl Fn(&str) -> Option<(u32, u32)>) -> Vec<BackgroundTask> {
    runs.into_iter()
        .map(|live| {
            let run = live.run;
            let waiting = live.state == "waiting";
            let mut t = BackgroundTask::new(
                Source::Workflow,
                &run.id,
                Kind::Workflow,
                if waiting { Status::Waiting } else { Status::Running },
            );
            let agent_id = types::keyparser::agent_id_from_workflow_id(&run.workflow_id);
            t.agent_id = agent_id.unwrap_or_default().to_string();
            let binding = run.trigger_detail.as_deref().and_then(|d| d.split(':').next()).filter(|b| !b.is_empty());
            t.title = match (agent_id, binding) {
                (Some(_), Some(binding)) => binding.to_string(),
                _ => run.workflow_id.clone(),
            };
            t.detail = if waiting {
                live.wait_reason.clone().unwrap_or_default()
            } else {
                run.current_activity.clone().unwrap_or_default()
            };
            t.wait = waiting.then(|| wait_of(live.wait_kind.as_deref()));
            t.started_at = Some(run.started_at);
            t.last_activity_at = live.last_activity;
            t.next_run_at = if waiting { live.wait_deadline } else { None };
            t.created_by = run_starter(&run.trigger_type).to_string();
            t.session_key = run.session_key.clone();
            if let Some((used, cap)) = turns(&run.id) {
                t.turns_used = Some(used);
                t.turns_cap = Some(cap);
            }
            t.actions = vec![if waiting { Action::Cancel } else { Action::Stop }];
            t
        })
        .collect()
}

/// The unix moment a schedule row's `created_at` names (UTC).
fn row_time(created_at: Option<&str>) -> Option<i64> {
    chrono::NaiveDateTime::parse_from_str(created_at?, "%Y-%m-%d %H:%M:%S").ok().map(|t| t.and_utc().timestamp())
}

/// The employee a schedule fires for: its own, or the one a workflow
/// binding's command names.
fn job_employee(job: &db::models::CronJob) -> String {
    let own = job.agent_id.as_deref().unwrap_or_default();
    if !own.is_empty() {
        return employee_id(own).to_string();
    }
    match job.command.splitn(3, ':').collect::<Vec<_>>()[..] {
        ["agent", agent_id, _] if matches!(job.task_type.as_str(), "agent_workflow" | "role_workflow") => agent_id.to_string(),
        _ => String::new(),
    }
}

/// Every enabled schedule, with when it fires next.
pub(crate) fn timers(store: &db::Store) -> Vec<BackgroundTask> {
    let jobs = match store.list_enabled_cron_jobs() {
        Ok(jobs) => jobs,
        Err(e) => {
            warn!(error = %e, "background: could not read schedules");
            return Vec::new();
        }
    };
    let next = store.cron_next_fires().unwrap_or_default();
    jobs.into_iter()
        .map(|job| {
            let mut t = BackgroundTask::new(Source::Timer, &job.id.to_string(), Kind::Loop, Status::Scheduled);
            t.agent_id = job_employee(&job);
            t.title = job.name.clone();
            t.detail = job.reason.clone();
            t.trigger = Some("schedule".into());
            t.started_at = row_time(job.created_at.as_deref());
            t.next_run_at = next.get(&job.id).copied();
            t.created_by = job.created_by.clone();
            t.source_run_id = job.created_by_run.clone();
            t.session_key = job.created_in.clone();
            t.actions = match job.task_type.as_str() {
                // A workflow's schedule is the workflow's: it is turned off
                // with it, never paused or deleted under it.
                "agent_workflow" | "role_workflow" => vec![Action::Off],
                _ => vec![Action::Pause, Action::Delete],
            };
            t
        })
        .collect()
}

/// Workflow bindings that wait for something to happen.
pub(crate) fn watches(store: &db::Store) -> Vec<BackgroundTask> {
    let bindings = match store.list_active_watch_bindings() {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, "background: could not read watches");
            return Vec::new();
        }
    };
    let next: HashMap<String, i64> = store
        .engine_pending_timers("binding")
        .unwrap_or_default()
        .into_iter()
        .filter_map(|t| Some((t.target_id, t.due_at?)))
        .collect();
    bindings
        .into_iter()
        .map(|(b, degraded)| {
            let native = format!("{}:{}", b.agent_id, b.binding_name);
            let status = if degraded.is_some() { Status::Degraded } else { Status::Watching };
            let mut t = BackgroundTask::new(Source::Watch, &native, Kind::Monitor, status);
            t.agent_id = b.agent_id.clone();
            t.title = b.description.clone().filter(|d| !d.trim().is_empty()).unwrap_or_else(|| b.binding_name.clone());
            t.detail = degraded.unwrap_or_default();
            t.trigger = Some(b.trigger_type.clone());
            t.next_run_at = next.get(&format!("hb:{native}")).copied();
            t.created_by = "workflow".into();
            t.actions = vec![Action::Off];
            t
        })
        .collect()
}

/// Entities whose heartbeat is on: the main bot, employees, channels.
pub(crate) fn heartbeats(store: &db::Store, enabled: &[crate::heartbeat::Enabled]) -> Vec<BackgroundTask> {
    let next: HashMap<String, i64> = store
        .engine_pending_timers("entity")
        .unwrap_or_default()
        .into_iter()
        .filter_map(|t| Some((t.target_id, t.due_at?)))
        .collect();
    enabled
        .iter()
        .map(|e| {
            let native = format!("{}:{}", e.entity_type, e.entity_id);
            let mut t = BackgroundTask::new(Source::Heartbeat, &native, Kind::Loop, Status::Scheduled);
            t.agent_id = if e.entity_type == "agent" { e.entity_id.clone() } else { String::new() };
            t.title = if e.entity_type == "channel" { e.entity_id.clone() } else { String::new() };
            t.trigger = Some("heartbeat".into());
            t.next_run_at = next.get(&e.target()).copied();
            t.created_by = "owner".into();
            t.actions = vec![Action::Off];
            t
        })
        .collect()
}

// ── actions ─────────────────────────────────────────────────────────────

/// Why an action could not be taken.
#[derive(Debug, PartialEq, Eq)]
pub enum ActError {
    /// No such work, or it already ended.
    NotFound,
    /// The work can't be acted on that way.
    NotAllowed,
    Failed(String),
}

/// Take `action` on the background work `id` (as [`collect`] named it).
pub async fn act(state: &AppState, id: &str, action: &str) -> Result<(), ActError> {
    let task = collect(state, None).await.into_iter().find(|t| t.id == id).ok_or(ActError::NotFound)?;
    let action = task
        .actions
        .iter()
        .copied()
        .find(|a| serde_json::to_value(a).ok().and_then(|v| v.as_str().map(str::to_string)).as_deref() == Some(action))
        .ok_or(ActError::NotAllowed)?;
    let native = id.split_once(':').map(|(_, n)| n).unwrap_or_default();
    let failed = |e: String| ActError::Failed(e);
    match (task.source, action) {
        (Source::Helper, Action::Stop) => {
            let parent = task.session_key.as_deref().unwrap_or_default();
            state.helpers.stop(parent, native).map(|_| ()).map_err(failed)
        }
        (Source::Shell, Action::Stop) => state.tools.process_registry().kill_session(native).await.map_err(failed),
        (Source::Turn, Action::Stop) => {
            if let Some(session) = task.session_key.as_deref() {
                state.helpers.stop_session(Some(session));
            }
            state.run_registry.cancel(native).await.then_some(()).ok_or(ActError::NotFound)
        }
        (Source::Handoff, Action::Stop) => crate::handoff::stop(state, native).await.map_err(failed),
        (Source::Workflow, Action::Stop | Action::Cancel) => crate::handlers::workflows::cancel_workflow_run(state, native).await.map_err(failed),
        (Source::Workflow, Action::Approve) => {
            use agent::harness::permissions::{AnsweredVia, Answer};
            let ask = state.permission_asks.for_run(native).map_err(|e| failed(e.to_string()))?.ok_or(ActError::NotFound)?;
            state.permission_asks.answer(&ask.id, Answer::ThisOnce, AnsweredVia::Inbox).map(|_| ()).map_err(|e| failed(e.to_string()))
        }
        (Source::Timer, Action::Pause) => state.store.set_cron_job_enabled(timer_id(native)?, false).map_err(|e| failed(e.to_string())),
        (Source::Timer, Action::Delete) => state.store.delete_cron_job(timer_id(native)?).map_err(|e| failed(e.to_string())),
        (Source::Timer, Action::Off) => {
            let job = state.store.get_cron_job(timer_id(native)?).map_err(|e| failed(e.to_string()))?.ok_or(ActError::NotFound)?;
            match job.command.splitn(3, ':').collect::<Vec<_>>()[..] {
                ["agent", agent_id, binding] => crate::handlers::agents::switch_binding_off(state, agent_id, binding).await.map_err(|e| failed(e.to_string())),
                _ => Err(ActError::NotAllowed),
            }
        }
        (Source::Watch, Action::Off) => {
            let (agent_id, binding) = native.split_once(':').ok_or(ActError::NotFound)?;
            crate::handlers::agents::switch_binding_off(state, agent_id, binding).await.map_err(|e| failed(e.to_string()))
        }
        (Source::Heartbeat, Action::Off) => {
            let (entity_type, entity_id) = native.split_once(':').ok_or(ActError::NotFound)?;
            state
                .store
                .upsert_entity_config(entity_type, entity_id, &serde_json::json!({ "heartbeatEnabled": false }))
                .map(|_| ())
                .map_err(|e| failed(e.to_string()))
        }
        _ => Err(ActError::NotAllowed),
    }?;
    info!(id, action = ?action, "background work: owner action taken");
    changed();
    Ok(())
}

fn timer_id(native: &str) -> Result<i64, ActError> {
    native.parse().map_err(|_| ActError::NotFound)
}

/// Stop everything that runs: every helper of every session, every turn on
/// the bot (scheduled, heartbeat and messaged ones included), every
/// workflow run, and every background command. Timers and watches stay;
/// they start nothing until they fire. Every client hears it
/// (`chat_cancelled` for "all"). Returns how many were stopped.
pub async fn stop_everything(state: &AppState) -> usize {
    let mut count = crate::handlers::ws::apply_cancel_all(&state.helpers, &state.run_registry).await
        + tools::workflows::WorkflowManager::cancel_all_runs(&*state.workflow_manager).await;
    let processes = state.tools.process_registry();
    for (session, _) in processes.running_for(None).await {
        if processes.kill_session(&session.id).await.is_ok() {
            count += 1;
        }
    }
    info!(count, "stop everything: background work stopped");
    state.hub.broadcast("chat_cancelled", serde_json::json!({ "session_id": "all" }));
    changed();
    count
}

// ── output ──────────────────────────────────────────────────────────────

/// Most of an item's output shown at once: its end.
const OUTPUT_TAIL_CHARS: usize = 4000;

/// The end of what a piece of work has produced so far.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputTail {
    pub output: String,
    /// It had more before what is shown.
    pub truncated: bool,
}

fn tail(text: &str) -> OutputTail {
    let chars = text.chars().count();
    OutputTail {
        output: text.chars().skip(chars.saturating_sub(OUTPUT_TAIL_CHARS)).collect(),
        truncated: chars > OUTPUT_TAIL_CHARS,
    }
}

/// The last thing said in conversation `session_key`: its last reply or
/// tool result.
fn last_words(store: &db::Store, session_key: &str) -> String {
    let Ok(Some(session)) = store.get_session_by_name(session_key) else {
        return String::new();
    };
    let chat_id = store.resolve_session_chat_id(&session.id);
    store
        .get_chat_messages(&chat_id)
        .unwrap_or_default()
        .into_iter()
        .rev()
        .find(|m| matches!(m.role.as_str(), "assistant" | "tool") && !m.content.trim().is_empty())
        .map(|m| m.content)
        .unwrap_or_default()
}

/// The end of the output of background work `id`, running or finished.
pub async fn output(state: &AppState, id: &str) -> Result<OutputTail, ActError> {
    let (source, native) = id.split_once(':').ok_or(ActError::NotFound)?;
    let text = match source {
        "shell" => state.tools.process_registry().get_any_session(native).await.ok_or(ActError::NotFound)?.get_output().await,
        "helper" => {
            let row = state.store.get_pending_task(native).map_err(|e| ActError::Failed(e.to_string()))?.ok_or(ActError::NotFound)?;
            row.output.filter(|o| !o.trim().is_empty()).unwrap_or_else(|| last_words(&state.store, &row.session_key))
        }
        "workflow" => {
            let run = state.store.get_workflow_run(native).map_err(|e| ActError::Failed(e.to_string()))?.ok_or(ActError::NotFound)?;
            run.output
                .or(run.error)
                .filter(|o| !o.trim().is_empty())
                .unwrap_or_else(|| run.session_key.as_deref().map(|k| last_words(&state.store, k)).unwrap_or_default())
        }
        "turn" => {
            let session = finished_or_live(state, id).await.and_then(|t| t.session_key).ok_or(ActError::NotFound)?;
            last_words(&state.store, &session)
        }
        "timer" => {
            let job_id = timer_id(native)?;
            let last = state.store.list_cron_history(job_id, 1, 0).map_err(|e| ActError::Failed(e.to_string()))?;
            last.into_iter().next().map(|h| h.output.or(h.error).unwrap_or_default()).unwrap_or_default()
        }
        "watch" | "heartbeat" => String::new(),
        _ => return Err(ActError::NotFound),
    };
    Ok(tail(&text))
}

/// Work `id` as the list or the finished list last showed it.
async fn finished_or_live(state: &AppState, id: &str) -> Option<BackgroundTask> {
    if let Some(t) = collect(state, None).await.into_iter().find(|t| t.id == id) {
        return Some(t);
    }
    finished(None).into_iter().find(|f| f.task.id == id).map(|f| f.task)
}

// ── what ended, and telling the app ─────────────────────────────────────

/// How many ended pieces of work are kept for the finished list.
const FINISHED_KEPT: usize = 50;

static FINISHED: LazyLock<Mutex<VecDeque<FinishedTask>>> = LazyLock::new(Default::default);

/// Woken by anything that changes the list, so the app hears it at once
/// rather than at the next look.
static CHANGED: LazyLock<tokio::sync::Notify> = LazyLock::new(tokio::sync::Notify::new);

/// Background work changed (started, stopped, switched): look again now.
pub fn changed() {
    CHANGED.notify_one();
}

/// The work that ended lately, newest first; employee `agent_id`'s alone
/// when given.
pub fn finished(agent_id: Option<&str>) -> Vec<FinishedTask> {
    FINISHED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .rev()
        .filter(|f| agent_id.is_none_or(|a| f.task.belongs_to(a)))
        .cloned()
        .collect()
}

fn remember_finished(ended: FinishedTask) {
    let mut kept = FINISHED.lock().unwrap_or_else(|e| e.into_inner());
    kept.push_back(ended);
    while kept.len() > FINISHED_KEPT {
        kept.pop_front();
    }
}

/// How a piece of work that left the list ended, read from the record it
/// left behind.
async fn outcome_of(state: &AppState, task: &BackgroundTask) -> Outcome {
    let native = task.id.split_once(':').map(|(_, n)| n).unwrap_or_default();
    match task.source {
        Source::Helper => match state.store.get_pending_task(native).ok().flatten().map(|r| r.status) {
            Some(s) if s == "failed" => Outcome::Failed,
            Some(s) if s == "cancelled" => Outcome::Stopped,
            _ => Outcome::Done,
        },
        Source::Workflow => match state.store.get_workflow_run(native).ok().flatten().map(|r| r.status) {
            Some(s) if matches!(s.as_str(), "failed" | "denied") => Outcome::Failed,
            Some(s) if s == "cancelled" => Outcome::Stopped,
            _ => Outcome::Done,
        },
        Source::Shell => match state.tools.process_registry().get_any_session(native).await {
            Some(s) if s.exit_code == Some(0) => Outcome::Done,
            Some(s) if s.exit_code.is_none() => Outcome::Stopped,
            Some(_) => Outcome::Failed,
            None => Outcome::Done,
        },
        Source::Turn => Outcome::Done,
        Source::Handoff => match state.store.get_handoff(native).ok().flatten().map(|h| h.status) {
            Some(s) if s == "failed" => Outcome::Failed,
            Some(s) if s == "stopped" => Outcome::Stopped,
            _ => Outcome::Done,
        },
        Source::Timer => match timer_id(native).ok().and_then(|id| state.store.get_cron_job(id).ok().flatten()) {
            // A one-shot that fired is done; anything else that left was
            // switched off or deleted.
            Some(job) if db::is_one_shot(&job.schedule) && job.run_count.unwrap_or(0) > 0 => Outcome::Done,
            _ => Outcome::Removed,
        },
        Source::Watch | Source::Heartbeat => Outcome::Removed,
    }
}

/// What changed between two looks: the work that left (with how it ended)
/// and the timers that appeared.
pub(crate) fn diff<'a>(before: &'a [BackgroundTask], after: &'a [BackgroundTask]) -> (Vec<&'a BackgroundTask>, Vec<&'a BackgroundTask>) {
    let ids_after: HashSet<&str> = after.iter().map(|t| t.id.as_str()).collect();
    let ids_before: HashSet<&str> = before.iter().map(|t| t.id.as_str()).collect();
    let gone = before.iter().filter(|t| !ids_after.contains(t.id.as_str())).collect();
    let new_timers = after
        .iter()
        .filter(|t| t.source == Source::Timer && !ids_before.contains(t.id.as_str()))
        .collect();
    (gone, new_timers)
}

/// How often the list is looked at when nothing wakes the loop.
const LOOK_EVERY: Duration = Duration::from_secs(3);

/// Watch the list: tell the app when it changes (`background_changed`, the
/// whole list), when a piece of work ends (`background_finished`, how it
/// ended) and when a timer is made (`background_timer_created`). A workflow
/// run that ends retires the schedules that were only there for it.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut before = collect(&state, None).await;
        loop {
            tokio::select! {
                _ = CHANGED.notified() => {}
                _ = tokio::time::sleep(LOOK_EVERY) => {}
            }
            let after = collect(&state, None).await;
            if after == before {
                continue;
            }
            let (gone, new_timers) = diff(&before, &after);
            let mut workflow_ended = false;
            for task in gone {
                let outcome = outcome_of(&state, task).await;
                workflow_ended |= task.source == Source::Workflow;
                let ended = FinishedTask { task: task.clone(), outcome, ended_at: now() };
                state.hub.broadcast("background_finished", serde_json::json!(ended));
                remember_finished(ended);
            }
            for task in new_timers {
                state.hub.broadcast("background_timer_created", serde_json::json!({ "task": task }));
            }
            state.hub.broadcast("background_changed", serde_json::json!({ "tasks": after }));
            before = after;
            if workflow_ended {
                let retired = crate::engine::retire_finished_schedules(&state.store, now());
                if retired > 0 {
                    changed();
                }
            }
        }
    });
}

/// A chat turn ended. Stopped by the owner, the schedules it made go with
/// it, and the app hears they did.
pub(crate) fn turn_ended(state: &AppState, run: &crate::run_registry::RunHandle, cancel: &tokio_util::sync::CancellationToken) {
    if !cancel.is_cancelled() || run.stalled.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    match state.store.retire_schedules_made_by(&run.run_id) {
        Ok(jobs) if !jobs.is_empty() => {
            for job in &jobs {
                info!(job = job.name.as_str(), run = %run.run_id, "the turn that made this schedule was stopped; schedule retired");
            }
            changed();
        }
        Ok(_) => {}
        Err(e) => warn!(run = %run.run_id, error = %e, "could not retire the schedules a stopped turn made"),
    }
}

// ── the model's note ────────────────────────────────────────────────────

/// Most lines the note lists; the rest are counted.
const NOTE_LINES: usize = 8;

/// The note one turn of employee `agent_id` reads about its own background
/// work: what still runs, waits or is set to fire. Empty when nothing is.
/// The turn itself (`session_key`'s own run) is left out. Times are on the
/// owner's clock, absolute, so the note reads the same until something
/// changes.
pub fn note(tasks: &[BackgroundTask], session_key: &str, zone: tools::owner_clock::OwnerZone) -> String {
    let at = |ts: i64| {
        chrono::DateTime::from_timestamp(ts, 0)
            .map(|t| t.with_timezone(&zone).format("%a %b %-d %H:%M").to_string())
            .unwrap_or_default()
    };
    let lines: Vec<String> = tasks
        .iter()
        .filter(|t| !(matches!(t.source, Source::Turn | Source::Handoff) && t.session_key.as_deref() == Some(session_key)))
        .filter(|t| !matches!(t.source, Source::Watch | Source::Heartbeat))
        .map(|t| {
            let here = t.session_key.as_deref() == Some(session_key);
            let id = t.id.split_once(':').map(|(_, n)| n).unwrap_or(&t.id);
            match t.source {
                Source::Helper => format!(
                    "- helper {id} \"{}\" is still running (since {}{}). Its result comes to you as a notification; don't start another for the same task, and don't report its work as done.",
                    t.title,
                    t.started_at.map(at).unwrap_or_default(),
                    if here { "" } else { ", from another conversation" },
                ),
                Source::Shell => format!(
                    "- background command {id} \"{}\" is still running (since {}). Don't start it again; stop it first to restart it.",
                    t.title,
                    t.started_at.map(at).unwrap_or_default(),
                ),
                Source::Turn => format!("- a {} turn is running (since {}).", t.created_by, t.started_at.map(at).unwrap_or_default()),
                Source::Handoff => format!(
                    "- {} is working on \"{}\", passed on (since {}). Its answer comes as a notification to the conversation that passed it; don't pass it again or do it yourself.",
                    t.employee,
                    t.title,
                    t.started_at.map(at).unwrap_or_default(),
                ),
                Source::Workflow if t.status == Status::Waiting => format!(
                    "- workflow run {id} \"{}\" is waiting for {}.",
                    t.title,
                    match t.wait.as_deref() {
                        Some("expert") => "an expert's reply",
                        Some("reply") => "a reply",
                        Some("answer") => "the owner's answer",
                        _ => "the owner's approval",
                    }
                ),
                Source::Workflow => format!(
                    "- workflow run {id} \"{}\" is running (since {}{}). Don't start it again.",
                    t.title,
                    t.started_at.map(at).unwrap_or_default(),
                    match (t.turns_used, t.turns_cap) {
                        (Some(u), Some(c)) => format!(", {u} of {c} turns used"),
                        _ => String::new(),
                    }
                ),
                Source::Timer => format!(
                    "- schedule \"{}\"{} fires next {}. Don't make another for the same thing.",
                    t.title,
                    if t.detail.is_empty() { String::new() } else { format!(" ({})", t.detail) },
                    t.next_run_at.map(at).unwrap_or_else(|| "soon".into()),
                ),
                Source::Watch | Source::Heartbeat => String::new(),
            }
        })
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    let more = lines.len().saturating_sub(NOTE_LINES);
    let mut text = String::from("Your background work right now:\n");
    text.push_str(&lines.into_iter().take(NOTE_LINES).collect::<Vec<_>>().join("\n"));
    if more > 0 {
        text.push_str(&format!("\n- and {more} more (list_schedules, read_output)."));
    }
    text
}

/// The note for one turn of employee `agent_id` in `session_key`.
pub async fn note_for(state: &AppState, agent_id: &str, session_key: &str) -> String {
    let tasks = collect(state, Some(agent_id)).await;
    note(&tasks, session_key, tools::owner_clock::OwnerZone::of(&state.store))
}

#[cfg(test)]
mod tests;
