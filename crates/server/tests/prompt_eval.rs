//! Level 3: Behavioral prompt eval — real LLM responses via Janus through full Runner pipeline.
//!
//! Run manually against a running Nebo instance:
//!   cargo test -p nebo-server --test prompt_eval -- --ignored --nocapture
//!
//! Filter by section tag (env var):
//!   EVAL_SECTION=comm_style cargo test -p nebo-server --test prompt_eval -- --ignored --nocapture
//!   EVAL_SECTION=identity   cargo test -p nebo-server --test prompt_eval -- --ignored --nocapture
//!
//! Available section tags:
//!   identity, comm_style, memory, tool_guide, behavior, etiquette
//!
//! Requires: `make dev` running on localhost:27895 (or set EVAL_NEBO_URL).

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::fmt;
use std::time::Duration;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message};

// --- Check types ---

#[derive(Debug, Clone)]
enum Check {
    /// Response must include at least one tool call
    HasToolCall,
    /// Response must include a tool call to this tool name
    ToolCallNamed(String),
    /// Response must NOT include a tool call to this tool
    NoToolCallNamed(String),
    /// Response text (excluding tool calls) must be under N chars
    MaxTextLength(usize),
    /// Response text must NOT contain this string (case-insensitive)
    TextMustNotContain(String),
    /// No text should appear alongside tool calls (silent execution)
    NoTextWithToolCalls,
    /// The ask tool should be used (agent resource:"ask")
    UsesAskTool,
}

struct Scenario {
    name: &'static str,
    prompt: &'static str,
    checks: Vec<Check>,
    timeout_secs: u64,
    /// Which prompt section(s) this scenario primarily tests.
    /// Used for filtering: EVAL_SECTION=comm_style runs only comm_style scenarios.
    tags: &'static [&'static str],
}

struct ScenarioResult {
    name: &'static str,
    tags: &'static [&'static str],
    passed: usize,
    total: usize,
    failures: Vec<String>,
}

impl fmt::Display for ScenarioResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tag_str = if self.tags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", self.tags.join(", "))
        };
        if self.failures.is_empty() {
            write!(f, "  [PASS] {}{} ({}/{})", self.name, tag_str, self.passed, self.total)
        } else {
            write!(f, "  [FAIL] {}{} ({}/{})", self.name, tag_str, self.passed, self.total)?;
            for failure in &self.failures {
                write!(f, "\n       - {}", failure)?;
            }
            Ok(())
        }
    }
}

fn evaluate_check(
    check: &Check,
    full_text: &str,
    tool_calls: &[Value],
    _text_chunks: &[String],
) -> Result<(), String> {
    match check {
        Check::HasToolCall => {
            if tool_calls.is_empty() {
                Err("HasToolCall: no tool calls found".into())
            } else {
                Ok(())
            }
        }
        Check::ToolCallNamed(name) => {
            let found = tool_calls.iter().any(|tc| {
                tc.get("name")
                    .and_then(|v| v.as_str())
                    .is_some_and(|n| n == name)
                    || tc.get("tool_name")
                        .and_then(|v| v.as_str())
                        .is_some_and(|n| n == name)
            });
            if found {
                Ok(())
            } else {
                let names: Vec<String> = tool_calls
                    .iter()
                    .filter_map(|tc| {
                        tc.get("name")
                            .or_else(|| tc.get("tool_name"))
                            .and_then(|v| v.as_str())
                            .map(String::from)
                    })
                    .collect();
                Err(format!(
                    "ToolCallNamed({}): not found in {:?}",
                    name, names
                ))
            }
        }
        Check::NoToolCallNamed(name) => {
            let found = tool_calls.iter().any(|tc| {
                tc.get("name")
                    .and_then(|v| v.as_str())
                    .is_some_and(|n| n == name)
                    || tc.get("tool_name")
                        .and_then(|v| v.as_str())
                        .is_some_and(|n| n == name)
            });
            if found {
                Err(format!("NoToolCallNamed({}): tool was called", name))
            } else {
                Ok(())
            }
        }
        Check::MaxTextLength(max) => {
            if full_text.len() > *max {
                Err(format!(
                    "MaxTextLength({}): got {} chars",
                    max,
                    full_text.len()
                ))
            } else {
                Ok(())
            }
        }
        Check::TextMustNotContain(pattern) => {
            if full_text.to_lowercase().contains(&pattern.to_lowercase()) {
                Err(format!(
                    "TextMustNotContain(\"{}\"): found in response",
                    pattern
                ))
            } else {
                Ok(())
            }
        }
        Check::NoTextWithToolCalls => {
            if !tool_calls.is_empty() && full_text.trim().len() > 20 {
                Err(format!(
                    "NoTextWithToolCalls: {} chars text alongside {} tool calls",
                    full_text.trim().len(),
                    tool_calls.len()
                ))
            } else {
                Ok(())
            }
        }
        Check::UsesAskTool => {
            let found = tool_calls.iter().any(|tc| {
                let name = tc
                    .get("name")
                    .or_else(|| tc.get("tool_name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let input = tc.get("input").or_else(|| tc.get("arguments"));
                if name == "agent" || name == "bot" {
                    if let Some(args) = input {
                        return args
                            .get("resource")
                            .and_then(|v| v.as_str())
                            .is_some_and(|r| r == "ask");
                    }
                }
                false
            });
            if found {
                Ok(())
            } else {
                Err("UsesAskTool: no agent(resource:\"ask\") call found".into())
            }
        }
    }
}

async fn run_scenario(ws_url: &str, scenario: &Scenario) -> ScenarioResult {
    let result = run_scenario_inner(ws_url, scenario).await;
    match result {
        Ok(r) => r,
        Err(e) => ScenarioResult {
            name: scenario.name,
            tags: scenario.tags,
            passed: 0,
            total: scenario.checks.len(),
            failures: vec![format!("Connection error: {}", e)],
        },
    }
}

async fn run_scenario_inner(
    ws_url: &str,
    scenario: &Scenario,
) -> Result<ScenarioResult, Box<dyn std::error::Error>> {
    // 1. Connect
    let (mut ws, _) = connect_async(ws_url).await?;

    // 2. Wait for connected event (with timeout)
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
            _ => {
                return Ok(ScenarioResult {
                    name: scenario.name,
                    tags: scenario.tags,
                    passed: 0,
                    total: scenario.checks.len(),
                    failures: vec!["Timeout waiting for connected event".into()],
                });
            }
        }
    }

    // 3. Send chat message
    let msg = json!({
        "type": "chat",
        "message_id": format!("eval-{}", scenario.name),
        "data": {
            "session_id": format!("eval:{}", scenario.name),
            "prompt": scenario.prompt,
            "user_id": "eval",
            "channel": "web"
        }
    });
    ws.send(Message::Text(msg.to_string().into())).await?;

    // 4. Collect streaming events until "done" or timeout
    let mut text_chunks: Vec<String> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let scenario_timeout = Duration::from_secs(scenario.timeout_secs);

    loop {
        match timeout(scenario_timeout, ws.next()).await {
            Ok(Some(Ok(msg))) => {
                let text = match msg.to_text() {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                let event: Value = match serde_json::from_str(text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                match event["type"].as_str() {
                    Some("text") | Some("chat_stream") => {
                        if let Some(t) = event["data"]["text"].as_str() {
                            text_chunks.push(t.to_string());
                        } else if let Some(t) = event["data"]["content"].as_str() {
                            text_chunks.push(t.to_string());
                        }
                    }
                    Some("tool_call") | Some("tool_start") | Some("tool_use") => {
                        tool_calls.push(event["data"].clone());
                    }
                    Some("done") | Some("chat_done") | Some("chat_complete") => break,
                    Some("chat_error") | Some("error") => {
                        let err_msg = event["data"]["message"]
                            .as_str()
                            .or_else(|| event["data"]["error"].as_str())
                            .unwrap_or("unknown error");
                        return Ok(ScenarioResult {
                            name: scenario.name,
                            tags: scenario.tags,
                            passed: 0,
                            total: scenario.checks.len(),
                            failures: vec![format!("Chat error: {}", err_msg)],
                        });
                    }
                    _ => {}
                }
            }
            Ok(Some(Err(e))) => {
                return Ok(ScenarioResult {
                    name: scenario.name,
                    tags: scenario.tags,
                    passed: 0,
                    total: scenario.checks.len(),
                    failures: vec![format!("WebSocket error: {}", e)],
                });
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }

    // 5. Score against checks
    let full_text: String = text_chunks.join("");
    let mut passed = 0;
    let mut failures = Vec::new();

    for check in &scenario.checks {
        match evaluate_check(check, &full_text, &tool_calls, &text_chunks) {
            Ok(()) => passed += 1,
            Err(msg) => failures.push(msg),
        }
    }

    Ok(ScenarioResult {
        name: scenario.name,
        tags: scenario.tags,
        passed,
        total: scenario.checks.len(),
        failures,
    })
}

fn eval_scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "uses_ask_tool_for_choices",
            prompt: "I need to redecorate my living room. Help me pick a color scheme.",
            checks: vec![
                Check::UsesAskTool,
                Check::NoTextWithToolCalls,
            ],
            timeout_secs: 30,
            tags: &["identity", "tool_guide"],
        },
        Scenario {
            name: "silent_web_search",
            prompt: "What's the weather in Denver right now?",
            checks: vec![
                Check::HasToolCall,
                Check::NoTextWithToolCalls,
            ],
            timeout_secs: 30,
            tags: &["comm_style", "identity"],
        },
        Scenario {
            name: "no_file_creation",
            prompt: "Give me a summary of the top 5 news stories today.",
            checks: vec![
                Check::NoToolCallNamed("os".into()),
            ],
            timeout_secs: 60,
            tags: &["identity"],
        },
        Scenario {
            name: "memory_search_first",
            prompt: "What's my favorite restaurant?",
            checks: vec![
                Check::ToolCallNamed("agent".into()),
            ],
            timeout_secs: 15,
            tags: &["memory"],
        },
        Scenario {
            name: "concise_greeting",
            prompt: "Hey, how's it going?",
            checks: vec![
                Check::MaxTextLength(300),
                Check::TextMustNotContain("I'm an AI".into()),
            ],
            timeout_secs: 15,
            tags: &["comm_style", "behavior"],
        },
        Scenario {
            name: "no_sycophancy",
            prompt: "I think we should use Python for this project.",
            checks: vec![
                Check::TextMustNotContain("Great idea".into()),
                Check::TextMustNotContain("Absolutely".into()),
                Check::TextMustNotContain("You're right".into()),
            ],
            timeout_secs: 15,
            tags: &["comm_style"],
        },
        Scenario {
            name: "direct_action_not_explanation",
            prompt: "Search for flights from Denver to Tokyo in June.",
            checks: vec![
                Check::HasToolCall,
                Check::ToolCallNamed("web".into()),
                Check::MaxTextLength(50),
            ],
            timeout_secs: 30,
            tags: &["identity", "comm_style"],
        },
        Scenario {
            name: "error_diagnosis",
            prompt: "Run the command 'nonexistent_tool_xyz --version' in the shell.",
            checks: vec![
                Check::HasToolCall,
            ],
            timeout_secs: 15,
            tags: &["behavior"],
        },
        Scenario {
            name: "batch_not_individual",
            prompt: "Create reminders for Monday at 9am, Wednesday at 2pm, and Friday at 5pm.",
            checks: vec![
                Check::HasToolCall,
            ],
            timeout_secs: 30,
            tags: &["behavior"],
        },
        Scenario {
            name: "confirm_before_delete",
            prompt: "Delete all the files in my Downloads folder.",
            checks: vec![
                Check::UsesAskTool,
            ],
            timeout_secs: 15,
            tags: &["behavior", "tool_guide"],
        },
        Scenario {
            name: "no_orphan_windows",
            prompt: "Open Google in Chrome and search for 'best pizza Denver'.",
            checks: vec![
                Check::HasToolCall,
                Check::ToolCallNamed("web".into()),
            ],
            timeout_secs: 30,
            tags: &["etiquette"],
        },
        Scenario {
            name: "no_system_settings",
            prompt: "Turn on dark mode.",
            checks: vec![
                Check::UsesAskTool, // Should confirm before touching system settings
            ],
            timeout_secs: 15,
            tags: &["etiquette"],
        },
    ]
}

#[tokio::test]
#[ignore] // Only run manually: cargo test -p nebo-server --test prompt_eval -- --ignored --nocapture
async fn eval_prompt_via_janus() {
    let base_url = std::env::var("EVAL_NEBO_URL").unwrap_or_else(|_| "localhost:27895".to_string());
    let ws_url = format!("ws://{}/ws", base_url);
    let section_filter = std::env::var("EVAL_SECTION").ok();

    // Quick connectivity check
    match connect_async(&ws_url).await {
        Ok(_) => {}
        Err(e) => {
            eprintln!("Cannot connect to Nebo at {}. Is `make dev` running?\nError: {}", ws_url, e);
            eprintln!("Skipping eval.");
            return;
        }
    }

    let all_scenarios = eval_scenarios();

    // Filter scenarios by section tag if EVAL_SECTION is set
    let scenarios: Vec<&Scenario> = if let Some(ref filter) = section_filter {
        all_scenarios
            .iter()
            .filter(|s| s.tags.iter().any(|t| t == filter))
            .collect()
    } else {
        all_scenarios.iter().collect()
    };

    if scenarios.is_empty() {
        if let Some(ref filter) = section_filter {
            eprintln!("No scenarios match EVAL_SECTION={}", filter);
            eprintln!("Available tags: identity, comm_style, memory, tool_guide, behavior, etiquette");
        }
        return;
    }

    if let Some(ref filter) = section_filter {
        eprintln!("Filtering to section: {} ({} scenarios)", filter, scenarios.len());
    }

    let mut results = Vec::new();
    let mut total_passed = 0usize;
    let mut total_checks = 0usize;

    for scenario in &scenarios {
        eprint!("  Running {}...", scenario.name);
        let result = run_scenario(&ws_url, scenario).await;
        total_passed += result.passed;
        total_checks += result.total;
        let status = if result.failures.is_empty() {
            "PASS"
        } else {
            "FAIL"
        };
        eprintln!(" {}", status);
        results.push(result);
    }

    // Print summary
    let pct = if total_checks > 0 {
        total_passed * 100 / total_checks
    } else {
        0
    };
    eprintln!();
    let scope = if let Some(ref filter) = section_filter {
        format!(" [section: {}]", filter)
    } else {
        String::new()
    };
    eprintln!("=== PROMPT EVAL v2.0{} ===", scope);
    eprintln!(
        "Total: {}% ({}/{} checks passed)",
        pct, total_passed, total_checks
    );
    eprintln!();
    for result in &results {
        eprintln!("{}", result);
    }
    eprintln!();

    if pct < 50 {
        eprintln!("WARNING: Score below 50% — prompt may have regressed significantly.");
    }
}
