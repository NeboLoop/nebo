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
    // Heard before the turn starts, so a running turn's end is never missed.
    let mut hub_events = state.hub.subscribe();

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
                // The turn running there answers it: when that turn ends,
                // its answer is delivered by the same rule as one of our own.
                info!(job = job.name.as_str(), session = %session_key, "scheduler: the conversation was busy; its running turn answers the schedule");
                drop(run_handle);
                // A turn that ends without having heard it hands it to the
                // next; each end is checked for the answer.
                let ended = tokio::time::timeout(JOINED_ANSWER_WAIT, async {
                    loop {
                        match hub_events.recv().await {
                            Ok(e) if e.event_type == "chat_complete" && e.payload["session_id"] == session_key.as_str() => {
                                let watching = state.presence.get("_global").await == Some(agent::proactive::Presence::Focused);
                                if let Some(answered) = answered_in(&state.store, &session_key, job, watching).filter(|a| a.message_id.is_some()) {
                                    return Some(answered);
                                }
                            }
                            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                        }
                    }
                })
                .await;
                match ended {
                    Ok(Some(answered)) => {
                        let said = answered.text.clone().unwrap_or_default();
                        deliver_reply(&state.store, &state.hub, &state.config.neboai.api_url, job, started, &said, Some(&answered));
                    }
                    _ => warn!(job = job.name.as_str(), session = %session_key, "scheduler: nothing answered the schedule in time; no Inbox pointer"),
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
            if deliver {
                let watching = state.presence.get("_global").await == Some(agent::proactive::Presence::Focused);
                let answered = match &origin {
                    Some(o) => answered_in(&state.store, &o.session_key, job, watching),
                    None => None,
                };
                deliver_reply(&state.store, &state.hub, &state.config.neboai.api_url, job, started, &reply, answered.as_ref());
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

/// How long a schedule that joined a turn already running in its
/// conversation waits for that turn to end before it gives up pointing the
/// owner at the answer.
const JOINED_ANSWER_WAIT: Duration = Duration::from_secs(30 * 60);

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
    if now_in.is_empty() {
        return None;
    }
    Some(OriginChat { session_key: session_key.to_string(), moved_from: (now_in != asked_in).then(|| asked_in.to_string()) })
}

/// The end of conversation `chat_id`, as the request carries it into the
/// one its session moved on to: its last messages, each clipped.
fn earlier_context(store: &Store, chat_id: &str) -> String {
    store
        .get_recent_chat_messages(chat_id, 12)
        .unwrap_or_default()
        .into_iter()
        .map(|m| {
            let who = if m.role == "assistant" { "You" } else { "Owner" };
            format!("{who}: {}", crate::truncate_str(m.content.trim(), 600))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Where a one-shot's run answered in the conversation it was asked in:
/// that conversation, and the employee's message in it, when it wrote one.
pub(crate) struct Answered {
    pub chat_id: String,
    pub message_id: Option<String>,
    /// What that message says.
    pub text: Option<String>,
    /// The owner is in the app, looking at it: the answer reaches him there.
    pub watching: bool,
}

/// How the request a one-shot writes into the owner's conversation begins
/// ([`scheduled_turn`]): how its answer is found after it.
fn request_marker(job: &CronJob) -> String {
    format!("The schedule \"{}\"", job.name)
}

/// The conversation session `session_key` holds, and the employee's answer
/// to `job` in it: its last message after the job's latest request there,
/// before anything the owner wrote next. No message when nothing has
/// answered the request yet.
fn answered_in(store: &Store, session_key: &str, job: &CronJob, watching: bool) -> Option<Answered> {
    let chat_id = tools::owner_notify::link::conversation(store, session_key);
    if chat_id.is_empty() {
        return None;
    }
    let rows = store.get_chat_messages(&chat_id).unwrap_or_default();
    let marker = request_marker(job);
    let answer = rows
        .iter()
        .rposition(|m| m.role == "user" && m.content.starts_with(&marker))
        .and_then(|asked| {
            rows[asked + 1..]
                .iter()
                .take_while(|m| !(m.role == "user" && conversation_said_by_owner(m)))
                .filter(|m| m.role == "assistant" && !m.content.trim().is_empty())
                .last()
        });
    Some(Answered {
        chat_id,
        message_id: answer.map(|m| m.id.clone()),
        text: answer.map(|m| m.content.clone()),
        watching,
    })
}

/// A row the owner wrote, not one the platform or a notification wrote.
fn conversation_said_by_owner(m: &db::models::ChatMessage) -> bool {
    !m.metadata.as_deref().is_some_and(|meta| meta.contains("\"isMeta\":true"))
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
/// when it began), it lands in his Inbox — unless the run already put it
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
    let link = match answered.and_then(|a| a.message_id.as_deref().map(|m| (a, m))) {
        Some((a, message_id)) => tools::owner_notify::link::message(agent_id, &a.chat_id, message_id),
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

    /// The schedule's request and the employee's answer, as the turn that
    /// ran in conversation X wrote them.
    fn answer_in_x(store: &Store, text: &str) -> String {
        let asked = format!("{} set in this conversation is due now.", request_marker(&store.get_cron_job_by_name("one-minute-timer").unwrap().unwrap()));
        store.create_chat_message("x-asked", "X", "user", &asked, Some(r#"{"isMeta":true,"hiddenPrompt":true}"#)).unwrap();
        store.create_chat_message("x-reply", "X", "assistant", text, None).unwrap().id
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
    /// answer in X. Delivering the same run again changes nothing.
    #[tokio::test]
    async fn a_one_shot_from_a_chat_answers_in_it_and_its_inbox_item_opens_the_answer() {
        let (_d, store, job) = delivery_rig(true, true);
        let origin = origin_chat(&store, &job).expect("it runs where it was asked");
        assert_eq!((origin.session_key.as_str(), origin.moved_from.as_deref()), ("agent:ava:thread:X", None));
        let hub = crate::handlers::ws::ClientHub::new();
        let mut rx = hub.subscribe();
        let started = now_secs();
        let message = answer_in_x(&store, "Ding! Your one-minute timer just went off.");
        let away = answered_in(&store, "agent:ava:thread:X", &job, false).unwrap();
        assert_eq!((away.chat_id.as_str(), away.message_id.as_deref()), ("X", Some(message.as_str())));
        deliver_reply(&store, &hub, "", &job, started, "  Ding! Your one-minute timer just went off.  ", Some(&away));

        assert_eq!(said(&store, "X"), ["Ding! Your one-minute timer just went off."], "the answer is in X once");
        let items = inbox(&store);
        assert_eq!(items.len(), 1, "{items:#?}");
        assert_eq!(items[0].title, "one-minute-timer");
        assert_eq!(items[0].body.as_deref(), Some("Ding! Your one-minute timer just went off."));
        assert_eq!(items[0].agent_id.as_deref(), Some("ava"));
        assert_eq!(items[0].link_label.as_deref(), Some("Chat"));
        assert_eq!(items[0].action_url.as_deref(), Some(format!("/ava/threads/X?message={message}").as_str()), "it opens the answer");
        let events = heard(&mut rx);
        assert!(events.iter().any(|e| e == "notification"), "away, a timer going off is loud: {events:?}");

        deliver_reply(&store, &hub, "", &job, started, "Ding! Your one-minute timer just went off.", Some(&away));
        assert_eq!(inbox(&store).len(), 1, "once per run");

        // Watching: the same item, waiting quietly.
        let (_d, store, job) = delivery_rig(true, true);
        let mut rx = hub.subscribe();
        let started = now_secs();
        answer_in_x(&store, "Ding!");
        let watching = answered_in(&store, "agent:ava:thread:X", &job, true).unwrap();
        deliver_reply(&store, &hub, "", &job, started, "Ding!", Some(&watching));
        assert_eq!(inbox(&store).len(), 1);
        let events = heard(&mut rx);
        assert!(events.iter().any(|e| e == "notification_created") && !events.iter().any(|e| e == "notification"), "{events:?}");
    }

    /// The owner deleted the conversation a timer was asked in: it answers in
    /// its own thread and the Inbox, and nothing is written into a chat. The
    /// owner started a new conversation in the same session instead: it
    /// answers in the new one, carrying the end of the one it was asked in.
    #[tokio::test]
    async fn a_deleted_conversation_gets_the_inbox_and_a_moved_on_one_gets_the_earlier_words() {
        let (_d, store, job) = delivery_rig(true, true);
        store.create_chat_message("x-1", "X", "user", "Get my GPS now, and again in 60 seconds.", None).unwrap();
        store.create_chat_message("x-2", "X", "assistant", "First reading: 40.232353, -112.175299 at 5:46:05 PM.", None).unwrap();
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
        assert!(tools::owner_notify::link::conversation(&store, "agent:ava:thread:X").is_empty(), "a deleted conversation is gone");
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
        let started = now_secs();
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
        answer_in_x(&store, "Ding!");
        let answered = answered_in(&store, "agent:ava:thread:X", &job, false);
        deliver_reply(&store, &hub, "", &job, started, "Ding!", answered.as_ref());
        assert_eq!(inbox(&store).len(), 2, "the run's own Inbox item is the only one about it");
        let (_d, store, job) = delivery_rig(true, true);
        let started = now_secs();
        tools::owner_notify::emit(
            &store,
            None,
            &tools::owner_notify::OwnerNotification { id: "other", kind: "info", title: "Report", body: Some("sent"), action_url: Some("/ava/threads/Z"), agent_id: Some("ava"), ..Default::default() },
        );
        answer_in_x(&store, "Ding!");
        deliver_reply(&store, &hub, "", &job, started, "Ding!", answered_in(&store, "agent:ava:thread:X", &job, false).as_ref());
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
