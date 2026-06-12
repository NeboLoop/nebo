use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::warn;

use super::fixture::Fixture;
use super::trace::*;

/// Grade a trace using an LLM-as-judge via the `claude` CLI.
/// Always uses the CLI path to bypass Nebo's chat pipeline (which injects
/// the full system prompt and causes the grader to respond conversationally).
pub async fn grade(
    trace: &Trace,
    fixture: &Fixture,
    _server: &str,
    grader_model: &str,
) -> Result<GradeResult, String> {
    let grader_prompt = build_grader_prompt(trace, fixture);
    let result_text = grade_with_claude_code(&grader_prompt, grader_model).await?;
    parse_grade_response(&result_text)
}

/// Grade using the `claude` CLI directly — no Nebo server dependency for grading.
async fn grade_with_claude_code(prompt: &str, model: &str) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new("claude");
    cmd.args([
        "--print",
        "--verbose",
        "--output-format", "stream-json",
        "--dangerously-skip-permissions",
        "--model", model,
    ])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

    // Grade from a neutral workspace so the judge doesn't auto-discover the
    // repo's CLAUDE.md or the operator's project memory as context.
    let workspace = std::env::temp_dir().join("nebo-cli-workspace");
    if std::fs::create_dir_all(&workspace).is_ok() {
        cmd.current_dir(&workspace);
    }
    cmd.env("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1")
        .env("CLAUDE_CODE_DISABLE_CLAUDE_MDS", "1");

    #[cfg(unix)]
    {
        unsafe {
            cmd.pre_exec(|| {
                libc::setpgid(0, 0);
                Ok(())
            });
        }
    }

    let mut child = cmd.spawn().map_err(|e| {
        format!("failed to start `claude` CLI: {}. Is Claude Code installed?", e)
    })?;

    if let Some(mut stdin) = child.stdin.take() {
        let content = prompt.to_string();
        tokio::spawn(async move {
            let _ = stdin.write_all(content.as_bytes()).await;
            let _ = stdin.shutdown().await;
        });
    }

    let mut result_text = String::new();

    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            if line.is_empty() {
                continue;
            }
            let Ok(raw) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };

            let event_type = raw.get("type").and_then(|v| v.as_str()).unwrap_or("");

            match event_type {
                "assistant" => {
                    // Extract text from assistant message content blocks
                    if let Some(msg) = raw.get("message") {
                        if let Some(content) = msg.get("content").and_then(|v| v.as_array()) {
                            for block in content {
                                if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                                    if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                        result_text.push_str(text);
                                    }
                                }
                            }
                        }
                    }
                }
                "result" => {
                    // Fallback: extract from result field if assistant message was missed
                    if result_text.is_empty() {
                        if let Some(text) = raw.get("result").and_then(|v| v.as_str()) {
                            result_text = text.to_string();
                        }
                    }
                    break;
                }
                "error" => {
                    let msg = raw.get("error").map(|v| format!("{}", v)).unwrap_or_else(|| "CLI error".to_string());
                    return Err(format!("Claude Code grader error: {}", msg));
                }
                _ => {}
            }
        }
    }

    let status = child.wait().await.map_err(|e| format!("wait: {}", e))?;
    if !status.success() {
        return Err(format!("claude CLI exited with status {}", status));
    }

    if result_text.is_empty() {
        return Err("Claude Code grader returned empty response".to_string());
    }

    Ok(result_text)
}

fn build_grader_prompt(trace: &Trace, fixture: &Fixture) -> String {
    let trace_json = serde_json::to_string_pretty(trace).unwrap_or_default();

    let mut assertions_text = String::new();
    for a in fixture.prompt_assertions.all() {
        assertions_text.push_str(&format!(
            "- [{}] ({:?}) {}\n",
            a.id, a.severity, a.text
        ));
    }
    for a in &fixture.integrated_assertions {
        assertions_text.push_str(&format!(
            "- [{}] ({:?}) {}\n",
            a.id, a.severity, a.text
        ));
    }

    let budget_text = fixture
        .tool_config
        .values()
        .filter_map(|tc| tc.response_budget.as_ref())
        .map(|b| format!("{} chars max", b.max_chars))
        .next()
        .unwrap_or_else(|| "500 chars".to_string());

    format!(
        r#"You are evaluating an AI agent's tool usage in a controlled test.

Your job is to assess TWO things:
1. TOOL QUALITY — Did the tool return a response the model could work with?
2. MODEL BEHAVIOR — Did the model use the tool correctly and handle the response well?

## Scenario
{description}

## Transcript (JSON trace)
{trace}

## Tool Quality Checklist
For each tool call, evaluate the TOOL'S response:
- Was the response human-readable? (not a stack trace, not a raw error code)
- Was the response appropriately sized? (under {budget})
- Did the response name the specific problem? (not just "error")
- Could a human reading the response understand what went wrong?

## Model Behavior Checklist
For each tool call, evaluate the MODEL's behavior:
- Did the model call the correct tool?
- Did the model pass the correct arguments?
- Did the model interpret the response correctly?
- Did the model retry unnecessarily?
- Did the model recover gracefully from errors?

## First-Call Success
For each tool call, determine:
- Did the model achieve its INTENT on this call?
- Intent is not "command succeeds" — intent is "model gets the information it needs
  to proceed with the task." A tool call that correctly returns "file not found" is
  a successful first call if the model's intent was to check whether the file exists.

## Assertions to evaluate
{assertions}

## Context Pollution
Classify each section of the transcript as:
- PRODUCTIVE: Advances the task
- POLLUTION: Noise from failures (retries, error dumps, redundant attempts)

Calculate: pollution_tokens / total_tokens

Respond with ONLY valid JSON (no markdown fences, no explanation outside JSON):
{{
  "tool_quality": [
    {{
      "tool_call_sequence": 1,
      "tool": "tool_name",
      "response_parseable": true,
      "response_human_readable": true,
      "response_actionable": true,
      "response_within_budget": true,
      "score": 1.0,
      "notes": "..."
    }}
  ],
  "model_behavior": [
    {{
      "tool_call_sequence": 1,
      "correct_tool": true,
      "correct_args": true,
      "correct_interpretation": true,
      "unnecessary_retry": false,
      "score": 1.0,
      "notes": "..."
    }}
  ],
  "assertions": [
    {{"id": "assertion-id", "passed": true, "evidence": "..."}}
  ],
  "first_call_success_rate": 1.0,
  "context_pollution_score": 0.0,
  "overall_notes": "..."
}}"#,
        description = fixture.description,
        trace = trace_json,
        budget = budget_text,
        assertions = assertions_text,
    )
}

fn parse_grade_response(text: &str) -> Result<GradeResult, String> {
    // Strip markdown fences and Claude Code artifacts
    let cleaned = text.trim();
    let cleaned = cleaned
        .strip_prefix("```json")
        .or_else(|| cleaned.strip_prefix("```"))
        .unwrap_or(cleaned);
    let cleaned = cleaned.strip_suffix("```").unwrap_or(cleaned).trim();

    // Find the JSON object boundaries (first { to last })
    let start = cleaned.find('{');
    let end = cleaned.rfind('}');
    let json_str = match (start, end) {
        (Some(s), Some(e)) if e > s => &cleaned[s..=e],
        _ => cleaned,
    };

    serde_json::from_str::<GradeResult>(json_str).map_err(|e| {
        warn!(raw_response = %text, "failed to parse grader response");
        format!("parse grader JSON: {} (raw: {}...)", e, &text[..text.len().min(200)])
    })
}
