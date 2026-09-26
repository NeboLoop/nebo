use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{info, warn};

use super::fixture::{Fixture, Interrupt};
use super::trace::*;

/// Print what a turn opens with: the system prompt every turn sends, then
/// the identity row the fixture's employee is told, each with its size.
pub fn inspect_prompt(fixture: Option<&Fixture>) {
    use crate::harness::prompt::{Identity, Role, system_prompt};

    let name = fixture.map(|f| f.target_component.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| "Nebo".to_string());
    let identity = Identity { name, role: Role::Employee, personality_snippet: None, soul: None, rules: None, persona: None }.text();
    let system = system_prompt();
    println!("=== SYSTEM PROMPT (chars: {}) ===", system.chars().count());
    println!("{system}");
    println!("\n=== IDENTITY ROW (chars: {}) ===", identity.chars().count());
    println!("{identity}");
    let total = system.chars().count() + identity.chars().count();
    println!("\n--- Total: {} chars (~{} tokens) ---", total, total / crate::CHARS_PER_TOKEN);
}

/// Run a fixture live against a running Nebo server.
pub async fn run_live(
    fixture: &Fixture,
    server: &str,
    model: Option<&str>,
    runs: usize,
) -> Result<Vec<Trace>, String> {
    let ws_url = format!("ws://{}/ws", server);

    // Quick connectivity check
    match connect_async(&ws_url).await {
        Ok(_) => {}
        Err(e) => {
            return Err(format!(
                "Cannot connect to Nebo at {}. Is `make dev` running?\nError: {}",
                ws_url, e
            ));
        }
    }

    let mut traces = Vec::new();
    for run_idx in 0..runs {
        let run_id = format!("run-{}", run_idx + 1);

        // Run setup commands before each run
        for cmd in &fixture.setup {
            info!(fixture = %fixture.id, run = %run_id, cmd = %cmd, "running setup");
            // Setup and teardown address the server under test as
            // `${NEBO_TEST_SERVER:-localhost:27895}`, so a fixture runs against
            // any port the runner was pointed at, not only the dev server.
            let output = std::process::Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .env("NEBO_TEST_SERVER", server)
                .output()
                .map_err(|e| format!("setup command failed: {}", e))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!("setup command failed: {} — {}", cmd, stderr));
            }
        }

        info!(fixture = %fixture.id, run = %run_id, "starting live test run");

        let result = match with_agent_id(fixture, server).await {
            Ok(bound) => run_single(&ws_url, &bound, &run_id, model).await,
            Err(e) => Err(e),
        };

        // Run teardown commands after each run (even if the run failed)
        for cmd in &fixture.teardown {
            info!(fixture = %fixture.id, run = %run_id, cmd = %cmd, "running teardown");
            let _ = std::process::Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .env("NEBO_TEST_SERVER", server)
                .output();
        }

        match result {
            Ok(trace) => traces.push(trace),
            Err(e) => {
                warn!(fixture = %fixture.id, run = %run_id, error = %e, "run failed");
                return Err(format!("run {} failed: {}", run_id, e));
            }
        }

        // Brief pause between runs to avoid overwhelming the server
        if run_idx + 1 < runs {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    Ok(traces)
}

async fn run_single(
    ws_url: &str,
    fixture: &Fixture,
    run_id: &str,
    model: Option<&str>,
) -> Result<Trace, String> {
    let start = Instant::now();

    // Connect
    let (mut ws, _) = connect_async(ws_url)
        .await
        .map_err(|e| format!("WS connect: {}", e))?;

    // Wait for connected event
    let deadline = Duration::from_secs(5);
    loop {
        match timeout(deadline, ws.next()).await {
            Ok(Some(Ok(msg))) => {
                if let Ok(text) = msg.to_text() {
                    if let Ok(event) = serde_json::from_str::<Value>(text) {
                        if event["type"].as_str() == Some("connected") {
                            break;
                        }
                    }
                }
            }
            _ => return Err("Timeout waiting for connected event".into()),
        }
    }

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    // With an agent, the id carries the agent prefix: the server keeps a
    // client session id only when it already does, and replaces anything
    // else with the agent's main session, whose events would never be ours.
    let session_id = match fixture.agent.as_deref() {
        Some(agent) => format!(
            "{}eval:{}:{}:{}",
            types::keyparser::agent_session_prefix(agent),
            fixture.id,
            run_id,
            ts
        ),
        None => format!("eval:{}:{}:{}", fixture.id, run_id, ts),
    };

    let mut rec = Recorder::default();

    // Send each conversation turn
    let user_turns: Vec<_> = fixture
        .conversation
        .iter()
        .filter(|t| t.role == "user")
        .collect();

    for (turn_idx, turn) in user_turns.iter().enumerate() {
        let mut msg_data = json!({
            "session_id": session_id,
            "prompt": turn.content,
            "user_id": "eval",
            "channel": "web",
        });

        if let Some(model) = model {
            msg_data["model_override"] = json!(model);
        }
        if let Some(cwd) = fixture.cwd.as_deref() {
            msg_data["cwd"] = json!(cwd);
        }
        if let Some(agent) = fixture.agent.as_deref() {
            msg_data["agent_id"] = json!(agent);
        }

        // message_id must be unique per turn — the server's idempotency check
        // silently drops duplicates, which killed every multi-turn fixture
        // (turn 2 reused turn 1's id and the run timed out waiting).
        let msg = json!({
            "type": "chat",
            "message_id": format!("eval-{}-{}-t{}", fixture.id, run_id, turn_idx),
            "data": msg_data.clone(),
        });
        // Tool calls started in this turn; the fixture's interrupts fire on it.
        let mut tool_starts = 0usize;
        rec.open_turn(false);

        ws.send(Message::Text(msg.to_string().into()))
            .await
            .map_err(|e| format!("WS send: {}", e))?;

        // Collect events until done. This is a silence cap, not a run cap:
        // it resets on this run's own progress — reply text or tool activity,
        // the same two things the server's stall guard watches. It must
        // outlast the slowest single tool execution (web navigation, plugin
        // exec) — efficiency is judged by cost assertions, not by killing the
        // run mid-tool.
        //
        // Progress, not traffic: the socket also carries keepalives and every
        // other client's broadcasts, and a cap that reset on those never
        // fired. A silent run then sat until the server's own 15-minute stall
        // guard ended it, and two such runs of one smoke fixture spent 30
        // minutes of a 60-minute gate job doing nothing (2026-09-20).
        let turn_timeout = Duration::from_secs(180);
        let mut last_progress = Instant::now();

        loop {
            let silence_left = turn_timeout.saturating_sub(last_progress.elapsed());
            match timeout(silence_left, ws.next()).await {
                Ok(Some(Ok(msg))) => {
                    let Some(event) = session_event(&msg, &session_id) else { continue };
                    if is_progress(event["type"].as_str()) {
                        last_progress = Instant::now();
                    }
                    match rec.on_event(&event) {
                        Step::Continue => {}
                        Step::ToolStarted => {
                            tool_starts += 1;
                            for (n, it) in fixture.interrupts.iter().enumerate() {
                                if it.turn == turn_idx + 1 && it.after_tool_calls == tool_starts {
                                    send_interrupt(&mut ws, it, &session_id, &msg_data, &fixture.id, run_id, n).await?;
                                }
                            }
                        }
                        Step::Reply(replies) => send_replies(&mut ws, replies, &fixture.id, run_id).await,
                        Step::TurnEnded => break,
                        // A run the server stopped (a step or spending
                        // limit, a provider error) still has a story: the
                        // recorder keeps the calls made so far and the
                        // reason, so the stop can be read and turned into a
                        // fixture.
                        Step::Stopped(err) => {
                            warn!(fixture = %fixture.id, run = %run_id, error = %err, "run stopped by the server; keeping the partial trace");
                            cancel_run(&mut ws, &session_id).await;
                            break;
                        }
                    }
                }
                Ok(Some(Err(e))) => {
                    cancel_run(&mut ws, &session_id).await;
                    return Err(format!("WS error: {}", e));
                }
                Ok(None) => {
                    // The socket closed mid-turn: the turn has no ending.
                    rec.close_turn(Some(""));
                    break;
                }
                Err(_) => {
                    cancel_run(&mut ws, &session_id).await;
                    return Err(format!(
                        "no reply text and no tool activity for {}s — the run was ended \
                         instead of left hanging",
                        turn_timeout.as_secs()
                    ));
                }
            }
        }
        rec.close_turn(None);
    }

    settle(&mut ws, &mut rec, &session_id, &fixture.id, run_id).await;

    // An empty run — no text, no tool calls — is a failure, not a pass. Silent
    // empties (transient provider errors that still emit chat_complete) used to
    // produce valid-looking zero-call traces that skewed results.
    if rec.text.join("").trim().is_empty() && rec.calls.iter().all(Option::is_none) {
        cancel_run(&mut ws, &session_id).await;
        return Err(
            "Empty run: chat completed with no response text and no tool calls \
             (likely a transient provider error) — treat as failed and re-run"
                .to_string(),
        );
    }

    let total_latency = start.elapsed().as_millis() as u64;
    Ok(rec.into_trace(&fixture.id, run_id, model, session_id, total_latency))
}

/// How long the runner waits, after the owner's last turn, for the work the
/// run started in the background to report. Helpers run in the background
/// by default and their results arrive as turns the session starts on its
/// own; a run that closed at the last `chat_complete` never saw them
/// (2026-09-25: agent-spawn-parallel's two summaries, both in a woken turn,
/// were graded as missing on every run). Bounded, so a helper that never
/// ends cannot hold the gate.
const SETTLE_CAP: Duration = Duration::from_secs(300);

/// After a helper reports, how long the runner still listens for the turn
/// that hears it. A report that lands after a turn's last step gets a turn of
/// its own, which starts a moment after the completion event.
const WAKE_GRACE: Duration = Duration::from_secs(10);

/// What one event did to the run, for the socket loop to act on.
#[derive(Debug, PartialEq)]
enum Step {
    Continue,
    /// A tool call started (the fixture's interrupts count these).
    ToolStarted,
    /// Answers to send back: a declined card, a granted approval.
    Reply(Vec<Value>),
    /// The turn ended (`chat_complete`).
    TurnEnded,
    /// The server stopped the turn (`chat_error`), with its reason.
    Stopped(String),
}

/// A tool call that started and has not returned: its place in the run's
/// calls, found again by the `tool_id` its result carries.
struct OpenCall {
    tool_id: String,
    slot: usize,
    tool: String,
    arguments: Value,
    started: Instant,
}

struct OpenTurn {
    metrics: TurnMetrics,
    started: Instant,
    cancelled: bool,
}

/// Everything one fixture run records, event by event, across the owner's
/// turns and the turns the session starts on its own afterwards.
#[derive(Default)]
struct Recorder {
    /// One slot per started call, in the order the model made them; filled
    /// when the call's result comes back. Parallel calls all start before any
    /// returns, and their results come back in any order: each result goes
    /// to the call with its `tool_id` (2026-09-25: one pending slot turned two
    /// parallel delegates into one call carrying the other's result).
    calls: Vec<Option<TracedToolCall>>,
    open_calls: Vec<OpenCall>,
    text: Vec<String>,
    turns: Vec<TurnMetrics>,
    turn: Option<OpenTurn>,
    input_tokens: usize,
    output_tokens: usize,
    cache_read_tokens: usize,
    cache_creation_tokens: usize,
    /// The session's helpers that started and have not finished
    /// (`subagent_start` / `subagent_complete`, by task id).
    helpers_running: std::collections::HashSet<String>,
    helpers_finished: usize,
    /// Calls that started work which reports later: a background helper or
    /// command, in either arm's vocabulary.
    background_launches: usize,
    woken_turns: usize,
    /// A helper finished and no turn of the session has ended since, so its
    /// report is not heard yet.
    unheard: bool,
    /// When the last helper finished, while no woken turn has ended since:
    /// a turn of the owner's that ended after it may not have heard it, and
    /// the turn that does can still be on its way (see [`WAKE_GRACE`]).
    unwoken_finish: Option<Instant>,
}

impl Recorder {
    fn open_turn(&mut self, woken: bool) {
        let n = self.turns.len() + 1;
        // The reply text is one transcript; a woken turn's part says where
        // it starts, so a reader can tell it from the owner's last turn.
        if woken {
            self.text.push("\n\n[a later turn: the session woke on its own when its background work reported]\n".to_string());
        }
        self.turn = Some(OpenTurn {
            metrics: TurnMetrics { turn: n, woken, ..TurnMetrics::default() },
            started: Instant::now(),
            cancelled: false,
        });
    }

    /// The turn this event belongs to. After the owner's last turn, the
    /// session's own activity opens a woken turn.
    fn active_turn(&mut self) -> &mut OpenTurn {
        if self.turn.is_none() {
            self.open_turn(true);
        }
        self.turn.as_mut().expect("a turn is open")
    }

    /// Close the open turn, if any, as `end` (default: how its events ended
    /// it, else `complete`).
    fn close_turn(&mut self, end: Option<&str>) {
        let Some(mut t) = self.turn.take() else { return };
        if let Some(end) = end {
            t.metrics.end = end.to_string();
        } else if t.metrics.end.is_empty() {
            t.metrics.end = if t.cancelled { "cancelled" } else { "complete" }.to_string();
        }
        t.metrics.latency_ms = t.started.elapsed().as_millis() as u64;
        if t.metrics.woken {
            self.woken_turns += 1;
            self.unwoken_finish = None;
        }
        self.turns.push(t.metrics);
    }

    fn on_event(&mut self, event: &Value) -> Step {
        let data = &event["data"];
        match event["type"].as_str() {
            Some("chat_stream") => {
                if let Some(t) = data["content"].as_str().or_else(|| data["text"].as_str()) {
                    let turn = self.active_turn();
                    if turn.metrics.first_reply_ms.is_none() && !t.trim().is_empty() {
                        turn.metrics.first_reply_ms = Some(turn.started.elapsed().as_millis() as u64);
                    }
                    self.text.push(t.to_string());
                }
                Step::Continue
            }
            Some("tool_start") => {
                self.active_turn();
                let tool = data["tool"].as_str().or_else(|| data["name"].as_str()).unwrap_or("unknown").to_string();
                self.open_calls.push(OpenCall {
                    tool_id: data["tool_id"].as_str().unwrap_or("").to_string(),
                    slot: self.calls.len(),
                    tool,
                    arguments: data["input"].clone(),
                    started: Instant::now(),
                });
                self.calls.push(None);
                Step::ToolStarted
            }
            Some("tool_result") => {
                // By id; a result without one answers the oldest open call.
                let id = data["tool_id"].as_str().unwrap_or("");
                let at = if id.is_empty() {
                    (!self.open_calls.is_empty()).then_some(0)
                } else {
                    self.open_calls.iter().position(|c| c.tool_id == id)
                };
                let Some(at) = at else { return Step::Continue };
                let call = self.open_calls.remove(at);
                let content = data["result"].as_str().or_else(|| data["content"].as_str()).unwrap_or("").to_string();
                let is_error = data["is_error"].as_bool().unwrap_or(false);
                if !is_error && launches_background_work(&call.tool, &call.arguments) {
                    self.background_launches += 1;
                }
                let turn = self.active_turn();
                turn.metrics.tool_calls += 1;
                turn.metrics.tool_errors += usize::from(is_error);
                self.calls[call.slot] = Some(TracedToolCall {
                    sequence: 0,
                    tool: call.tool,
                    arguments: call.arguments,
                    response: TracedToolResponse { char_count: content.len(), content, is_error },
                    latency_ms: call.started.elapsed().as_millis() as u64,
                });
                Step::Continue
            }
            Some("usage") => {
                let n = |k: &str| data[k].as_u64().unwrap_or(0) as usize;
                let (input, output) = (n("input_tokens"), n("output_tokens"));
                let (read, created) = (n("cache_read_input_tokens"), n("cache_creation_input_tokens"));
                self.input_tokens += input;
                self.output_tokens += output;
                self.cache_read_tokens += read;
                self.cache_creation_tokens += created;
                if let Some(t) = self.turn.as_mut() {
                    let tm = &mut t.metrics;
                    tm.model_calls += 1;
                    tm.input_tokens += input;
                    tm.output_tokens += output;
                    tm.cache_read_tokens += read;
                    tm.cache_creation_tokens += created;
                    tm.max_prompt_tokens = tm.max_prompt_tokens.max(input + read + created);
                }
                Step::Continue
            }
            Some("subagent_start") => {
                if let Some(id) = data["task_id"].as_str() {
                    self.helpers_running.insert(id.to_string());
                }
                Step::Continue
            }
            Some("subagent_complete") => {
                if let Some(id) = data["task_id"].as_str() {
                    self.helpers_running.remove(id);
                }
                self.helpers_finished += 1;
                self.unheard = true;
                self.unwoken_finish = Some(Instant::now());
                Step::Continue
            }
            // A parked question (an install card, a connect card, a plan to
            // approve). Nobody is at this keyboard: the card itself is the
            // observable outcome, so it is recorded in the transcript and
            // answered as an owner who declined, and the run continues. Left
            // unanswered, the run sat parked until the 15-minute idle guard
            // ended it — two of three skill-plugin-choreography runs, every
            // gate run, 2026-09-20.
            Some("ask_request") => {
                let (note, reply) = decline_card(event);
                self.active_turn().metrics.cards += 1;
                self.text.push(note);
                Step::Reply(vec![reply])
            }
            // A tool approval card. The harness stands in for the owner and
            // the fixture asked for the work, so the call is approved; the
            // transcript records it so a check can see what was approved.
            // Unanswered, a desktop fixture sat parked until the silence cap
            // (Stadium, 2026-09-23).
            Some("approval_request") => {
                let mut replies = Vec::new();
                for (note, reply) in approve_calls(event) {
                    self.active_turn().metrics.approvals += 1;
                    self.text.push(note);
                    replies.push(reply);
                }
                Step::Reply(replies)
            }
            Some("chat_complete") => {
                // A message typed mid-turn gets its own short stream that
                // ends at once with the typed "queued" stop; the real turn is
                // still running.
                if data["stop_reason"].as_str() == Some("queued_into_running_turn") {
                    return Step::Continue;
                }
                self.close_turn(None);
                self.unheard = false;
                Step::TurnEnded
            }
            // The stop landed. The cancelled turn still closes with its own
            // chat_complete a moment later; wait for it, or it ends the NEXT
            // turn's collection instead.
            Some("chat_cancelled") => {
                if let Some(t) = self.turn.as_mut() {
                    t.cancelled = true;
                }
                Step::Continue
            }
            // The queued message's status line ("still on the last
            // thing…"): the real turn is still running.
            Some("chat_error") if data["stop_reason"].as_str() == Some("queued_into_running_turn") => Step::Continue,
            Some("chat_error") => {
                let err = data["error"].as_str().unwrap_or("unknown error").to_string();
                self.text.insert(0, format!("[run stopped: {err}]\n"));
                self.close_turn(Some("error"));
                self.unheard = false;
                Step::Stopped(err)
            }
            _ => Step::Continue,
        }
    }

    /// Nothing the run started is still out: no turn running, no helper
    /// running, every background launch accounted for by a finished helper
    /// or a turn the session woke for, and every finished helper heard.
    fn settled(&self) -> bool {
        self.turn.is_none()
            && self.helpers_running.is_empty()
            && self.background_launches <= self.helpers_finished.max(self.woken_turns)
            && !self.unheard
    }

    /// What is still out, for the transcript note when the wait runs out.
    fn outstanding(&self) -> String {
        let mut parts = Vec::new();
        if self.turn.is_some() {
            parts.push("a turn still running".to_string());
        }
        if !self.helpers_running.is_empty() {
            parts.push(format!("{} helper(s) still running", self.helpers_running.len()));
        }
        let heard = self.helpers_finished.max(self.woken_turns);
        if self.background_launches > heard {
            parts.push(format!("{} background launch(es) not reported back", self.background_launches - heard));
        }
        if self.unheard {
            parts.push("a finished helper's report not yet heard".to_string());
        }
        parts.join(", ")
    }

    fn into_trace(self, fixture_id: &str, run_id: &str, model: Option<&str>, session_id: String, total_latency_ms: u64) -> Trace {
        let tool_calls: Vec<TracedToolCall> = self
            .calls
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(i, mut c)| {
                c.sequence = i + 1;
                c
            })
            .collect();
        Trace {
            fixture_id: fixture_id.to_string(),
            run_id: run_id.to_string(),
            // No stream event names the model the server used (`usage` carries
            // tokens, `chat_complete` artifacts), so this is the requested model.
            model: model.unwrap_or("default").to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            overrides: Vec::new(),
            final_response: TracedResponse { content: self.text.join(""), tokens: self.output_tokens },
            metrics: TraceMetrics {
                total_tool_calls: tool_calls.len(),
                total_tokens: self.input_tokens + self.output_tokens,
                input_tokens: self.input_tokens,
                output_tokens: self.output_tokens,
                total_latency_ms,
                cache_read_tokens: self.cache_read_tokens,
                cache_creation_tokens: self.cache_creation_tokens,
            },
            tool_calls,
            grade: None,
            failure_reason: None,
            session_id,
            turns: self.turns,
        }
    }
}

/// Whether a call starts work that reports in a later turn: a helper in the
/// background or a background command, in either arm's vocabulary. The
/// rewrite's `delegate` runs in the background unless told not to; the old
/// loop's `agent` spawn waits unless told not to.
fn launches_background_work(tool: &str, args: &Value) -> bool {
    let flag = |k: &str| args.get(k).and_then(Value::as_bool);
    let action = args.get("action").and_then(Value::as_str);
    match tool {
        "delegate" => flag("background").unwrap_or(true),
        "agent" => action == Some("spawn") && flag("wait") == Some(false),
        "run_command" => flag("background") == Some(true),
        "os" => action == Some("exec") && flag("background") == Some(true),
        _ => false,
    }
}

/// After the owner's last turn: keep recording until everything the run
/// started in the background has reported and been heard, or [`SETTLE_CAP`]
/// has passed. A run that started nothing ends here at once.
async fn settle(ws: &mut Ws, rec: &mut Recorder, session_id: &str, fixture_id: &str, run_id: &str) {
    let deadline = Instant::now() + SETTLE_CAP;
    loop {
        let now = Instant::now();
        let until = if rec.settled() {
            match rec.unwoken_finish.map(|t| t + WAKE_GRACE) {
                Some(grace) if grace > now => grace,
                _ => return,
            }
        } else {
            deadline
        };
        if now >= deadline {
            let outstanding = rec.outstanding();
            warn!(fixture = %fixture_id, run = %run_id, %outstanding, "stopped waiting for the run's background work");
            rec.text.push(format!(
                "\n[the harness stopped waiting after {} s: {outstanding}]\n",
                SETTLE_CAP.as_secs()
            ));
            rec.close_turn(Some("unfinished"));
            cancel_run(ws, session_id).await;
            return;
        }
        match timeout(until.min(deadline) - now, ws.next()).await {
            Ok(Some(Ok(msg))) => {
                let Some(event) = session_event(&msg, session_id) else { continue };
                match rec.on_event(&event) {
                    Step::Reply(replies) => send_replies(ws, replies, fixture_id, run_id).await,
                    Step::Stopped(err) => {
                        warn!(fixture = %fixture_id, run = %run_id, error = %err, "a woken turn was stopped by the server")
                    }
                    Step::Continue | Step::ToolStarted | Step::TurnEnded => {}
                }
            }
            Ok(Some(Err(e))) => {
                warn!(fixture = %fixture_id, run = %run_id, error = %e, "the socket failed while waiting for background work");
                rec.close_turn(Some("unfinished"));
                return;
            }
            Ok(None) => {
                rec.close_turn(Some("unfinished"));
                return;
            }
            Err(_) => {}
        }
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The event in a socket frame, when it is this run's (see
/// [`event_belongs_to_session`]).
fn session_event(msg: &Message, session_id: &str) -> Option<Value> {
    let event: Value = serde_json::from_str(msg.to_text().ok()?).ok()?;
    // Filter events by our session to avoid cross-talk. The hub broadcasts
    // to every client, and a scheduled workflow's completion carries
    // `chatId`, not `session_id`; it ended fixture runs early as "Empty run"
    // while the server kept running the fixture's turn.
    event_belongs_to_session(&event, session_id).then_some(event)
}

async fn send_replies(ws: &mut Ws, replies: Vec<Value>, fixture_id: &str, run_id: &str) {
    for reply in replies {
        if ws.send(Message::Text(reply.to_string().into())).await.is_err() {
            warn!(fixture = %fixture_id, run = %run_id, "could not answer a card or an approval; the run will stall");
        }
    }
}

/// The fixture with its employee named by id. A fixture that hires its own
/// employee in setup knows only the name it gave (`records-clerk-{{tag}}`),
/// and the chat payload takes an id, so the name is looked up once setup has
/// run. An id is kept as it is.
async fn with_agent_id(fixture: &Fixture, server: &str) -> Result<Fixture, String> {
    let mut bound = fixture.clone();
    let Some(agent) = fixture.agent.as_deref() else {
        return Ok(bound);
    };
    let url = format!("http://{server}/api/v1/agents");
    let list: Value = reqwest::get(&url)
        .await
        .map_err(|e| format!("list employees: {e}"))?
        .json()
        .await
        .map_err(|e| format!("list employees: {e}"))?;
    let id = agent_id_in(&list, agent).ok_or_else(|| format!("no employee with the id or name `{agent}`"))?;
    bound.agent = Some(id);
    Ok(bound)
}

/// The id of the employee in a `GET /agents` listing whose id, or failing
/// that whose name, is `agent`.
fn agent_id_in(list: &Value, agent: &str) -> Option<String> {
    let agents = list["agents"].as_array()?;
    agents
        .iter()
        .find(|a| a["id"].as_str() == Some(agent))
        .or_else(|| agents.iter().find(|a| a["name"].as_str() == Some(agent)))
        .and_then(|a| a["id"].as_str())
        .map(str::to_string)
}

/// Build experiment metadata from current git state.
pub fn build_experiment_metadata(name: &str, runs: usize) -> ExperimentMetadata {
    let git_commit = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let git_branch = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    ExperimentMetadata {
        name: name.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        git_commit,
        git_branch,
        strap_doc_hashes: compute_strap_hashes(),
        runs_per_fixture: runs,
    }
}

/// Save a complete experiment to disk (metadata.json + traces/ + result.json).
pub fn save_experiment(
    dir: &std::path::Path,
    result: &ExperimentResult,
    traces: &[Trace],
) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create dir: {}", e))?;

    let meta_path = dir.join("metadata.json");
    let meta_json = serde_json::to_string_pretty(&result.metadata)
        .map_err(|e| format!("serialize metadata: {}", e))?;
    std::fs::write(&meta_path, meta_json)
        .map_err(|e| format!("write {}: {}", meta_path.display(), e))?;

    let traces_dir = dir.join("traces");
    for trace in traces {
        trace.save(&traces_dir)?;
    }

    let result_path = dir.join("result.json");
    let result_json = serde_json::to_string_pretty(result)
        .map_err(|e| format!("serialize result: {}", e))?;
    std::fs::write(&result_path, result_json)
        .map_err(|e| format!("write {}: {}", result_path.display(), e))?;

    Ok(())
}

/// Does this event move the run on? Reply text or tool activity — the two
/// things the server's stall guard watches (`guardrails::stall_notice`) — plus
/// a card, which is this run's own session asking and which `decline_card`
/// answers on the spot, so the turn carries on. Everything else the socket
/// carries (keepalives, `usage`, presence, another client's broadcast) is
/// traffic, not progress, and must not hold the silence cap open.
fn is_progress(event_type: Option<&str>) -> bool {
    matches!(
        event_type,
        Some("chat_stream") | Some("tool_start") | Some("tool_result") | Some("ask_request")
    )
}

/// True when a hub event is ours: a terminal event (`chat_complete`,
/// `chat_error`) must name our session; any other event is ours unless it
/// names a different session (or a different chat via `chatId`).
fn event_belongs_to_session(event: &Value, session_id: &str) -> bool {
    let data = &event["data"];
    let named = data["session_id"].as_str().or_else(|| data["chatId"].as_str());
    match event["type"].as_str() {
        Some("chat_complete") | Some("chat_error") => named == Some(session_id),
        _ => named.is_none_or(|s| s == session_id),
    }
}

/// A card nobody will act on: the transcript line that records it, and the
/// `ask_response` that declines it so the tool returns and the turn goes on.
fn decline_card(event: &Value) -> (String, Value) {
    let data = &event["data"];
    let request_id = data["request_id"].as_str().unwrap_or("");
    let prompt = data["prompt"].as_str().unwrap_or("");
    let kinds: Vec<&str> = data["widgets"]
        .as_array()
        .map(|w| w.iter().filter_map(|x| x["type"].as_str()).collect())
        .unwrap_or_default();
    let note = format!(
        "[card shown: {}{} — declined by the harness, nobody is at this keyboard]\n",
        prompt.trim(),
        if kinds.is_empty() { String::new() } else { format!(" ({})", kinds.join(", ")) }
    );
    let reply = json!({ "type": "ask_response", "data": { "request_id": request_id, "value": "declined" } });
    (note, reply)
}

/// One `approval_response` per gated call on the card (a batch card lists
/// them under `batch`; a single card is its own request), each with the
/// transcript line that records it.
fn approve_calls(event: &Value) -> Vec<(String, Value)> {
    let data = &event["data"];
    let mut calls: Vec<(String, String)> = Vec::new();
    if let Some(batch) = data["batch"].as_array() {
        for c in batch {
            calls.push((c["id"].as_str().unwrap_or("").to_string(), c["tool"].as_str().unwrap_or("").to_string()));
        }
    }
    let first = data["request_id"].as_str().unwrap_or("");
    if !first.is_empty() && !calls.iter().any(|(id, _)| id == first) {
        calls.insert(0, (first.to_string(), data["tool"].as_str().unwrap_or("").to_string()));
    }
    calls
        .into_iter()
        .filter(|(id, _)| !id.is_empty())
        .map(|(id, tool)| {
            (
                format!("[approval granted by the harness: {tool} {id}]\n"),
                json!({ "type": "approval_response", "data": { "request_id": id, "approved": true } }),
            )
        })
        .collect()
}

/// What the owner does mid-turn, sent over the same socket the turn runs on.
async fn send_interrupt<S>(
    ws: &mut S,
    it: &Interrupt,
    session_id: &str,
    base: &Value,
    fixture_id: &str,
    run_id: &str,
    n: usize,
) -> Result<(), String>
where
    S: SinkExt<Message> + Unpin,
{
    let msg = match it.action.as_str() {
        "cancel" => json!({ "type": "cancel", "data": { "session_id": session_id } }),
        "message" => {
            let mut data = base.clone();
            data["prompt"] = json!(it.content);
            json!({
                "type": "chat",
                "message_id": format!("eval-{}-{}-i{}", fixture_id, run_id, n),
                "data": data,
            })
        }
        other => return Err(format!("unknown interrupt action `{other}` (cancel | message)")),
    };
    info!(fixture = %fixture_id, run = %run_id, action = %it.action, after_tool_calls = it.after_tool_calls, "interrupting the run");
    if ws.send(Message::Text(msg.to_string().into())).await.is_err() {
        return Err("WS send (interrupt): the socket is gone".to_string());
    }
    Ok(())
}

/// A fixture that gives up (timeout, empty run, error) must not leave its
/// turn running on the server, burning tokens against the next fixture.
async fn cancel_run<S>(ws: &mut S, session_id: &str)
where
    S: SinkExt<Message> + Unpin,
{
    let msg = json!({ "type": "cancel", "data": { "session_id": session_id } });
    // Best effort: the socket may already be gone, and the run's own caps end it then.
    if ws.send(Message::Text(msg.to_string().into())).await.is_err() {
        warn!(session_id, "could not send cancel for an abandoned fixture run");
    }
}

#[cfg(test)]
mod session_filter_tests {
    use super::*;

    /// A single approval card gets one approval; a batch card gets one per
    /// listed call, the leading request first and never twice.
    #[test]
    fn approvals_answer_every_gated_call_once() {
        let single = json!({"type":"approval_request","data":{"session_id":"s","request_id":"c1","tool":"os","input":{}}});
        let a = approve_calls(&single);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].1["data"]["request_id"], "c1");
        assert_eq!(a[0].1["data"]["approved"], true);
        assert!(a[0].0.contains("os c1"));
        let batch = json!({"type":"approval_request","data":{"request_id":"c1","tool":"os","batch":[{"id":"c1","tool":"os"},{"id":"c2","tool":"web"}]}});
        let b = approve_calls(&batch);
        assert_eq!(b.iter().map(|(_, r)| r["data"]["request_id"].as_str().unwrap().to_string()).collect::<Vec<_>>(), ["c1", "c2"]);
        assert!(approve_calls(&json!({"type":"approval_request","data":{}})).is_empty());
    }

    /// A card is answered with a decline addressed to its request id, and
    /// the transcript keeps what was offered so a check can see the offer.
    #[test]
    fn a_card_is_declined_and_recorded() {
        let event = json!({
            "type": "ask_request",
            "data": {
                "session_id": "eval:x:run-1",
                "request_id": "req-7",
                "prompt": "**X** can do this. Install it on the card and I'll pick up right where I left off.",
                "widgets": [{ "type": "install_plugin", "plugin": "x" }]
            }
        });
        let (note, reply) = decline_card(&event);
        assert_eq!(reply["type"], "ask_response");
        assert_eq!(reply["data"]["request_id"], "req-7");
        assert_eq!(reply["data"]["value"], "declined");
        assert!(note.contains("**X** can do this"), "{note}");
        assert!(note.contains("(install_plugin)"), "{note}");
        assert!(note.contains("declined by the harness"), "{note}");
    }

    #[test]
    fn a_foreign_completion_never_ends_our_run() {
        let mine = "eval:os-checkpoint:run-1:1";
        let workflow_done = json!({"type": "chat_complete", "data": {"chatId": "wf-daily", "content": "x"}});
        assert!(!event_belongs_to_session(&workflow_done, mine));
        let anonymous_done = json!({"type": "chat_complete", "data": {"content": "x"}});
        assert!(!event_belongs_to_session(&anonymous_done, mine));
        let ours = json!({"type": "chat_complete", "data": {"session_id": mine}});
        assert!(event_belongs_to_session(&ours, mine));
        // Non-terminal events without an id still flow (usage, presence).
        let usage = json!({"type": "usage", "data": {"input_tokens": 5}});
        assert!(event_belongs_to_session(&usage, mine));
        let other_stream = json!({"type": "chat_stream", "data": {"session_id": "someone-else", "content": "y"}});
        assert!(!event_belongs_to_session(&other_stream, mine));
    }

    #[test]
    fn only_reply_text_and_tool_activity_hold_the_silence_cap_open() {
        for moving in ["chat_stream", "tool_start", "tool_result", "ask_request"] {
            assert!(is_progress(Some(moving)), "{moving} is progress");
        }
        // Traffic that used to reset the cap and let a silent run sit for the
        // server's full 15-minute stall window.
        for traffic in ["connected", "usage", "presence", "chat_cancelled"] {
            assert!(!is_progress(Some(traffic)), "{traffic} is not progress");
        }
        assert!(!is_progress(None));
    }
}

#[cfg(test)]
mod agent_lookup_tests {
    use super::*;

    #[test]
    fn an_employee_is_found_by_id_then_by_name() {
        let list = json!({ "agents": [
            { "id": "a1", "name": "records-clerk-1f2e" },
            { "id": "records-clerk-1f2e", "name": "odd" },
        ]});
        assert_eq!(agent_id_in(&list, "a1").as_deref(), Some("a1"));
        assert_eq!(agent_id_in(&list, "records-clerk-1f2e").as_deref(), Some("records-clerk-1f2e"), "an id wins over a name");
        assert_eq!(agent_id_in(&list, "odd").as_deref(), Some("records-clerk-1f2e"));
        assert_eq!(agent_id_in(&list, "nobody"), None);
    }
}

/// The runner end to end against a scripted server: the real socket loop,
/// fed the event order a Nebo server produces, so what reaches the trace is
/// what the checks and the judge would read.
#[cfg(test)]
mod recording_tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use super::*;
    use tokio::net::TcpListener;

    /// One reply batch per chat message the runner sends: each event after
    /// its delay (ms). `$S` in any string is the run's session id.
    type Batch = Vec<(u64, Value)>;

    async fn scripted_server(batches: Vec<Batch>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr").to_string();
        let batches = Arc::new(Mutex::new(VecDeque::from(batches)));
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let batches = batches.clone();
                tokio::spawn(async move {
                    let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else { return };
                    let _ = ws.send(Message::Text(json!({"type": "connected"}).to_string().into())).await;
                    while let Some(Ok(msg)) = ws.next().await {
                        let Ok(text) = msg.to_text() else { continue };
                        let Ok(sent) = serde_json::from_str::<Value>(text) else { continue };
                        if sent["type"] != "chat" {
                            continue;
                        }
                        let session = sent["data"]["session_id"].as_str().unwrap_or("").to_string();
                        let Some(batch) = batches.lock().unwrap().pop_front() else { continue };
                        for (delay, event) in batch {
                            if delay > 0 {
                                tokio::time::sleep(Duration::from_millis(delay)).await;
                            }
                            let text = event.to_string().replace("$S", &session);
                            if ws.send(Message::Text(text.into())).await.is_err() {
                                return;
                            }
                        }
                    }
                });
            }
        });
        addr
    }

    fn fixture(turns: &[&str]) -> Fixture {
        let conversation: Vec<Value> = turns.iter().map(|t| json!({"role": "user", "content": t})).collect();
        serde_json::from_value(json!({"id": "recording", "name": "recording", "conversation": conversation}))
            .expect("fixture")
    }

    fn ev(kind: &str, data: Value) -> Value {
        let mut data = data;
        data["session_id"] = json!("$S");
        json!({"type": kind, "data": data})
    }

    fn start(id: &str, tool: &str, input: Value) -> (u64, Value) {
        (0, ev("tool_start", json!({"tool_id": id, "tool": tool, "input": input})))
    }

    fn result(id: &str, tool: &str, content: &str) -> (u64, Value) {
        (0, ev("tool_result", json!({"tool_id": id, "tool_name": tool, "result": content, "is_error": false})))
    }

    fn text(delay: u64, t: &str) -> (u64, Value) {
        (delay, ev("chat_stream", json!({"content": t})))
    }

    fn complete(delay: u64) -> (u64, Value) {
        (delay, ev("chat_complete", json!({})))
    }

    async fn record(turns: &[&str], batches: Vec<Batch>) -> Trace {
        let server = scripted_server(batches).await;
        let mut traces = run_live(&fixture(turns), &server, None, 1).await.expect("run");
        traces.pop().expect("one trace")
    }

    /// Two delegate calls and four reads, each started before any returns
    /// (the model's parallel calls), their results back in another order.
    /// Every call is recorded once, in the order the model made them, with
    /// its own arguments and its own result.
    #[tokio::test]
    async fn parallel_calls_are_each_recorded_with_their_own_result() {
        let mut turn = vec![
            start("d1", "delegate", json!({"description": "fees", "prompt": "Research TC fees.", "background": false})),
            start("d2", "delegate", json!({"description": "crm", "prompt": "Compare CRMs.", "background": false})),
            result("d2", "delegate", "Helper [h-crm] finished: CRM report"),
            result("d1", "delegate", "Helper [h-fees] finished: fees report"),
        ];
        for n in 1..=4 {
            turn.push(start(&format!("r{n}"), "read_file", json!({"path": format!("note{n}.txt")})));
        }
        for n in [3, 1, 4, 2] {
            turn.push(result(&format!("r{n}"), "read_file", &format!("contents of note {n}")));
        }
        turn.push(text(0, "Done."));
        turn.push(complete(0));
        let trace = record(&["go"], vec![turn]).await;

        let calls: Vec<(usize, &str, String, &str)> = trace
            .tool_calls
            .iter()
            .map(|c| {
                let arg = c.arguments["description"].as_str().or(c.arguments["path"].as_str()).unwrap_or("").to_string();
                (c.sequence, c.tool.as_str(), arg, c.response.content.as_str())
            })
            .collect();
        assert_eq!(
            calls,
            vec![
                (1, "delegate", "fees".to_string(), "Helper [h-fees] finished: fees report"),
                (2, "delegate", "crm".to_string(), "Helper [h-crm] finished: CRM report"),
                (3, "read_file", "note1.txt".to_string(), "contents of note 1"),
                (4, "read_file", "note2.txt".to_string(), "contents of note 2"),
                (5, "read_file", "note3.txt".to_string(), "contents of note 3"),
                (6, "read_file", "note4.txt".to_string(), "contents of note 4"),
            ]
        );
        assert_eq!(trace.metrics.total_tool_calls, 6);
        assert_eq!(trace.turns[0].tool_calls, 6);
    }

    /// A helper launched in the background reports in a later turn the
    /// session starts on its own. The runner keeps listening after the
    /// owner's last turn until the helper has finished and the session has
    /// heard it, and that turn's calls and reply are in the trace.
    #[tokio::test]
    async fn a_turn_woken_by_a_finished_helper_is_recorded() {
        let turn = vec![
            start("d1", "delegate", json!({"description": "read the notes", "prompt": "Read the notes."})),
            (0, ev("subagent_start", json!({"task_id": "h-1", "description": "read the notes"}))),
            result("d1", "delegate", "Helper h-1 is working in the background."),
            text(0, "It's running in the background."),
            complete(0),
            (800, ev("subagent_complete", json!({"task_id": "h-1", "description": "read the notes", "success": true}))),
            start("w1", "read_file", json!({"path": "summary.md"})),
            result("w1", "read_file", "The access code is 4417."),
            text(0, "The helper finished: the access code is 4417."),
            complete(0),
        ];
        let trace = record(&["have a helper read the notes"], vec![turn]).await;

        assert!(trace.final_response.content.contains("the access code is 4417"), "{}", trace.final_response.content);
        assert!(trace.final_response.content.contains("[a later turn: the session woke on its own"), "{}", trace.final_response.content);
        assert_eq!(trace.tool_calls.iter().map(|c| c.tool.as_str()).collect::<Vec<_>>(), ["delegate", "read_file"]);
        assert_eq!(trace.turns.len(), 2, "{:?}", trace.turns);
        assert!(!trace.turns[0].woken && trace.turns[1].woken, "{:?}", trace.turns);
        assert_eq!(trace.turns[1].tool_calls, 1);
    }

    /// A helper that finishes as the owner's turn is closing: that turn
    /// has made its last check for input, so the report gets a turn of its
    /// own a moment later. The runner still waits for it.
    #[tokio::test]
    async fn a_report_landing_as_the_turn_closes_is_still_heard() {
        let turn = vec![
            start("d1", "delegate", json!({"description": "count", "prompt": "Count the files."})),
            (0, ev("subagent_start", json!({"task_id": "h-2", "description": "count"}))),
            result("d1", "delegate", "Helper h-2 is working in the background."),
            (0, ev("subagent_complete", json!({"task_id": "h-2", "description": "count", "success": true}))),
            text(0, "Started a helper."),
            complete(0),
            text(1500, "The helper counted 12 files."),
            complete(0),
        ];
        let trace = record(&["count the files"], vec![turn]).await;
        assert!(trace.final_response.content.contains("12 files"), "{}", trace.final_response.content);
        assert!(trace.turns[1].woken);
    }

    /// The old loop says nothing on the socket when its background helper
    /// ends: its only sign is the turn that wakes the session. A launch
    /// (`agent` spawn with `wait: false`) is waited for until that turn.
    #[tokio::test]
    async fn a_background_launch_is_waited_for_until_a_turn_hears_it() {
        let turn = vec![
            start("s1", "agent", json!({"resource": "task", "action": "spawn", "prompt": "Research rates.", "wait": false})),
            result("s1", "agent", "Sub-agent spawned in background."),
            text(0, "Started it."),
            complete(0),
            text(1500, "The research finished: rates fell 0.1 points."),
            complete(0),
        ];
        let trace = record(&["kick it off"], vec![turn]).await;
        assert!(trace.final_response.content.contains("rates fell"), "{}", trace.final_response.content);
        assert_eq!(trace.turns.len(), 2);
    }

    /// With nothing started in the background the run ends at the owner's
    /// last turn: nothing to wait for, no time spent waiting.
    #[tokio::test]
    async fn a_run_with_no_background_work_ends_at_its_last_turn() {
        let turn = vec![
            start("r1", "read_file", json!({"path": "a.txt"})),
            result("r1", "read_file", "a"),
            text(0, "Read it."),
            complete(0),
            text(3000, "an unrelated later turn"),
            complete(0),
        ];
        let begun = Instant::now();
        let trace = record(&["read a.txt"], vec![turn]).await;
        assert!(begun.elapsed() < Duration::from_secs(2), "{:?}", begun.elapsed());
        assert!(!trace.final_response.content.contains("unrelated"));
        assert_eq!(trace.turns.len(), 1);
    }

    #[test]
    fn background_launches_in_either_vocabulary() {
        assert!(launches_background_work("delegate", &json!({"prompt": "x"})), "delegate runs in the background by default");
        assert!(!launches_background_work("delegate", &json!({"prompt": "x", "background": false})));
        assert!(launches_background_work("run_command", &json!({"command": "sleep 9", "background": true})));
        assert!(!launches_background_work("run_command", &json!({"command": "ls"})));
        assert!(launches_background_work("agent", &json!({"resource": "task", "action": "spawn", "wait": false})));
        assert!(!launches_background_work("agent", &json!({"resource": "task", "action": "spawn"})), "spawn waits by default");
        assert!(!launches_background_work("agent", &json!({"resource": "task", "action": "spawn_parallel"})));
        assert!(launches_background_work("os", &json!({"resource": "shell", "action": "exec", "command": "x", "background": true})));
        assert!(!launches_background_work("os", &json!({"resource": "shell", "action": "exec", "command": "x"})));
        assert!(!launches_background_work("read_file", &json!({"path": "x"})));
    }
}
