use std::collections::HashMap;
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{info, warn};

use super::fixture::Fixture;
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
            let output = std::process::Command::new("sh")
                .arg("-c")
                .arg(cmd)
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
    let session_id = format!("eval:{}:{}:{}", fixture.id, run_id, ts);

    // Collect results across all conversation turns
    let mut all_tool_calls: Vec<TracedToolCall> = Vec::new();
    let mut all_text: Vec<String> = Vec::new();
    let mut tool_seq = 0usize;
    let mut total_input_tokens = 0usize;
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

        // message_id must be unique per turn — the server's idempotency check
        // silently drops duplicates, which killed every multi-turn fixture
        // (turn 2 reused turn 1's id and the run timed out waiting).
        let msg = json!({
            "type": "chat",
            "message_id": format!("eval-{}-{}-t{}", fixture.id, run_id, turn_idx),
            "data": msg_data,
        });

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

                    // Filter events by our session to avoid cross-talk
                    if let Some(ev_sid) = event["data"]["session_id"].as_str() {
                        if ev_sid != session_id {
                            continue;
                        }
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
                        }
                        Some("chat_complete") => break,
                        Some("chat_error") => {
                            let err = event["data"]["error"]
                                .as_str()
                                .unwrap_or("unknown error");
                            return Err(format!("Chat error: {}", err));
                        }
                        _ => {}
                    }
                }
                Ok(Some(Err(e))) => return Err(format!("WS error: {}", e)),
                Ok(None) => break,
                Err(_) => return Err("Timeout waiting for response".into()),
            }
        }
    }

    let final_text = all_text.join("");

    // An empty run — no text, no tool calls — is a failure, not a pass. Silent
    // empties (transient provider errors that still emit chat_complete) used to
    // produce valid-looking zero-call traces that skewed results.
    if final_text.trim().is_empty() && all_tool_calls.is_empty() {
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
