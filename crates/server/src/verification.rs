//! Verified, as the app reads it: each piece of verified code work with its
//! badge and every step of its history (review rounds, the catch-up, the
//! landing, the live checks), each with its time and what it cost. The
//! record is `db::Verification` and its rounds, written by the verify loop
//! (`agent::harness::verify::engine`); every change it makes is broadcast
//! as [`EVENT`] with the view ([`view`]).
//!
//! The badge never says more than the record: "Verified" only when a round
//! with no blocking or major issue open passed the project's own checks;
//! otherwise "Not verified" with why. Work that isn't code has no badge at
//! all, because it has no record.

pub use crate::handlers::verifications::{Badge, RoundView, VerificationView};
use crate::state::AppState;

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
        reason: v.reason.clone(),
        landed: v.landed_ref.clone(),
    };
    let agent_name = store
        .get_agent(&v.agent_id)
        .ok()
        .flatten()
        .map(|a| a.name)
        .unwrap_or_default();
    let history = rounds
        .into_iter()
        .map(|round| {
            let cost_microcents = store.reviewers_cost(&round.reviewer_prefix).unwrap_or(0);
            RoundView {
                round,
                cost_microcents,
            }
        })
        .collect();
    let cost_microcents = store.reviewers_cost(&v.reviewer_prefix).unwrap_or(0);
    VerificationView {
        verification: v,
        agent_name,
        badge,
        history,
        cost_microcents,
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
                reason: "Stopped by you with 1 open issue(s)",
                rounds: 2,
                open_majors: 1,
                ..Default::default()
            },
            3,
        )
        .unwrap();
        let v = view(&s, s.get_verification("v-1").unwrap().unwrap());
        assert_eq!(
            (
                v.badge.state.as_str(),
                v.badge.reason.as_str(),
                v.badge.open_majors
            ),
            ("not_verified", "Stopped by you with 1 open issue(s)", 1)
        );
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(
            json["history"][0]["kind"], "review",
            "a round's own fields sit beside its cost"
        );
        assert_eq!(json["history"][0]["costMicrocents"], 0);
        assert_eq!(json["rounds"], 2, "the count of review rounds");
        assert_eq!(json["projectRoot"], "/site");
    }
}
