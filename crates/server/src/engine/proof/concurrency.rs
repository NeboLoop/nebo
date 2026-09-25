//! Concurrency: many employees at once, and one employee with many things
//! in flight at once — cases, tasks, heartbeats, approvals. The engine
//! serializes exactly two things: one live turn per case, and one turn per
//! session. Everything else runs side by side, and the store takes the
//! contention. Every scenario here asserts that nothing is lost, nothing
//! is doubled, and nothing errors under pressure.

use super::*;
use serde_json::json;
use std::sync::Arc;

fn lead(agent: &'static str) -> CaseBinding<'static> {
    World::binding(agent, "work-lead", "lead", 3 * DAY)
}

/// Four employees, forty people, eight threads arriving at once, one tick
/// thread running the whole time. Every person gets exactly one case and
/// one first turn; no employee's work waits on another's.
#[test]
fn four_employees_forty_people_arrive_at_once() {
    let store = Arc::new(fresh_store());
    let t = 1_700_000_000;
    let ticking = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let ticker = {
        let (s, ticking) = (store.clone(), ticking.clone());
        std::thread::spawn(move || {
            let mut reports = Vec::new();
            while ticking.load(std::sync::atomic::Ordering::Relaxed) {
                reports.push(tick(&s, t, &idle, &no_steer));
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            reports
        })
    };
    let agents = ["ic", "closer", "support", "billing"];
    let workers: Vec<_> = (0..8)
        .map(|i| {
            let s = store.clone();
            std::thread::spawn(move || {
                let agent = agents[i % 4];
                let b = lead(agent);
                for p in 0..5 {
                    let email = format!("p{}-{}@x.com", i, p);
                    let payload = json!({"email": email, "message": "hi"});
                    let idem = format!("arrive:{i}:{p}");
                    let r = signal_or_open(&s, &b, "email", &email, &payload, "event", &idem, t).expect("no store error under contention");
                    assert!(matches!(r, Routed::Opened { .. }), "{email}: {r:?}");
                }
            })
        })
        .collect();
    for h in workers {
        h.join().unwrap();
    }
    // Let the ticker see the final state, then stop it.
    std::thread::sleep(std::time::Duration::from_millis(50));
    ticking.store(false, std::sync::atomic::Ordering::Relaxed);
    let reports = ticker.join().unwrap();
    assert!(!reports.is_empty());

    let cases = store.engine_cases(None, 100).unwrap();
    let mut seen = std::collections::HashMap::<String, Vec<String>>::new();
    for c in &cases {
        let aliases = c.inputs.as_deref().and_then(|i| serde_json::from_str::<serde_json::Value>(i).ok()).map(|v| v["_case"]["aliases"].to_string()).unwrap_or_default();
        seen.entry(aliases).or_default().push(format!("{} {} {}", &c.id[..8], c.agent_id, c.state));
    }
    let dup: Vec<_> = seen.iter().filter(|(_, v)| v.len() > 1).collect();
    assert_eq!(cases.len(), 40, "one case per person; people with more than one case: {dup:?}");
    for agent in agents {
        assert_eq!(store.engine_cases(Some(agent), 100).unwrap().len(), 10, "{agent} has its ten");
    }
    let turns = store.engine_queued_runs_of_kind("workflow", 100).unwrap();
    assert_eq!(turns.len(), 40, "one first turn each, none doubled by the ticker");
    let mut parents: Vec<_> = turns.iter().map(|t| t.parent_run_id.clone().unwrap()).collect();
    parents.sort();
    parents.dedup();
    assert_eq!(parents.len(), 40);
    assert!(store.engine_undelivered_signals().unwrap().is_empty(), "nothing left owed");
}

/// One employee with everything in flight at once: twenty cases, five
/// tasks, a heartbeat, a parked approval. One tick delivers all of it; the
/// twenty first turns run side by side; each case still has exactly one.
#[test]
fn one_employee_many_things_at_once() {
    let mut w = World::new();
    let b = lead("ic");
    let mut cases = Vec::new();
    for p in 0..20 {
        let email = format!("c{p}@x.com");
        let (case, _) = w.open(&b, &email, "hello", &format!("m{p}"));
        cases.push(case);
    }
    for i in 0..5 {
        w.s.create_pending_task(&format!("task-{i}"), "task", "agent:ic:web", None, "do", None, None, None, 0, None).unwrap();
    }
    w.s.engine_enqueue_event(&NewEvent { kind: "timer", target_type: "entity", target_id: "heartbeat:agent:ic", idem_key: "hb", due_at: Some(w.t), schedule: Some("1800"), ..Default::default() }).unwrap();
    w.s.engine_create_run(&NewRun { id: "wf-park", kind: "workflow", session_key: "agent:ic:workflow:wf-park", agent_id: "ic", lane: "main", ..Default::default() }).unwrap();
    w.s.engine_declare_wait("wf-park", &NewWait { action: "resume", on_kind: "approval", key: "approval:wf-park", parked: Some("{}"), reason: "sign", ..Default::default() }, w.t).unwrap();
    w.s.engine_enqueue_event(&NewEvent { kind: "approval", target_type: "run", target_id: "approval:wf-park", payload: "yes", idem_key: "ok", durable: true, ..Default::default() }).unwrap();

    let r = w.tick();
    assert_eq!((r.fired, r.resumed, r.unrouted), (1, 1, 0));
    assert_eq!(w.s.engine_queued_runs_of_kind("workflow", 100).unwrap().len(), 21, "twenty first turns and the resumed workflow");
    assert_eq!(w.s.engine_queued_runs_of_kind("task", 100).unwrap().len(), 5);
    assert_eq!(w.s.engine_queued_runs_of_kind("heartbeat", 100).unwrap().len(), 1);

    // All twenty turns run at once (distinct sessions); a second message to
    // each person rides its own running turn, never a second turn.
    for case in &cases {
        let turn = w.queued_turn(case).unwrap();
        w.start(&turn.id);
    }
    for (p, case) in cases.iter().enumerate() {
        let email = format!("c{p}@x.com");
        assert_eq!(w.arrive(&b, "email", &email, json!({"email": email, "message": "again"}), &format!("again{p}")), Routed::Signaled { case_id: case.clone() });
    }
    let live = |k: &str| Some(format!("{k}:run::0"));
    let r = tick(&w.s, w.t, &live, &no_steer);
    assert_eq!((r.steered, r.children_started), (20, 0), "twenty steered into twenty running turns");
    for case in &cases {
        assert_eq!(w.s.engine_children(case).unwrap().len(), 1, "one turn per case, still");
    }
    // The turns settle in any order; each case waits on its own clock.
    for (p, case) in cases.iter().enumerate() {
        let turn = w.s.engine_children(case).unwrap().remove(0);
        w.t += 1;
        w.finish(&turn, &waits("contacted", "sent", "signal", &format!("{}d", 1 + p % 3), "reply"));
        assert_eq!(w.run(case).state, "waiting");
    }
    let deadlines: std::collections::HashSet<i64> = cases.iter().map(|c| w.wait(c).unwrap().deadline.unwrap()).collect();
    assert!(deadlines.len() >= 3, "independent clocks");
}

/// A burst larger than one tick's claim batch is drained over ticks with
/// nothing lost and nothing doubled: a hundred and twenty people in one
/// second become a hundred and twenty cases and a hundred and twenty turns.
#[test]
fn a_burst_is_drained_over_ticks_without_loss() {
    let w = World::new();
    let b = lead("ic");
    let mut opened = 0;
    for p in 0..120 {
        let email = format!("b{p}@x.com");
        if matches!(w.arrive(&b, "email", &email, json!({"email": email}), &format!("burst{p}")), Routed::Opened { .. }) {
            opened += 1;
        }
    }
    assert_eq!(opened, 120);
    // Their first turns are queued at arrival; a second wave of replies is
    // what the ticks must drain: 120 signals, 50 per tick.
    for p in 0..120 {
        let email = format!("b{p}@x.com");
        w.arrive(&b, "email", &email, json!({"email": email, "message": "reply"}), &format!("reply{p}"));
    }
    let mut steered = 0;
    let mut ticks = 0;
    while steered < 120 {
        let r = w.tick();
        steered += r.steered;
        ticks += 1;
        assert!(ticks <= 10, "drained in a bounded number of ticks");
        assert_eq!(r.children_started, 0, "the replies ride the queued first turns");
    }
    assert_eq!(steered, 120);
    assert!(ticks >= 3, "more than one tick's batch");
    assert_eq!(w.s.engine_queued_runs_of_kind("workflow", 200).unwrap().len(), 120, "still one turn per person");
    assert!(w.s.engine_undelivered_signals().unwrap().is_empty());
}

/// The store under contention: sixteen threads writing waits, events, and
/// ledger rows while a tick thread runs. No write fails, every row lands,
/// and the ledger's one-message-per-person rule holds across threads.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_store_and_the_ledger_hold_under_contention() {
    let store = Arc::new(fresh_store());
    let t = 1_700_000_000;
    for i in 0..16 {
        store.engine_create_run(&NewRun { id: &format!("run-{i}"), kind: "workflow", session_key: &format!("agent:ic:workflow:run-{i}"), agent_id: "ic", lane: "main", ..Default::default() }).unwrap();
    }
    let ticking = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let ticker = {
        let (s, ticking) = (store.clone(), ticking.clone());
        std::thread::spawn(move || {
            while ticking.load(std::sync::atomic::Ordering::Relaxed) {
                tick(&s, t, &idle, &no_steer);
            }
        })
    };
    let mut handles = Vec::new();
    for i in 0..16 {
        let s = store.clone();
        handles.push(tokio::spawn(async move {
            let run = format!("run-{i}");
            let ctx = World::ctx("ic", &run);
            let mut sent = 0;
            for k in 0..25 {
                s.engine_enqueue_event(&NewEvent { kind: "note", target_type: "run", target_id: &run, payload: "x", idem_key: &format!("n:{i}:{k}"), durable: true, ..Default::default() }).expect("enqueue under contention");
                s.engine_declare_wait(&run, &NewWait { action: "resume", on_kind: "signal", key: &format!("k{i}"), reason: "w", ..Default::default() }, t).expect("wait under contention");
                // Every thread tries to message the same person four times in
                // four wordings; the ledger lets one through per run.
                let input = json!({"to": "same@person.com", "text": format!("wording {k}")});
                let r = guarded_send(&s, &ctx, "messaging", "mail-app", "mail.message.send", &input, || async { SendOutcome::Sent("ok".into(), None) }).await;
                if !r.is_error {
                    sent += 1;
                }
            }
            sent
        }));
    }
    let mut total_sent = 0;
    for h in handles {
        total_sent += h.await.unwrap();
    }
    ticking.store(false, std::sync::atomic::Ordering::Relaxed);
    ticker.join().unwrap();
    assert_eq!(total_sent, 16, "one message to the person per run, sixteen runs");
    for i in 0..16 {
        let run = format!("run-{i}");
        let completed = store.engine_effects_for_run(&run).unwrap().iter().filter(|e| e.state == "completed").count();
        assert_eq!(completed, 1, "{run}");
        assert_eq!(store.engine_waits_for_run(&run).unwrap().len(), 25, "{run}: every wait landed");
    }
}

/// Given words for every model call: the plan request gets `tasks`
/// independent sub-tasks, every other call gets "done" after a short think.
/// For `reject_for` after the first call past the plan, every call is
/// refused with a 429 and Retry-After 1 s — a wave the way Janus sends one:
/// everyone, for a moment. `live`/`peak` count the calls in flight at the
/// provider. The permits, the runner's loop and retries, the store and the
/// DAG scheduler are the product's own.
struct GivenWords(Arc<Words>);

struct Words {
    tasks: usize,
    reject_for: std::time::Duration,
    reject_until: std::sync::Mutex<Option<std::time::Instant>>,
    /// Calls refused in the wave.
    refused: std::sync::atomic::AtomicUsize,
    live: std::sync::atomic::AtomicUsize,
    peak: std::sync::atomic::AtomicUsize,
    /// Peak calls in flight after the wave ended.
    peak_after_reject: std::sync::atomic::AtomicUsize,
}

impl Words {
    fn new(tasks: usize, reject_for: std::time::Duration) -> Arc<Self> {
        use std::sync::atomic::AtomicUsize;
        Arc::new(Self {
            tasks,
            reject_for,
            reject_until: std::sync::Mutex::new(None),
            refused: AtomicUsize::new(0),
            live: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            peak_after_reject: AtomicUsize::new(0),
        })
    }

    /// Whether the wave is on right now, starting it on the first call.
    fn refusing(&self) -> bool {
        if self.reject_for.is_zero() {
            return false;
        }
        let now = std::time::Instant::now();
        let mut until = self.reject_until.lock().unwrap();
        let end = *until.get_or_insert(now + self.reject_for);
        now < end
    }

    fn wave_over(&self) -> bool {
        let until = self.reject_until.lock().unwrap();
        matches!(*until, Some(end) if std::time::Instant::now() >= end)
    }
}

#[async_trait::async_trait]
impl ai::Provider for GivenWords {
    fn id(&self) -> &str {
        "given-words"
    }
    async fn stream(&self, req: &ai::ChatRequest) -> Result<ai::EventReceiver, ai::ProviderError> {
        use std::sync::atomic::Ordering::SeqCst;
        let asks_for_plan = req
            .messages
            .iter()
            .any(|m| m.content.contains("Break this task into independent sub-tasks"));
        let words = &self.0;
        let text = if asks_for_plan {
            let plan: Vec<_> = (1..=words.tasks)
                .map(|i| json!({"id": i.to_string(), "description": format!("part-{i}"), "prompt": format!("do part {i}"), "agent_type": "general", "depends_on": []}))
                .collect();
            serde_json::to_string(&plan).unwrap()
        } else {
            if words.refusing() {
                words.refused.fetch_add(1, SeqCst);
                return Err(ai::ProviderError::RateLimit { retry_after_secs: Some(1) });
            }
            "done".to_string()
        };
        let me = words.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tokio::spawn(async move {
            let now = me.live.fetch_add(1, SeqCst) + 1;
            me.peak.fetch_max(now, SeqCst);
            if me.wave_over() {
                me.peak_after_reject.fetch_max(now, SeqCst);
            }
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            me.live.fetch_sub(1, SeqCst);
            let _ = tx.send(ai::StreamEvent::text(text)).await;
            let _ = tx.send(ai::StreamEvent::done()).await;
        });
        Ok(rx)
    }
}

/// A real runner over a fresh store with only the model's words given.
fn given_words_runner(
    words: Arc<Words>,
    concurrency: Arc<agent::ConcurrencyController>,
) -> (Arc<agent::Runner>, Arc<db::Store>) {
    let store = Arc::new(fresh_store());
    let runner = Arc::new(agent::Runner::new(
        store.clone(),
        Arc::new(tools::Registry::new(Arc::new(agent::Check::new(store.clone())))),
        vec![Arc::new(GivenWords(words)) as Arc<dyn ai::Provider>],
        agent::selector::ModelSelector::new(Default::default()),
        concurrency,
        Arc::new(napp::HookDispatcher::new()),
        None,
        Default::default(),
        None,
    ));
    (runner, store)
}

/// The run a proof's fan-out is spawned from: the owner's, with no limits.
fn owner_run() -> tools::SpawnRequest {
    tools::SpawnRequest {
        parent_session_id: "agent:ops:web".into(),
        parent_session_key: "agent:ops:web".into(),
        user_id: "owner".into(),
        ..Default::default()
    }
}

/// A fan-out finishes when only two model calls may run at once. A permit
/// is taken where the resource is spent, at the call, never around a unit
/// of work that makes calls: the DAG once held an LLM permit per sub-task
/// while each sub-task's runner took one per call, so at the permit floor
/// two sub-tasks held both and waited forever. Three independent sub-tasks
/// run through the real runner and all three report back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fan_out_finishes_at_the_permit_floor() {
    use tools::SubAgentOrchestrator as _;

    let concurrency = Arc::new(agent::ConcurrencyController::new(Some(2)));
    assert_eq!(concurrency.ceiling(), 2, "the scenario runs at the permit floor");
    let (runner, store) = given_words_runner(Words::new(3, std::time::Duration::ZERO), concurrency);
    let orchestrator = agent::Orchestrator::new(runner, store);

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        orchestrator.execute_dag("three independent jobs", owner_run()),
    )
    .await
    .expect("the fan-out deadlocked: a permit is held around work that needs permits")
    .expect("the fan-out runs");
    assert!(result.success, "every sub-task completes: {:?}", result.error);
    for part in ["part-1", "part-2", "part-3"] {
        assert!(result.output.contains(part), "sub-task '{part}' is missing from: {}", result.output);
    }
}

/// A 429 slows the whole bot, not the one call that got it, and the bot
/// recovers on its own. Eight sub-tasks fan out over eight permits and
/// Janus refuses everyone for a moment (Retry-After 1 s). The pool halves
/// once for the wave (not once per refusal), the refused calls wait the
/// second they were told, no later moment runs all eight again until
/// successes rebuild the pool, every sub-task still finishes, and the pool
/// is back to eight — with no header from Janus and no resource probe.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_429_slows_the_whole_bot_and_it_recovers() {
    use std::sync::atomic::Ordering::SeqCst;
    use tools::SubAgentOrchestrator as _;

    let concurrency = Arc::new(agent::ConcurrencyController::new(Some(8)));
    concurrency.set_ceiling(8);
    assert_eq!(concurrency.effective_permits(), 8);
    let words = Words::new(8, std::time::Duration::from_millis(300));
    let (runner, store) = given_words_runner(words.clone(), concurrency.clone());
    let orchestrator = agent::Orchestrator::new(runner, store);

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        orchestrator.execute_dag("eight independent jobs", owner_run()),
    )
    .await
    .expect("the fan-out finished")
    .expect("the fan-out runs");
    assert!(result.success, "every sub-task completes after the 429 wave: {:?}", result.error);
    for i in 1..=8 {
        let part = format!("part-{i}");
        assert!(result.output.contains(&part), "sub-task '{part}' is missing from: {}", result.output);
    }
    assert!(words.refused.load(SeqCst) >= 1, "the wave refused someone");
    let after = words.peak_after_reject.load(SeqCst);
    assert!(
        (1..8).contains(&after),
        "after the 429 wave the bot ran at most half again, not all eight at once: peak {after}"
    );
    assert_eq!(concurrency.effective_permits(), 8, "successes grew the pool back to the machine's bound");
}

/// A sub-agent's model, given call by call. Every call on the child's task
/// waits until the scenario lets it answer, then gives the next scripted
/// reply; what each call was shown is recorded. Calls that are not the
/// child's (none are expected) answer at once.
struct Scripted(Arc<Script>);

enum Reply {
    /// Call a tool: the step ends and the loop goes on.
    Tool,
    /// Answer in words: the turn can end.
    Text(&'static str),
}

struct Script {
    /// Words only the child's own calls carry.
    marker: &'static str,
    replies: std::sync::Mutex<std::collections::VecDeque<Reply>>,
    /// The user-role messages each of the child's calls was shown.
    seen: std::sync::Mutex<Vec<Vec<String>>>,
    /// One permit per call allowed to answer.
    go: tokio::sync::Semaphore,
    started_tx: tokio::sync::mpsc::UnboundedSender<usize>,
    started_rx: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<usize>>,
}

impl Script {
    fn new(marker: &'static str, replies: impl IntoIterator<Item = Reply>) -> Arc<Self> {
        let (started_tx, started_rx) = tokio::sync::mpsc::unbounded_channel();
        Arc::new(Self {
            marker,
            replies: std::sync::Mutex::new(replies.into_iter().collect()),
            seen: std::sync::Mutex::new(Vec::new()),
            go: tokio::sync::Semaphore::new(0),
            started_tx,
            started_rx: tokio::sync::Mutex::new(started_rx),
        })
    }

    /// Wait until the child's call number `n` (0-based) is in flight.
    async fn started(&self, n: usize) {
        let mut rx = self.started_rx.lock().await;
        loop {
            let got = tokio::time::timeout(std::time::Duration::from_secs(20), rx.recv())
                .await
                .expect("the sub-agent made no model call")
                .expect("script dropped");
            if got == n {
                return;
            }
        }
    }

    /// Let the next `n` calls answer.
    fn answer(&self, n: usize) {
        self.go.add_permits(n);
    }

    fn seen(&self, n: usize) -> Vec<String> {
        self.seen.lock().unwrap()[n].clone()
    }

    fn calls(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

#[async_trait::async_trait]
impl ai::Provider for Scripted {
    fn id(&self) -> &str {
        "scripted"
    }
    async fn stream(&self, req: &ai::ChatRequest) -> Result<ai::EventReceiver, ai::ProviderError> {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let script = self.0.clone();
        if !req.messages.iter().any(|m| m.content.contains(script.marker)) {
            tokio::spawn(async move {
                let _ = tx.send(ai::StreamEvent::text("ok")).await;
                let _ = tx.send(ai::StreamEvent::done()).await;
            });
            return Ok(rx);
        }
        let n = {
            let mut seen = script.seen.lock().unwrap();
            seen.push(req.messages.iter().filter(|m| m.role == "user").map(|m| m.content.clone()).collect());
            seen.len() - 1
        };
        let _ = script.started_tx.send(n);
        tokio::spawn(async move {
            let Ok(permit) = script.go.acquire().await else { return };
            permit.forget();
            let reply = script.replies.lock().unwrap().pop_front().unwrap_or(Reply::Text("done"));
            let event = match reply {
                Reply::Tool => ai::StreamEvent::tool_call(ai::ToolCall {
                    id: format!("call-{n}"),
                    name: "look_around".into(),
                    input: json!({}),
                }),
                Reply::Text(words) => ai::StreamEvent::text(words),
            };
            let _ = tx.send(event).await;
            let _ = tx.send(ai::StreamEvent::done()).await;
        });
        Ok(rx)
    }
}

/// The parent a scenario's sub-agent reports to: an interactive session.
const PARENT: &str = "agent:ops:web";

/// A real orchestrator over a real runner and store, the child's words given.
fn scripted_orchestrator(script: Arc<Script>) -> (agent::Orchestrator, Arc<agent::Runner>, Arc<db::Store>) {
    let store = Arc::new(fresh_store());
    let runner = Arc::new(agent::Runner::new(
        store.clone(),
        Arc::new(tools::Registry::new(Arc::new(agent::Check::new(store.clone())))),
        vec![Arc::new(Scripted(script)) as Arc<dyn ai::Provider>],
        agent::selector::ModelSelector::new(Default::default()),
        Arc::new(agent::ConcurrencyController::new(Some(4))),
        Arc::new(napp::HookDispatcher::new()),
        None,
        Default::default(),
        None,
    ));
    (agent::Orchestrator::new(runner.clone(), store.clone()), runner, store)
}

/// A background child of `PARENT` whose prompt carries the script's marker.
fn background_child(marker: &str) -> tools::SpawnRequest {
    tools::SpawnRequest {
        prompt: format!("{marker}: research the market"),
        description: "market research".into(),
        agent_type: "general".into(),
        model_override: String::new(),
        parent_session_id: String::new(),
        parent_session_key: PARENT.into(),
        user_id: "owner".into(),
        wait: false,
        parent_cancel: None,
        max_iterations: 10,
        skills: vec![],
        parent_stream_tx: None,
        handoff_depth: 0,
        isolate: String::new(),
        workspace: String::new(),
        seat: Default::default(),
    }
}

/// The child's report once it holds `expect`, as the parent is woken with it.
async fn report_holding(store: &db::Store, task_id: &str, expect: &str) -> String {
    for _ in 0..1000 {
        if let Ok(Some(task)) = store.get_pending_task(task_id)
            && let Some(output) = task.output
            && output.contains(expect)
        {
            return output;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("the sub-agent never reported '{expect}'");
}

/// The child's thread, as stored.
fn child_thread(runner: &agent::Runner, task_id: &str) -> Vec<db::models::ChatMessage> {
    let sessions = runner.sessions();
    let id = sessions.resolve_session_id_by_key(&format!("subagent:{PARENT}:{task_id}")).expect("the child's session");
    sessions.get_messages(&id).expect("the child's thread")
}

/// A parent reaches a sub-agent that is still working, the way the owner
/// reaches an employee mid-turn. Two messages sent while its first step runs
/// both come back "delivered" at once, are in its thread as sent (marked as
/// the parent's, with the parent's taint), and its next step reads both, in
/// order, framed as the parent's. Its report reaches the parent as spawned.
/// Once it has finished, a message continues it as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parents_message_reaches_its_running_sub_agent() {
    use tools::SubAgentOrchestrator as _;
    let script = Script::new("MARKET-7", [Reply::Tool, Reply::Text("Report: pricing and edge cases covered.")]);
    let (orchestrator, runner, store) = scripted_orchestrator(script.clone());
    let task = orchestrator.spawn(background_child("MARKET-7")).await.expect("spawned").task_id;
    script.started(0).await;

    let web = vec![types::provenance::ProvenanceClass::Web];
    for words in ["Also cover pricing.", "And the edge cases."] {
        let sent = orchestrator.send(&task, words, PARENT, web.clone(), None, None).await.expect("sent");
        assert!(matches!(sent, tools::FollowUp::Delivered { .. }), "a running child takes the message: {sent:?}");
    }
    assert_eq!(script.calls(), 1, "delivering does not start a second run beside the first");

    script.answer(2);
    let report = report_holding(&store, &task, "pricing and edge cases").await;
    assert!(report.contains("Report: pricing and edge cases covered."), "{report}");

    let second = script.seen(1);
    let pricing = second.iter().position(|m| m.contains("Also cover pricing.")).expect("the next step read the first message");
    let edges = second.iter().position(|m| m.contains("And the edge cases.")).expect("the next step read the second message");
    assert!(pricing < edges, "in the order sent");
    assert!(second[pricing].starts_with("The employee who gave you this task sent you a message"), "{}", second[pricing]);
    assert!(!script.seen(0).iter().any(|m| m.contains("Also cover pricing.")), "the first step was already in flight");

    let thread = child_thread(&runner, &task);
    let rows: Vec<_> = thread.iter().filter(|m| m.content == "Also cover pricing." || m.content == "And the edge cases.").collect();
    assert_eq!(rows.len(), 2, "both persist in the child's thread, as sent");
    for row in rows {
        let meta: serde_json::Value = serde_json::from_str(row.metadata.as_deref().unwrap()).unwrap();
        assert_eq!(meta["from"], "parent");
        assert_eq!(meta["parentSessionKey"], PARENT);
        assert_eq!(meta["taskId"], task.as_str());
        assert_eq!(meta["provenance"], json!(["web"]), "the parent's taint rides with its words");
    }

    // Finished: a message continues it on its own session, as before.
    script.answer(1);
    let sent = orchestrator.send(&task, "One more thing.", PARENT, vec![], None, None).await.expect("sent");
    assert!(matches!(sent, tools::FollowUp::Continued(ref r) if r.success), "a finished child continues: {sent:?}");
    report_holding(&store, &task, "done").await;
    let follow_up = child_thread(&runner, &task).into_iter().find(|m| m.content.ends_with("One more thing.")).expect("the follow-up turn");
    assert!(follow_up.content.starts_with(agent::orchestrator::CONTINUATION_FRAME), "{}", follow_up.content);
}

/// A message that lands while the sub-agent's last step is writing its
/// report is not lost: the step it missed ends, the loop reads the message
/// before ending the turn, and the report the parent gets answers it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_that_lands_as_the_sub_agent_finishes_is_heard() {
    use tools::SubAgentOrchestrator as _;
    let script = Script::new("MARKET-8", [Reply::Text("First report."), Reply::Text("Revised: prices in euros.")]);
    let (orchestrator, runner, store) = scripted_orchestrator(script.clone());
    let task = orchestrator.spawn(background_child("MARKET-8")).await.expect("spawned").task_id;
    script.started(0).await;

    let sent = orchestrator.send(&task, "Prices in euros, please.", PARENT, vec![], None, None).await.expect("sent");
    assert!(matches!(sent, tools::FollowUp::Delivered { .. }), "{sent:?}");
    script.answer(2);

    let report = report_holding(&store, &task, "Revised").await;
    assert!(report.contains("First report.") && report.contains("Revised: prices in euros."), "{report}");
    assert!(script.seen(1).iter().any(|m| m.contains("Prices in euros, please.")), "the step after the report read it");
    let thread = child_thread(&runner, &task);
    let at = thread.iter().position(|m| m.content == "Prices in euros, please.").expect("persisted");
    assert!(thread[at + 1..].iter().any(|m| m.role == "assistant" && m.content.contains("Revised")), "answered after it");
}

/// Cancel still stops a sub-agent that has a message waiting for it: the run
/// ends, it takes no further model call, and it is no longer running.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sub_agent_with_a_message_waiting_can_still_be_cancelled() {
    use tools::SubAgentOrchestrator as _;
    let script = Script::new("MARKET-9", [Reply::Tool, Reply::Text("never")]);
    let (orchestrator, _runner, store) = scripted_orchestrator(script.clone());
    let task = orchestrator.spawn(background_child("MARKET-9")).await.expect("spawned").task_id;
    script.started(0).await;

    let sent = orchestrator.send(&task, "Stop at the summary.", PARENT, vec![], None, None).await.expect("sent");
    assert!(matches!(sent, tools::FollowUp::Delivered { .. }), "{sent:?}");
    orchestrator.cancel(&task).await.expect("cancelled");
    assert!(orchestrator.list_active().await.iter().all(|(id, _, _)| id != &task), "no longer running");

    script.answer(2);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(script.calls(), 1, "a cancelled child makes no further model call");
    let task_row = store.get_pending_task(&task).unwrap().expect("the task");
    assert_ne!(task_row.status, "completed", "a cancelled child does not complete");
}
