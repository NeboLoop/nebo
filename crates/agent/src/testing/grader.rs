use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::warn;

use super::fixture::{Fixture, Severity};
use super::trace::*;

/// What grading one trace found.
#[derive(Debug, Default)]
pub struct GradeOutcome {
    /// Failed critical program checks, as `fixture / assertion: evidence`.
    pub critical_failures: Vec<String>,
    /// Why the judge could not grade the trace; also kept on its grade.
    pub judge_error: Option<String>,
}

/// The one way a trace is graded, live (`nebo-cli test run`) or offline from
/// a kept run (`nebo-cli test grade`). Program checks first, decided from the
/// trace before any judge; then, with a grader model, the judge (`claude -p`)
/// on the prose-only assertions, reading the trace with its program-check
/// rows attached. The result replaces any earlier grade. A judge that fails
/// is recorded on the grade and returned, never fatal: the program checks
/// still decide. `Err` = a malformed check, which fails the run (never open).
pub async fn grade_trace(
    trace: &mut Trace,
    fixture: &Fixture,
    grader_model: Option<&str>,
) -> Result<GradeOutcome, String> {
    let verified = super::checks::evaluate_fixture_checks(fixture, trace)
        .map_err(|e| format!("fixture {}: {}", fixture.id, e))?;
    let critical_failures = verified
        .iter()
        .filter(|v| !v.passed)
        .filter(|v| {
            fixture
                .prompt_assertions
                .all()
                .into_iter()
                .chain(fixture.integrated_assertions.iter())
                .any(|a| a.id == v.id && a.severity == Severity::Critical)
        })
        .map(|v| format!("{} / {}: {}", fixture.id, v.id, v.evidence))
        .collect();
    trace.grade = (!verified.is_empty()).then(|| GradeResult::program_only(verified));

    let mut judge_error = None;
    if let Some(model) = grader_model {
        match grade(trace, fixture, model).await {
            Ok(judged) => match &mut trace.grade {
                // Keep the verified rows in front of the judged ones.
                Some(existing) => {
                    existing.assertions.extend(judged.assertions);
                    existing.first_call_success_rate = judged.first_call_success_rate;
                    existing.context_pollution_score = judged.context_pollution_score;
                    existing.tool_quality = judged.tool_quality;
                    existing.model_behavior = judged.model_behavior;
                    existing.overall_notes = judged.overall_notes;
                    existing.judge = Some(model.to_string());
                }
                None => trace.grade = Some(GradeResult { judge: Some(model.to_string()), ..judged }),
            },
            Err(e) => {
                trace
                    .grade
                    .get_or_insert_with(|| GradeResult::program_only(Vec::new()))
                    .judge_error = Some(e.clone());
                judge_error = Some(e);
            }
        }
    }
    Ok(GradeOutcome { critical_failures, judge_error })
}

/// Grade a trace using an LLM-as-judge via the `claude` CLI.
/// Always uses the CLI path to bypass Nebo's chat pipeline (which injects
/// the full system prompt and causes the grader to respond conversationally).
async fn grade(
    trace: &Trace,
    fixture: &Fixture,
    grader_model: &str,
) -> Result<GradeResult, String> {
    let grader_prompt = build_grader_prompt(trace, fixture)?;
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

    // Read stderr alongside stdout: it is what says why the CLI failed, and
    // a pipe nobody drains stalls the CLI once it fills.
    let stderr_task = child.stderr.take().map(|mut stderr| {
        tokio::spawn(async move {
            let mut buf = String::new();
            let _ = tokio::io::AsyncReadExt::read_to_string(&mut stderr, &mut buf).await;
            buf
        })
    });

    if let Some(mut stdin) = child.stdin.take() {
        let content = prompt.to_string();
        tokio::spawn(async move {
            let _ = stdin.write_all(content.as_bytes()).await;
            let _ = stdin.shutdown().await;
        });
    }

    let mut result_text = String::new();
    // The CLI's `result` event marks a failed session (login, usage limit)
    // with `is_error`, and its text is the reason.
    let mut result_is_error = false;

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
                    result_is_error =
                        raw.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
                    if result_is_error {
                        result_text =
                            raw.get("result").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    }
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
    let stderr = match stderr_task {
        Some(task) => task.await.unwrap_or_default(),
        None => String::new(),
    };
    if !status.success() || result_is_error {
        let reason = result_is_error.then_some(result_text.as_str());
        return Err(cli_failure(&status.to_string(), reason, &stderr));
    }

    if result_text.is_empty() {
        return Err("Claude Code grader returned empty response".to_string());
    }

    Ok(result_text)
}

/// Lines of the CLI's stderr carried in a grading error.
const STDERR_TAIL_LINES: usize = 20;

/// Why the judge failed, in the CLI's own words: its exit status, the reason
/// its `result` event gave, and the last lines of its stderr, with anything
/// that looks like a credential removed.
fn cli_failure(status: &str, reason: Option<&str>, stderr: &str) -> String {
    let mut msg = format!("claude CLI exited with status {}", status);
    if let Some(reason) = reason.map(str::trim).filter(|r| !r.is_empty()) {
        msg.push_str(&format!(": {}", scrub_secrets(reason)));
    }
    let lines: Vec<&str> =
        stderr.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        msg.push_str(" (stderr empty)");
    } else {
        let tail = &lines[lines.len().saturating_sub(STDERR_TAIL_LINES)..];
        msg.push_str(&format!("\n  stderr:\n    {}", scrub_secrets(&tail.join("\n    "))));
    }
    msg
}

/// Replace anything that looks like a credential with `[redacted]`: the value
/// after `Bearer` or after a secret-named label (`token=`, `api_key:`), API
/// and OAuth keys (`sk-…`), JWTs, and any long opaque run of token characters.
fn scrub_secrets(text: &str) -> String {
    type Patterns = Vec<(regex::Regex, &'static str)>;
    static PATTERNS: std::sync::LazyLock<Patterns> = std::sync::LazyLock::new(|| {
        [
            (r"(?i)\b(bearer)(\s+)[^\s\x22',;]+", "$1$2[redacted]"),
            (
                r"(?i)\b(token|api[_-]?key|secret|password|authorization)(\s*[:=]\s*[\x22']?)[^\s\x22',;]+",
                "$1$2[redacted]",
            ),
            (r"\bsk-[A-Za-z0-9_-]{8,}", "[redacted]"),
            (r"\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+(\.[A-Za-z0-9_-]+)?", "[redacted]"),
            (r"[A-Za-z0-9_+=-]{32,}", "[redacted]"),
        ]
        .into_iter()
        .map(|(re, with)| (regex::Regex::new(re).expect("secret pattern"), with))
        .collect()
    });
    PATTERNS
        .iter()
        .fold(text.to_string(), |out, (re, with)| re.replace_all(&out, *with).into_owned())
}

/// The judge reads the fixture bound to the trace's run (`{{scratch}}`,
/// `{{tag}}`), the same binding the program checks use, so it compares the
/// transcript against the values that run was given.
fn build_grader_prompt(trace: &Trace, fixture: &Fixture) -> Result<String, String> {
    let fixture = &super::scratch::bind(fixture, &trace.run_id)?;
    let trace_json = serde_json::to_string_pretty(trace).unwrap_or_default();

    // Prose-only assertions. Check-bearing ones are program-verified before
    // any grader call and never routed to the judge (WS1-R3).
    let mut assertions_text = String::new();
    for a in super::checks::judged_assertions(fixture) {
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

    Ok(format!(
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
    ))
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

    serde_json::from_str::<GradeResult>(json_str)
        .map(|mut g| {
            for a in &mut g.assertions {
                a.mode = super::checks::MODE_JUDGED.to_string();
            }
            g
        })
        .map_err(|e| {
            warn!(raw_response = %text, "failed to parse grader response");
            format!("parse grader JSON: {} (raw: {}...)", e, &text[..text.len().min(200)])
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_judge_says_why_without_its_credentials() {
        let stderr = (1..=25).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n")
            + "\nAuthorization: Bearer abc.def.ghi"
            + "\nCLAUDE_CODE_OAUTH_TOKEN=sk-ant-oat01-Zx9_secretsecret\n";
        let msg = cli_failure(
            "exit status: 1",
            Some("Failed to authenticate. API Error: 401 OAuth access token is invalid."),
            &stderr,
        );
        let head = "claude CLI exited with status exit status: 1: Failed to authenticate.";
        assert!(msg.starts_with(head), "{msg}");
        assert!(msg.contains("OAuth access token is invalid."), "the reason survives the scrub: {msg}");
        let last_20 = !msg.contains("line 7\n") && msg.contains("line 8") && msg.contains("line 25");
        assert!(last_20, "last 20 lines: {msg}");
        for secret in ["abc.def.ghi", "sk-ant", "secretsecret"] {
            assert!(!msg.contains(secret), "{secret} leaked: {msg}");
        }
    }

    #[test]
    fn silent_stderr_is_said_out_loud() {
        assert_eq!(
            cli_failure("exit status: 1", None, "\n \n"),
            "claude CLI exited with status exit status: 1 (stderr empty)"
        );
    }

    #[test]
    fn scrub_removes_every_credential_shape() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2lnbmF0dXJl";
        let cases = [
            ("key sk-ant-api03-AAAAbbbb1234 rejected", "key [redacted] rejected"),
            (&*format!("got {jwt} back"), "got [redacted] back"),
            ("token=abc123 api_key: \"xyz\"", "token=[redacted] api_key: \"[redacted]\""),
            ("id 0123456789abcdef0123456789abcdef!", "id [redacted]!"),
            ("usage limit reached; resets 5am", "usage limit reached; resets 5am"),
        ];
        for (input, want) in cases {
            assert_eq!(scrub_secrets(input), want, "{input}");
        }
    }

    /// Gate run 36099651233: the judge was handed `{{scratch}}` unfilled while
    /// the transcript carried the run's real path.
    #[test]
    fn the_judge_reads_the_runs_own_scratch() {
        let fixture: Fixture = serde_yaml::from_str(
            r#"
id: read-file
name: t
description: 'The user asks for {{scratch}}/notes.txt'
conversation:
  - role: user
    content: 'read {{scratch}}/notes.txt'
prompt_assertions:
  first_call:
    - id: reports-contents
      text: reports what {{scratch}}/notes.txt says
      severity: critical
"#,
        )
        .expect("fixture");
        let trace = Trace::failed("read-file", "run-2", None, "unused");
        let prompt = build_grader_prompt(&trace, &fixture).unwrap();
        let dir = super::super::scratch::dir("read-file", "run-2");
        assert!(!prompt.contains("{{scratch}}"), "{prompt}");
        assert!(prompt.contains(&format!("The user asks for {dir}/notes.txt")), "{prompt}");
        assert!(prompt.contains(&format!("reports what {dir}/notes.txt says")), "{prompt}");
    }
}
