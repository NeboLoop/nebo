use super::*;
use db::models::{ScheduleCreator, ScheduleProvenance};
use db::{NewEvent, NewWait};

fn store() -> (std::sync::Arc<db::Store>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(db::Store::new(&dir.path().join("nebo.db").to_string_lossy()).unwrap());
    (store, dir)
}

fn schedule(store: &db::Store, name: &str, task_type: &str, command: &str, agent: Option<&str>, by: ScheduleProvenance) -> db::models::CronJob {
    store
        .create_cron_job(name, "0 9 * * *", command, task_type, Some("check the inbox"), None, None, true, agent, None, None, &by)
        .unwrap()
}

/// Every enabled schedule is listed as a timer: who made it, in which run
/// and conversation, why, when it fires next, and what the owner can do
/// to it. A workflow's own schedule is turned off with its workflow, never
/// paused or deleted under it. A paused schedule is not background work.
#[test]
fn timers_carry_their_maker_reason_next_fire_and_actions() {
    let (s, _d) = store();
    let made = schedule(
        &s,
        "call-back",
        "agent",
        "",
        Some("ava"),
        ScheduleProvenance::new(ScheduleCreator::Chat, "Kristi asked for a call back").in_run(Some("run-7"), "agent:ava:web"),
    );
    let binding = schedule(&s, "agent-ava-sweep", "agent_workflow", "agent:ava:sweep", None, ScheduleProvenance::new(ScheduleCreator::Workflow, "the sweep workflow's schedule"));
    let paused = schedule(&s, "paused", "agent", "", Some("ava"), ScheduleProvenance::new(ScheduleCreator::Owner, "x"));
    s.set_cron_job_enabled(paused.id, false).unwrap();
    s.engine_enqueue_event(&NewEvent {
        kind: "timer",
        target_type: "binding",
        target_id: &db::cron_ref(made.id),
        idem_key: "t-1",
        due_at: Some(4_000_000_000),
        schedule: Some("0 9 * * *"),
        ..Default::default()
    })
    .unwrap();

    let timers = timers(&s);
    assert_eq!(timers.len(), 2, "{timers:#?}");
    let t = timers.iter().find(|t| t.id == format!("timer:{}", made.id)).unwrap();
    assert_eq!((t.kind, t.source, t.status), (Kind::Loop, Source::Timer, Status::Scheduled));
    assert_eq!(t.agent_id, "ava");
    assert_eq!(t.title, "call-back");
    assert_eq!(t.detail, "Kristi asked for a call back");
    assert_eq!(t.created_by, "chat");
    assert_eq!(t.source_run_id.as_deref(), Some("run-7"));
    assert_eq!(t.session_key.as_deref(), Some("agent:ava:web"));
    assert_eq!(t.next_run_at, Some(4_000_000_000));
    assert_eq!(t.actions, vec![Action::Pause, Action::Delete]);
    let b = timers.iter().find(|t| t.id == format!("timer:{}", binding.id)).unwrap();
    assert_eq!(b.agent_id, "ava", "a binding's schedule belongs to the employee its command names");
    assert_eq!(b.created_by, "workflow");
    assert_eq!(b.actions, vec![Action::Off]);
}

/// A schedule made before makers were kept reads as made by someone
/// unknown, with no reason, until it is remade.
#[test]
fn a_schedule_from_before_reads_as_unknown() {
    let (s, _d) = store();
    let job = schedule(&s, "old", "agent", "", None, ScheduleProvenance::new(ScheduleCreator::Owner, "x"));
    s.conn_exec_for_test(&format!("UPDATE cron_jobs SET created_by = 'unknown', reason = '' WHERE id = {}", job.id));
    let t = timers(&s).pop().unwrap();
    assert_eq!(t.created_by, "unknown");
    assert_eq!(t.detail, "", "no reason was kept");
}

/// Workflow runs still going are listed with the employee, the binding,
/// the step running now and the turns used of the budget; a parked one
/// says what it waits for and when it gives up, and can be cancelled.
#[test]
fn workflow_runs_show_their_step_turns_and_wait() {
    let (s, _d) = store();
    s.create_workflow_run("r-run", "agent:ava", "schedule", Some("sweep"), None, Some("agent:ava:workflow:r-run"), None).unwrap();
    s.update_workflow_run("r-run", None, Some("read-inbox"), None, None, None).unwrap();
    s.create_workflow_run("r-wait", "agent:ava", "agent", Some("intake:gmail"), None, None, None).unwrap();
    s.engine_declare_wait(
        "r-wait",
        &NewWait { action: "resume", on_kind: "expert_reply", key: "expert:r-wait", deadline: Some(4_000_000_000), parked: None, reason: "asked the tax expert" },
        1,
    )
    .unwrap();
    s.create_workflow_run("r-done", "agent:ava", "manual", Some("sweep"), None, None, None).unwrap();
    s.complete_workflow_run("r-done", "completed", 0, None, None, None).unwrap();

    let live = s.list_live_workflow_runs().unwrap();
    assert_eq!(live.len(), 2, "an ended run is not background work");
    let tasks = workflows(live, |id| (id == "r-run").then_some((42, 150)));
    let run = tasks.iter().find(|t| t.id == "workflow:r-run").unwrap();
    assert_eq!((run.kind, run.status), (Kind::Workflow, Status::Running));
    assert_eq!((run.agent_id.as_str(), run.title.as_str(), run.detail.as_str()), ("ava", "sweep", "read-inbox"));
    assert_eq!((run.turns_used, run.turns_cap), (Some(42), Some(150)));
    assert_eq!(run.created_by, "schedule");
    assert_eq!(run.actions, vec![Action::Stop]);
    let wait = tasks.iter().find(|t| t.id == "workflow:r-wait").unwrap();
    assert_eq!(wait.status, Status::Waiting);
    assert_eq!(wait.title, "intake", "a binding's event source is not its name");
    assert_eq!(wait.wait.as_deref(), Some("expert"));
    assert_eq!(wait.detail, "asked the tax expert");
    assert_eq!(wait.next_run_at, Some(4_000_000_000), "when the wait gives up");
    assert_eq!(wait.created_by, "chat");
    assert_eq!(wait.actions, vec![Action::Cancel]);
}

fn snapshot(run_id: &str, session_key: &str, entity_id: &str, origin: &str) -> crate::run_registry::RunSnapshot {
    crate::run_registry::RunSnapshot {
        run_id: run_id.into(),
        session_key: session_key.into(),
        entity_id: entity_id.into(),
        entity_name: "Cron: digest".into(),
        origin: origin.into(),
        channel: "cron".into(),
        iteration_count: 3,
        tool_call_count: 2,
        current_tool: "read_file".into(),
        activity: "Reading notes.md".into(),
        elapsed_secs: 90,
        idle_secs: 5,
        parent_run_id: None,
        child_count: 0,
    }
}

/// Turns nobody watches are background work: a schedule's, a message's.
/// The owner's own chat turn is in front of him and is not. A turn parked
/// on a question waits for an answer.
#[test]
fn turns_list_what_runs_unwatched() {
    let runs = vec![
        snapshot("t-cron", "agent:ava:cron:digest", "ava", "cron"),
        snapshot("t-owner", "agent:ava:web", "ava", "user"),
        snapshot("t-loop", "neboai:dm:1", "main", "comm"),
    ];
    let parked: HashSet<String> = ["neboai:dm:1".to_string()].into();
    let tasks = turns(runs, &parked, 1_000);
    assert_eq!(tasks.len(), 2);
    let cron = &tasks[0];
    assert_eq!((cron.id.as_str(), cron.created_by.as_str(), cron.status), ("turn:t-cron", "schedule", Status::Running));
    assert_eq!(cron.detail, "Reading notes.md");
    assert_eq!((cron.started_at, cron.last_activity_at), (Some(910), Some(995)));
    let msg = &tasks[1];
    assert_eq!(msg.agent_id, "", "the main bot is the empty employee");
    assert_eq!((msg.created_by.as_str(), msg.status, msg.wait.as_deref()), ("message", Status::Waiting, Some("answer")));
}

fn helper(task_id: &str, running: bool, parent: &str, agent: &str) -> agent::harness::delegation::HelperStatus {
    agent::harness::delegation::HelperStatus {
        task_id: task_id.into(),
        description: "price the Rivera order".into(),
        running,
        session_key: format!("subagent:{parent}:{task_id}"),
        activity: "Reading orders.csv".into(),
        parent_key: parent.into(),
        agent_id: agent.into(),
        started_at: 500,
    }
}

/// Running helpers are listed under the employee that started them, with
/// the conversation they report to; a helper that finished is not.
#[test]
fn helpers_list_the_running_ones_where_they_report() {
    let tasks = helpers(vec![helper("h-1", true, "agent:ava:web", "ava"), helper("h-2", false, "agent:ava:web", "ava")]);
    assert_eq!(tasks.len(), 1);
    let t = &tasks[0];
    assert_eq!((t.id.as_str(), t.kind, t.agent_id.as_str()), ("helper:h-1", Kind::Agent, "ava"));
    assert_eq!((t.title.as_str(), t.detail.as_str()), ("price the Rivera order", "Reading orders.csv"));
    assert_eq!(t.session_key.as_deref(), Some("agent:ava:web"));
    assert_eq!(t.started_at, Some(500));
    assert_eq!(t.actions, vec![Action::Stop]);
}

/// An employee's list is its own work and the work it passed on; "main"
/// and "" are the main bot.
#[test]
fn an_employee_sees_its_own_and_what_it_passed_on() {
    let mut mine = BackgroundTask::new(Source::Helper, "h", Kind::Agent, Status::Running);
    mine.agent_id = "ava".into();
    let mut passed = BackgroundTask::new(Source::Turn, "t", Kind::Agent, Status::Running);
    passed.agent_id = "ben".into();
    passed.from_agent_id = Some("ava".into());
    let main = BackgroundTask::new(Source::Timer, "1", Kind::Loop, Status::Scheduled);
    assert!(mine.belongs_to("ava") && passed.belongs_to("ava") && passed.belongs_to("ben"));
    assert!(!mine.belongs_to("ben"));
    assert!(main.belongs_to("main") && main.belongs_to("") && !main.belongs_to("ava"));
}

/// Running work comes first, oldest first; then what fires next, soonest
/// first.
#[test]
fn running_work_comes_first_then_what_fires_soonest() {
    let at = |source: Source, id: &str, status: Status, started: Option<i64>, next: Option<i64>| {
        let mut t = BackgroundTask::new(source, id, Kind::Agent, status);
        t.started_at = started;
        t.next_run_at = next;
        t
    };
    let mut tasks = vec![
        at(Source::Timer, "late", Status::Scheduled, None, Some(900)),
        at(Source::Helper, "new", Status::Running, Some(200), None),
        at(Source::Timer, "soon", Status::Scheduled, None, Some(100)),
        at(Source::Workflow, "old", Status::Waiting, Some(50), None),
    ];
    order(&mut tasks);
    let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["workflow:old", "helper:new", "timer:soon", "timer:late"]);
}

/// Between two looks: what left the list, and the timers that appeared.
#[test]
fn a_diff_names_what_ended_and_the_new_timers() {
    let helper = BackgroundTask::new(Source::Helper, "h", Kind::Agent, Status::Running);
    let old_timer = BackgroundTask::new(Source::Timer, "1", Kind::Loop, Status::Scheduled);
    let new_timer = BackgroundTask::new(Source::Timer, "2", Kind::Loop, Status::Scheduled);
    let shell = BackgroundTask::new(Source::Shell, "cmd-1", Kind::Shell, Status::Running);
    let before = vec![helper.clone(), old_timer.clone()];
    let after = vec![old_timer, new_timer.clone(), shell];
    let (gone, timers) = diff(&before, &after);
    assert_eq!(gone, vec![&helper]);
    assert_eq!(timers, vec![&new_timer], "a new command is not a new timer");
}

fn zone() -> tools::owner_clock::OwnerZone {
    tools::owner_clock::OwnerZone::parse(Some("America/Denver"))
}

/// The model's note: one line per piece of background work, on the owner's
/// clock, telling it not to start a duplicate or report unfinished work as
/// done. The turn reading it is not listed; watches are standing setup,
/// not work in flight. Nothing running: no note.
#[test]
fn the_note_lists_the_work_and_omits_the_turn_itself() {
    let mut h = BackgroundTask::new(Source::Helper, "h-1", Kind::Agent, Status::Running);
    h.title = "price the Rivera order".into();
    h.started_at = Some(1_760_115_600); // Fri Oct 10 2025 11:00 in Denver
    h.session_key = Some("agent:ava:web".into());
    let mut timer = BackgroundTask::new(Source::Timer, "7", Kind::Loop, Status::Scheduled);
    timer.title = "call-back".into();
    timer.detail = "Kristi asked for a call back".into();
    timer.next_run_at = Some(1_760_130_000); // 15:00 in Denver
    let mut wf = BackgroundTask::new(Source::Workflow, "r-1", Kind::Workflow, Status::Waiting);
    wf.title = "intake".into();
    wf.wait = Some("approval".into());
    let mut me = BackgroundTask::new(Source::Turn, "t-me", Kind::Agent, Status::Running);
    me.session_key = Some("agent:ava:web".into());
    let watch = BackgroundTask::new(Source::Watch, "ava:intake", Kind::Monitor, Status::Watching);

    let text = note(&[h, timer, wf, me, watch], "agent:ava:web", zone());
    assert_eq!(
        text,
        "Your background work right now:\n\
         - helper h-1 \"price the Rivera order\" is still running (since Fri Oct 10 11:00). Its result comes to you as a notification; don't start another for the same task, and don't report its work as done.\n\
         - schedule \"call-back\" (Kristi asked for a call back) fires next Fri Oct 10 15:00. Don't make another for the same thing.\n\
         - workflow run r-1 \"intake\" is waiting for the owner's approval."
    );
    assert_eq!(note(&[], "agent:ava:web", zone()), "");
    let only_me = BackgroundTask { session_key: Some("agent:ava:web".into()), ..BackgroundTask::new(Source::Turn, "t", Kind::Agent, Status::Running) };
    assert_eq!(note(&[only_me], "agent:ava:web", zone()), "", "the turn itself is not background work to it");
}

/// The note stays tiny: past its line cap the rest are counted.
#[test]
fn the_note_counts_what_it_does_not_list() {
    let tasks: Vec<BackgroundTask> = (0..11)
        .map(|i| {
            let mut t = BackgroundTask::new(Source::Timer, &i.to_string(), Kind::Loop, Status::Scheduled);
            t.title = format!("s{i}");
            t
        })
        .collect();
    let text = note(&tasks, "agent:ava:web", zone());
    assert_eq!(text.lines().count(), 1 + NOTE_LINES + 1);
    assert!(text.ends_with("- and 3 more (list_schedules, read_output)."), "{text}");
}

/// The end of a long output is shown, and says it was cut.
#[test]
fn output_shows_its_end() {
    let long = format!("{}END", "x".repeat(OUTPUT_TAIL_CHARS));
    let t = tail(&long);
    assert!(t.truncated && t.output.ends_with("END") && t.output.chars().count() == OUTPUT_TAIL_CHARS);
    assert_eq!(tail("ok"), OutputTail { output: "ok".into(), truncated: false });
}

// ── over the API, on a running server ───────────────────────────────────

/// The owner's list over HTTP: a schedule the owner makes shows with its
/// reason under its employee, its output reads, an action it doesn't have
/// is refused, pausing it takes it off the list and records it finished,
/// and the stop that stops everything answers.
#[tokio::test]
async fn the_api_lists_acts_and_stops_everything() {
    let nebo = crate::staffed_proof::session().await;
    let agent = format!("bg-{}", uuid::Uuid::new_v4().simple());
    let name = format!("{agent}-daily");
    let made = nebo
        .post_ok(
            "/tasks",
            &serde_json::json!({ "name": name, "schedule": "0 9 * * *", "taskType": "agent", "message": "digest", "agentId": agent, "reason": "the morning digest" }),
        )
        .await;
    let id = format!("timer:{}", made["id"].as_i64().unwrap());

    let listed = nebo.get_ok(&format!("/background?agentId={agent}")).await;
    let tasks = listed["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1, "{listed}");
    let t = &tasks[0];
    assert_eq!(t["id"], id.as_str());
    assert_eq!(t["kind"], "loop");
    assert_eq!(t["createdBy"], "owner");
    assert_eq!(t["detail"], "the morning digest");
    assert_eq!(t["actions"], serde_json::json!(["pause", "delete"]));

    let out = nebo.get_ok(&format!("/background/{}/output", urlencode(&id))).await;
    assert_eq!(out, serde_json::json!({ "output": "", "truncated": false }), "it has not run yet");

    let (status, _) = nebo.post(&format!("/background/{}/approve", urlencode(&id)), &serde_json::json!({})).await;
    assert_eq!(status, 400, "a timer has nothing to approve");
    let (status, _) = nebo.post("/background/timer:999999999/pause", &serde_json::json!({})).await;
    assert_eq!(status, 404);

    nebo.post_ok(&format!("/background/{}/pause", urlencode(&id)), &serde_json::json!({})).await;
    let job = nebo.store().get_cron_job_by_name(&name).unwrap().unwrap();
    assert_eq!(job.enabled, Some(0), "paused, kept");
    let after = nebo.get_ok(&format!("/background?agentId={agent}")).await;
    assert!(after["tasks"].as_array().unwrap().is_empty(), "{after}");

    let stopped = nebo.post_ok("/background/stop-all", &serde_json::json!({})).await;
    assert!(stopped["stopped"].is_u64(), "{stopped}");
}

fn urlencode(id: &str) -> String {
    id.replace(':', "%3A")
}

/// A turn the owner stopped takes the schedules it made with it: they are
/// switched off, never deleted. A turn that ended by itself, or stalled,
/// leaves them alone.
#[tokio::test]
async fn a_stopped_turn_retires_the_schedules_it_made() {
    let nebo = crate::staffed_proof::session().await;
    let state = &nebo.state;
    let register = |token: &tokio_util::sync::CancellationToken| {
        state.run_registry.register(crate::run_registry::RegisterParams {
            session_key: "agent:ava:web".into(),
            entity_id: "ava".into(),
            entity_name: "Ava".into(),
            origin: "user".into(),
            channel: "web".into(),
            cancel_token: token.clone(),
            parent_run_id: None,
        })
    };
    let made_in = |run_id: &str, name: &str| {
        state
            .store
            .create_cron_job(
                name,
                "0 9 * * *",
                "",
                "agent",
                Some("x"),
                None,
                None,
                true,
                Some("ava"),
                None,
                None,
                &ScheduleProvenance::new(ScheduleCreator::Chat, "x").in_run(Some(run_id), "agent:ava:web"),
            )
            .unwrap()
    };
    let suffix = uuid::Uuid::new_v4().simple().to_string();

    let stopped_token = tokio_util::sync::CancellationToken::new();
    let stopped = register(&stopped_token).await;
    let doomed = made_in(&stopped.run_id, &format!("doomed-{suffix}"));
    stopped_token.cancel();
    turn_ended(state, &stopped, &stopped_token);
    let doomed = state.store.get_cron_job(doomed.id).unwrap().unwrap();
    assert_eq!(doomed.enabled, Some(0), "switched off");

    let ended_token = tokio_util::sync::CancellationToken::new();
    let ended = register(&ended_token).await;
    let kept = made_in(&ended.run_id, &format!("kept-{suffix}"));
    turn_ended(state, &ended, &ended_token);
    assert_eq!(state.store.get_cron_job(kept.id).unwrap().unwrap().enabled, Some(1), "a turn that ended keeps its schedules");

    let stalled_token = tokio_util::sync::CancellationToken::new();
    let stalled = register(&stalled_token).await;
    let also_kept = made_in(&stalled.run_id, &format!("stalled-{suffix}"));
    stalled.stalled.store(true, std::sync::atomic::Ordering::SeqCst);
    stalled_token.cancel();
    turn_ended(state, &stalled, &stalled_token);
    assert_eq!(state.store.get_cron_job(also_kept.id).unwrap().unwrap().enabled, Some(1), "a stall is not the owner's stop");
}
