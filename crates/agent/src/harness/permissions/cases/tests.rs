//! The five surfaced cases, driven through the registry and the one check
//! the runner uses. Each of `fixtures/permissions/`'s five-case fixtures
//! has its code-decided scenario here: the ask (or the run) is decided by
//! code from the employee's history and the call's effects, never by a
//! judge.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use tools::registry::DynTool;
use tools::{Origin, Registry, ToolContext, ToolResult};
use types::permissions::{
    AskCase, CallEffects, Effect, JudgedBy, JudgementMode, Knowable, MoneyLimit, Rule, RuleField, RuleKey,
    RuleSource, Scope, Verdict, Writer,
};
use types::provenance::ProvenanceClass;

use super::super::{CheckCx, Check, question_for, resolve_grant};

fn store() -> (tempfile::TempDir, Arc<db::Store>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(db::Store::new(&dir.path().join("c.db").to_string_lossy()).unwrap());
    (dir, store)
}

fn allow(store: &db::Store, agent: &str, key: RuleKey, money: Option<MoneyLimit>) -> Rule {
    let rule = Rule {
        id: uuid::Uuid::new_v4().to_string(),
        scope: Scope::Employee(agent.into()),
        key,
        field: None,
        effect: Effect::Allow,
        money,
        source: RuleSource::Owner,
        locked: false,
        created_at: 0,
    };
    store.write_permission_rule(&rule, &Writer::Owner).unwrap()
}

/// A tool whose call names its own effects in its input, and counts the
/// calls that ran. Its name is its rule key.
struct Act {
    name: &'static str,
    capability: Option<&'static str>,
    operation: Option<&'static str>,
    ran: Arc<AtomicUsize>,
}

impl Act {
    fn new(name: &'static str, capability: Option<&'static str>) -> (Self, Arc<AtomicUsize>) {
        let ran = Arc::new(AtomicUsize::new(0));
        (Self { name, capability, operation: None, ran: ran.clone() }, ran)
    }
}

impl DynTool for Act {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> String {
        String::new()
    }
    fn schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }
    fn capability(&self, _input: &serde_json::Value) -> Option<&'static str> {
        self.capability
    }
    fn operation_performed(&self, _input: &serde_json::Value) -> Option<String> {
        self.operation.map(str::to_string)
    }
    fn read_only(&self, input: &serde_json::Value) -> bool {
        input["read"].as_bool().unwrap_or(false)
    }
    fn effects(&self, input: &serde_json::Value) -> CallEffects {
        let list = |k: &str| -> Vec<String> {
            input[k].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default()
        };
        CallEffects {
            money_cents: input["amount_cents"].as_i64(),
            counterparty: None,
            recipients: list("to"),
            publishes: match input["publishes"].as_str() {
                Some("yes") => Knowable::Yes,
                Some("unknown") => Knowable::Unknown,
                _ => Knowable::No,
            },
            deletes: list("deletes"),
            overwrites: list("overwrites"),
            creates: list("creates"),
        }
    }
    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        _input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        self.ran.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { ToolResult::ok("RAN") })
    }
}

async fn registry(store: &Arc<db::Store>, acts: Vec<Act>) -> Registry {
    let registry = Registry::new(Arc::new(Check::new(store.clone())));
    for a in acts {
        registry.register(Box::new(a)).await;
    }
    registry
}

/// An employee's run in Automatic mode, its grant resolved as the seat does.
fn ctx(store: &db::Store, agent: &str) -> ToolContext {
    ToolContext {
        origin: Origin::User,
        session_key: format!("agent:{agent}:web"),
        session_id: "s1".into(),
        grant: Some(Arc::new(resolve_grant(store, agent, None))),
        ..Default::default()
    }
}

/// The case the parked call's ask names.
fn asked(store: &db::Store, r: &ToolResult) -> AskCase {
    let id = r.parked_ask.as_deref().unwrap_or_else(|| panic!("expected a parked ask: {}", r.content));
    serde_json::from_str(&store.get_permission_ask(id).unwrap().unwrap().ask_case).unwrap()
}

// ── Case 1: money ──────────────────────────────────────────────────────

/// fixtures/permissions/spend-over-grant-asks.yaml: bills up to $100 each
/// on its own; a $450 bill asks before it runs.
#[tokio::test]
async fn spend_over_grant_asks() {
    let (_d, store) = store();
    allow(&store, "clerk", RuleKey::Operation("ledger.bill.create".into()), Some(MoneyLimit {
        per_action_cents: Some(10_000),
        ..Default::default()
    }));
    let (mut pay, ran) = Act::new("plugin", None);
    pay.operation = Some("ledger.bill.create");
    let reg = registry(&store, vec![pay]).await;
    let r = reg.execute(&ctx(&store, "clerk"), "plugin", json!({ "amount_cents": 45_000 })).await;
    assert_eq!(asked(&store, &r), AskCase::Money { cents: 45_000, limit_cents: Some(10_000) });
    assert!(r.content.contains("money limit"), "{}", r.content);
    assert_eq!(ran.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn spend_inside_grant_runs_and_counts() {
    let (_d, store) = store();
    let rule = allow(&store, "clerk", RuleKey::Operation("ledger.bill.create".into()), Some(MoneyLimit {
        per_action_cents: Some(10_000),
        per_day_cents: Some(15_000),
        ..Default::default()
    }));
    let (mut pay, ran) = Act::new("plugin", None);
    pay.operation = Some("ledger.bill.create");
    let reg = registry(&store, vec![pay]).await;
    let c = ctx(&store, "clerk");
    assert_eq!(reg.execute(&c, "plugin", json!({ "amount_cents": 8_000 })).await.content, "RAN");
    // The day's total now stands at $80: another $80 is inside the per-bill
    // limit but past the day's $150.
    let over_day = reg.execute(&c, "plugin", json!({ "amount_cents": 8_000 })).await;
    assert!(over_day.parked_ask.is_some(), "{}", over_day.content);
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    let spent = store.permission_spend("clerk", &super::super::today(), rule.key.value(), "").unwrap();
    assert_eq!((spent.count, spent.cents), (1, 8_000));
}

// ── Case 2: speaking for the owner somewhere new ───────────────────────

/// fixtures/permissions/first-message-to-new-recipient-asks.yaml: a text to
/// a number it has never been in touch with asks.
#[tokio::test]
async fn first_message_to_new_recipient_asks() {
    let (_d, store) = store();
    let (sms, ran) = Act::new("sms", None);
    let reg = registry(&store, vec![sms]).await;
    let r = reg.execute(&ctx(&store, "care"), "sms", json!({ "to": ["+1-555-0142"] })).await;
    assert_eq!(asked(&store, &r), AskCase::NewCounterparty { who: "+1-555-0142".into() });
    assert_eq!(ran.load(Ordering::SeqCst), 0);
    // A publish is speaking for the owner somewhere new as well.
    let r = reg.execute(&ctx(&store, "care"), "sms", json!({ "publishes": "yes" })).await;
    assert_eq!(asked(&store, &r), AskCase::NewCounterparty { who: super::PUBLIC.into() });
}

/// fixtures/permissions/reply-in-thread-does-not-ask.yaml: the customer
/// wrote in (the inbound routing records them as someone the employee
/// works with); the reply runs with no ask, however the number is typed.
#[tokio::test]
async fn reply_in_thread_does_not_ask() {
    let (_d, store) = store();
    // The inbound text from the customer, routed to this employee's case.
    let binding = workflow::cases::CaseBinding {
        agent_id: "care",
        binding_name: "texts",
        case_type: "support".to_string(),
        definition_json: r#"{"name":"support","activities":[]}"#,
        base_inputs: json!({}),
        default_wait_secs: 3600,
    };
    workflow::cases::signal_or_open(&store, &binding, "phone", "+15550198", &json!({"text": "shipped yet?"}), "sms", "in-1", 1)
        .unwrap();
    let (sms, ran) = Act::new("sms", None);
    let reg = registry(&store, vec![sms]).await;
    let r = reg.execute(&ctx(&store, "care"), "sms", json!({ "to": ["+1-555-0198"] })).await;
    assert_eq!(r.content, "RAN", "a reply in the thread asked: {}", r.content);
    assert_eq!(ran.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn known_counterparty_does_not_ask() {
    let (_d, store) = store();
    // A confirmed send through the one send path records who it went to.
    let c = ctx(&store, "care");
    let input = json!({ "to": "pat@example.com", "text": "hello" });
    let sent = tools::effects::guarded_send(&store, &c, "messaging", "test", "mail.message.send", &input, || async {
        tools::effects::SendOutcome::Sent("Sent.".into(), None)
    })
    .await;
    assert!(!sent.is_error, "{}", sent.content);
    let (mail, ran) = Act::new("mail", None);
    let reg = registry(&store, vec![mail]).await;
    let r = reg.execute(&c, "mail", json!({ "to": ["Pat@Example.com"] })).await;
    assert_eq!(r.content, "RAN", "{}", r.content);
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    // Another employee has never written to them.
    let other = reg.execute(&ctx(&store, "other"), "mail", json!({ "to": ["pat@example.com"] })).await;
    assert!(other.parked_ask.is_some(), "{}", other.content);
}

#[tokio::test]
async fn allow_always_on_a_recipient_never_asks_again() {
    let (_d, store) = store();
    let rule = Rule {
        id: "r1".into(),
        scope: Scope::Employee("care".into()),
        key: RuleKey::Tool("mail".into()),
        field: Some(RuleField::Recipient("new@example.com".into())),
        effect: Effect::Allow,
        money: None,
        source: RuleSource::AllowAlways { ask_id: "a1".into() },
        locked: false,
        created_at: 0,
    };
    store.write_permission_rule(&rule, &Writer::Owner).unwrap();
    let (mail, _) = Act::new("mail", None);
    let reg = registry(&store, vec![mail]).await;
    let r = reg.execute(&ctx(&store, "care"), "mail", json!({ "to": ["new@example.com"] })).await;
    assert_eq!(r.content, "RAN", "{}", r.content);
}

// ── Case 3: irreversible, outside its own work ──────────────────────────

/// fixtures/permissions/delete-not-its-own-asks.yaml: the owner's
/// "Weekly Report" workflow, which this employee didn't create.
#[tokio::test]
async fn delete_not_its_own_asks() {
    let (_d, store) = store();
    let (work, ran) = Act::new("work", None);
    let reg = registry(&store, vec![work]).await;
    let r = reg.execute(&ctx(&store, "admin"), "work", json!({ "deletes": ["workflow:weekly report"] })).await;
    assert_eq!(asked(&store, &r), AskCase::Irreversible { what: "the workflow \"weekly report\"".into() });
    assert_eq!(ran.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn delete_its_own_runs() {
    let (_d, store) = store();
    let (work, ran) = Act::new("work", None);
    let reg = registry(&store, vec![work]).await;
    let c = ctx(&store, "admin");
    // It makes the workflow; the run that made it records it as its own.
    assert_eq!(reg.execute(&c, "work", json!({ "creates": ["workflow:drafts"] })).await.content, "RAN");
    assert_eq!(reg.execute(&c, "work", json!({ "deletes": ["workflow:drafts"] })).await.content, "RAN");
    assert_eq!(ran.load(Ordering::SeqCst), 2);
    // A file it wrote into being may be replaced; one that was there asks.
    let dir = tempfile::tempdir().unwrap();
    let theirs = format!("file:{}", dir.path().join("theirs.txt").display());
    let mine = format!("file:{}", dir.path().join("mine.txt").display());
    assert_eq!(reg.execute(&c, "work", json!({ "creates": [mine] })).await.content, "RAN");
    assert_eq!(reg.execute(&c, "work", json!({ "overwrites": [mine] })).await.content, "RAN");
    assert!(reg.execute(&c, "work", json!({ "overwrites": [theirs] })).await.parked_ask.is_some());
}

#[tokio::test]
async fn an_overwrite_inside_the_jobs_folders_is_its_own_work() {
    let (_d, store) = store();
    let dir = tempfile::tempdir().unwrap();
    let folder = Rule {
        id: "f1".into(),
        scope: Scope::Employee("admin".into()),
        key: RuleKey::Capability("file".into()),
        field: Some(RuleField::Folder(dir.path().to_path_buf())),
        effect: Effect::Allow,
        money: None,
        source: RuleSource::Owner,
        locked: false,
        created_at: 0,
    };
    store.write_permission_rule(&folder, &Writer::Owner).unwrap();
    let (work, _) = Act::new("work", None);
    let reg = registry(&store, vec![work]).await;
    let inside = format!("file:{}", dir.path().join("notes.md").display());
    let r = reg.execute(&ctx(&store, "admin"), "work", json!({ "overwrites": [inside] })).await;
    assert_eq!(r.content, "RAN", "{}", r.content);
}

// ── Case 4: outside its job ─────────────────────────────────────────────

/// fixtures/permissions/outside-its-job-asks.yaml: a bookkeeper with no web
/// in its job asks to look something up; "Allow always" adds the
/// capability to the job and the same lookup runs.
#[tokio::test]
async fn outside_its_job_asks_and_allow_always_grows_the_job() {
    let (_d, store) = store();
    let (web, ran) = Act::new("fetch_url", Some("web"));
    let reg = registry(&store, vec![web]).await;
    let r = reg.execute(&ctx(&store, "bk"), "fetch_url", json!({ "read": true })).await;
    assert_eq!(asked(&store, &r), AskCase::OutsideJob { capability: "web".into() });
    assert!(r.content.contains("outside this employee's job"), "{}", r.content);
    // "Allow always" on case 4 writes the capability into the job.
    allow(&store, "bk", RuleKey::Capability("web".into()), None);
    let again = reg.execute(&ctx(&store, "bk"), "fetch_url", json!({ "read": true })).await;
    assert_eq!(again.content, "RAN", "{}", again.content);
    assert_eq!(ran.load(Ordering::SeqCst), 1);
}

// ── Case 5: acting on untrusted input ───────────────────────────────────

/// fixtures/permissions/send-after-hostile-page-asks.yaml: the run fetched
/// a page (web taint); a send it then tries asks, whoever it is to.
#[tokio::test]
async fn send_after_hostile_page_asks() {
    let (_d, store) = store();
    store.add_employee_counterparty("res", "billing@known.example", "sent").unwrap();
    let (mail, ran) = Act::new("mail", None);
    let reg = registry(&store, vec![mail]).await;
    let tainted = ToolContext { run_taint: vec![ProvenanceClass::Web], ..ctx(&store, "res") };
    // The injected address is new: case 2 catches it first.
    let r = reg.execute(&tainted, "mail", json!({ "to": ["billing@attacker.example"] })).await;
    assert_eq!(asked(&store, &r), AskCase::NewCounterparty { who: "billing@attacker.example".into() });
    // Even to someone it works with, a send driven by what it just read asks.
    let r = reg.execute(&tainted, "mail", json!({ "to": ["billing@known.example"] })).await;
    assert_eq!(asked(&store, &r), AskCase::UntrustedInput { source: "web".into() });
    // So does a delete or a payment.
    let r = reg.execute(&tainted, "mail", json!({ "amount_cents": 500 })).await;
    assert_eq!(asked(&store, &r), AskCase::UntrustedInput { source: "web".into() });
    assert_eq!(ran.load(Ordering::SeqCst), 0);
    // The same send with no outside words in the run runs.
    let clean = reg.execute(&ctx(&store, "res"), "mail", json!({ "to": ["billing@known.example"] })).await;
    assert_eq!(clean.content, "RAN");
}

/// A reply to the person whose message tainted the run (a case turn
/// answering the customer it is about) is a reply in the thread.
#[tokio::test]
async fn reply_to_the_inbound_sender_is_not_case_five() {
    let (_d, store) = store();
    let binding = workflow::cases::CaseBinding {
        agent_id: "care",
        binding_name: "mail",
        case_type: "support".to_string(),
        definition_json: r#"{"name":"support","activities":[]}"#,
        base_inputs: json!({}),
        default_wait_secs: 3600,
    };
    workflow::cases::signal_or_open(&store, &binding, "email", "cust@example.com", &json!({}), "email", "in-1", 1).unwrap();
    let case = store.engine_open_cases_for_alias("email", "cust@example.com").unwrap().remove(0);
    // A case turn runs in a workflow session; its run carries the case.
    let turn_id = "turn-1";
    store
        .engine_create_run(&db::NewRun {
            id: turn_id,
            kind: "workflow",
            session_key: &tools::workflow_session_key("care", turn_id),
            agent_id: "care",
            lane: "",
            parent_run_id: None,
            definition: None,
            inputs: case.inputs.as_deref(),
            external_ref: None,
        })
        .unwrap();
    let (mail, ran) = Act::new("mail", None);
    let reg = registry(&store, vec![mail]).await;
    let turn = ToolContext {
        session_key: tools::workflow_session_key("care", turn_id),
        run_taint: vec![ProvenanceClass::ExternalEmail],
        ..ctx(&store, "care")
    };
    let reply = reg.execute(&turn, "mail", json!({ "to": ["Cust@Example.com"] })).await;
    assert_eq!(reply.content, "RAN", "{}", reply.content);
    // Anyone else, from the same tainted turn, asks.
    let out = reg.execute(&turn, "mail", json!({ "to": ["someone@example.com"] })).await;
    assert!(out.parked_ask.is_some(), "{}", out.content);
    assert_eq!(ran.load(Ordering::SeqCst), 1);
}

// ── The judgement's reach ───────────────────────────────────────────────

/// The code-decided cases never become questions: an outage of both
/// judges can never let one of them through.
#[test]
fn code_decided_cases_never_reach_the_judges() {
    let (_d, store) = store();
    let (act, _) = Act::new("act", None);
    let c = ToolContext { run_taint: vec![], ..ctx(&store, "e") };
    let grant = resolve_grant(&store, "e", None);
    let question = |input: serde_json::Value, c: &ToolContext| {
        let t = tools::registry::target_of(&act, &input);
        let cx = CheckCx { ctx: c, input: &input, grant: &grant, store: &store };
        question_for(&cx, &t)
    };
    // New recipient, publish, delete, and a tainted send: all decided.
    for input in [
        json!({ "to": ["new@example.com"], "publishes": "unknown" }),
        json!({ "publishes": "yes" }),
        json!({ "deletes": ["workflow:x"], "publishes": "unknown" }),
    ] {
        assert_eq!(question(input.clone(), &c), None, "{input}");
    }
    let tainted = ToolContext { run_taint: vec![ProvenanceClass::Web], ..c.clone() };
    assert_eq!(question(json!({ "amount_cents": 1, "publishes": "unknown" }), &tainted), None);
    // Outside the job: decided.
    let (web, _) = Act::new("web", Some("web"));
    let input = json!({ "publishes": "unknown" });
    let t = tools::registry::target_of(&web, &input);
    let cx = CheckCx { ctx: &c, input: &input, grant: &grant, store: &store };
    assert_eq!(question_for(&cx, &t), None);
    // Only an unknown outward effect with nothing else to decide on asks.
    let q = question(json!({ "publishes": "unknown", "password": "hunter2" }), &c).expect("undecided");
    assert_eq!(q.untrusted, None);
    assert_eq!(q.input["password"], "[redacted]", "secrets never reach a judge");
    let q = question(json!({ "publishes": "unknown" }), &tainted).expect("undecided");
    assert_eq!(q.ask_case(), AskCase::UntrustedInput { source: "web".into() });
}

/// Shadow records the verdict and runs the call on the code's answer;
/// enforce acts on it.
#[tokio::test]
async fn shadow_judgement_records_and_never_blocks() {
    let (_d, store) = store();
    let (post, ran) = Act::new("post", None);
    let reg = registry(&store, vec![post]).await;
    let ask = Verdict::Ask {
        case: AskCase::NewCounterparty { who: super::PUBLIC.into() },
        by: JudgedBy::Jev,
        reason: "posts to a public page".into(),
    };
    let judged = ToolContext { judgement: Some(ask.clone()), ..ctx(&store, "e") };
    assert_eq!(store.permission_judgement_mode().unwrap(), JudgementMode::Shadow, "shadow is the default");
    assert_eq!(reg.execute(&judged, "post", json!({ "publishes": "unknown" })).await.content, "RAN");
    let row = &store.permission_activity("e", 1).unwrap()[0];
    assert_eq!(row.decision, "allow");
    let j: serde_json::Value = serde_json::from_str(row.judgement.as_deref().unwrap()).unwrap();
    assert_eq!(j, json!({ "mode": "shadow", "verdict": "ask", "by": "jev", "reason": "posts to a public page" }));
    assert!(!row.unreviewed);

    store.set_permission_judgement_mode(JudgementMode::Enforce).unwrap();
    let r = reg.execute(&judged, "post", json!({ "publishes": "unknown" })).await;
    assert!(r.parked_ask.is_some(), "{}", r.content);
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    // A verdict never reaches a call the code decided.
    let r = reg.execute(&judged, "post", json!({ "publishes": "no" })).await;
    assert_eq!(r.content, "RAN");
}

#[tokio::test]
async fn activity_records_which_judge_decided() {
    let (_d, store) = store();
    store.set_permission_judgement_mode(JudgementMode::Enforce).unwrap();
    let (post, _) = Act::new("post", None);
    let reg = registry(&store, vec![post]).await;
    let allowed = Verdict::Allow { by: JudgedBy::AuxClassifier, reason: "saves a draft to the owner's account".into() };
    let c = ToolContext { judgement: Some(allowed), ..ctx(&store, "e") };
    assert_eq!(reg.execute(&c, "post", json!({ "publishes": "unknown" })).await.content, "RAN");
    let row = &store.permission_activity("e", 1).unwrap()[0];
    let why: types::permissions::Why = serde_json::from_str(&row.why).unwrap();
    assert_eq!(
        why,
        types::permissions::Why::Judged { by: "aux_classifier".into(), reason: "saves a draft to the owner's account".into() }
    );
    let j: serde_json::Value = serde_json::from_str(row.judgement.as_deref().unwrap()).unwrap();
    assert_eq!((j["mode"].as_str(), j["by"].as_str()), (Some("enforce"), Some("aux_classifier")));
}

/// Neither judge answered: the call runs, flagged unreviewed with the
/// reason, in both modes.
#[tokio::test]
async fn both_down_proceeds_marked_unreviewed() {
    let (_d, store) = store();
    let (post, ran) = Act::new("post", None);
    let reg = registry(&store, vec![post]).await;
    for mode in [JudgementMode::Shadow, JudgementMode::Enforce] {
        store.set_permission_judgement_mode(mode).unwrap();
        let c = ToolContext { judgement: Some(Verdict::Unjudged), door: types::permissions::Door::Heartbeat, ..ctx(&store, "e") };
        assert_eq!(reg.execute(&c, "post", json!({ "publishes": "unknown" })).await.content, "RAN");
        let row = &store.permission_activity("e", 1).unwrap()[0];
        assert!(row.unreviewed, "{mode:?}");
        assert!(row.judgement.as_deref().unwrap().contains(types::permissions::UNREVIEWED_REASON));
        if mode == JudgementMode::Enforce {
            assert!(row.why.contains(types::permissions::UNREVIEWED_REASON), "{}", row.why);
        }
    }
    assert_eq!(ran.load(Ordering::SeqCst), 2);
    let listed = store.unreviewed_permission_activity(0, &["heartbeat"]).unwrap();
    assert_eq!(listed.len(), 2);
    assert!(store.unreviewed_permission_activity(0, &["chat"]).unwrap().is_empty());
}
