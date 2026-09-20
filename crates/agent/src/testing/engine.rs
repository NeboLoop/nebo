use std::collections::HashMap;
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{info, warn};

use super::fixture::{Fixture, Interrupt};
use super::trace::*;

/// Inspect the assembled prompt with optional overrides. Prints to stdout.
pub fn inspect_prompt(
    fixture: Option<&Fixture>,
    overrides: &HashMap<String, String>,
) {
    use crate::prompt;

    let mut pctx = prompt::PromptContext::default();
    if let Some(f) = fixture {
        pctx.agent_name = f.target_component.clone();
    }

    let static_system = prompt::build_static(&pctx);

    // Apply overrides to STRAP sections
    let final_prompt = if overrides.is_empty() {
        static_system.clone()
    } else {
        apply_overrides(&static_system, overrides)
    };

    // Print with section markers and sizes
    print_annotated_prompt(&final_prompt, overrides);

    let dctx = prompt::DynamicContext::default();
    let dynamic = prompt::build_dynamic_suffix(&dctx);

    if !dynamic.trim().is_empty() {
        println!("\n=== DYNAMIC SUFFIX (chars: {}) ===", dynamic.len());
        println!("{}", dynamic);
    }

    let total = final_prompt.len() + dynamic.len();
    // ~4 chars per token is a rough estimate
    println!("\n--- Total: {} chars (~{} tokens) ---", total, total / 4);
}

/// Run a fixture live against a running Nebo server.
pub async fn run_live(
    fixture: &Fixture,
    server: &str,
    model: Option<&str>,
    overrides: &HashMap<String, String>,
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

    // Build system override if overrides were provided.
    // STRAP docs are NOT in build_static() — they're built separately by
    // build_strap_section() and appended by the runner. To override a STRAP doc,
    // we must build the full prompt (static + strap), apply replacements, then send
    // it as a complete system prompt.
    let system_override = if overrides.is_empty() {
        None
    } else if let Some(full_system) = overrides.get("system") {
        // Full system prompt replacement — used for naming convention A/B tests
        Some(full_system.clone())
    } else {
        let pctx = crate::prompt::PromptContext::default();
        let static_system = crate::prompt::build_static(&pctx);
        let all_tool_names: Vec<String> = vec![
            "os".into(),
            "agent".into(),
            "web".into(),
            "event".into(),
            "loop".into(),
            "message".into(),
            "skill".into(),
        ];
        let strap_section =
            crate::prompt::build_strap_section(&all_tool_names, &[], &all_tool_names);
        let full = format!("{}\n\n{}", static_system, strap_section);
        Some(apply_overrides(&full, overrides))
    };

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

        let result =
            run_single(&ws_url, fixture, &run_id, model, system_override.as_deref()).await;

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
    system_override: Option<&str>,
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

    // Collect results across all conversation turns
    let mut all_tool_calls: Vec<TracedToolCall> = Vec::new();
    let mut all_text: Vec<String> = Vec::new();
    let mut tool_seq = 0usize;
    let mut total_input_tokens = 0usize;
    let mut total_cache_read = 0usize;
    let mut total_cache_creation = 0usize;
    let mut total_output_tokens = 0usize;

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
        if let Some(sys) = system_override {
            msg_data["system"] = json!(sys);
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

        ws.send(Message::Text(msg.to_string().into()))
            .await
            .map_err(|e| format!("WS send: {}", e))?;

        // Collect events until done. This is an inter-event silence cap, not a
        // run cap: it resets on every WS event. It must outlast the slowest
        // single tool execution (web navigation, plugin exec) — efficiency is
        // judged by cost assertions, not by killing the run mid-tool.
        let turn_timeout = Duration::from_secs(180);
        let mut pending_tool: Option<(String, Value, Instant)> = None;

        loop {
            match timeout(turn_timeout, ws.next()).await {
                Ok(Some(Ok(msg))) => {
                    let text = match msg.to_text() {
                        Ok(t) => t,
                        Err(_) => continue,
                    };
                    let event: Value = match serde_json::from_str(text) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    // Filter events by our session to avoid cross-talk. The
                    // hub broadcasts to every client, and a scheduled
                    // workflow's completion carries `chatId`, not
                    // `session_id`; it ended fixture runs early as "Empty run"
                    // while the server kept running the fixture's turn.
                    if !event_belongs_to_session(&event, &session_id) {
                        continue;
                    }

                    match event["type"].as_str() {
                        Some("chat_stream") => {
                            if let Some(t) = event["data"]["content"]
                                .as_str()
                                .or_else(|| event["data"]["text"].as_str())
                            {
                                all_text.push(t.to_string());
                            }
                        }
                        Some("tool_start") => {
                            let tool_name = event["data"]["tool"]
                                .as_str()
                                .or_else(|| event["data"]["name"].as_str())
                                .unwrap_or("unknown")
                                .to_string();
                            let args = event["data"]["input"].clone();
                            pending_tool = Some((tool_name, args, Instant::now()));
                            tool_starts += 1;
                            if turn_idx == 0 {
                                for (n, it) in fixture.interrupts.iter().enumerate() {
                                    if it.after_tool_calls == tool_starts {
                                        send_interrupt(&mut ws, it, &session_id, &msg_data, &fixture.id, run_id, n).await?;
                                    }
                                }
                            }
                        }
                        Some("tool_result") => {
                            if let Some((tool_name, args, tool_start)) = pending_tool.take() {
                                tool_seq += 1;
                                let content = event["data"]["result"]
                                    .as_str()
                                    .or_else(|| event["data"]["content"].as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let is_error = event["data"]["is_error"]
                                    .as_bool()
                                    .unwrap_or(false);
                                let char_count = content.len();

                                all_tool_calls.push(TracedToolCall {
                                    sequence: tool_seq,
                                    tool: tool_name,
                                    arguments: args,
                                    response: TracedToolResponse {
                                        content,
                                        is_error,
                                        char_count,
                                    },
                                    latency_ms: tool_start.elapsed().as_millis() as u64,
                                });
                            }
                        }
                        Some("usage") => {
                            if let Some(input) = event["data"]["input_tokens"].as_u64() {
                                total_input_tokens += input as usize;
                            }
                            if let Some(output) = event["data"]["output_tokens"].as_u64() {
                                total_output_tokens += output as usize;
                            }
                            if let Some(n) = event["data"]["cache_read_input_tokens"].as_u64() {
                                total_cache_read += n as usize;
                            }
                            if let Some(n) = event["data"]["cache_creation_input_tokens"].as_u64() {
                                total_cache_creation += n as usize;
                            }
                        }
                        // A parked question (an install card, a connect card, a
                        // plan to approve). Nobody is at this keyboard: the card
                        // itself is the observable outcome, so it is recorded in
                        // the transcript and answered as an owner who declined,
                        // and the run continues. Left unanswered, the run sat
                        // parked until the 15-minute idle guard ended it — two of
                        // three skill-plugin-choreography runs, every gate run,
                        // 2026-09-20.
                        Some("ask_request") => {
                            let (note, reply) = decline_card(&event);
                            all_text.push(note);
                            if ws.send(Message::Text(reply.to_string().into())).await.is_err() {
                                warn!(fixture = %fixture.id, run = %run_id, "could not answer a card; the run will stall");
                            }
                        }
                        Some("chat_complete") => {
                            // A message typed mid-turn gets its own short stream
                            // that ends at once with the typed "queued" stop;
                            // the real turn is still running.
                            if event["data"]["stop_reason"].as_str() == Some("queued_into_running_turn") {
                                continue;
                            }
                            break;
                        }
                        // The stop landed. The cancelled turn still closes with
                        // its own chat_complete a moment later; wait for it, or
                        // it ends the NEXT turn's collection instead.
                        Some("chat_cancelled") => {
                            info!(fixture = %fixture.id, run = %run_id, "run cancelled by the fixture's interrupt");
                            continue;
                        }
                        Some("chat_error")
                            if event["data"]["stop_reason"].as_str() == Some("queued_into_running_turn") =>
                        {
                            // The queued message's status line ("still on the
                            // last thing…"): the real turn is still running.
                            continue;
                        }
                        Some("chat_error") => {
                            // A run the server stopped (a spiral guard, a
                            // provider error) still has a story: keep the
                            // calls made so far and the reason, so the stop
                            // can be read and turned into a fixture. Two
                            // SWE-bench runs on 2026-09-06 ended in the
                            // identical-call stop with nothing on disk.
                            let err = event["data"]["error"]
                                .as_str()
                                .unwrap_or("unknown error");
                            warn!(fixture = %fixture.id, run = %run_id, error = %err, "run stopped by the server; keeping the partial trace");
                            cancel_run(&mut ws, &session_id).await;
                            all_text.insert(0, format!("[run stopped: {err}]\n"));
                            break;
                        }
                        _ => {}
                    }
                }
                Ok(Some(Err(e))) => {
                    cancel_run(&mut ws, &session_id).await;
                    return Err(format!("WS error: {}", e));
                }
                Ok(None) => break,
                Err(_) => {
                    cancel_run(&mut ws, &session_id).await;
                    return Err("Timeout waiting for response".into());
                }
            }
        }
    }

    let final_text = all_text.join("");

    // An empty run — no text, no tool calls — is a failure, not a pass. Silent
    // empties (transient provider errors that still emit chat_complete) used to
    // produce valid-looking zero-call traces that skewed results.
    if final_text.trim().is_empty() && all_tool_calls.is_empty() {
        cancel_run(&mut ws, &session_id).await;
        return Err(
            "Empty run: chat completed with no response text and no tool calls \
             (likely a transient provider error) — treat as failed and re-run"
                .to_string(),
        );
    }

    let total_latency = start.elapsed().as_millis() as u64;
    let total_tokens = total_input_tokens + total_output_tokens;

    let now = chrono::Utc::now().to_rfc3339();

    Ok(Trace {
        fixture_id: fixture.id.clone(),
        run_id: run_id.to_string(),
        // No stream event names the model the server used (`usage` carries
        // tokens, `chat_complete` artifacts), so this is the requested model.
        model: model.unwrap_or("default").to_string(),
        timestamp: now,
        overrides: Vec::new(),
        tool_calls: all_tool_calls,
        final_response: TracedResponse {
            content: final_text,
            tokens: total_output_tokens,
        },
        metrics: TraceMetrics {
            total_tool_calls: tool_seq,
            total_tokens,
            input_tokens: total_input_tokens,
            output_tokens: total_output_tokens,
            total_latency_ms: total_latency,
            cache_read_tokens: total_cache_read,
            cache_creation_tokens: total_cache_creation,
        },
        grade: None,
    })
}

/// Apply prompt overrides by replacing STRAP tool doc sections.
fn apply_overrides(prompt: &str, overrides: &HashMap<String, String>) -> String {
    let mut result = prompt.to_string();
    for (component, replacement) in overrides {
        if let Some(tool_name) = component.strip_prefix("tool.") {
            if let Some(original_doc) = crate::prompt::strap_tool_doc(tool_name) {
                result = result.replace(original_doc, replacement);
            }
        }
    }
    result
}

/// Parse override args like "tool.shell:./overrides/shell-v2.md" into a map.
pub fn parse_overrides(args: &[String]) -> Result<HashMap<String, String>, String> {
    let mut map = HashMap::new();
    for arg in args {
        let (component, path) = arg
            .split_once(':')
            .ok_or_else(|| format!("invalid override format '{}', expected 'component:path'", arg))?;
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("read override {}: {}", path, e))?;
        map.insert(component.to_string(), content);
    }
    Ok(map)
}

/// Build experiment metadata from current git state and overrides.
pub fn build_experiment_metadata(
    name: &str,
    overrides: &HashMap<String, String>,
    runs: usize,
) -> ExperimentMetadata {
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
        overrides: overrides.keys().cloned().collect(),
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

fn print_annotated_prompt(prompt: &str, overrides: &HashMap<String, String>) {
    // Split by known section markers and annotate
    let sections = vec![
        ("STRAP", "## Tool Guide"),
        ("IDENTITY", "You are"),
        ("BEHAVIOR", "## Behavior"),
        ("MEMORY", "## Memory"),
        ("ETIQUETTE", "## Etiquette"),
    ];

    let mut printed_header = false;
    for line in prompt.lines() {
        // Check if this line starts a known section
        for (label, marker) in &sections {
            if line.contains(marker) && !printed_header {
                let overridden = overrides.keys().any(|k| {
                    k.starts_with(&format!("tool.")) && line.contains("Tool Guide")
                        || k == &label.to_lowercase()
                });
                let suffix = if overridden { " [OVERRIDDEN]" } else { "" };
                println!("\n=== {} (starts here){} ===", label, suffix);
                printed_header = true;
            }
        }
        println!("{}", line);
        printed_header = false;
    }
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
}
