//! Housekeeping loop and the ONE way a scheduled job executes. Firing —
//! when a job is due, that it fires once, that a fire interrupted by a
//! restart is retried once — is the engine's (`crate::engine`); this module
//! runs the job the engine hands it and returns the outcome.

use std::sync::Arc;
use std::time::Duration;

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
        let booted = chrono::Utc::now().timestamp();
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

        // Checklist items whose run is no longer live (ended, or gone) are
        // closed: left by builds that did not close a run's lists at its
        // end, or by a process that died first. Status only; rows stay.
        match state.store.close_orphaned_task_items() {
            Ok(0) => {}
            Ok(n) => info!(count = n, "checklist items left open by ended runs closed"),
            Err(e) => warn!(error = %e, "checklist items left open by ended runs not swept"),
        }

        // Session wakes persisted but not delivered before a crash (session
        // wake rail, R1): redeliver on boot.
        crate::wake::recover_pending_wakes(&state).await;

        // Pointers to schedules' answers the last process left waiting:
        // sent once the answer is there; a request no turn answered is
        // answered now.
        follow_up_after_restart(&state, booted).await;

        // Hand-offs between employees whose runs died with the last process
        // are ended as stopped, so none shows as running forever.
        match state.store.stop_orphaned_handoffs(booted, chrono::Utc::now().timestamp()) {
            Ok(0) => {}
            Ok(n) => info!(count = n, "hand-offs left running by the last process recorded stopped"),
            Err(e) => warn!(error = %e, "hand-offs left running by the last process not swept"),
        }

        // Code work being verified when the last process ended can't finish
        // on its own: it is recorded stopped (its copy kept), and its open
        // steps are closed. A paused one keeps waiting on the owner.
        match state.store.stop_orphaned_verifications(booted, chrono::Utc::now().timestamp()) {
            Ok(0) => {}
            Ok(n) => info!(count = n, "verifications left running by the last process recorded stopped"),
            Err(e) => warn!(error = %e, "verifications left running by the last process not swept"),
        }

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

    // Expire staged self-improvement writes past their 30-day TTL; their
    // Inbox cards move to History as expired (audited gap: approvals pending
    // forever). Nothing is deleted.
    match store.expire_pending_writes() {
        Ok(ids) => {
            for pid in ids {
                tools::owner_notify::settle(store, None, &format!("learn:{pid}"), "Expired", None);
            }
        }
        Err(e) => warn!("failed to expire pending writes: {}", e),
    }
}

/// Execute one fire of a job. Returns (success, output, error). `deliver`:
/// a prompt job's reply is brought to the owner ([`deliver_reply`]) — a
/// scheduled fire; a run-now answers whoever asked for it instead.
pub(crate) async fn execute_job(state: &AppState, job: &CronJob, deliver: bool) -> (bool, String, Option<String>) {
    match job.task_type.as_str() {
        "bash" | "shell" | "" => execute_shell(state, job).await,
        "agent" => execute_agent(state, job, deliver).await,
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


/// A scheduled command runs as its employee's command, through the one door
/// every command goes through (`run_command` in the registry): the
/// permission check under the employee's grant, Nebo's own files and server
/// closed to it, Nebo's own settings out of its environment, and no network
/// when the employee's web access is off. Nothing waits on an ask here: a
/// step that needs the owner's OK is refused and recorded, and the failed
/// fire tells the owner (`engine::settle_task`). It was `sh -c` with Nebo's
/// whole environment and no check, the one command no fence reached.
async fn execute_shell(state: &AppState, job: &CronJob) -> (bool, String, Option<String>) {
    let agent_id = job.agent_id.as_deref().unwrap_or("");
    let mut ctx = tools::ToolContext::new(Origin::System);
    // No session key: nobody hears from a command that outlives its call,
    // so a long one stops at its timeout instead of moving to the background.
    ctx.session_id = format!("cron-{}", job.id);
    ctx.door = types::permissions::Door::Schedule;
    ctx.grant = Some(Arc::new(agent::harness::permissions::resolve_grant(&state.store, agent_id, None)));
    ctx.cannot_wait = true;
    let input = serde_json::json!({
        "command": job.command,
        "description": job.name,
        // As long as run_command waits for any command.
        "timeout": u64::MAX,
    });
    let result = state.tools.execute(&ctx, "run_command", input).await;
    if result.is_error {
        (false, String::new(), Some(result.content))
    } else {
        (true, result.content, None)
    }
}

/// The turn a scheduled job runs: the job's message, with its standing
/// instructions after it, run by its employee under the employee's own
/// grant (scheduling grants nothing new). In the job's own thread it is
/// written as the owner scheduled it. In the owner's conversation it was
/// asked for in (`in_conversation`) it is a prompt the platform writes,
/// which the owner never sees as something he said: what he sees is the
/// employee's answer. `in_conversation` carries what was said where it was
/// asked when the session has since moved on to a new conversation, so the
/// answer still makes sense there ("" when it has not).
fn scheduled_turn(
    job: &CronJob,
    session_key: &str,
    agent_id: &str,
    channel: &str,
    channel_ctx: Option<tools::ChannelContext>,
    cancel: tokio_util::sync::CancellationToken,
    in_conversation: Option<&str>,
) -> agent::TurnRequest {
    use agent::harness::{Delivery, SeatRequest, TurnInput, TurnMode};

    let prompt = job.message.as_deref().unwrap_or(&job.command);
    let text = match job.instructions.as_deref().map(str::trim).filter(|i| !i.is_empty()) {
        Some(instructions) => format!("{prompt}\n\n{instructions}"),
        None => prompt.to_string(),
    };
    let input = match in_conversation {
        Some("") => TurnInput::Platform {
            text: format!(
                "{} set in this conversation is due now. Do what it asks, then answer the owner here, \
                 in this conversation: your reply is what he reads, and he is told it is here, so don't also send \
                 it to his Inbox.\n\n{text}",
                request_marker(job)
            ),
        },
        Some(earlier) => TurnInput::Platform {
            text: format!(
                "{} the owner asked for in an earlier conversation with you is due now. That \
                 conversation ended with:\n\n{earlier}\n\nDo what the schedule asks, then answer the owner here, in \
                 this conversation: your reply is what he reads, and he is told it is here, so don't also send it to \
                 his Inbox.\n\n{text}",
                request_marker(job)
            ),
        },
        None => TurnInput::Owner { text, images: Vec::new(), attachments: Vec::new(), message_id: None },
    };
    agent::TurnRequest {
        session_key: session_key.to_string(),
        input,
        seat: SeatRequest {
            agent_id: agent_id.to_string(),
            user_id: String::new(),
            origin: Origin::System,
            door: types::permissions::Door::Schedule,
            mode: None,
            narrow: None,
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
            platform: None,
            mention_briefing: None,
        },
        cancel,
        progress: None,
    }
}

/// Every fire of a job starts a fresh conversation in the job's one thread
/// (`checkpoint::fresh_run`): the owner reads every run there, one after
/// another, but the model reads only this run, with one line on how the last
/// one ended. Carried in one conversation, each fire resent every earlier
/// run and the model, seeing the same request forty times, refused it
/// ("This is the 40th identical request", 2026-10-02). The first fire has
/// nothing before it.
fn start_fresh(state: &AppState, job: &CronJob, session_key: &str) {
    let Ok(Some(session)) = state.store.get_session_by_name(session_key) else {
        return;
    };
    let chat_id = state.store.resolve_session_chat_id(&session.id);
    let last_run = state
        .store
        .list_cron_history(job.id, 5, 0)
        .unwrap_or_default()
        .into_iter()
        .find(|run| run.finished_at.is_some())
        .map(|run| {
            let at = run.started_at.unwrap_or_default();
            match (run.success, run.error) {
                (Some(1), _) => format!("The last run ({at} UTC) finished."),
                (_, Some(error)) => format!(
                    "The last run ({at} UTC) failed: {}",
                    error.trim().chars().take(300).collect::<String>()
                ),
                _ => format!("The last run ({at} UTC) did not finish."),
            }
        });
    if let Err(e) = agent::harness::compact::checkpoint::fresh_run(&state.store, &chat_id, last_run.as_deref()) {
        warn!(job = job.name.as_str(), error = %e, "scheduler: could not start the run on a fresh conversation");
    }
}

/// How often a running scheduled turn checks that its schedule still exists.
const SCHEDULE_WATCH: std::time::Duration = std::time::Duration::from_secs(2);

/// Stops a scheduled turn when its schedule is deleted, whichever way it was
/// deleted (the tasks API, delete_schedule, an employee's removal): the
/// turn's token is cancelled, so its tool calls stop with it (2026-10-08: two
/// scheduled turns kept writing to Odoo for minutes after their schedules
/// were deleted). Dropping the watch ends it.
struct ScheduleWatch(tokio::task::JoinHandle<()>);

impl ScheduleWatch {
    fn start(store: Arc<Store>, job_id: i64, cancel: tokio_util::sync::CancellationToken) -> Self {
        Self(tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(SCHEDULE_WATCH) => {}
                }
                if let Ok(None) = store.get_cron_job(job_id) {
                    info!(job_id, "scheduler: the schedule was deleted; stopping its running turn");
                    cancel.cancel();
                    return;
                }
            }
        }))
    }
}

impl Drop for ScheduleWatch {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn execute_agent(state: &AppState, job: &CronJob, deliver: bool) -> (bool, String, Option<String>) {
    // If this job was created from an agent-bound channel conversation, route
    // the response back through that channel's bridge — same pathway inbound
    // replies use. Without this, the cron-fired agent run posts nothing
    // visible to the user (chat_stream broadcast doesn't reach Slack).
    if let (Some(agent_id), Some(ctx_json)) =
        (job.agent_id.as_deref(), job.channel_ctx_json.as_deref())
    {
        if !agent_id.is_empty() && !ctx_json.is_empty() {
            return execute_agent_channel_bound(state, job, agent_id, ctx_json).await;
        }
    }

    // A job an employee scheduled runs AS that employee: its session key
    // carries the employee (tools scope to it — its workflows, its runs),
    // and its operation policy governs. Run as the owner's front desk, an
    // employee's check on its own workflow run found no such workflow and
    // said so every fire.
    let agent_id = job.agent_id.as_deref().filter(|a| !a.is_empty());
    // A one-shot asked for in a conversation runs in that conversation: the
    // employee reads what was said there, and its answer is its next
    // message in it, where the owner asked (2026-10-20: "in 60 seconds get
    // my location again and work out my speed" ran in a thread of its own,
    // and the answer was only in the Inbox). Every other prompt job runs in
    // its own thread, on a fresh conversation each fire. A run-now answers
    // whoever asked for it, in the job's thread.
    let origin = if deliver { origin_chat(&state.store, job) } else { None };
    let session_key = origin.as_ref().map(|o| o.session_key.clone()).unwrap_or_else(|| job_session_key(job));
    if origin.is_none() {
        start_fresh(state, job, &session_key);
    }
    let earlier = origin.as_ref().map(|o| o.moved_from.as_deref().map(|chat| earlier_context(&state.store, chat)).unwrap_or_default());
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

    let _watch = ScheduleWatch::start(state.store.clone(), job.id, cancel_token.clone());
    let req = scheduled_turn(job, &session_key, agent_id.unwrap_or_default(), "cron", None, cancel_token, earlier.as_deref());
    let started = now_secs();

    match state.harness.start_turn(req).await {
        Ok(handle) => {
            let mut rx = handle.events;
            let mut full_text = String::new();
            // The reply: what the run said after its last call.
            let mut reply = String::new();
            // The conversation was busy: the request joined the turn running
            // there, which answers it in that conversation itself.
            let mut joined = false;
            while let Some(event) = rx.recv().await {
                run_handle.touch();
                if event.stop_reason.as_deref() == Some(agent::harness::session_gate::QUEUED_INTO_RUNNING_TURN) {
                    joined = true;
                }
                match event.event_type {
                    ai::StreamEventType::ToolCall => reply.clear(),
                    ai::StreamEventType::Text => {
                        full_text.push_str(&event.text);
                        reply.push_str(&event.text);
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
            if joined {
                // The turn running there answers it. This fire is done: the
                // pointer to the answer is the follow-up's, sent when the
                // answer lands.
                info!(job = job.name.as_str(), session = %session_key, "scheduler: the conversation was busy; its running turn answers the schedule");
                drop(run_handle);
                if !hand_over(state, &session_key, job, started, false).await {
                    warn!(job = job.name.as_str(), session = %session_key, "scheduler: the request that joined the running turn was not found in its conversation");
                }
                return (true, full_text, None);
            }
            if origin.is_some() {
                // The open view of the conversation finalizes the answer it
                // streamed, as it does any turn's.
                let payload = serde_json::json!({ "session_id": session_key, "agentId": agent_id.unwrap_or_default() });
                crate::chat_dispatch::finish_turn(state, &run_handle, crate::chat_dispatch::TurnEnd { payload, artifacts: &[], control_stop: None }).await;
            }
            drop(run_handle);
            if origin.is_some() {
                if !hand_over(state, &session_key, job, started, false).await {
                    warn!(job = job.name.as_str(), session = %session_key, "scheduler: the schedule's request was not found in its conversation");
                }
            } else if deliver {
                deliver_reply(&state.store, &state.hub, &state.config.neboai.api_url, job, started, &reply, None);
            }
            (true, full_text, None)
        }
        Err(e) => {
            error!(job = job.name.as_str(), error = %e, "agent run failed");
            drop(run_handle);
            (false, String::new(), Some(e.to_string()))
        }
    }
}

/// The owner's conversation a one-shot answers in.
struct OriginChat {
    /// The session it runs on: the one it was asked for in.
    session_key: String,
    /// The conversation it was asked in, when that session has since moved
    /// on to a new one (the owner started a new chat in it): what was said
    /// there rides with the request.
    moved_from: Option<String>,
}

/// Where a one-shot prompt schedule answers: the owner's own conversation it
/// was asked for in (recorded when it was made, `created_in_chat`). None
/// for a repeating job; for one made anywhere else (a helper, a colleague,
/// a call, an outside app, a workflow, the owner's settings); and for one
/// whose conversation the owner deleted, or whose session is gone: those
/// answer in the job's own thread and the Inbox.
fn origin_chat(store: &Store, job: &CronJob) -> Option<OriginChat> {
    if !delivers_reply(job) {
        return None;
    }
    let asked_in = job.created_in_chat.as_deref().filter(|c| !c.is_empty())?;
    let session_key = job.created_in.as_deref().filter(|k| !k.is_empty())?;
    if !matches!(store.get_chat(asked_in), Ok(Some(_))) {
        return None;
    }
    let now_in = tools::owner_notify::link::conversation(store, session_key);
    if now_in.is_empty() || !matches!(store.get_chat(&now_in), Ok(Some(_))) {
        return None;
    }
    Some(OriginChat { session_key: session_key.to_string(), moved_from: (now_in != asked_in).then(|| asked_in.to_string()) })
}

/// The end of conversation `chat_id`, as the request carries it into the
/// one its session moved on to: its last messages that say something, each
/// clipped (a message that only called a tool says nothing).
fn earlier_context(store: &Store, chat_id: &str) -> String {
    store
        .get_recent_chat_messages(chat_id, 12)
        .unwrap_or_default()
        .into_iter()
        .filter(|m| !m.content.trim().is_empty())
        .map(|m| {
            let who = if m.role == "assistant" { "You" } else { "Owner" };
            format!("{who}: {}", crate::truncate_str(m.content.trim(), 600))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Where a one-shot answered in the conversation it was asked in: that
/// conversation, and the employee's message in it.
pub(crate) struct Answered {
    pub chat_id: String,
    pub message_id: String,
    /// The owner is in the app, looking at it: the answer reaches him there.
    pub watching: bool,
}

/// How the request a one-shot writes into the owner's conversation begins
/// ([`scheduled_turn`]): how the request is found there.
fn request_marker(job: &CronJob) -> String {
    format!("The schedule \"{}\"", job.name)
}

/// The key of the follow-up that points the owner at the answer to request
/// row `request_id` (`engine_effects.idem_key`).
const FOLLOW_UP: &str = "schedule-answer:";

/// The request a fire of `job` wrote into a conversation of the owner's
/// session `session_key` at or after `since`.
fn request_in(store: &Store, session_key: &str, job: &CronJob, since: i64) -> Option<db::models::ChatMessage> {
    store.last_hidden_prompt_in_session(session_key, &request_marker(job), since).ok().flatten()
}

/// The employee's answer to `request` in its conversation: its last message
/// with words in the first reply after the request, which ends at the
/// owner's next message. What the owner wrote before any reply is answered
/// with it, by the turn that takes both in. None until a reply has words.
fn answer_to(store: &Store, request: &db::models::ChatMessage) -> Option<db::models::ChatMessage> {
    let rows = store.get_chat_messages(&request.chat_id).ok()?;
    let asked = rows.iter().position(|m| m.id == request.id)?;
    let after = &rows[asked + 1..];
    let first_reply = after.iter().position(|m| m.role == "assistant")?;
    after[first_reply..]
        .iter()
        .take_while(|m| !(m.role == "user" && conversation_said_by_owner(m)))
        .filter(|m| m.role == "assistant" && !m.content.trim().is_empty())
        .last()
        .cloned()
}

/// A row the owner wrote, not one the platform or a notification wrote.
fn conversation_said_by_owner(m: &db::models::ChatMessage) -> bool {
    !m.metadata.as_deref().is_some_and(|meta| meta.contains("\"isMeta\":true"))
}

/// A fire of `job` wrote its request into the owner's conversation on
/// `session_key` (at or after `since`): the answer is that conversation's
/// from here, and the fire is done. The Inbox pointer and push to the answer
/// are a follow-up kept in the effects ledger, keyed to the request row, so
/// it is sent once, when the answer lands, whatever restarts come between
/// ([`settle_follow_ups`]). Settled at once when the answer is already
/// there. `after_restart`: the turn that took the request in ended with the
/// last process; when nothing answered it, the conversation is asked to
/// answer it again, once ([`ask_again`]). False when no such request is
/// there.
pub(crate) async fn hand_over(state: &AppState, session_key: &str, job: &CronJob, since: i64, after_restart: bool) -> bool {
    let Some(request) = request_in(&state.store, session_key, job, since) else {
        return false;
    };
    let follow_up = match keep_follow_up(&state.store, session_key, job, &request) {
        Ok(Some(e)) => e,
        Ok(None) => return true,
        Err(e) => {
            warn!(job = job.name.as_str(), error = %e, "scheduler: the follow-up to the schedule's answer was not kept");
            return true;
        }
    };
    if follow_up.state == "pending" && !state.harness.is_session_busy(session_key) && !settle_follow_up(state, &follow_up).await && after_restart {
        ask_again(state, &follow_up);
    }
    true
}

/// The follow-up that points the owner at the answer to `request`, kept
/// once per request: a second call returns the same row.
fn keep_follow_up(store: &Store, session_key: &str, job: &CronJob, request: &db::models::ChatMessage) -> Result<Option<db::EngineEffect>, types::NeboError> {
    let key = format!("{FOLLOW_UP}{}", request.id);
    let id = store.engine_effect_pending(session_key, "messaging", &key, "inbox", &job.id.to_string(), "owner")?;
    store.engine_get_effect(id)
}

/// Why a queued fire of a prompt job does not run, when it doesn't.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FireSettled {
    /// Its schedule was switched off after the fire was queued.
    SwitchedOff,
    /// Its request is already in the owner's conversation: the fire was cut
    /// off by a restart after writing it. The conversation answers it, and
    /// the follow-up points the owner at the answer ([`hand_over`]).
    HandedOver,
}

/// Before a queued fire of `job` (queued at `queued_at`) runs: whether it
/// still has anything to do. The engine queues a fire again when a restart
/// cut it off, so this is what keeps that from asking twice. A run-now
/// (`manual`) is the owner's own ask and always runs.
pub(crate) async fn fire_settled(state: &AppState, job: &CronJob, queued_at: i64, manual: bool) -> Option<FireSettled> {
    if manual {
        return None;
    }
    // A one-shot is switched off by its own fire (`retire_spent_one_shot`):
    // that is no reason not to run it.
    if job.enabled.unwrap_or(0) == 0 && !db::is_one_shot(&job.schedule) {
        return Some(FireSettled::SwitchedOff);
    }
    let asked_in_chat = job.created_in_chat.as_deref().is_some_and(|c| !c.is_empty());
    let session_key = job.created_in.as_deref().filter(|k| !k.is_empty())?;
    if delivers_reply(job) && asked_in_chat && hand_over(state, session_key, job, queued_at, true).await {
        return Some(FireSettled::HandedOver);
    }
    None
}

/// Engine pass: every follow-up whose conversation has no turn running is
/// settled once its answer has landed.
pub(crate) async fn settle_follow_ups(state: &AppState) {
    for e in pending_follow_ups(&state.store) {
        if !state.harness.is_session_busy(&e.run_id) {
            settle_follow_up(state, &e).await;
        }
    }
}

/// Boot: the follow-ups the last process left (kept before `booted`). Each
/// whose answer landed is settled; one whose request no turn answered (the
/// turn that took it in ended with that process) has its conversation asked
/// to answer it.
pub(crate) async fn follow_up_after_restart(state: &AppState, booted: i64) {
    for e in pending_follow_ups(&state.store).into_iter().filter(|e| e.created_at < booted) {
        if !state.harness.is_session_busy(&e.run_id) && !settle_follow_up(state, &e).await {
            ask_again(state, &e);
        }
    }
}

fn pending_follow_ups(store: &Store) -> Vec<db::EngineEffect> {
    store.engine_pending_effects().unwrap_or_default().into_iter().filter(|e| e.idem_key.starts_with(FOLLOW_UP)).collect()
}

/// The turn that took a schedule's request in ended with the last process
/// before it answered: the conversation answers what it holds, as any
/// stopped thread does (`wake::answer_thread`). Once per follow-up: the
/// ledger row records it.
fn ask_again(state: &AppState, e: &db::EngineEffect) {
    if e.result.is_some() {
        return;
    }
    if let Err(err) = state.store.engine_effect_held(e.id, "asked again: the turn that took the request in ended with the last process") {
        warn!(follow_up = e.id, error = %err, "scheduler: could not record asking again; not asked");
        return;
    }
    info!(session = %e.run_id, follow_up = e.id, "scheduler: a schedule's request was left unanswered by the last process; its conversation answers it");
    let state = state.clone();
    let session_key = e.run_id.clone();
    tokio::spawn(async move { crate::wake::answer_thread(&state, &session_key, 0, Vec::new()).await });
}

async fn settle_follow_up(state: &AppState, e: &db::EngineEffect) -> bool {
    let watching = state.presence.get("_global").await == Some(agent::proactive::Presence::Focused);
    settle_follow_up_with(&state.store, &state.hub, &state.config.neboai.api_url, e, watching)
}

/// Point the owner at the answer follow-up `e` waits for, when it has
/// landed ([`deliver_reply`]), and close the follow-up. True when it is
/// closed: pointed, or its schedule or its conversation is gone.
fn settle_follow_up_with(store: &Store, hub: &crate::handlers::ws::ClientHub, api_url: &str, e: &db::EngineEffect, watching: bool) -> bool {
    // One settler at a time: the scheduler's own fire and the engine pass
    // may reach the same follow-up together.
    static SETTLING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = SETTLING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if !matches!(store.engine_get_effect(e.id), Ok(Some(now)) if now.state == "pending") {
        return true;
    }
    let job = e.provider_key.parse::<i64>().ok().and_then(|id| store.get_cron_job(id).ok().flatten());
    let request = e.idem_key.strip_prefix(FOLLOW_UP).and_then(|id| store.get_chat_message(id).ok().flatten());
    let (Some(job), Some(request)) = (job, request) else {
        if let Err(err) = store.engine_effect_failed(e.id, "its schedule or its conversation was removed", now_secs()) {
            warn!(follow_up = e.id, error = %err, "scheduler: could not close a follow-up with nothing to point at");
        }
        return true;
    };
    let Some(answer) = answer_to(store, &request) else {
        return false;
    };
    let answered = Answered { chat_id: request.chat_id.clone(), message_id: answer.id.clone(), watching };
    deliver_reply(store, hub, api_url, &job, request.created_at, &answer.content, Some(&answered));
    if let Err(err) = store.engine_effect_completed(e.id, Some(&answer.id), Some("the owner was pointed at the answer"), now_secs()) {
        warn!(follow_up = e.id, error = %err, "scheduler: the owner was pointed at the answer, but the follow-up was not closed");
    }
    true
}

/// Whether a prompt job's replies reach the owner through [`deliver_reply`]:
/// a one-shot (a timer, a reminder) does — it is the answer to what the
/// owner asked for. A repeating one never does, wherever it was made. The
/// desktop's "completed" line would only repeat it.
fn delivers_reply(job: &CronJob) -> bool {
    job.task_type == "agent" && db::is_one_shot(&job.schedule)
}

/// A one-shot prompt job's run ended with `reply`: the system, not the
/// model, brings it to the owner, so a timer or a "tell me when" never
/// depends on the model choosing to send it. Once per run (`started` is
/// when it began, or, asked for in a conversation, when its request was
/// written there), it lands in his Inbox — unless the run already put it
/// there itself (`push_notification`, `message_owner`).
///
/// Asked for in a conversation, the run answered there (`answered`): the
/// employee's message in that conversation IS the answer, and the item is
/// its pointer, opening that conversation at that message (owner,
/// 2026-10-20). It is loud (a popup, and a push that sounds on his phone)
/// unless he is watching the app: then the Inbox row arrives quietly and the
/// phone's push is passive, listed without a sound or a banner. Made
/// anywhere else, the run answered in the job's own thread: the item opens
/// that thread, and is always loud.
///
/// A repeating job is background work and its reply goes nowhere: never a
/// report per fire (2026-10-18: an hourly calendar check said "nothing
/// actionable" into his chat at 4 PM). Its fire is a run in the Workforce;
/// what it has for the owner it posts to his Inbox itself
/// (`push_notification`), and a fire with nothing for him posts nowhere.
pub(crate) fn deliver_reply(
    store: &Store,
    hub: &crate::handlers::ws::ClientHub,
    api_url: &str,
    job: &CronJob,
    started: i64,
    reply: &str,
    answered: Option<&Answered>,
) {
    let reply = reply.trim();
    if reply.is_empty() || !delivers_reply(job) {
        return;
    }
    let agent_id = job.agent_id.as_deref().filter(|a| !a.is_empty()).unwrap_or(tools::team_tool::PRIMARY_AGENT_ID);
    // The conversation the run answered in: an item the run posted itself
    // points there (`push_notification`, `message_owner`), and so does this
    // one. Another item of the same employee's, about other work, is not it.
    let place = match answered {
        Some(a) => tools::owner_notify::link::chat(agent_id, &a.chat_id),
        None => tools::owner_notify::link::session_chat(store, agent_id, &job_session_key(job)),
    };
    let user = store.ensure_local_user_id().unwrap_or_default();
    let posted_itself = store
        .list_user_notifications(&user, db::models::InboxView::All, Some(agent_id), 20, 0)
        .unwrap_or_default()
        .iter()
        .any(|n| n.created_at >= started && n.action_url.as_deref().is_some_and(|url| url.starts_with(&place)));
    if posted_itself {
        info!(job = job.name.as_str(), "scheduler: the run already posted to the Inbox; no second item");
        return;
    }
    let id = format!("schedule:{}:{started}", job.id);
    let link = match answered {
        Some(a) => tools::owner_notify::link::message(agent_id, &a.chat_id, &a.message_id),
        None => place,
    };
    let body = crate::truncate_str(reply, 500);
    let n = tools::owner_notify::OwnerNotification {
        id: &id,
        kind: "info",
        title: &job.name,
        body: Some(body),
        action_url: Some(&link),
        agent_id: Some(agent_id),
        loud: answered.is_none_or(|a| !a.watching),
        link_label: Some("Chat"),
        ..Default::default()
    };
    tools::owner_notify::emit(store, Some(&|ev, payload| hub.broadcast(ev, payload)), &n);
    // The phone's copy: the chat it opens, and, while the owner is watching
    // the answer arrive, a push that waits in his notifications without
    // sounding.
    let extra = match answered {
        Some(a) if a.watching => serde_json::json!({ "chatId": a.chat_id, "interruptionLevel": "passive" }),
        Some(a) => serde_json::json!({ "chatId": a.chat_id }),
        None => serde_json::json!({}),
    };
    crate::codes::push_inbox_via(store, api_url, n.hub_item(extra));
}

/// The one thread every fire of a prompt job runs in: the employee's, when
/// an employee scheduled it.
fn job_session_key(job: &CronJob) -> String {
    match job.agent_id.as_deref().filter(|a| !a.is_empty()) {
        Some(id) => format!("agent:{}:cron:{}", id, job.name),
        None => format!("cron-{}", job.name),
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

    let _watch = ScheduleWatch::start(state.store.clone(), job.id, cancel_token.clone());
    let req = scheduled_turn(job, &session_key, agent_id, &saved.kind, Some(channel_ctx.clone()), cancel_token, None);

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
        .run(workflow_id, serde_json::Value::Null, "cron", None, None)
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
            None,
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
            ..Default::default()
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

    /// Deleting a schedule stops its running turn; a schedule that stays
    /// leaves its turn running, and ending the watch stops nothing.
    #[tokio::test]
    async fn deleting_a_schedule_stops_its_running_turn() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("w.db").to_string_lossy()).unwrap());
        let job = store.create_cron_job("retry", "0 9 * * *", "", "agent", Some("x"), None, None, true, Some("emp"), None, None, &db::models::ScheduleProvenance::new(db::models::ScheduleCreator::Owner, "")).unwrap();
        let kept = store.create_cron_job("kept", "0 9 * * *", "", "agent", Some("x"), None, None, true, Some("emp"), None, None, &db::models::ScheduleProvenance::new(db::models::ScheduleCreator::Owner, "")).unwrap();
        let (turn, other) = (tokio_util::sync::CancellationToken::new(), tokio_util::sync::CancellationToken::new());
        let _w1 = ScheduleWatch::start(store.clone(), job.id, turn.clone());
        let _w2 = ScheduleWatch::start(store.clone(), kept.id, other.clone());
        tokio::time::sleep(SCHEDULE_WATCH * 2).await;
        assert!(!turn.is_cancelled(), "a schedule that exists keeps its turn");
        store.delete_cron_job_by_name("retry").unwrap();
        tokio::time::sleep(SCHEDULE_WATCH + SCHEDULE_WATCH / 2).await;
        assert!(turn.is_cancelled(), "the deleted schedule's turn stops");
        assert!(!other.is_cancelled(), "another schedule's turn runs on");
    }

    /// A store holding the owner's conversation `X` with employee `ava`,
    /// and a prompt schedule made in it (`from_chat`) or by the owner's API,
    /// one-shot or repeating.
    fn delivery_rig(one_shot: bool, from_chat: bool) -> (tempfile::TempDir, Arc<Store>, CronJob) {
        use db::models::{ScheduleCreator, ScheduleProvenance};
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("d.db").to_string_lossy()).unwrap());
        let key = "agent:ava:thread:X";
        store.create_session("s-x", Some(key), None, None, None).unwrap();
        store.create_chat_for_session("X", key, "Timers", None).unwrap();
        store.set_session_active_chat_id("s-x", "X").unwrap();
        let schedule = if one_shot { "0 18 8 16 10 * 2099" } else { "0 * * * *" };
        let by = if from_chat {
            ScheduleProvenance::new(ScheduleCreator::Chat, "").in_run(Some("run-1"), key).asked_in_chat("X")
        } else {
            ScheduleProvenance::new(ScheduleCreator::Owner, "")
        };
        let job = store
            .create_cron_job("one-minute-timer", schedule, "", "agent", Some("Notify the owner"), None, None, true, Some("ava"), None, None, &by)
            .unwrap();
        (dir, store, job)
    }

    fn said(store: &Store, chat: &str) -> Vec<String> {
        store.get_chat_messages(chat).unwrap().into_iter().filter(|m| m.role == "assistant").map(|m| m.content).collect()
    }

    fn inbox(store: &Store) -> Vec<db::models::Notification> {
        let user = store.ensure_local_user_id().unwrap();
        store.list_user_notifications(&user, db::models::InboxView::All, None, 50, 0).unwrap()
    }

    /// The schedule's request, as a fire wrote it into conversation X.
    fn ask_in_x(store: &Store) -> db::models::ChatMessage {
        let asked = format!("{} set in this conversation is due now.", request_marker(&store.get_cron_job_by_name("one-minute-timer").unwrap().unwrap()));
        store.create_chat_message(&uuid::Uuid::new_v4().to_string(), "X", "user", &asked, Some(r#"{"isMeta":true,"hiddenPrompt":true}"#)).unwrap()
    }

    /// The employee's message in conversation X.
    fn reply_in_x(store: &Store, text: &str) -> String {
        store.create_chat_message(&uuid::Uuid::new_v4().to_string(), "X", "assistant", text, None).unwrap().id
    }

    /// The fire that wrote `request` hands it over: the follow-up it keeps.
    fn handed_over(store: &Store, job: &CronJob, request: &db::models::ChatMessage) -> db::EngineEffect {
        let found = request_in(store, "agent:ava:thread:X", job, request.created_at).expect("the request is found where it was written");
        assert_eq!(found.id, request.id);
        keep_follow_up(store, "agent:ava:thread:X", job, &found).unwrap().unwrap()
    }

    fn heard(rx: &mut tokio::sync::broadcast::Receiver<crate::handlers::ws::HubEvent>) -> Vec<String> {
        let mut events = Vec::new();
        while let Ok(e) = rx.try_recv() {
            events.push(e.event_type);
        }
        events
    }

    /// A timer asked for in chat X runs in X and answers there: the Inbox
    /// item points at that answer (X, at its message), loud when the owner
    /// is away and quiet while he watches. It is never a second copy of the
    /// answer in X. Settling the same follow-up again changes nothing.
    #[tokio::test]
    async fn a_one_shot_from_a_chat_answers_in_it_and_its_inbox_item_opens_the_answer() {
        let (_d, store, job) = delivery_rig(true, true);
        let origin = origin_chat(&store, &job).expect("it runs where it was asked");
        assert_eq!((origin.session_key.as_str(), origin.moved_from.as_deref()), ("agent:ava:thread:X", None));
        let hub = crate::handlers::ws::ClientHub::new();
        let mut rx = hub.subscribe();
        let request = ask_in_x(&store);
        reply_in_x(&store, "");
        let message = reply_in_x(&store, "  Ding! Your one-minute timer just went off.  ");
        let follow_up = handed_over(&store, &job, &request);
        assert!(settle_follow_up_with(&store, &hub, "", &follow_up, false));

        assert_eq!(said(&store, "X"), ["", "  Ding! Your one-minute timer just went off.  "], "the answer is in X once");
        let items = inbox(&store);
        assert_eq!(items.len(), 1, "{items:#?}");
        assert_eq!(items[0].title, "one-minute-timer");
        assert_eq!(items[0].body.as_deref(), Some("Ding! Your one-minute timer just went off."));
        assert_eq!(items[0].agent_id.as_deref(), Some("ava"));
        assert_eq!(items[0].link_label.as_deref(), Some("Chat"));
        assert_eq!(items[0].action_url.as_deref(), Some(format!("/ava/threads/X?message={message}").as_str()), "it opens the answer");
        let events = heard(&mut rx);
        assert!(events.iter().any(|e| e == "notification"), "away, a timer going off is loud: {events:?}");
        assert_eq!(store.engine_get_effect(follow_up.id).unwrap().unwrap().state, "completed");

        assert!(settle_follow_up_with(&store, &hub, "", &follow_up, false));
        assert_eq!(handed_over(&store, &job, &request).id, follow_up.id, "one follow-up per request");
        assert!(settle_follow_up_with(&store, &hub, "", &handed_over(&store, &job, &request), false));
        assert_eq!(inbox(&store).len(), 1, "once per request");
        assert!(heard(&mut rx).is_empty(), "nothing is said twice");

        // Watching: the same item, waiting quietly.
        let (_d, store, job) = delivery_rig(true, true);
        let mut rx = hub.subscribe();
        let request = ask_in_x(&store);
        reply_in_x(&store, "Ding!");
        assert!(settle_follow_up_with(&store, &hub, "", &handed_over(&store, &job, &request), true));
        assert_eq!(inbox(&store).len(), 1);
        let events = heard(&mut rx);
        assert!(events.iter().any(|e| e == "notification_created") && !events.iter().any(|e| e == "notification"), "{events:?}");
    }

    /// The request joined a turn that then ended with the process (a
    /// restart, or a turn that took its time): the follow-up waits, durably,
    /// and the pointer goes out once when the answer lands, however many
    /// times it is looked at, and only to the answer. What the owner wrote
    /// before any reply is answered with the request.
    #[tokio::test]
    async fn the_pointer_waits_for_the_answer_and_goes_out_once() {
        let (_d, store, job) = delivery_rig(true, true);
        let hub = crate::handlers::ws::ClientHub::new();
        let request = ask_in_x(&store);
        let follow_up = handed_over(&store, &job, &request);
        assert!(!settle_follow_up_with(&store, &hub, "", &follow_up, false), "nothing answered yet");
        store.create_chat_message("x-owner", "X", "user", "Also, what's the weather?", None).unwrap();
        assert!(!settle_follow_up_with(&store, &hub, "", &follow_up, false), "the owner's words are not the answer");
        assert!(inbox(&store).is_empty());
        assert_eq!(pending_follow_ups(&store).len(), 1, "it waits in the ledger, through any restart");

        let answer = reply_in_x(&store, "68 mph. And it's sunny.");
        store.create_chat_message("x-owner-2", "X", "user", "Thanks", None).unwrap();
        reply_in_x(&store, "Any time.");
        for _ in 0..3 {
            assert!(settle_follow_up_with(&store, &hub, "", &store.engine_get_effect(follow_up.id).unwrap().unwrap(), false));
        }
        let items = inbox(&store);
        assert_eq!(items.len(), 1, "{items:#?}");
        assert_eq!(items[0].action_url.as_deref(), Some(format!("/ava/threads/X?message={answer}").as_str()), "the reply to the request, not a later one");
        assert!(pending_follow_ups(&store).is_empty());

        // Its schedule removed: nothing to point at, and it is closed.
        let (_d, store, job) = delivery_rig(true, true);
        let follow_up = handed_over(&store, &job, &ask_in_x(&store));
        store.delete_cron_job_by_name("one-minute-timer").unwrap();
        assert!(settle_follow_up_with(&store, &hub, "", &follow_up, false));
        assert!(pending_follow_ups(&store).is_empty() && inbox(&store).is_empty());
    }

    /// A fire is queued again by the boot sweep once a restart cut it off.
    /// Its request already in the owner's conversation is found, so it is
    /// never asked twice; a fire queued after the request is not it.
    #[tokio::test]
    async fn a_fire_whose_request_is_already_there_is_found() {
        let (_d, store, job) = delivery_rig(true, true);
        let queued_at = now_secs();
        assert!(request_in(&store, "agent:ava:thread:X", &job, queued_at).is_none(), "nothing asked yet");
        store.create_chat_message("x-owner", "X", "user", &request_marker(&job), None).unwrap();
        assert!(request_in(&store, "agent:ava:thread:X", &job, queued_at).is_none(), "the owner's words are never the request");
        let request = ask_in_x(&store);
        assert_eq!(request_in(&store, "agent:ava:thread:X", &job, queued_at).map(|m| m.id), Some(request.id.clone()));
        assert!(request_in(&store, "agent:ava:thread:X", &job, request.created_at + 1).is_none(), "a later fire's own request is its own");
        // The owner started a new conversation in the session since.
        store.create_chat_for_session("Y", "agent:ava:thread:X", "New chat", None).unwrap();
        store.set_session_active_chat_id("s-x", "Y").unwrap();
        assert_eq!(request_in(&store, "agent:ava:thread:X", &job, queued_at).map(|m| m.id), Some(request.id));
    }

    /// The owner deleted the conversation a timer was asked in: it answers in
    /// its own thread and the Inbox, and nothing is written into a chat. The
    /// owner started a new conversation in the same session instead: it
    /// answers in the new one, carrying the end of the one it was asked in.
    #[tokio::test]
    async fn a_deleted_conversation_gets_the_inbox_and_a_moved_on_one_gets_the_earlier_words() {
        let (_d, store, job) = delivery_rig(true, true);
        store.create_chat_message("x-1", "X", "user", "Get my GPS now, and again in 60 seconds.", None).unwrap();
        store.create_chat_message("x-2", "X", "assistant", "", None).unwrap();
        store.create_chat_message("x-3", "X", "assistant", "First reading: 40.232353, -112.175299 at 5:46:05 PM.", None).unwrap();
        store.create_chat_for_session("Y", "agent:ava:thread:X", "New chat", None).unwrap();
        store.set_session_active_chat_id("s-x", "Y").unwrap();
        let moved = origin_chat(&store, &job).expect("the session is still the owner's");
        assert_eq!(moved.moved_from.as_deref(), Some("X"));
        let earlier = earlier_context(&store, "X");
        assert_eq!(earlier, "Owner: Get my GPS now, and again in 60 seconds.\nYou: First reading: 40.232353, -112.175299 at 5:46:05 PM.");
        let text = turn_text(&job, Some(&earlier));
        assert!(text.contains("an earlier conversation") && text.contains("First reading: 40.232353") && text.contains("Notify the owner"), "{text}");

        let (_d, store, job) = delivery_rig(true, true);
        store.delete_chat("X").unwrap();
        assert!(origin_chat(&store, &job).is_none(), "a deleted conversation is never written into");
    }

    /// The text a scheduled turn's request carries.
    fn turn_text(job: &CronJob, in_conversation: Option<&str>) -> String {
        let req = scheduled_turn(job, "agent:ava:thread:X", "ava", "cron", None, tokio_util::sync::CancellationToken::new(), in_conversation);
        match req.input {
            agent::harness::TurnInput::Platform { text } | agent::harness::TurnInput::Owner { text, .. } => text,
            _ => panic!("a scheduled turn is a request"),
        }
    }

    /// The run already posted to the Inbox itself (push_notification or
    /// message_owner): its own item is the only one.
    #[tokio::test]
    async fn a_run_that_reached_the_owner_itself_is_not_repeated() {
        let hub = crate::handlers::ws::ClientHub::new();
        let (_d, store, job) = delivery_rig(true, true);
        let request = ask_in_x(&store);
        tools::owner_notify::emit(
            &store,
            None,
            &tools::owner_notify::OwnerNotification { id: "own", kind: "info", title: "Timer", body: Some("up"), action_url: Some("/ava/threads/X"), agent_id: Some("ava"), ..Default::default() },
        );
        // Another item of the same employee's, about other work, is not it.
        tools::owner_notify::emit(
            &store,
            None,
            &tools::owner_notify::OwnerNotification { id: "other", kind: "info", title: "Report", body: Some("sent"), action_url: Some("/ava/threads/Z"), agent_id: Some("ava"), ..Default::default() },
        );
        reply_in_x(&store, "Ding!");
        assert!(settle_follow_up_with(&store, &hub, "", &handed_over(&store, &job, &request), false));
        assert_eq!(inbox(&store).len(), 2, "the run's own Inbox item is the only one about it");
        let (_d, store, job) = delivery_rig(true, true);
        let request = ask_in_x(&store);
        tools::owner_notify::emit(
            &store,
            None,
            &tools::owner_notify::OwnerNotification { id: "other", kind: "info", title: "Report", body: Some("sent"), action_url: Some("/ava/threads/Z"), agent_id: Some("ava"), ..Default::default() },
        );
        reply_in_x(&store, "Ding!");
        assert!(settle_follow_up_with(&store, &hub, "", &handed_over(&store, &job, &request), false));
        assert!(inbox(&store).iter().any(|n| n.id.starts_with("schedule:")), "an unrelated item never stands in for the timer's");
    }

    /// Made outside a conversation, a one-shot runs in its own thread,
    /// reaches the Inbox and opens that thread. A repeating job is
    /// background work: it runs in its own thread wherever it was made, and
    /// its reply is delivered nowhere — the 4 PM "nothing actionable" of
    /// 2026-10-18 stays out of the owner's chat and out of his Inbox. An
    /// empty reply delivers nothing.
    #[tokio::test]
    async fn where_each_kind_of_schedule_delivers() {
        let hub = crate::handlers::ws::ClientHub::new();

        let (_d, store, job) = delivery_rig(true, false);
        store.create_session("s-c", Some("agent:ava:cron:one-minute-timer"), None, None, None).unwrap();
        store.create_chat_for_session("agent:ava:cron:one-minute-timer", "agent:ava:cron:one-minute-timer", "Timer", None).unwrap();
        assert!(delivers_reply(&job));
        assert!(origin_chat(&store, &job).is_none(), "made outside a conversation, it runs in its own thread");
        deliver_reply(&store, &hub, "", &job, now_secs(), "Ding!", None);
        assert!(said(&store, "X").is_empty());
        let items = inbox(&store);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].action_url.as_deref(), Some("/ava/threads/agent%3Aava%3Acron%3Aone-minute-timer"), "it opens the run's thread");

        let (_d, store, job) = delivery_rig(false, true);
        assert!(!delivers_reply(&job));
        assert!(origin_chat(&store, &job).is_none(), "a repeating job never runs in the owner's chat");
        deliver_reply(&store, &hub, "", &job, now_secs(), "It's 4 PM Sunday, nothing actionable to act on right now.", None);
        assert!(said(&store, "X").is_empty(), "the chat it was made in hears nothing per fire");
        assert!(inbox(&store).is_empty(), "no Inbox item per repeating fire");

        let (_d, store, job) = delivery_rig(false, false);
        assert!(!delivers_reply(&job));
        deliver_reply(&store, &hub, "", &job, now_secs(), "Nothing changed.", None);
        assert!(said(&store, "X").is_empty() && inbox(&store).is_empty());

        let (_d, store, job) = delivery_rig(true, true);
        deliver_reply(&store, &hub, "", &job, now_secs(), "   ", None);
        assert!(said(&store, "X").is_empty() && inbox(&store).is_empty());
    }

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
