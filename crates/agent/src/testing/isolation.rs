//! The judged gate's fixtures must be able to run side by side.
//!
//! One measurement found four fixtures sharing `/tmp/nebo-fixture` — three of
//! them `rm -rf`ing it in teardown — and two more creating and deleting the
//! same employee. Nothing about a single-file run says so; it only shows up
//! the first time two fixtures run at once, as work deleted underneath a run
//! that was passing. This test reads both suites and says it out loud: after
//! [`scratch::bind`], no two fixture runs name the same directory, the same
//! file or the same employee, and no teardown reaches outside its own scratch.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::fixture::{self, Fixture};
use super::scratch;

/// The suites the gate runs, and the ones a pool would run first.
const SUITES: [&str; 2] = ["suites/error-correction.yaml", "suites/smoke.yaml"];

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
