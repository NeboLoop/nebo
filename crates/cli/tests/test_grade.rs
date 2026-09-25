//! `nebo-cli test grade`: a kept gate run's traces are graded offline, by the
//! same program checks and the same judge `test run` uses. The judge is the
//! `claude` CLI; here a stub on PATH prints a canned stream-json verdict, so
//! no model is called.

use std::path::PathBuf;
use std::process::{Command, Output};

const FIXTURE: &str = r#"id: demo-fixture
name: "Demo"
description: "A demo scenario."
target_component: agent
conversation:
  - role: user
    content: "Run two helpers in scratch copies."
prompt_assertions:
  first_call:
    - id: parallel-isolated
      text: "The model calls agent with isolate: worktree"
      severity: critical
      check: { tool: agent, arg: isolate, equals: worktree }
  recovery:
    - id: reads-well
      text: "The reply says what the helpers did"
      severity: important
"#;

/// A verdict for the judged assertion, as the CLI prints it with
/// `--output-format stream-json`: the assistant's text, then the result.
const VERDICT: &str = r#"{"tool_quality":[],"model_behavior":[],"assertions":[{"id":"reads-well","passed":true,"evidence":"it did"}],"first_call_success_rate":1.0,"context_pollution_score":0.0,"overall_notes":"fine"}"#;

fn trace(run: &str) -> serde_json::Value {
    serde_json::json!({
        "fixture_id": "demo-fixture",
        "run_id": run,
        "model": "janus/nebo-1",
        "timestamp": "2026-09-25T16:00:00Z",
        "tool_calls": [{
            "sequence": 1,
            "tool": "agent",
            "arguments": {"isolate": "worktree"},
            "response": {"content": "merged", "is_error": false, "char_count": 6},
            "latency_ms": 10
        }],
        "final_response": {"content": "Both helpers merged.", "tokens": 5},
        "metrics": {"total_tool_calls": 1, "total_tokens": 5, "input_tokens": 3,
                    "output_tokens": 2, "total_latency_ms": 10},
        "session_id": "s-1",
        "turns": [{"turn": 1, "latency_ms": 10, "first_reply_ms": 5, "model_calls": 1,
                   "tool_calls": 1, "tool_errors": 0, "input_tokens": 3, "output_tokens": 2,
                   "cache_read_tokens": 0, "cache_creation_tokens": 0,
                   "max_prompt_tokens": 3, "cards": 0, "approvals": 0, "end": "complete"}]
    })
}

/// A kept run laid out the way the gate keeps it: the instrument checkout
/// (suites/, fixtures/) and the run (run.json, traces/<entry>/...).
struct Kept {
    dir: tempfile::TempDir,
}

impl Kept {
    fn new(judge_script: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        for d in ["root/suites", "root/fixtures/tools", "run/traces/smoke", "bin", "home"] {
            std::fs::create_dir_all(p.join(d)).unwrap();
        }
        std::fs::write(p.join("root/fixtures/tools/demo.yaml"), FIXTURE).unwrap();
        std::fs::write(
            p.join("root/suites/smoke.yaml"),
            "name: smoke\nfixtures:\n  - ../fixtures/tools/demo.yaml\n",
        )
        .unwrap();
        std::fs::write(
            p.join("run/run.json"),
            r#"{"arm":"a","suites":"suites/smoke.yaml","judge":false}"#,
        )
        .unwrap();
        for run in ["run-1", "run-2"] {
            std::fs::write(
                p.join(format!("run/traces/smoke/demo-fixture_{run}.json")),
                serde_json::to_string_pretty(&trace(run)).unwrap(),
            )
            .unwrap();
        }
        // A run that never completed: nothing to judge, left as it was.
        let mut failed = trace("run-3");
        failed["failure_reason"] = "silence cap".into();
        std::fs::write(
            p.join("run/traces/smoke/demo-fixture_run-3_FAILED_silence-cap.json"),
            serde_json::to_string_pretty(&failed).unwrap(),
        )
        .unwrap();

        let claude = p.join("bin/claude");
        std::fs::write(&claude, judge_script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self { dir }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn grade(&self, extra: &[&str]) -> Output {
        let path = format!(
            "{}:{}",
            self.path("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        Command::new(env!("CARGO_BIN_EXE_nebo-cli"))
            .args(["test", "grade", "--traces"])
            .arg(self.path("run"))
            .arg("--fixtures-root")
            .arg(self.path("root"))
            .args(["--grader", "sonnet"])
            .args(extra)
            .current_dir(self.dir.path())
            .env("PATH", path)
            .env("NEBO_HOME", self.path("home"))
            .env("JUDGE_CALLS", self.path("calls"))
            .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
            .output()
            .unwrap()
    }

    fn judge_calls(&self) -> usize {
        std::fs::read_to_string(self.path("calls")).map(|s| s.lines().count()).unwrap_or(0)
    }

    fn read(&self, rel: &str) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(self.path(rel)).unwrap()).unwrap()
    }
}

fn judge_that_passes() -> String {
    let assistant = serde_json::json!({
        "type": "assistant",
        "message": {"content": [{"type": "text", "text": VERDICT}]}
    });
    let result = serde_json::json!({"type": "result", "is_error": false, "result": VERDICT});
    format!(
        "#!/bin/sh\ncat > /dev/null\necho \"$*\" >> \"$JUDGE_CALLS\"\ncat <<'EOF'\n{assistant}\n{result}\nEOF\n"
    )
}

fn judge_at_its_limit() -> String {
    let result = serde_json::json!({
        "type": "result", "is_error": true,
        "result": "Claude AI usage limit reached|1758900000"
    });
    format!(
        "#!/bin/sh\ncat > /dev/null\necho \"$*\" >> \"$JUDGE_CALLS\"\ncat <<'EOF'\n{result}\nEOF\nexit 1\n"
    )
}

fn assertion<'a>(t: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
    t["grade"]["assertions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == id)
        .unwrap_or_else(|| panic!("no assertion {id} in {t}"))
}

fn stdout(o: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn grades_a_kept_run_in_place_and_skips_what_is_judged() {
    let kept = Kept::new(&judge_that_passes());
    let out = kept.grade(&[]);
    assert!(out.status.success(), "{}", stdout(&out));
    // The same per-fixture summary `test run` prints.
    assert!(stdout(&out).contains("nebo test run — demo-fixture"), "{}", stdout(&out));
    // Each trace is announced as its grade is written.
    assert!(stdout(&out).contains("graded smoke/demo-fixture_run-1.json"), "{}", stdout(&out));
    // One judge call per completed run, with the model asked for.
    assert_eq!(kept.judge_calls(), 2);
    assert!(std::fs::read_to_string(kept.path("calls")).unwrap().contains("--model sonnet"));

    for run in ["run-1", "run-2"] {
        let t = kept.read(&format!("run/traces/smoke/demo-fixture_{run}.json"));
        let checked = assertion(&t, "parallel-isolated");
        assert_eq!(checked["mode"], "verified");
        assert_eq!(checked["passed"], true);
        let judged = assertion(&t, "reads-well");
        assert_eq!(judged["mode"], "judged");
        assert_eq!(judged["passed"], true);
        assert_eq!(t["grade"]["judge"], "sonnet");
        assert!(t["grade"].get("judge_error").is_none());
        // Everything the runner recorded is still there.
        assert_eq!(t["turns"][0]["model_calls"], 1);
        assert_eq!(t["session_id"], "s-1");
    }
    let failed = kept.read("run/traces/smoke/demo-fixture_run-3_FAILED_silence-cap.json");
    assert!(failed.get("grade").is_none());

    // Judged traces are skipped; --force judges them again.
    let again = kept.grade(&[]);
    assert!(again.status.success(), "{}", stdout(&again));
    assert_eq!(kept.judge_calls(), 2);
    let forced = kept.grade(&["--force"]);
    assert!(forced.status.success(), "{}", stdout(&forced));
    assert_eq!(kept.judge_calls(), 4);
    let t = kept.read("run/traces/smoke/demo-fixture_run-1.json");
    // A re-grade replaces the grade, it never stacks a second copy.
    assert_eq!(t["grade"]["assertions"].as_array().unwrap().len(), 2);
}

#[test]
fn out_leaves_the_kept_traces_as_they_were() {
    let kept = Kept::new(&judge_that_passes());
    let before = std::fs::read_to_string(kept.path("run/traces/smoke/demo-fixture_run-1.json")).unwrap();
    let out_dir = kept.path("graded");
    let out = kept.grade(&["--out", out_dir.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stdout(&out));
    assert_eq!(
        std::fs::read_to_string(kept.path("run/traces/smoke/demo-fixture_run-1.json")).unwrap(),
        before
    );
    let t = kept.read("graded/smoke/demo-fixture_run-1.json");
    assert_eq!(assertion(&t, "reads-well")["mode"], "judged");
}

#[test]
fn a_failed_judge_is_recorded_per_trace_and_the_batch_goes_on() {
    let kept = Kept::new(&judge_at_its_limit());
    let out = kept.grade(&[]);
    assert!(!out.status.success(), "a failed judge must not exit 0");
    assert!(stdout(&out).contains("usage limit"), "{}", stdout(&out));
    // Both runs were tried: one failure does not stop the batch.
    assert_eq!(kept.judge_calls(), 2);
    for run in ["run-1", "run-2"] {
        let t = kept.read(&format!("run/traces/smoke/demo-fixture_{run}.json"));
        // The program checks still decide; the judge's failure says why.
        assert_eq!(assertion(&t, "parallel-isolated")["passed"], true);
        let err = t["grade"]["judge_error"].as_str().unwrap();
        assert!(err.contains("usage limit"), "{err}");
    }
    // Not judged, so a later grade tries again.
    let again = kept.grade(&[]);
    assert!(!again.status.success());
    assert_eq!(kept.judge_calls(), 4);
}

#[test]
fn a_run_without_its_suites_is_refused() {
    let kept = Kept::new(&judge_that_passes());
    std::fs::remove_file(kept.path("run/run.json")).unwrap();
    let out = kept.grade(&[]);
    assert!(!out.status.success());
    assert!(stdout(&out).contains("--suites"), "{}", stdout(&out));
    assert_eq!(kept.judge_calls(), 0);
    // Naming the suites the run used grades it.
    let named = kept.grade(&["--suites", "suites/smoke.yaml"]);
    assert!(named.status.success(), "{}", stdout(&named));
    assert_eq!(kept.judge_calls(), 2);
}
