//! The judged gate's fixtures must be able to run side by side.
//!
//! One measurement found four fixtures sharing `/tmp/nebo-fixture` — three of
//! them `rm -rf`ing it in teardown — and two more creating and deleting the
//! same employee. Nothing about a single-file run says so; it only shows up
//! the first time two fixtures run at once, as work deleted underneath a run
//! that was passing. This test reads the pooled suites and says it out loud: after
//! [`scratch::bind`], no two fixture runs name the same directory, the same
//! file or the same employee, and no teardown reaches outside its own scratch.
//!
//! What a run leaves on the server (memory, sessions, files in the bot's
//! home) is kept from the next run by the gate itself: every run of every
//! fixture gets a fresh server (`scripts/gate-run.sh`), tested here too.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::fixture::{self, Fixture};
use super::scratch;

/// The suites the gate runs, and the ones a pool would run first.
const SUITES: [&str; 3] = ["suites/error-correction.yaml", "suites/smoke.yaml", "suites/turn-controller.yaml"];

/// Two runs is enough to prove runs of one fixture are separated too.
const RUNS: [&str; 2] = ["run-1", "run-2"];

/// The one path a fixture may name that is not its own: a system location the
/// model is refused, which no run ever creates. Two runs naming it collide
/// over nothing, because nothing is ever written there — the refusal is the
/// test. Every other path a fixture names must live in its own scratch.
const REFUSED: [&str; 1] = ["/var/root/nebo-test.txt"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

/// Every fixture of both suites, bound to each run.
fn bound_runs() -> Vec<(Fixture, &'static str)> {
    let root = repo_root();
    let mut out = Vec::new();
    for suite_rel in SUITES {
        let suite_path = root.join(suite_rel);
        let suite = fixture::load_suite(&suite_path)
            .unwrap_or_else(|e| panic!("load suite {suite_rel}: {e}"));
        let dir = suite_path.parent().expect("suite directory").to_path_buf();
        for rel in &suite.fixtures {
            let path = dir.join(rel);
            let fix = fixture::load_fixture(&path)
                .unwrap_or_else(|e| panic!("load fixture {}: {e}", path.display()));
            for run in RUNS {
                let bound = scratch::bind(&fix, run)
                    .unwrap_or_else(|e| panic!("bind {}: {e}", fix.id));
                out.push((bound, run));
            }
        }
    }
    assert!(out.len() > 20, "the suites went missing: {} runs", out.len());
    out
}

/// Absolute paths a command or a prompt names. Only the directories a fixture
/// can actually write are interesting; a URL path (`/api/v1/agents`) is not a
/// file and never matches.
fn paths(text: &str) -> Vec<String> {
    let re = regex::Regex::new(r"/(?:tmp|var|private|Users)[A-Za-z0-9_./@+-]*")
        .expect("path pattern");
    re.find_iter(text)
        .map(|m| m.as_str().trim_end_matches(['.', ',', '/']).to_string())
        .filter(|p| p.len() > 5)
        .collect()
}

/// Every path anywhere in a bound fixture: setup, teardown, cwd, conversation
/// and the assertions that quote them.
fn all_paths(fix: &Fixture) -> Vec<String> {
    let text = serde_yaml::to_string(fix).expect("render fixture");
    paths(&text)
}

/// The employees a fixture hires, read off the setup call that creates them.
fn employees(fix: &Fixture) -> Vec<String> {
    let re = regex::Regex::new(r#""name"\s*:\s*"([^"]+)""#).expect("name pattern");
    fix.setup
        .iter()
        .filter(|cmd| cmd.contains("/api/v1/agents"))
        .flat_map(|cmd| re.captures_iter(cmd).map(|c| c[1].to_string()))
        .collect()
}

#[test]
fn no_two_fixture_runs_name_the_same_path() {
    let mut owner: HashMap<String, String> = HashMap::new();
    for (fix, run) in bound_runs() {
        let who = format!("{} {}", fix.id, run);
        for path in all_paths(&fix) {
            if REFUSED.contains(&path.as_str()) {
                continue;
            }
            match owner.get(&path) {
                Some(other) if *other != who => panic!(
                    "{path} is named by both {other} and {who}: \
                     one of them deletes the other's work in a pool"
                ),
                _ => {
                    owner.insert(path, who.clone());
                }
            }
        }
    }
}

#[test]
fn no_two_fixture_runs_hire_the_same_employee() {
    let mut owner: HashMap<String, String> = HashMap::new();
    for (fix, run) in bound_runs() {
        let who = format!("{} {}", fix.id, run);
        for name in employees(&fix) {
            if let Some(other) = owner.insert(name.clone(), who.clone()) {
                panic!("the employee {name} is hired by both {other} and {who}");
            }
        }
    }
}

#[test]
fn a_fixture_deletes_exactly_the_employee_it_hired() {
    for (fix, run) in bound_runs() {
        let hired = employees(&fix);
        if hired.is_empty() {
            continue;
        }
        let teardown = fix.teardown.join("\n");
        for name in &hired {
            assert!(
                teardown.contains(name),
                "{} {run} hires {name} and never deletes it",
                fix.id
            );
        }
        // A teardown that deletes by any other employee's name would take a
        // coworker's work with it.
        assert!(
            !teardown.contains("front-desk'") && !teardown.contains("chief-of-staff'"),
            "{} {run} deletes an employee by a bare shared name: {teardown}",
            fix.id
        );
    }
}

#[test]
fn teardown_reaches_no_further_than_its_own_scratch() {
    for (fix, run) in bound_runs() {
        let own = scratch::dir(&fix.id, run);
        for cmd in &fix.teardown {
            for path in paths(cmd) {
                assert!(
                    path.starts_with(&own),
                    "{} {run} tears down {path}, which is outside its own {own}",
                    fix.id
                );
            }
        }
    }
}

/// The gate's lanes run fixtures only through `scripts/gate-run.sh`, which
/// gives every run a fresh server. A lane that called the runner with a whole
/// suite (`--suite`) or its own server address (`--server`) would run
/// several runs on one server again, and a run could quote what an earlier
/// run of its fixture saved to memory (2026-09-25).
#[test]
fn every_gate_lane_runs_fixtures_through_gate_run() {
    let wf = std::fs::read_to_string(repo_root().join(".github/workflows/harness-gate.yml")).expect("read the gate workflow");
    for line in wf.lines().filter(|l| !l.trim_start().starts_with('#')) {
        assert!(
            !line.contains("--suite") && !line.contains("--server"),
            "a lane runs the test runner around scripts/gate-run.sh: {}",
            line.trim()
        );
    }
    assert!(wf.matches("scripts/gate-run.sh ").count() >= 5, "the gate, nightly and sweep lanes all go through gate-run.sh");
}

/// `gate-run.sh` itself, with a stand-in server script beside it and a
/// stand-in runner: a fresh server before every run of every fixture, of a
/// suite and of a replay set alike, each run asked for on its own and keeping
/// its number, the bot's credentials never passed to the runner, and a run
/// that fails leaves the next runs to go ahead and fails the whole.
#[cfg(unix)]
#[test]
fn gate_run_starts_a_fresh_server_before_every_run() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let t = tmp.path();
    let write = |rel: &str, body: &str| {
        let p = t.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
        if rel.ends_with(".sh") {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    };
    std::fs::create_dir_all(t.join("scripts")).unwrap();
    std::fs::copy(repo_root().join("scripts/gate-run.sh"), t.join("scripts/gate-run.sh")).expect("copy gate-run.sh");
    write("scripts/gate-server.sh", "echo \"fresh $1\" >> \"$LOG\"; echo 27999 > \"$GATE_JOB/server.port\"\n");
    // Fails run 1 of fixture a, once.
    write(
        "runner.sh",
        "echo \"run $* bot=${A_BOT_ID-none}\" >> \"$LOG\"\ncase \"$*\" in *a.yaml*--first-run\\ 1\\ *) exit 1 ;; esac\n",
    );
    write("suites/s.yaml", "name: s\nfixtures:\n  - ../fixtures/a.yaml\n  - ../fixtures/b.yaml\n");
    write("home/harness-replays/set1/suite.yaml", "name: set1\nfixtures:\n  - thread-1.yaml\n");
    write("home/harness-replays/set1/thread-1.yaml", "id: thread-1\n");
    std::fs::create_dir_all(t.join("job")).unwrap();
    let log = t.join("log");

    let run = |entry: &str, runs: &str| {
        std::process::Command::new("bash")
            .arg(t.join("scripts/gate-run.sh"))
            .args([entry, runs, "out", "bash"])
            .arg(t.join("runner.sh"))
            .args(["test", "run", "--no-judge"])
            .current_dir(t)
            .env("GATE_JOB", t.join("job"))
            .env("HOME", t.join("home"))
            .env("LOG", &log)
            .env("A_BOT_ID", "the-bot-secret")
            .status()
            .expect("run gate-run.sh")
    };

    let suite = run("suites/s.yaml", "2");
    assert!(!suite.success(), "a failed run fails the entry");
    let lines: Vec<String> = std::fs::read_to_string(&log).unwrap().lines().map(str::to_string).collect();
    let fixtures = t.canonicalize().unwrap().join("fixtures");
    let expected: Vec<String> = [("a", 1), ("a", 2), ("b", 1), ("b", 2)]
        .iter()
        .flat_map(|(f, n)| {
            [
                "fresh fresh".to_string(),
                format!(
                    "run test run --no-judge --fixture {}/{f}.yaml --runs 1 --first-run {n} --server localhost:27999 --output out bot=none",
                    fixtures.display()
                ),
            ]
        })
        .collect();
    assert_eq!(lines, expected);

    std::fs::remove_file(&log).unwrap();
    assert!(run("replays/set1/suite.yaml", "2").success());
    let lines: Vec<String> = std::fs::read_to_string(&log).unwrap().lines().map(str::to_string).collect();
    let copied = t.join("job/replays/set1/thread-1.yaml");
    assert!(copied.exists(), "the replay set is copied into the job's own directory");
    assert_eq!(lines.len(), 4, "{lines:?}");
    for (i, n) in [1, 2].iter().enumerate() {
        assert_eq!(lines[2 * i], "fresh fresh");
        assert!(lines[2 * i + 1].contains(&format!("thread-1.yaml --runs 1 --first-run {n} ")), "{}", lines[2 * i + 1]);
        assert!(lines[2 * i + 1].ends_with("bot=none"));
    }
}
