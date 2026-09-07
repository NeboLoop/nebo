//! Proof scenarios: platform-level (uc52–uc56). See `mod.rs`.

use super::*;
use serde_json::json;

fn lead() -> CaseBinding<'static> {
    World::binding("ic", "work-lead", "lead", 3 * DAY)
}

/// uc52 — Any external system as the source of truth, with the case
/// holding only a reference. A ticket keyed by the CRM's id: the case
/// carries the id as its alias, every update to that record rides the one
/// case, and a run bound to the record is found by the reference alone.
#[test]
fn uc52_an_external_system_is_the_source_of_truth_and_the_case_holds_a_reference() {
    let w = World::new();
    let b = World::binding("ops", "work-ticket", "ticket", 2 * DAY);
    let Routed::Opened { case_id } = w.arrive(&b, "crm", "INV-1042", json!({"crm_id": "INV-1042", "status": "open"}), "crm-1") else { panic!("opened") };
    let inputs: serde_json::Value = serde_json::from_str(w.run(&case_id).inputs.as_deref().unwrap()).unwrap();
    let aliases = inputs["_case"]["aliases"].as_array().unwrap();
    assert_eq!(aliases.len(), 1);
    assert_eq!(aliases[0]["kind"], json!("crm"));
    assert!(aliases[0]["value"].as_str().unwrap().eq_ignore_ascii_case("INV-1042"), "the reference, not the record");
    assert!(!inputs["_case"].to_string().contains("\"status\""), "the case does not copy the record's fields");

    // The record changes: the same case hears it.
    assert_eq!(w.arrive(&b, "crm", "INV-1042", json!({"crm_id": "INV-1042", "status": "paid"}), "crm-2"), Routed::Signaled { case_id: case_id.clone() });

    // A run bound to the record by reference is found by that reference.
    w.s.engine_create_run(&NewRun { id: "sync-1", kind: "task", session_key: "agent:ops:web", agent_id: "ops", lane: "main", external_ref: Some("crm:INV-1042"), ..Default::default() }).unwrap();
    assert!(w.s.engine_has_live_run_for_ref("crm:INV-1042").unwrap());
    assert_eq!(w.s.engine_runs_for_ref("crm:INV-1042", 10, 0).unwrap().len(), 1);
}

/// uc53 — Sub-agent fan-out with real parent links, so cancelling a parent
/// cancels its children. A root with two children and a grandchild:
/// cancelling the root takes every live descendant, and leaves the child
/// that had already finished as it ended.
#[test]
fn uc53_cancelling_a_parent_cancels_its_whole_fan_out() {
    let w = World::new();
    let mk = |id: &str, parent: Option<&str>| {
        w.s.create_pending_task(id, "subagent", "agent:a:web", None, "do the thing", None, None, None, 0, parent).unwrap();
    };
    mk("root", None);
    mk("c1", Some("root"));
    mk("c2", Some("root"));
    mk("g1", Some("c1"));
    assert_eq!(w.run("c1").parent_run_id.as_deref(), Some("root"), "the parent link is written");
    assert_eq!(w.run("g1").parent_run_id.as_deref(), Some("c1"));
    w.s.engine_set_run_state("c1", "running", w.t, None).unwrap();
    w.s.engine_set_run_state("c2", "done", w.t, None).unwrap();

    w.s.cancel_task("root").unwrap();
    w.s.cancel_child_tasks("root").unwrap();
    assert_eq!(w.run("root").state, "cancelled");
    assert_eq!(w.run("c1").state, "cancelled", "a running child");
    assert_eq!(w.run("g1").state, "cancelled", "a grandchild, not yet started");
    assert_eq!(w.run("c2").state, "done", "a finished child keeps its ending");
    assert!(w.s.engine_queued_runs("main", 10).unwrap().is_empty(), "nothing of the fan-out is left to start");
}

/// uc54 — Restart-safe everything: at most one resume, then a human
/// decides. A turn interrupted by a crash resumes once; interrupted again
/// it fails, the owner is told, and the case's history says so. A clean
/// shutdown spends none of that budget.
#[test]
fn uc54_one_resume_after_a_crash_then_a_human_decides() {
    let w = World::new();
    let b = lead();
    let (case, queued) = w.open(&b, "dee@x.com", "hello", "m1");
    let turn = w.start(&queued.id);

    // Clean shutdown: back to the queue, budget untouched.
    suspend_for_shutdown(&w.s);
    assert_eq!(recover(&w.s), 0);
    let run = w.run(&turn.id);
    assert_eq!((run.state.as_str(), run.resume_attempted), ("queued", 0));

    // Crash once: resumed, once.
    w.start(&turn.id);
    assert_eq!(recover(&w.s), 1);
    let run = w.run(&turn.id);
    assert_eq!((run.state.as_str(), run.resume_attempted), ("queued", 1));

    // Crash again: failed, not retried, the owner told, the case told.
    w.start(&turn.id);
    assert_eq!(recover(&w.s), 0);
    assert_eq!(w.run(&turn.id).state, "failed");
    assert!(w.card(&turn.id).unwrap().contains("interrupted by a restart twice"));
    assert!(w.history_has(&case, "needs_attention", "interrupted by a restart twice"));
    assert!(w.queued_turn(&case).is_none(), "no third attempt starts on its own");
}

/// uc55 — A single audit history per case, readable by the owner. Every
/// durable thing that happened — turns, a failure, a give-up, a
/// reopening — is one ordered history on the case, read by the same store
/// queries the inspector uses, and history rows wake nothing.
#[test]
fn uc55_one_audit_history_per_case() {
    let mut w = World::new();
    let b = lead();
    let (case, _) = w.open(&b, "aud@x.com", "hello", "m1");
    w.turn(&case, &waits("contacted", "sent first contact", "signal", "1d", "their answer"));
    w.advance(DAY);
    let second = w.start(&w.queued_turn(&case).unwrap().id);
    w.fail(&second, "provider unreachable");
    w.t += 60;
    w.tick();
    w.turn(&case, &closes("unresponsive", "no answer; door stays open"));
    w.t += 30 * DAY;
    assert!(matches!(w.arrive(&b, "email", "aud@x.com", json!({"email": "aud@x.com", "message": "back"}), "m2"), Routed::Reopened { .. }));

    let history = w.history(&case);
    let kinds: Vec<&str> = history.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, ["turn_result", "turn_failed", "turn_result", "reopened"], "{kinds:?}");
    assert!(history.windows(2).all(|p| p[0].id < p[1].id && p[0].created_at <= p[1].created_at), "ordered");
    assert!(history[0].payload.contains("sent first contact") && history[0].payload.contains("no send on the ledger this turn"));
    assert!(history[1].payload.contains("provider unreachable"));
    assert!(history[3].payload.contains("unresponsive"));
    assert!(w.undelivered().is_empty(), "history rows wake nothing");

    // The inspector's reads: the case in the list, its turns, its waits.
    let cases = w.s.engine_cases(Some("ic"), 10).unwrap();
    assert_eq!(cases.len(), 1);
    assert_eq!(w.s.engine_children(&case).unwrap().len(), 4, "three turns and the one the reopening queued");
    let waits = w.s.engine_waits_for_run(&case).unwrap();
    assert_eq!(waits.iter().filter(|x| x.superseded_at.is_none()).count(), 1, "one live wait, the rest superseded and kept");
    assert!(waits.len() >= 4);
}

/// uc56 — Cross-employee handoffs where one case moves between roles
/// without losing history. The intake employee hands the case to the
/// closer: the case keeps its id and history, the closer's binding takes
/// the next message, the intake binding is refused as a conflict, and the
/// turns show who worked it when.
#[test]
fn uc56_a_case_moves_between_employees_with_its_history() {
    let w = World::new();
    let ic = lead();
    let closer = World::binding("closer", "close-deal", "lead", 3 * DAY);
    let (case, _) = w.open(&ic, "deal@x.com", "ready to buy", "m1");
    let first = w.turn(&case, &waits("qualified", "qualified; handing to the closer", "signal", "3d", "the closer's first call"));
    assert_eq!(first.agent_id, "ic");
    assert!(w.s.engine_reassign_run(&case, "closer").unwrap());
    assert_eq!(w.run(&case).agent_id, "closer");

    assert_eq!(w.arrive(&closer, "email", "deal@x.com", json!({"email": "deal@x.com", "message": "when can we talk?"}), "m2"), Routed::Signaled { case_id: case.clone() });
    assert_eq!(w.tick().children_started, 1);
    let turn = w.queued_turn(&case).unwrap();
    assert_eq!(turn.agent_id, "closer", "the closer works the next turn");
    assert!(turn.inputs.as_deref().unwrap().contains("qualified; handing to the closer"), "with the intake history");

    let routed = w.arrive(&ic, "email", "deal@x.com", json!({"email": "deal@x.com", "message": "stray"}), "m3");
    assert_eq!(routed, Routed::Conflict { case_id: case.clone(), owner: "closer".into() });
    assert!(w.card(&format!("conflict:{case}:ic")).is_some(), "the owner decides the routing");
    let turns = w.s.engine_children(&case).unwrap();
    assert_eq!(turns.iter().map(|t| t.agent_id.as_str()).collect::<Vec<_>>(), ["ic", "closer"]);
}
