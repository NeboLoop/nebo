//! The one check, driven through the registry the runner uses: every call
//! is decided by it, in its fixed order, whichever door it came through.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use tools::registry::DynTool;
use tools::{Origin, Registry, ToolContext, ToolResult};
use types::permissions::{
    AskCase, Ceiling, Decision, Door, Effect, Grant, Mode, MoneyLimit, Rule, RuleField, RuleKey, RuleSource,
    Scope, Why, Writer,
};

use super::*;

fn store() -> (tempfile::TempDir, Arc<db::Store>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(db::Store::new(&dir.path().join("p.db").to_string_lossy()).unwrap());
    (dir, store)
}

fn rule(scope: Scope, key: RuleKey, field: Option<RuleField>, effect: Effect) -> Rule {
    Rule {
        id: uuid::Uuid::new_v4().to_string(),
        scope,
        key,
        field,
        effect,
        money: None,
        source: RuleSource::Owner,
        locked: false,
        created_at: 0,
    }
}

fn put(store: &db::Store, r: Rule) -> Rule {
    store.write_permission_rule(&r, &Writer::Owner).unwrap()
}

fn activity_of(agent: &str) -> db::PermissionActivityFilter {
    db::PermissionActivityFilter { agent_id: Some(agent.into()), limit: 100, ..Default::default() }
}

fn cap(c: &str) -> RuleKey {
    RuleKey::Capability(c.into())
}

/// A tool that says what its call is and counts the calls that ran.
struct Probe {
    name: &'static str,
    key: &'static str,
    capability: Option<&'static str>,
    read_only: bool,
    operation: Option<&'static str>,
    ran: Arc<AtomicUsize>,
}

impl Probe {
    fn new(name: &'static str, key: &'static str, capability: Option<&'static str>) -> (Self, Arc<AtomicUsize>) {
        let ran = Arc::new(AtomicUsize::new(0));
        (Self { name, key, capability, read_only: false, operation: None, ran: ran.clone() }, ran)
    }
}

impl DynTool for Probe {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> String {
        String::new()
    }
    fn schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }
    fn read_only(&self, _input: &serde_json::Value) -> bool {
        self.read_only
    }
    fn rule_key(&self, _input: &serde_json::Value) -> String {
        self.key.to_string()
    }
    fn capability(&self, _input: &serde_json::Value) -> Option<&'static str> {
        self.capability
    }
    fn operation_performed(&self, _input: &serde_json::Value) -> Option<String> {
        self.operation.map(str::to_string)
    }
    fn effects(&self, input: &serde_json::Value) -> types::permissions::CallEffects {
        let mut e = types::permissions::CallEffects::unknown();
        e.money_cents = input.get("amount_cents").and_then(|v| v.as_i64());
        if let Some(to) = input.get("to").and_then(|v| v.as_str()) {
            e.recipients = vec![to.to_string()];
        }
        e
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

async fn registry(store: &Arc<db::Store>, probes: Vec<Probe>) -> Registry {
    let registry = Registry::new(Arc::new(Check::new(store.clone())));
    for p in probes {
        registry.register(Box::new(p)).await;
    }
    registry
}

/// The context of an employee's run: its own grant, as the runner resolves it.
fn ctx(store: &db::Store, agent: &str, origin: Origin) -> ToolContext {
    ToolContext {
        origin,
        session_key: format!("agent:{agent}:web"),
        session_id: "s1".into(),
        grant: Some(Arc::new(resolve_grant(store, agent, None))),
        ..Default::default()
    }
}

fn with_mode(mut c: ToolContext, mode: Mode) -> ToolContext {
    let mut g = (**c.grant.as_ref().unwrap()).clone();
    g.mode = mode;
    c.grant = Some(Arc::new(g));
    c
}

#[tokio::test]
async fn registry_runs_nothing_without_the_gate() {
    // The registry takes its gate at construction: there is no registry
    // without one. A denied call and a parked call never reach the tool.
    let (_d, store) = store();
    put(&store, rule(Scope::Company, cap("web"), None, Effect::Deny));
    put(&store, rule(Scope::Company, RuleKey::Tool("send_invoice".into()), None, Effect::Ask));
    let (web, web_ran) = Probe::new("web", "fetch_url", Some("web"));
    let (inv, inv_ran) = Probe::new("invoices", "send_invoice", None);
    let reg = registry(&store, vec![web, inv]).await;
    let c = ctx(&store, "", Origin::User);
    let denied = reg.execute(&c, "web", json!({})).await;
    assert!(denied.is_error && denied.content.contains("permission is off"), "{}", denied.content);
    let parked = reg.execute(&c, "invoices", json!({})).await;
    assert!(parked.parked_ask.is_some(), "{}", parked.content);
    assert_eq!((web_ran.load(Ordering::SeqCst), inv_ran.load(Ordering::SeqCst)), (0, 0));
}

#[tokio::test]
async fn hard_limits_run_before_rules_and_modes() {
    let (_d, store) = store();
    put(&store, rule(Scope::Company, cap("shell"), None, Effect::Allow));
    put(&store, rule(Scope::Company, RuleKey::Tool("run_command".into()), None, Effect::Allow));
    let reg = registry(&store, vec![]).await;
    reg.register_defaults().await;
    let full = with_mode(ctx(&store, "", Origin::User), Mode::FullAccess);
    let db_dir = config::data_dir().unwrap().join("data").to_string_lossy().into_owned();
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran");
    let command = format!("touch {}; test -d '{db_dir}'", marker.display());
    let r = reg.execute(&full, "run_command", json!({ "command": command, "description": "Touch a marker" })).await;
    assert!(r.is_error && r.content.contains("BLOCKED"), "{}", r.content);
    assert!(!marker.exists(), "Full Access and an allow rule lifted the safeguard");
    // An origin limit: a chat channel never runs a command.
    let comm = with_mode(ctx(&store, "", Origin::Comm), Mode::FullAccess);
    let r = reg
        .execute(&comm, "run_command", json!({ "command": format!("touch {}", marker.display()), "description": "Touch a marker" }))
        .await;
    assert!(r.is_error && r.content.contains("not permitted"), "{}", r.content);
    assert!(!marker.exists());
}

/// A coworker's request that reaches past a reply is refused saying what
/// happened: a coworker asked, and its request can only be answered. Not a
/// chat channel, not someone outside the company.
#[tokio::test]
async fn a_coworkers_request_is_refused_as_a_coworkers() {
    let (_d, store) = store();
    put(&store, rule(Scope::Company, cap("file"), None, Effect::Allow));
    let (write, ran) = Probe::new("write_file", "write_file", Some("file"));
    let reg = registry(&store, vec![write]).await;
    let mut c = with_mode(ctx(&store, "emp", Origin::Comm), Mode::FullAccess);
    c.door = Door::Coworker { from: "supervisor".into() };
    let r = reg.execute(&c, "write_file", json!({})).await;
    assert!(r.is_error && r.content.contains("a coworker asked for this"), "{}", r.content);
    assert!(!r.content.contains("chat channel"), "{}", r.content);
    assert_eq!(ran.load(Ordering::SeqCst), 0);
    let rows = store.permission_activity(&activity_of("emp")).unwrap().0;
    let row = rows.iter().find(|r| r.rule_key == "write_file").expect("recorded");
    assert_eq!(serde_json::from_str::<Why>(&row.why).unwrap(), Why::HardLimit { limit: "coworker".into() });
}

#[tokio::test]
async fn origin_limited_run_can_only_reply() {
    let (_d, store) = store();
    for c in ["shell", "file", "web", "desktop"] {
        put(&store, rule(Scope::Company, cap(c), None, Effect::Allow));
    }
    let (fetch, fetch_ran) = Probe::new("web", "fetch_url", Some("web"));
    let (mail, mail_ran) = Probe::new("mail", "mail_message_send", None);
    let (recall, recall_ran) = Probe::new("memory", "recall", None);
    let reg = registry(&store, vec![fetch, mail, recall]).await;
    for origin in [Origin::Visitor, Origin::Caller] {
        let c = with_mode(ctx(&store, "", origin), Mode::FullAccess);
        for tool in ["web", "mail"] {
            let r = reg.execute(&c, tool, json!({})).await;
            assert!(r.is_error && r.content.contains("not permitted"), "{origin:?} {tool}: {}", r.content);
        }
        assert_eq!(reg.execute(&c, "memory", json!({})).await.content, "RAN", "{origin:?}: basic work stays");
    }
    assert_eq!((fetch_ran.load(Ordering::SeqCst), mail_ran.load(Ordering::SeqCst)), (0, 0));
    assert_eq!(recall_ran.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn credentials_never_leave_in_an_outbound_call() {
    let (_d, store) = store();
    put(&store, rule(Scope::Company, cap("web"), None, Effect::Allow));
    let (http, ran) = Probe::new("web", "http_request", Some("web"));
    let (mail, mail_ran) = Probe::new("mail", "mail_message_send", None);
    let reg = registry(&store, vec![http, mail]).await;
    let c = with_mode(ctx(&store, "", Origin::User), Mode::FullAccess);
    let secret = format!("sk-ant-{}", "a".repeat(48));
    let r = reg.execute(&c, "web", json!({ "body": format!("key={secret}") })).await;
    assert!(r.is_error && r.content.contains("secret"), "{}", r.content);
    let r = reg.execute(&c, "mail", json!({ "to": "someone@example.com", "body": secret })).await;
    assert!(r.is_error && r.content.contains("secret"), "{}", r.content);
    assert_eq!((ran.load(Ordering::SeqCst), mail_ran.load(Ordering::SeqCst)), (0, 0));
    // Without a secret the same calls run.
    assert_eq!(reg.execute(&c, "web", json!({ "body": "hello" })).await.content, "RAN");
    assert_eq!(reg.execute(&c, "mail", json!({ "to": "someone@example.com", "body": "hi" })).await.content, "RAN");
}

/// Full Access runs everything else without asking, but an ask rule still
/// asks and a deny rule still refuses: the rules are checked before the
/// mode, so no mode skips a rule the owner wrote.
#[tokio::test]
async fn full_access_keeps_ask_and_deny_rules() {
    let (_d, store) = store();
    put(&store, rule(Scope::Company, RuleKey::Tool("send_invoice".into()), None, Effect::Ask));
    put(&store, rule(Scope::Company, cap("desktop"), None, Effect::Deny));
    let (inv, _) = Probe::new("invoices", "send_invoice", None);
    let (shell, _) = Probe::new("run_command", "run_command", Some("shell"));
    let (click, click_ran) = Probe::new("desktop", "desktop_click", Some("desktop"));
    let reg = registry(&store, vec![inv, shell, click]).await;
    let full = with_mode(ctx(&store, "", Origin::User), Mode::FullAccess);
    assert!(reg.execute(&full, "invoices", json!({})).await.parked_ask.is_some(), "an ask rule asks");
    assert_eq!(reg.execute(&full, "run_command", json!({})).await.content, "RAN", "outside the job allows");
    let r = reg.execute(&full, "desktop", json!({})).await;
    assert!(r.is_error, "a deny rule held: {}", r.content);
    assert_eq!(click_ran.load(Ordering::SeqCst), 0);
    // And the same calls in Automatic: the ask parks, outside the job parks.
    let auto = ctx(&store, "", Origin::User);
    assert!(reg.execute(&auto, "invoices", json!({})).await.parked_ask.is_some());
    let outside = reg.execute(&auto, "run_command", json!({})).await;
    assert!(outside.parked_ask.is_some() && outside.content.contains("outside"), "{}", outside.content);
}

#[tokio::test]
async fn plan_mode_is_read_only_until_approved() {
    let (_d, store) = store();
    put(&store, rule(Scope::Company, cap("file"), None, Effect::Allow));
    let (mut read, read_ran) = Probe::new("reader", "read_file", Some("file"));
    read.read_only = true;
    let (write, write_ran) = Probe::new("writer", "write_file", Some("file"));
    let reg = registry(&store, vec![read, write]).await;
    let plan = with_mode(ctx(&store, "", Origin::User), Mode::Plan);
    assert_eq!(reg.execute(&plan, "reader", json!({})).await.content, "RAN");
    let r = reg.execute(&plan, "writer", json!({})).await;
    assert!(r.is_error && r.content.contains("Plan mode"), "{}", r.content);
    assert_eq!((read_ran.load(Ordering::SeqCst), write_ran.load(Ordering::SeqCst)), (1, 0));
    // The approved plan ends the mode: the employee's own mode decides again.
    let approved = with_mode(plan, Mode::Automatic);
    assert_eq!(reg.execute(&approved, "writer", json!({})).await.content, "RAN");
}

/// D11 (parity 7.3, 7.4): Plan mode writes its plan and has a way out.
/// The plan document goes through; the way out always asks the owner
/// ("Exit plan mode?"), and is refused outside Plan mode.
#[tokio::test]
async fn plan_mode_writes_its_plan_and_asks_the_owner_to_leave() {
    let (_d, store) = store();
    put(&store, rule(Scope::Company, cap("file"), None, Effect::Allow));
    let (plan_doc, plan_ran) = Probe::new("planner", "write_plan", Some("file"));
    let (exit, exit_ran) = Probe::new("leaver", "exit_plan_mode", None);
    let reg = registry(&store, vec![plan_doc, exit]).await;
    let plan = with_mode(ctx(&store, "", Origin::User), Mode::Plan);
    assert_eq!(reg.execute(&plan, "planner", json!({})).await.content, "RAN", "the plan document is written in Plan mode");
    let asked = reg.execute(&plan, "leaver", json!({})).await;
    assert!(asked.parked_ask.is_some(), "leaving plan mode asks the owner: {}", asked.content);
    let full = with_mode(ctx(&store, "", Origin::User), Mode::FullAccess);
    let outside = reg.execute(&full, "leaver", json!({})).await;
    assert!(outside.is_error && outside.content.contains("not in plan mode"), "{}", outside.content);
    assert_eq!((plan_ran.load(Ordering::SeqCst), exit_ran.load(Ordering::SeqCst)), (1, 0));
}

#[tokio::test]
async fn ask_mode_asks_every_change_not_owner_allowed() {
    let (_d, store) = store();
    put(&store, rule(Scope::Company, cap("file"), None, Effect::Allow));
    put(&store, rule(Scope::Company, RuleKey::Tool("edit_file".into()), None, Effect::Allow));
    let (mut read, _) = Probe::new("reader", "read_file", Some("file"));
    read.read_only = true;
    let (write, write_ran) = Probe::new("writer", "write_file", Some("file"));
    let (edit, edit_ran) = Probe::new("editor", "edit_file", Some("file"));
    let reg = registry(&store, vec![read, write, edit]).await;
    let ask = with_mode(ctx(&store, "", Origin::User), Mode::Ask);
    assert_eq!(reg.execute(&ask, "reader", json!({})).await.content, "RAN", "reads run");
    let r = reg.execute(&ask, "writer", json!({})).await;
    assert!(r.parked_ask.is_some(), "a change the job covers still asks in Ask mode: {}", r.content);
    assert_eq!(reg.execute(&ask, "editor", json!({})).await.content, "RAN", "an owner's allow on the key covers it");
    assert_eq!((write_ran.load(Ordering::SeqCst), edit_ran.load(Ordering::SeqCst)), (0, 1));
}

#[tokio::test]
async fn every_door_goes_through_the_check() {
    let (_d, store) = store();
    put(&store, rule(Scope::Company, cap("web"), None, Effect::Deny));
    let (web, ran) = Probe::new("web", "fetch_url", Some("web"));
    let reg = registry(&store, vec![web]).await;
    let doors = [
        Door::Chat,
        Door::Helper,
        Door::Workflow,
        Door::Schedule,
        Door::Heartbeat,
        Door::Coworker { from: "a2".into() },
        Door::Voice,
        Door::Mcp,
        Door::LocalApi,
    ];
    for door in &doors {
        // The door's own context, with and without a resolved grant: a
        // caller that builds a bare context still meets the rules.
        for with_grant in [true, false] {
            let mut c = ctx(&store, "emp", Origin::User);
            if !with_grant {
                c.grant = None;
            }
            c.door = door.clone();
            let r = reg.execute(&c, "web", json!({})).await;
            assert!(r.is_error && r.content.contains("permission is off"), "{door:?}: {}", r.content);
        }
    }
    assert_eq!(ran.load(Ordering::SeqCst), 0, "a door skipped the check");
    let recorded: std::collections::HashSet<String> =
        store.permission_activity(&activity_of("emp")).unwrap().0.into_iter().map(|a| a.door).collect();
    for door in &doors {
        assert!(recorded.contains(door.label()), "{door:?} was not recorded: {recorded:?}");
    }
}

/// Every shape of a call is decided as it will run (#249): the os tool with
/// no action meets the safeguard, the folder fence, the Shell rule and the
/// origin limits; a notebook edit meets File and the folder fence.
#[tokio::test]
async fn call_resolved_as_it_will_run() {
    let (_d, store) = store();
    let dir = tempfile::tempdir().unwrap();
    let inside = dir.path().join("inside");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&inside).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    put(&store, rule(Scope::Company, cap("file"), None, Effect::Allow));
    put(&store, rule(Scope::Company, cap("shell"), None, Effect::Allow));
    put(&store, rule(Scope::Employee("fenced".into()), cap("file"), Some(RuleField::Folder(inside.clone())), Effect::Allow));
    put(&store, rule(Scope::Employee("noshell".into()), cap("shell"), None, Effect::Deny));
    put(&store, rule(Scope::Employee("nofile".into()), cap("file"), None, Effect::Deny));
    let reg = registry(&store, vec![]).await;
    reg.register_defaults().await;
    reg.register(Box::new(tools::notebook_tool::NotebookTool::new())).await;

    // A write outside every folder rule: outside the job, so it waits.
    let fenced = ctx(&store, "fenced", Origin::User);
    let target = outside.join("x.txt");
    let w = reg.execute(&fenced, "write_file", json!({ "path": target.to_string_lossy(), "content": "y" })).await;
    assert!(w.parked_ask.is_some(), "{}", w.content);
    assert!(!target.exists(), "the write landed outside the folders");
    let marker = outside.join("ran");
    let e = reg
        .execute(
            &fenced,
            "run_command",
            json!({ "command": format!("touch {}", marker.display()), "description": "Touch a marker", "cwd": outside.to_string_lossy() }),
        )
        .await;
    assert!(e.parked_ask.is_some(), "{}", e.content);
    assert!(!marker.exists(), "the command ran outside the folders");

    // A command is Shell.
    let noshell = ctx(&store, "noshell", Origin::User);
    let r = reg
        .execute(&noshell, "run_command", json!({ "command": format!("touch {}", marker.display()), "description": "Touch a marker" }))
        .await;
    assert!(r.is_error && r.content.contains("Shell Commands"), "{}", r.content);
    assert!(!marker.exists());

    // A notebook edit is a file write: File and the folder fence.
    let nb = json!({
        "cells": [{"cell_type": "code", "id": "c1", "metadata": {}, "source": "print(1)", "outputs": [], "execution_count": 1}],
        "metadata": {}, "nbformat": 4, "nbformat_minor": 5
    })
    .to_string();
    let edit = |p: &std::path::Path| json!({ "action": "edit", "notebook_path": p.to_string_lossy(), "cell_id": "c1", "new_source": "print(2)" });
    let out_nb = outside.join("n.ipynb");
    std::fs::write(&out_nb, &nb).unwrap();
    assert!(reg.execute(&fenced, "notebook", edit(&out_nb)).await.parked_ask.is_some());
    assert_eq!(std::fs::read_to_string(&out_nb).unwrap(), nb, "the edit landed outside the folders");
    let in_nb = inside.join("n.ipynb");
    std::fs::write(&in_nb, &nb).unwrap();
    let nofile = ctx(&store, "nofile", Origin::User);
    let r = reg.execute(&nofile, "notebook", edit(&in_nb)).await;
    assert!(r.is_error && r.content.contains("File Access"), "{}", r.content);
    assert_eq!(std::fs::read_to_string(&in_nb).unwrap(), nb, "the edit ran with File denied");
    // Reads stay open outside the folders, as file reads do.
    let read = json!({ "action": "read", "notebook_path": out_nb.to_string_lossy() });
    assert!(!reg.execute(&fenced, "notebook", read).await.is_error);
}

/// Through the one door: a chat channel's call to poll or stop a shell
/// session is refused before the tool runs.
#[tokio::test]
async fn a_chat_channel_cannot_poll_or_stop_a_shell_session() {
    let (_d, store) = store();
    let reg = registry(&store, vec![]).await;
    reg.register_defaults().await;
    let c = ctx(&store, "", Origin::Comm);
    for (tool, call) in [
        ("read_output", json!({ "task_id": "bg-0000aaaa" })),
        ("stop_task", json!({ "task_id": "bg-0000aaaa" })),
        ("send_input", json!({ "task_id": "bg-0000aaaa", "text": "y\n" })),
        ("list_processes", json!({})),
    ] {
        let r = reg.execute(&c, tool, call).await;
        assert!(r.is_error && r.content.contains("not permitted"), "{tool}: {}", r.content);
    }
}

/// Text typed into a running command meets the command safeguard: a
/// terminal running a shell is a shell, and Full Access does not lift it.
#[tokio::test]
async fn typing_into_a_command_meets_the_command_safeguard() {
    let (_d, store) = store();
    put(&store, rule(Scope::Company, cap("shell"), None, Effect::Allow));
    let reg = registry(&store, vec![]).await;
    reg.register_defaults().await;
    let full = with_mode(ctx(&store, "", Origin::User), Mode::FullAccess);
    let r = reg.execute(&full, "send_input", json!({ "task_id": "bg-0000aaaa", "text": "sudo whoami\n" })).await;
    assert!(r.is_error && r.content.contains("BLOCKED: sudo"), "{}", r.content);
    // Anything else reaches the tool, which has no such session.
    let r = reg.execute(&full, "send_input", json!({ "task_id": "bg-0000aaaa", "text": "yes\n" })).await;
    assert!(r.is_error && r.content.contains("no session bg-0000aaaa"), "{}", r.content);
}

#[tokio::test]
async fn helper_cannot_exceed_parent() {
    let (_d, store) = store();
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    let copy = dir.path().join("copy");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&copy).unwrap();
    // The parent: its employee's browser is denied and its files fenced to
    // `work`. The child's own rules (the company's) would allow both.
    put(&store, rule(Scope::Company, cap("web"), None, Effect::Allow));
    put(&store, rule(Scope::Company, cap("file"), None, Effect::Allow));
    put(&store, rule(Scope::Employee("emp".into()), RuleKey::Tool("browser_*".into()), None, Effect::Deny));
    put(&store, rule(Scope::Employee("emp".into()), cap("file"), Some(RuleField::Folder(work.clone())), Effect::Allow));
    let parent = resolve_grant(&store, "emp", None);
    let mut child = resolve_grant(&store, "", None);
    child.ceiling = Some(Ceiling::Parent { grant: Box::new(parent.clone()) });
    let (browser, browser_ran) = Probe::new("web", "browser_open", Some("web"));
    let (writer, writer_ran) = Probe::new("writer", "write_file", Some("file"));
    let reg = registry(&store, vec![browser, writer]).await;
    let c = ToolContext {
        session_key: "subagent:agent:emp:web:sa-1".into(),
        door: Door::Helper,
        grant: Some(Arc::new(child.clone())),
        ..Default::default()
    };
    let r = reg.execute(&c, "web", json!({})).await;
    assert!(r.is_error && r.content.contains("beyond what"), "{}", r.content);
    let outside = dir.path().join("x.txt");
    let r = reg.execute(&c, "writer", json!({ "path": outside.to_string_lossy() })).await;
    assert!(r.is_error && r.content.contains("beyond what"), "a helper wrote outside its parent's folders: {}", r.content);
    assert_eq!((browser_ran.load(Ordering::SeqCst), writer_ran.load(Ordering::SeqCst)), (0, 0));
    // Inside the parent's folders it runs.
    let inside = work.join("x.txt");
    assert_eq!(reg.execute(&c, "writer", json!({ "path": inside.to_string_lossy() })).await.content, "RAN");
    // An isolated helper is fenced to its own copy: even the parent's folder is out.
    let mut isolated = child;
    isolated.fence = Some(vec![copy.clone()]);
    let c = ToolContext { grant: Some(Arc::new(isolated)), ..c };
    let r = reg.execute(&c, "writer", json!({ "path": inside.to_string_lossy() })).await;
    assert!(r.is_error, "an isolated helper wrote outside its copy: {}", r.content);
    let d = decide(
        &CheckCx { ctx: &c, input: &json!({ "path": inside.to_string_lossy() }), grant: c.grant.as_ref().unwrap(), store: &store },
        &Target {
            tool: "writer".into(),
            key: "write_file".into(),
            operation: None,
            capability: Some("file".into()),
            field: None,
            subject: None,
            read_only: false,
            effects: types::permissions::CallEffects::unknown(),
        },
    );
    assert!(matches!(d, Decision::Deny { why: Why::Ceiling, .. }), "{d:?}");
}

#[test]
fn employee_cannot_widen_rules() {
    let (_d, store) = store();
    let by = Writer::Employee { agent_id: "chief".into() };
    let widen = rule(Scope::Employee("clerk".into()), cap("shell"), None, Effect::Allow);
    assert_eq!(store.write_permission_rule(&widen, &by), Err(types::permissions::RuleError::Widens));
    let narrow = rule(Scope::Employee("clerk".into()), cap("shell"), None, Effect::Deny);
    let narrowed = store.write_permission_rule(&narrow, &by).unwrap();
    assert_eq!(store.remove_permission_rule(&narrowed.id, &by), Err(types::permissions::RuleError::Widens));
}

#[test]
fn locked_rule_cannot_be_removed() {
    let (_d, store) = store();
    let law = Rule {
        locked: true,
        source: RuleSource::Law { pack: "acme".into() },
        ..rule(Scope::Company, RuleKey::Operation("esign.document.send".into()), None, Effect::Deny)
    };
    let law = store.write_permission_rule(&law, &Writer::Package { package: "acme".into() }).unwrap();
    assert_eq!(store.remove_permission_rule(&law.id, &Writer::Owner), Err(types::permissions::RuleError::Locked));
    assert!(store.get_permission_rule(&law.id).unwrap().is_some());
}

#[tokio::test]
async fn activity_names_the_rule() {
    let (_d, store) = store();
    let allow = put(&store, rule(Scope::Company, cap("web"), None, Effect::Allow));
    let (web, _) = Probe::new("web", "fetch_url", Some("web"));
    let (memory, _) = Probe::new("memory", "recall", None);
    let reg = registry(&store, vec![web, memory]).await;
    let c = ctx(&store, "emp", Origin::User);
    reg.execute(&c, "web", json!({})).await;
    reg.execute(&c, "memory", json!({})).await;
    let rows = store.permission_activity(&activity_of("emp")).unwrap().0;
    let web = rows.iter().find(|r| r.rule_key == "fetch_url").expect("recorded");
    assert_eq!(web.decision, "allow");
    let why: Why = serde_json::from_str(&web.why).unwrap();
    assert_eq!(why, Why::Rule { rule_id: allow.id });
    let basic = rows.iter().find(|r| r.rule_key == "recall").expect("recorded");
    assert_eq!(serde_json::from_str::<Why>(&basic.why).unwrap(), Why::BasicWork);
}

/// A tool an MCP server adds after the owner set it to "Always allow"
/// asks first, as on main: the sync that finds it pins an ask on it, the
/// tools the owner already saw run under the server's default, and a tool
/// the server stops offering is new again when it returns.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_tool_on_an_always_allowed_server_asks_first() {
    use mcp::bridge::ProxyToolRegistry;
    let (_d, store) = store();
    store
        .create_mcp_integration(
            "crm-1",
            "CRM",
            "crm",
            Some("https://mcp.example.com"),
            "none",
            None,
            None,
        )
        .unwrap();
    let (old, old_ran) = Probe::new("mcp__crm__lookup", "mcp__crm__lookup", None);
    let (new, new_ran) = Probe::new("mcp__crm__delete_all", "mcp__crm__delete_all", None);
    let reg = registry(&store, vec![old, new]).await;
    reg.set_store(store.clone());
    let synced = |names: &[&str]| -> Vec<(String, String)> {
        names
            .iter()
            .map(|n| (n.to_string(), format!("mcp__crm__{n}")))
            .collect()
    };
    // First connect: the server's default asks; the owner sees `lookup`
    // and sets the server to Always allow.
    reg.tools_synced("crm-1", "crm", &synced(&["lookup"]));
    let default = store
        .permission_rules_in(&Scope::Company)
        .unwrap()
        .into_iter()
        .find(|r| r.key == RuleKey::Tool("mcp__crm__*".into()))
        .expect("the server's default");
    assert_eq!(default.effect, Effect::Ask);
    put(
        &store,
        Rule {
            effect: Effect::Allow,
            ..default
        },
    );
    // A reconnect finds a tool the owner never saw.
    reg.tools_synced("crm-1", "crm", &synced(&["lookup", "delete_all"]));
    let c = ctx(&store, "", Origin::User);
    assert_eq!(
        reg.execute(&c, "mcp__crm__lookup", json!({})).await.content,
        "RAN",
        "a tool the owner saw"
    );
    let r = reg.execute(&c, "mcp__crm__delete_all", json!({})).await;
    assert!(
        r.parked_ask.is_some(),
        "a new tool asks first: {}",
        r.content
    );
    assert_eq!(
        (
            old_ran.load(Ordering::SeqCst),
            new_ran.load(Ordering::SeqCst)
        ),
        (1, 0)
    );
    // Syncing again changes nothing: the ask stays the owner's to change.
    reg.tools_synced("crm-1", "crm", &synced(&["lookup", "delete_all"]));
    assert!(
        reg.execute(&c, "mcp__crm__delete_all", json!({}))
            .await
            .parked_ask
            .is_some()
    );
    // Gone, then back: new again.
    reg.tools_synced("crm-1", "crm", &synced(&["lookup"]));
    assert!(
        !store
            .permission_rules_in(&Scope::Company)
            .unwrap()
            .iter()
            .any(|r| r.key == RuleKey::Tool("mcp__crm__delete_all".into())),
        "a gone tool's rule goes with it"
    );
    reg.tools_synced("crm-1", "crm", &synced(&["lookup", "delete_all"]));
    assert!(
        reg.execute(&c, "mcp__crm__delete_all", json!({}))
            .await
            .parked_ask
            .is_some(),
        "back is new"
    );
}

#[tokio::test]
async fn a_parked_call_writes_an_ask_and_the_owners_answer_runs_it() {
    let (_d, store) = store();
    put(&store, rule(Scope::Company, RuleKey::Tool("send_invoice".into()), None, Effect::Ask));
    put(&store, rule(Scope::Company, RuleKey::Tool("void_invoice".into()), None, Effect::Deny));
    let (send, send_ran) = Probe::new("send", "send_invoice", None);
    let (void, void_ran) = Probe::new("void", "void_invoice", None);
    let reg = registry(&store, vec![send, void]).await;
    let c = ctx(&store, "emp", Origin::Workflow);
    let parked = reg.execute(&c, "send", json!({})).await;
    let id = parked.parked_ask.clone().expect("parked");
    assert!(parked.content.contains("Waiting for the owner"), "{}", parked.content);
    let ask = store.get_permission_ask(&id).unwrap().expect("the ask is written");
    assert_eq!((ask.status.as_str(), ask.agent_id.as_str()), ("open", "emp"));
    assert_eq!(serde_json::from_str::<AskCase>(&ask.ask_case).unwrap(), AskCase::AskRule {
        rule_id: store.permission_rules("emp").unwrap().iter().find(|r| r.key.value() == "send_invoice").unwrap().id.clone()
    });
    // The owner's answer runs exactly that call; a deny rule still holds.
    let answered = ToolContext { answered_ask: Some(id), ..c };
    assert_eq!(reg.execute(&answered, "send", json!({})).await.content, "RAN");
    assert!(reg.execute(&answered, "void", json!({})).await.is_error);
    assert_eq!((send_ran.load(Ordering::SeqCst), void_ran.load(Ordering::SeqCst)), (1, 0));
}

#[tokio::test]
async fn money_limits_ask_past_the_grant_and_count_what_runs() {
    let (_d, store) = store();
    let mut grant = rule(Scope::Employee("bk".into()), RuleKey::Operation("ledger.billpayment.create".into()), None, Effect::Allow);
    grant.money = Some(MoneyLimit { per_action_cents: Some(1000), per_day_count: Some(2), ..Default::default() });
    let grant = put(&store, grant);
    let (mut pay, ran) = Probe::new("ledger_billpayment_create", "ledger_billpayment_create", None);
    pay.operation = Some("ledger.billpayment.create");
    let reg = registry(&store, vec![pay]).await;
    let c = ctx(&store, "bk", Origin::Workflow);
    assert_eq!(reg.execute(&c, "ledger_billpayment_create", json!({ "amount_cents": 500 })).await.content, "RAN");
    let over = reg.execute(&c, "ledger_billpayment_create", json!({ "amount_cents": 1500 })).await;
    assert!(over.parked_ask.is_some() && over.content.contains("money limit"), "{}", over.content);
    // A helper spends its employee's counters, never a fresh allowance.
    let mut helper = resolve_grant(&store, "", None);
    helper = Grant { agent_id: "bk".into(), rules: store.permission_rules("bk").unwrap(), ..helper };
    let hc = ToolContext { grant: Some(Arc::new(helper)), door: Door::Helper, ..c.clone() };
    assert_eq!(reg.execute(&hc, "ledger_billpayment_create", json!({ "amount_cents": 100 })).await.content, "RAN");
    let third = reg.execute(&c, "ledger_billpayment_create", json!({ "amount_cents": 100 })).await;
    assert!(third.parked_ask.is_some(), "the day's count was used: {}", third.content);
    assert_eq!(ran.load(Ordering::SeqCst), 2);
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let spent = store.permission_spend("bk", &today, grant.key.value(), "").unwrap();
    assert_eq!((spent.count, spent.cents), (2, 600));
}

#[tokio::test]
async fn untrusted_words_in_the_run_never_spend_a_gated_operation_unasked() {
    let (_d, store) = store();
    let (mut send, ran) = Probe::new("mail_message_send", "mail_message_send", None);
    send.operation = Some("mail.message.send");
    let reg = registry(&store, vec![send]).await;
    assert!(tools::interface_catalog::is_gated("mail.message.send"));
    assert_eq!(reg.execute(&ctx(&store, "", Origin::User), "mail_message_send", json!({})).await.content, "RAN");
    for origin in [Origin::Comm, Origin::Mcp, Origin::App] {
        let r = reg.execute(&ctx(&store, "", origin), "mail_message_send", json!({})).await;
        assert!(r.parked_ask.is_some(), "{origin:?}: {}", r.content);
    }
    let tainted = ToolContext { untrusted_input: true, ..ctx(&store, "", Origin::Workflow) };
    assert!(reg.execute(&tainted, "mail_message_send", json!({})).await.parked_ask.is_some());
    assert_eq!(ran.load(Ordering::SeqCst), 1);
}

/// A folder-fenced job gains the run's project folder; a job with no
/// folders stays unfenced (adding one would turn "anywhere" into "only here").
#[test]
fn a_fenced_job_gains_the_runs_folder() {
    let mut g = Grant::new("emp", Mode::Automatic);
    g.run_folders = vec!["/home/me/proj".into()];
    assert!(g.folders().is_empty(), "an unfenced job stays unfenced");
    g.rules.push(rule(Scope::Employee("emp".into()), cap("file"), Some(RuleField::Folder("/home/me/docs".into())), Effect::Allow));
    assert_eq!(g.folders(), vec![std::path::PathBuf::from("/home/me/docs"), "/home/me/proj".into()]);
    g.run_folders.push("/home/me/docs".into());
    assert_eq!(g.folders().len(), 2, "no duplicate folder");
}
