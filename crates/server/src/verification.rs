//! Verified, as the app reads it: each piece of verified code work with its
//! badge, what it changed, and every step of its history (review rounds with
//! each reviewer and each of the project's checks, the catch-up, the landing,
//! the live checks), each with its time. The record is `db::Verification` and
//! its rounds, written by the verify loop (`agent::harness::verify::engine`)
//! as a round goes; every change it makes is broadcast as [`EVENT`] with the
//! view ([`view`]).
//!
//! The badge never says more than the record: "Verified" only when a round
//! with no blocking or major issue open passed the project's own checks;
//! otherwise "Not verified" with why. Work that isn't code has no badge at
//! all, because it has no record.

pub use crate::handlers::verifications::{Badge, CheckView, FileView, ReviewerView, RoundView, VerificationView};
use crate::state::AppState;
use types::Why;

/// Where `v`'s work is, by what it is and whether it landed: an app's pages,
/// a helper's copy and a coding agent's folder are changed where they are;
/// work in a copy this loop made has landed, waits in that copy, or had
/// nothing to land.
fn place(v: &db::Verification, rounds: &[db::VerificationRound]) -> &'static str {
    if !v.landed_ref.is_empty() {
        return "landed";
    }
    match v.work_kind.as_str() {
        "app_pages" => "app",
        "helper_copy" => "helper",
        "folder" => "folder",
        _ if v.isolation == "none" => "folder",
        _ if rounds.iter().rev().find(|r| r.kind == "land").and_then(|r| Why::read(&r.outcome)).is_some_and(|w| w.code == "nothing_to_land") => "nothing",
        _ => "copy",
    }
}

/// What the loop is doing now, by `v`'s state and its last step.
fn phase(v: &db::Verification, rounds: &[db::VerificationRound]) -> &'static str {
    match v.state.as_str() {
        "running" => {}
        "paused" => return "waiting",
        _ => return "done",
    }
    let Some(last) = rounds.last() else { return "working" };
    match last.kind.as_str() {
        "review" if last.finished_at.is_none() => {
            if check_views(last).iter().any(|c| c.state == "waiting" || c.state == "running") {
                "checks"
            } else {
                "reviewers"
            }
        }
        "review" if Why::read(&last.outcome).is_some_and(|w| w.code == "review_clean") => "landing",
        "review" => "fixing",
        "live" => "live",
        _ => "landing",
    }
}

/// The project by the name the owner knows it: an app's pages by the app's
/// name, any other folder by its own.
fn project(store: &db::Store, v: &db::Verification) -> String {
    if v.work_kind == "app_pages"
        && let Ok(Some(name)) = store.app_name_by_ui_path(&v.project_root)
    {
        return name;
    }
    std::path::Path::new(&v.project_root)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| v.project_root.clone())
}

/// The files of a change as the loop writes it (`ChangeSet::stat`: one
/// `M src/lib.rs (+4 -1)` line per file).
fn files(stat: &str) -> Vec<FileView> {
    stat.lines()
        .filter_map(|line| {
            let (mark, rest) = line.split_once(' ')?;
            let (path, counts) = rest.rsplit_once(" (+")?;
            let (added, removed) = counts.trim_end_matches(')').split_once(" -")?;
            Some(FileView {
                path: path.rsplit_once(" -> ").map_or(path, |(_, to)| to).to_string(),
                status: match mark {
                    "A" => "added",
                    "D" => "deleted",
                    "R" => "renamed",
                    _ => "modified",
                }
                .to_string(),
                added: added.parse().ok()?,
                removed: removed.parse().ok()?,
            })
        })
        .collect()
}

/// The lines of a failed check's output the app shows: its last ones.
const TAIL_LINES: usize = 12;

/// Each of the project's checks in `round`: the ones it ran with what they
/// came to, then (while it runs) the one running and the ones waiting.
fn check_views(round: &db::VerificationRound) -> Vec<CheckView> {
    let ran: Vec<&serde_json::Value> = round.checks.as_array().into_iter().flatten().collect();
    let mut out: Vec<CheckView> = ran
        .iter()
        .map(|c| {
            let passed = c["passed"].as_bool().unwrap_or(false);
            let tail = if passed {
                String::new()
            } else {
                let lines: Vec<&str> = c["tail"].as_str().unwrap_or("").lines().collect();
                lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n")
            };
            CheckView {
                label: c["label"].as_str().unwrap_or("").to_string(),
                state: if passed { "passed" } else { "failed" }.to_string(),
                duration_ms: c["durationMs"].as_u64().unwrap_or(0),
                tail,
            }
        })
        .collect();
    if round.finished_at.is_none() {
        let planned = round.detail["checksPlanned"].as_array().into_iter().flatten().filter_map(|l| l.as_str());
        for (i, label) in planned.skip(ran.len()).enumerate() {
            out.push(CheckView {
                label: label.to_string(),
                state: if i == 0 { "running" } else { "waiting" }.to_string(),
                duration_ms: 0,
                tail: String::new(),
            });
        }
    }
    out
}

/// Each reviewer of `round`: its state as the loop recorded it while the
/// round ran, else as the finished round says.
fn reviewer_views(round: &db::VerificationRound) -> Vec<ReviewerView> {
    let findings: Vec<&serde_json::Value> = round.findings.as_array().into_iter().flatten().collect();
    let failed: Vec<&str> = round.detail["failedReviewers"].as_array().into_iter().flatten().filter_map(|f| f.as_str()).collect();
    // A round that could not be reviewed at all (its reviewers' copy could
    // not be made) names no reviewer that failed: every one of them did.
    let none_reviewed = round.finished_at.is_some()
        && round.detail["reviewers"].is_null()
        && failed.is_empty()
        && Why::read(&round.outcome).is_some_and(|w| w.code == "review_incomplete");
    round
        .lenses
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| l.as_str())
        .map(|lens| {
            let found = findings.iter().filter(|f| f["lens"].as_str() == Some(lens)).count();
            let state = match round.detail["reviewers"][lens].as_str() {
                Some(state) => state,
                None if round.finished_at.is_none() => "waiting",
                None if none_reviewed => "failed",
                None if failed.iter().any(|f| f.starts_with(&format!("{lens}:"))) => "failed",
                None if found > 0 => "found",
                None => "clean",
            };
            ReviewerView { lens: lens.to_string(), state: state.to_string(), findings: found }
        })
        .collect()
}

/// Hub event carrying one verification ([`VerificationView`]) whenever it
/// changes.
pub const EVENT: &str = "verification_updated";

/// `v` as the app reads it.
pub fn view(store: &db::Store, v: db::Verification) -> VerificationView {
    let rounds = store.verification_rounds(&v.id).unwrap_or_default();
    let reviews: Vec<&db::VerificationRound> =
        rounds.iter().filter(|r| r.kind == "review").collect();
    let mut lenses: Vec<String> = Vec::new();
    for r in &reviews {
        for l in r
            .lenses
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| l.as_str())
        {
            if !lenses.iter().any(|x| x == l) {
                lenses.push(l.to_string());
            }
        }
    }
    let reviewers = reviews
        .iter()
        .map(|r| r.lenses.as_array().map(|a| a.len()).unwrap_or(0))
        .sum();
    let last_checks: Vec<(String, bool)> = rounds
        .iter()
        .rev()
        .find(|r| r.checks.as_array().is_some_and(|a| !a.is_empty()))
        .map(|r| {
            r.checks
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| {
                    (
                        c["label"].as_str().unwrap_or("").to_string(),
                        c["passed"].as_bool().unwrap_or(false),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let state = match v.state.as_str() {
        "verified" => "verified",
        "running" => "checking",
        "paused" => "waiting",
        _ => "not_verified",
    };
    let badge = Badge {
        state: state.to_string(),
        reviewers,
        lenses,
        rounds: v.rounds,
        checks_passed: !last_checks.is_empty() && last_checks.iter().all(|(_, ok)| *ok),
        checks: last_checks.into_iter().map(|(l, _)| l).collect(),
        open_blockers: v.open_blockers,
        open_majors: v.open_majors,
        open_minors: v.open_minors,
        deferred: v.deferred.as_array().map(|a| a.len()).unwrap_or(0),
        reason: Why::read(&v.reason),
        landed: v.landed_ref.clone(),
        place: place(&v, &rounds).to_string(),
        phase: phase(&v, &rounds).to_string(),
        round: reviews.len(),
        project: project(store, &v),
    };
    // The whole change: as the last review round saw it, or (a round from
    // before it was recorded) the first round's, which reviewed it whole.
    let changed = rounds
        .iter()
        .rev()
        .find_map(|r| r.detail["whole"].as_str().map(files))
        .or_else(|| rounds.iter().find(|r| r.kind == "review").map(|r| files(&r.diff_stat)))
        .unwrap_or_default();
    let agent_name = store
        .get_agent(&v.agent_id)
        .ok()
        .flatten()
        .map(|a| a.name)
        .unwrap_or_default();
    let history = rounds
        .into_iter()
        .map(|round| RoundView {
            result: Why::read(&round.outcome),
            files: files(&round.diff_stat),
            reviewers: reviewer_views(&round),
            check_runs: check_views(&round),
            round,
        })
        .collect();
    VerificationView {
        verification: v,
        agent_name,
        badge,
        history,
        changed,
    }
}

/// Tell every app verification `id` changed.
pub(crate) fn broadcast(state: &AppState, id: &str) {
    let Ok(Some(v)) = state.store.get_verification(id) else {
        return;
    };
    match serde_json::to_value(view(&state.store, v)) {
        Ok(payload) => state.hub.broadcast(EVENT, payload),
        Err(e) => tracing::warn!(error = %e, "verification: event not serialized"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> db::Store {
        let path = std::env::temp_dir().join(format!(
            "nebo-verification-view-{}.db",
            uuid::Uuid::new_v4()
        ));
        db::Store::new(&path.to_string_lossy()).unwrap()
    }

    #[test]
    fn the_badge_says_what_the_record_holds_and_no_more() {
        let s = store();
        s.create_verification(
            &db::NewVerification {
                id: "v-1",
                session_key: "agent:dev:web",
                spec: "Add b",
                project_root: "/site",
                work_dir: "/copy",
                isolation: "copy",
                reviewer_prefix: "subagent:agent:dev:web:sa-verify-v-1-",
                ..Default::default()
            },
            1,
        )
        .unwrap();
        for (round, lenses, checks) in [
            (
                1,
                serde_json::json!(["correctness", "billing"]),
                serde_json::json!([{"label": "Rust tests", "passed": false}]),
            ),
            (
                2,
                serde_json::json!(["billing"]),
                serde_json::json!([{"label": "Rust build", "passed": true}, {"label": "Rust tests", "passed": true}]),
            ),
        ] {
            s.put_verification_round(&db::VerificationRound {
                verification_id: "v-1".into(),
                round,
                kind: "review".into(),
                lenses,
                checks,
                reviewer_prefix: format!("subagent:agent:dev:web:sa-verify-v-1-r{round}-"),
                started_at: 2,
                ..Default::default()
            })
            .unwrap();
        }
        let checking = view(&s, s.get_verification("v-1").unwrap().unwrap());
        assert_eq!(checking.badge.state, "checking");
        assert_eq!(
            checking.badge.reviewers, 3,
            "two in round 1, one in round 2"
        );
        assert_eq!(checking.badge.lenses, ["correctness", "billing"]);
        assert_eq!(
            checking.badge.checks,
            ["Rust build", "Rust tests"],
            "the last round's checks"
        );
        assert!(checking.badge.checks_passed);

        s.set_verification_state(
            "v-1",
            &db::VerificationState {
                state: "not_verified",
                reason: r#"{"code":"stopped_by_owner","args":{"open":1}}"#,
                rounds: 2,
                open_majors: 1,
                ..Default::default()
            },
            3,
        )
        .unwrap();
        let v = view(&s, s.get_verification("v-1").unwrap().unwrap());
        assert_eq!((v.badge.state.as_str(), v.badge.open_majors, v.badge.place.as_str()), ("not_verified", 1, "copy"));
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(
            json["badge"]["reason"],
            serde_json::json!({"code": "stopped_by_owner", "args": {"open": 1}}),
            "a code and its values, never a sentence"
        );
        assert_eq!(json["history"][0]["kind"], "review", "a round's own fields sit beside what the app shows of it");
        assert!(json.get("costMicrocents").is_none() && json["history"][0].get("costMicrocents").is_none(), "no cost is shown");
        assert_eq!(json["rounds"], 2, "the count of review rounds");
        assert_eq!(json["projectRoot"], "/site");
    }

    /// Where the work is follows what it is and whether it landed: never "in
    /// its own copy" for work that has none.
    #[test]
    fn the_badge_says_where_the_work_is() {
        let s = store();
        for (id, kind, isolation) in [("v-app", "app_pages", "none"), ("v-help", "helper_copy", "none"), ("v-dir", "folder", "none"), ("v-own", "isolated", "none"), ("v-cp", "isolated", "worktree")] {
            s.create_verification(
                &db::NewVerification { id, session_key: "agent:dev:web", work_kind: kind, isolation, ..Default::default() },
                1,
            )
            .unwrap();
        }
        let place = |id: &str| view(&s, s.get_verification(id).unwrap().unwrap()).badge.place;
        assert_eq!(
            ["v-app", "v-help", "v-dir", "v-own", "v-cp"].map(place),
            ["app", "helper", "folder", "folder", "copy"]
        );
        s.put_verification_round(&db::VerificationRound {
            verification_id: "v-cp".into(),
            round: 1,
            kind: "land".into(),
            outcome: r#"{"code":"nothing_to_land"}"#.into(),
            started_at: 2,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(place("v-cp"), "nothing");
        s.set_verification_landed("v-cp", "abc", 3).unwrap();
        assert_eq!(place("v-cp"), "landed");
        let json = serde_json::to_value(view(&s, s.get_verification("v-cp").unwrap().unwrap())).unwrap();
        assert_eq!(json["history"][0]["result"], serde_json::json!({"code": "nothing_to_land"}));
    }

    /// A round in progress shows what is being checked and how far it got:
    /// the files with their counts, each check (passed with its time, the
    /// one running, the ones waiting), each reviewer (its state and what it
    /// found so far), the round it is on and what the loop is doing.
    #[test]
    fn a_round_in_progress_shows_each_check_and_reviewer_as_it_goes() {
        let s = store();
        s.create_verification(
            &db::NewVerification { id: "v-live", session_key: "agent:dev:thread:c1", project_root: "/work/site", isolation: "worktree", ..Default::default() },
            1,
        )
        .unwrap();
        let shown = || view(&s, s.get_verification("v-live").unwrap().unwrap());
        assert_eq!((shown().badge.phase.as_str(), shown().badge.round), ("working", 0), "the change is still being written");
        assert_eq!(shown().badge.project, "site");

        let mut row = db::VerificationRound {
            verification_id: "v-live".into(),
            round: 1,
            kind: "review".into(),
            lenses: serde_json::json!(["correctness", "security"]),
            diff_stat: "M src/a.rs (+4 -1)\nA src/b.rs (+20 -0)\nR old.rs -> new.rs (+0 -2)".into(),
            detail: serde_json::json!({"whole": "M src/a.rs (+4 -1)\nA src/b.rs (+20 -0)\nR old.rs -> new.rs (+0 -2)", "checksPlanned": ["Rust build", "Rust tests"]}),
            checks: serde_json::json!([{"label": "Rust build", "command": "cargo build", "passed": true, "tail": "ok", "durationMs": 4200}]),
            started_at: 2,
            ..Default::default()
        };
        s.put_verification_round(&row).unwrap();
        let v = shown();
        assert_eq!((v.badge.phase.as_str(), v.badge.round), ("checks", 1));
        assert_eq!(
            v.changed,
            vec![
                FileView { path: "src/a.rs".into(), status: "modified".into(), added: 4, removed: 1 },
                FileView { path: "src/b.rs".into(), status: "added".into(), added: 20, removed: 0 },
                FileView { path: "new.rs".into(), status: "renamed".into(), added: 0, removed: 2 },
            ]
        );
        let checks: Vec<(&str, &str, u64)> = v.history[0].check_runs.iter().map(|c| (c.label.as_str(), c.state.as_str(), c.duration_ms)).collect();
        assert_eq!(checks, [("Rust build", "passed", 4200), ("Rust tests", "running", 0)]);
        assert!(v.history[0].reviewers.iter().all(|r| r.state == "waiting"), "reviewers start after the checks");

        let tail: String = (1..=30).map(|n| format!("line {n}\n")).collect();
        row.checks = serde_json::json!([
            {"label": "Rust build", "command": "cargo build", "passed": true, "tail": "ok", "durationMs": 4200},
            {"label": "Rust tests", "command": "cargo test", "passed": false, "tail": tail, "durationMs": 9000}
        ]);
        row.detail["reviewers"] = serde_json::json!({"correctness": "found", "security": "running"});
        row.findings = serde_json::json!([{"id": "r1-correctness-1", "lens": "correctness", "severity": "major", "title": "Off by one", "evidence": "src/a.rs:12"}]);
        s.put_verification_round(&row).unwrap();
        let v = shown();
        assert_eq!(v.badge.phase, "reviewers");
        let failed = &v.history[0].check_runs[1];
        assert_eq!(failed.state, "failed");
        assert_eq!(failed.tail.lines().count(), TAIL_LINES, "the last lines of a failed check");
        assert_eq!(failed.tail.lines().last(), Some("line 30"));
        assert_eq!(v.history[0].check_runs[0].tail, "", "a passed check shows no output");
        let reviewers: Vec<(&str, &str, usize)> = v.history[0].reviewers.iter().map(|r| (r.lens.as_str(), r.state.as_str(), r.findings)).collect();
        assert_eq!(reviewers, [("correctness", "found", 1), ("security", "running", 0)]);

        // The round ends with an issue open: the builder fixes it.
        row.detail["reviewers"]["security"] = serde_json::json!("clean");
        row.outcome = r#"{"code":"review_blocking","args":{"count":1}}"#.into();
        row.finished_at = Some(5);
        s.put_verification_round(&row).unwrap();
        assert_eq!(shown().badge.phase, "fixing");
        // Round 2 reviews only what changed since.
        s.put_verification_round(&db::VerificationRound {
            verification_id: "v-live".into(),
            round: 2,
            kind: "review".into(),
            lenses: serde_json::json!(["correctness"]),
            diff_stat: "M src/a.rs (+1 -1)".into(),
            detail: serde_json::json!({"whole": "M src/a.rs (+4 -1)\nA src/b.rs (+20 -0)", "checksPlanned": []}),
            started_at: 6,
            ..Default::default()
        })
        .unwrap();
        let v = shown();
        assert_eq!((v.badge.phase.as_str(), v.badge.round), ("reviewers", 2));
        assert_eq!(v.history[1].files, vec![FileView { path: "src/a.rs".into(), status: "modified".into(), added: 1, removed: 1 }]);
        assert_eq!(v.changed.len(), 2, "the whole change as the last round saw it");
    }

    /// A finished round from before its reviewers were recorded live reads
    /// each one's state from what it found and whether it failed.
    #[test]
    fn a_finished_round_reads_each_reviewer_from_its_findings() {
        let s = store();
        s.create_verification(&db::NewVerification { id: "v-old", session_key: "agent:dev:web", isolation: "none", ..Default::default() }, 1).unwrap();
        s.put_verification_round(&db::VerificationRound {
            verification_id: "v-old".into(),
            round: 1,
            kind: "review".into(),
            lenses: serde_json::json!(["correctness", "security", "data"]),
            findings: serde_json::json!([{"lens": "security", "severity": "blocker", "title": "t", "evidence": "a.rs:1"}]),
            detail: serde_json::json!({"failedReviewers": ["data: no balance"]}),
            started_at: 2,
            finished_at: Some(3),
            ..Default::default()
        })
        .unwrap();
        let v = view(&s, s.get_verification("v-old").unwrap().unwrap());
        let states: Vec<&str> = v.history[0].reviewers.iter().map(|r| r.state.as_str()).collect();
        assert_eq!(states, ["clean", "found", "failed"]);
    }

    /// A round that could not be reviewed at all (its reviewers' copy could
    /// not be made) shows each reviewer as failed, never "no issues".
    #[test]
    fn a_round_nobody_could_review_shows_each_reviewer_failed() {
        let s = store();
        s.create_verification(&db::NewVerification { id: "v-none", session_key: "agent:dev:web", isolation: "none", ..Default::default() }, 1).unwrap();
        s.put_verification_round(&db::VerificationRound {
            verification_id: "v-none".into(),
            round: 1,
            kind: "review".into(),
            lenses: serde_json::json!(["correctness", "security"]),
            outcome: r#"{"code":"review_incomplete"}"#.into(),
            started_at: 2,
            finished_at: Some(3),
            ..Default::default()
        })
        .unwrap();
        let v = view(&s, s.get_verification("v-none").unwrap().unwrap());
        let states: Vec<&str> = v.history[0].reviewers.iter().map(|r| r.state.as_str()).collect();
        assert_eq!(states, ["failed", "failed"]);
    }

    /// An app's pages go by the app's name, found by the folder they are
    /// served from.
    #[test]
    fn an_apps_pages_go_by_the_apps_name() {
        let s = store();
        s.create_agent("app-1", None, "Media Studio", "", "", "", None, None).unwrap();
        s.set_agent_app_fields("app-1", true, Some("/agents/Media Studio/ui"), None, None).unwrap();
        s.create_verification(
            &db::NewVerification { id: "v-app", session_key: "agent:dev:web", work_kind: "app_pages", isolation: "none", project_root: "/agents/Media Studio/ui", ..Default::default() },
            1,
        )
        .unwrap();
        assert_eq!(view(&s, s.get_verification("v-app").unwrap().unwrap()).badge.project, "Media Studio");
        assert_eq!(s.app_name_by_ui_path("/elsewhere").unwrap(), None);
    }
}

