//! Consent to a job: the owner's yes grants it, an employee's creation is
//! capped at its creator with one card for the rest, and only the owner
//! widens.

use std::sync::Arc;

use serde_json::json;
use tools::needs::{CapabilityTerm, DeclaredNeeds, DescriptionReader, JobConsent, JobSource, Needs};
use tools::{Origin, Registry, ToolContext};
use types::permissions::{
    AskCase, CallEffects, Ceiling, Decision, Effect, Mode, Rule, RuleKey, RuleSource, Scope, Target, Why, Writer,
};

use super::*;
use crate::harness::permissions::{decide, resolve_grant, Check, CheckCx};

fn store() -> (tempfile::TempDir, Arc<db::Store>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(db::Store::new(&dir.path().join("c.db").to_string_lossy()).unwrap());
    (dir, store)
}

fn needs(caps: &[&str]) -> Needs {
    Needs { capabilities: caps.iter().map(|c| c.to_string()).collect(), accounts: vec![] }
}

fn allow(scope: Scope, capability: &str) -> Rule {
    Rule {
        id: uuid::Uuid::new_v4().to_string(),
        scope,
        key: RuleKey::Capability(capability.into()),
        field: None,
        effect: Effect::Allow,
        money: None,
        source: RuleSource::Owner,
        locked: false,
        created_at: 0,
    }
}

fn employee_caps(store: &db::Store, agent: &str) -> Vec<String> {
    job_of(store, agent).unwrap().capabilities.into_iter().collect()
}

/// A call on one capability.
fn call(capability: &str) -> Target {
    Target {
        tool: "probe".into(),
        key: format!("{capability}_call"),
        operation: None,
        capability: Some(capability.into()),
        field: None,
        subject: None,
        read_only: false,
        effects: CallEffects::unknown(),
    }
}

fn decide_for(store: &db::Store, agent: &str, t: &Target) -> Decision {
    let grant = resolve_grant(store, agent, None);
    let ctx = ToolContext {
        origin: Origin::User,
        session_key: format!("agent:{agent}:web"),
        grant: Some(Arc::new(grant.clone())),
        ..Default::default()
    };
    let input = json!({});
    decide(&CheckCx { ctx: &ctx, input: &input, grant: &grant, store }, t)
}

/// Answers every description with fixed keys.
struct Fixed(Vec<&'static str>);

#[async_trait::async_trait]
impl DescriptionReader for Fixed {
    async fn capabilities_in(&self, _d: &str, _v: &[CapabilityTerm], _a: &str) -> Vec<String> {
        self.0.iter().map(|s| s.to_string()).collect()
    }
}

/// A reader that must never be asked.
struct NeverAsked;

#[async_trait::async_trait]
impl DescriptionReader for NeverAsked {
    async fn capabilities_in(&self, _d: &str, _v: &[CapabilityTerm], _a: &str) -> Vec<String> {
        panic!("a package's needs never reach a model");
    }
}

#[test]
fn hire_tap_grants_the_needs_as_allow_rules() {
    let (_d, store) = store();
    let granted = grant_job(
        &store,
        "receptionist",
        &needs(&["telephony", "mail", "calendar"]),
        RuleSource::Hire { package: "receptionist".into() },
    )
    .unwrap();
    assert_eq!(granted.len(), 3);
    for r in &granted {
        assert_eq!(r.scope, Scope::Employee("receptionist".into()));
        assert_eq!((r.effect, r.field.as_ref(), r.locked), (Effect::Allow, None, false));
        assert_eq!(r.source, RuleSource::Hire { package: "receptionist".into() });
    }
    assert_eq!(employee_caps(&store, "receptionist"), vec!["calendar", "mail", "telephony"]);
    assert!(store.permission_rules_in(&Scope::Company).unwrap().is_empty(), "the hire writes nothing company-wide");
    assert!(employee_caps(&store, "someone-else").is_empty());
    // A second tap changes nothing: one rule per capability.
    grant_job(&store, "receptionist", &needs(&["mail"]), RuleSource::Hire { package: "receptionist".into() }).unwrap();
    assert_eq!(store.permission_rules_in(&Scope::Employee("receptionist".into())).unwrap().len(), 3);
    // The job decides: mail runs, the shell it never declared asks.
    assert!(matches!(decide_for(&store, "receptionist", &call("mail")), Decision::Allow { .. }));
    assert!(matches!(
        decide_for(&store, "receptionist", &call("shell")),
        Decision::Ask { case: AskCase::OutsideJob { .. } }
    ));
}

/// A bookkeeping plugin, as the operation tools present its operations.
struct Books;

impl tools::operation_tools::OperationProvider for Books {
    fn provider(&self) -> &str {
        "books"
    }
    fn service(&self) -> String {
        "Books".into()
    }
    fn operations(&self) -> Vec<tools::operation_tools::ProvidedOperation> {
        ["ledger.invoice.list", "ledger.invoice.update"]
            .into_iter()
            .map(|op| tools::operation_tools::ProvidedOperation { operation: op.into(), ..Default::default() })
            .collect()
    }
    fn perform<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        _operation: &'a str,
        _input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = tools::ToolResult> + Send + 'a>> {
        Box::pin(async { tools::ToolResult::ok("RAN") })
    }
}

/// The interface a hired package binds is its job: the hire's consent
/// grants it, so the operations of that interface run without asking; an
/// employee that binds no such interface is outside its job (case 4).
#[tokio::test]
async fn a_bound_interface_is_inside_the_job() {
    let (_d, store) = store();
    let declared = DeclaredNeeds { interfaces: vec!["ledger".into()], plugins: vec![], watches: vec![] };
    let src = JobSource {
        name: "Bookkeeper",
        agent_id: "",
        description: "",
        skills: &[],
        plugins: &[],
        workflows: &[],
        declared: Some(&declared),
        installed: &[],
    };
    let needs = tools::needs::work_out_needs(&src, &NeverAsked).await;
    grant_job(&store, "bookkeeper", &needs, RuleSource::Hire { package: "bookkeeper".into() }).unwrap();

    let ops = tools::operation_tools::operation_tools(&[Arc::new(Books) as Arc<dyn tools::operation_tools::OperationProvider>]);
    for name in ["ledger_invoice_list", "ledger_invoice_update"] {
        let tool = ops.iter().find(|t| tools::registry::DynTool::name(*t) == name).unwrap();
        let t = tools::registry::target_of(tool, &json!({}));
        assert_eq!(t.capability.as_deref(), Some("ledger"), "{name}");
        let hired = decide_for(&store, "bookkeeper", &t);
        assert!(matches!(hired, Decision::Allow { .. }), "{name}: {hired:?}");
        let unbound = decide_for(&store, "receptionist", &t);
        assert!(
            matches!(&unbound, Decision::Ask { case: AskCase::OutsideJob { capability } } if capability == "ledger"),
            "{name}: {unbound:?}"
        );
    }
}

#[tokio::test]
async fn missing_account_goes_to_the_inbox_needs_flow() {
    let (_d, store) = store();
    // A package whose watch runs on mail, with no plugin bound for it yet.
    let declared = DeclaredNeeds { interfaces: vec!["mail".into()], plugins: vec![], watches: vec!["mail".into()] };
    let src = JobSource {
        name: "Inbox Keeper",
        agent_id: "",
        description: "",
        skills: &[],
        plugins: &[],
        workflows: &[],
        declared: Some(&declared),
        installed: &[],
    };
    let needs = tools::needs::work_out_needs(&src, &NeverAsked).await;
    assert_eq!(needs.accounts.iter().map(|a| a.capability.as_str()).collect::<Vec<_>>(), vec!["mail"]);
    grant_job(&store, "keeper", &needs, RuleSource::Hire { package: "keeper".into() }).unwrap();
    // The capability is the job; the account is not a permission. Nothing
    // was parked on the owner: a missing account reaches them through the
    // Inbox needs flow (the binding's pre-flight), not as an ask.
    assert_eq!(employee_caps(&store, "keeper"), vec!["mail"]);
    assert!(matches!(decide_for(&store, "keeper", &call("mail")), Decision::Allow { .. }));
    let keeper = db::PermissionActivityFilter { agent_id: Some("keeper".into()), limit: 10, ..Default::default() };
    assert!(store.permission_activity(&keeper).unwrap().0.is_empty());
}

#[test]
fn employee_made_employee_capped_at_creator_with_one_card() {
    let (_d, store) = store();
    // The creator's job: mail, not web.
    store.write_permission_rule(&allow(Scope::Employee("office".into()), "mail"), &Writer::Owner).unwrap();
    // The company allows web to everyone by default: the ceiling, not the
    // missing rule, is what keeps the new employee below its creator.
    store.write_permission_rule(&allow(Scope::Company, "web"), &Writer::Owner).unwrap();
    store
        .write_permission_rule(
            &Rule { effect: Effect::Deny, ..allow(Scope::Employee("office".into()), "web") },
            &Writer::Owner,
        )
        .unwrap();
    let creator = resolve_grant(&store, "office", None);
    let g = create_under_creator(&Asks::new(store.clone()), &creator, "researcher", "Lead Researcher", &needs(&["web", "mail"]), "d1", "agent:office:web")
        .unwrap();
    assert_eq!(g.granted, needs(&["mail"]));
    assert_eq!(g.extras, needs(&["web"]));
    let card = g.card.expect("one card for the extras");
    let ask = store.get_permission_ask(&card).unwrap().unwrap();
    assert_eq!(ask.agent_id, "researcher");
    assert_eq!(ask.sentence, "Lead Researcher will look things up on the web.");
    assert_eq!(
        serde_json::from_str::<AskCase>(&ask.ask_case).unwrap(),
        AskCase::CreatedExtras { capabilities: vec!["web".into()] }
    );
    // The creator wrote only what it holds, and never an allow for web.
    assert_eq!(employee_caps(&store, "researcher"), vec!["mail"]);
    // Until the owner answers, the employee works under its creator.
    let grant = resolve_grant(&store, "researcher", None);
    assert!(matches!(&grant.ceiling, Some(Ceiling::Creator { creator_id, .. }) if creator_id == "office"));
    assert!(matches!(decide_for(&store, "researcher", &call("web")), Decision::Deny { why: Why::Ceiling, .. }));
    assert!(matches!(decide_for(&store, "researcher", &call("mail")), Decision::Allow { .. }));
    // No leaves it as it is; Allow grants the extras and lifts the ceiling.
    answer_extras(&store, &card, false).unwrap();
    assert!(resolve_grant(&store, "researcher", None).ceiling.is_some());
    answer_extras(&store, &card, true).unwrap();
    assert_eq!(employee_caps(&store, "researcher"), vec!["mail", "web"]);
    assert!(resolve_grant(&store, "researcher", None).ceiling.is_none());
    assert!(matches!(decide_for(&store, "researcher", &call("web")), Decision::Allow { .. }));
    // An employee never widens: the creator's writer can't loosen a deny.
    let widen = Rule { effect: Effect::Allow, ..allow(Scope::Employee("office".into()), "web") };
    assert!(store.write_permission_rule(&widen, &Writer::Employee { agent_id: "office".into() }).is_err());
}

#[tokio::test]
async fn grant_authority_raises_a_card_and_writes_nothing() {
    let (_d, store) = store();
    let registry = Registry::new(Arc::new(Check::new(store.clone())));
    registry.register(Box::new(tools::authority_tool::AuthorityTool::new(store.clone()))).await;
    store.write_permission_rule(&allow(Scope::Company, "ledger"), &Writer::Owner).unwrap();
    for mode in [Mode::Automatic, Mode::FullAccess] {
        let mut grant = resolve_grant(&store, "manager", None);
        grant.mode = mode;
        let ctx = ToolContext {
            origin: Origin::User,
            session_key: "agent:manager:web".into(),
            grant: Some(Arc::new(grant)),
            ..Default::default()
        };
        let r = registry
            .execute(
                &ctx,
                "authority",
                json!({
                    "resource": "grant", "action": "grant", "agent": "bookkeeper",
                    "operation": "ledger.billpayment.create", "bounds": { "max_amount_cents": 1000 },
                    "display": "Let the Bookkeeper pay bills up to $10.00."
                }),
            )
            .await;
        let ask_id = r.parked_ask.clone().unwrap_or_else(|| panic!("{mode:?}: the grant parks on the owner: {}", r.content));
        let ask = store.get_permission_ask(&ask_id).unwrap().unwrap();
        assert_eq!(serde_json::from_str::<AskCase>(&ask.ask_case).unwrap(), AskCase::Widens, "{mode:?}");
    }
    assert!(store.permission_rules_in(&Scope::Employee("bookkeeper".into())).unwrap().is_empty(), "nothing written");
}

// ── Chat creation and job edits, through the create tool ─────────────────

struct Chat {
    _dir: tempfile::TempDir,
    store: Arc<db::Store>,
    employees: Vec<tools::employee_tools::EmployeeTool>,
    ctx: ToolContext,
}

/// The main assistant's chat, with the create tool wired to consent that
/// reads every description as `reads`. The assistant's own job: mail.
fn chat(reads: Vec<&'static str>) -> Chat {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(db::Store::new(&dir.path().join("c.db").to_string_lossy()).unwrap());
    store.write_permission_rule(&allow(Scope::Company, "mail"), &Writer::Owner).unwrap();
    let loader = Arc::new(napp::AgentLoader::new(dir.path().join("installed"), dir.path().join("user")));
    let cell: tools::needs::JobConsentCell = Arc::new(std::sync::RwLock::new(None));
    let persona = tools::agent_tool::PersonaTool::new(store.clone(), Default::default(), loader)
        .with_job_consent(cell.clone());
    *cell.write().unwrap() = Some(Arc::new(Consent::new(Arc::new(Asks::new(store.clone())), Arc::new(Fixed(reads)))));
    store.create_chat("chat-s1", "Owner chat").unwrap();
    let ctx = ToolContext {
        origin: Origin::User,
        session_key: "agent:assistant:web".into(),
        session_id: "s1".into(),
        grant: Some(Arc::new(resolve_grant(&store, "", None))),
        ..Default::default()
    };
    Chat { _dir: dir, store, employees: tools::employee_tools::tools(persona), ctx }
}

impl Chat {
    /// The turn is the owner's own message in his own chat, as the harness
    /// marks it (`owner_speaks`).
    fn owner_turn(mut self) -> Self {
        self.ctx.owner_request = true;
        self
    }

    /// A call to one of the employee tools, as the registry runs it.
    async fn call(&self, tool: &str, input: serde_json::Value) -> tools::ToolResult {
        use tools::registry::DynTool;
        let t = self.employees.iter().find(|t| t.name() == tool).expect("an employee tool");
        t.execute_dyn(&self.ctx, input).await
    }

    /// The owner says something in the chat, after the line was shown: the
    /// row the harness stores for the owner's own input.
    fn owner_says(&self, text: &str) {
        self.says(
            text,
            Some(&serde_json::json!({ db::OWNER_MARK: true }).to_string()),
        );
    }

    /// A message lands in the chat after the line was shown, stored with
    /// `metadata` as its door stores it.
    fn says(&self, text: &str, metadata: Option<&str>) {
        self.says_at(text, metadata, 5);
    }

    /// A message stored `offset` seconds from now: after the line when
    /// positive, before it was drafted when negative.
    fn says_at(&self, text: &str, metadata: Option<&str>, offset: i64) {
        let later = chrono::Utc::now().timestamp() + offset;
        self.store
            .create_chat_message_imported(
                &uuid::Uuid::new_v4().to_string(),
                "chat-s1",
                "user",
                text,
                None,
                metadata,
                later,
            )
            .unwrap();
    }

    fn draft_of(r: &tools::ToolResult) -> String {
        r.payload.as_ref().and_then(|p| p["draftId"].as_str()).expect("a drafted job carries its chip").to_string()
    }

    fn agent_id(&self, name: &str) -> String {
        self.store.get_agent_by_name(name).unwrap().expect("created").id
    }
}

#[tokio::test]
async fn chat_create_grants_only_after_an_owner_message() {
    let c = chat(vec!["mail", "calendar"]);
    let create = json!({
        "name": "invoice-chaser",
        "description": "Reads the accounting inbox and puts follow-ups on the calendar."
    });
    // The first call drafts: nothing is created, the line is the chip.
    let drafted = c.call("create_employee", create.clone()).await;
    assert!(!drafted.is_error, "{}", drafted.content);
    let line = "Invoice Chaser will manage your calendar and read and send email.";
    assert!(drafted.content.contains(line), "{}", drafted.content);
    let chip = drafted.payload.clone().unwrap();
    assert_eq!((chip["kind"].as_str(), chip["line"].as_str()), (Some("employee_consent"), Some(line)));
    assert!(c.store.get_agent_by_name("Invoice Chaser").unwrap().is_none(), "a draft creates nothing");

    // No owner message since the line: the create is the assistant's own,
    // capped at what it holds (mail), with the calendar on one card.
    let unasked = c.call("create_employee", json!({ "draft_id": Chat::draft_of(&drafted) })).await;
    assert!(!unasked.is_error, "{}", unasked.content);
    let id = c.agent_id("Invoice Chaser");
    assert_eq!(employee_caps(&c.store, &id), vec!["mail"]);
    assert!(resolve_grant(&c.store, &id, None).ceiling.is_some());
    assert!(unasked.content.contains("One approval card went to the owner"), "{}", unasked.content);
    // A draft is acted on once.
    let again = c.call("create_employee", json!({ "draft_id": Chat::draft_of(&drafted) })).await;
    assert!(again.is_error && again.content.contains("already used"), "{}", again.content);

    // The owner's "yes, create it" after the line is the consent: the whole
    // drafted job, as the owner's, with no ceiling.
    let drafted = c.call("create_employee", json!({ "name": "follow-upper", "description": "Same job." })).await;
    c.owner_says("Yes, create it.");
    let created = c.call("create_employee", json!({ "draft_id": Chat::draft_of(&drafted) })).await;
    assert!(!created.is_error, "{}", created.content);
    let id = c.agent_id("Follow Upper");
    assert_eq!(employee_caps(&c.store, &id), vec!["calendar", "mail"]);
    assert!(resolve_grant(&c.store, &id, None).ceiling.is_none());
    let sources: Vec<RuleSource> =
        c.store.permission_rules_in(&Scope::Employee(id)).unwrap().into_iter().map(|r| r.source).collect();
    assert!(sources.iter().all(|s| matches!(s, RuleSource::Created { .. })), "{sources:?}");
}

/// The owner's "just create it now" came before the draft: the create runs
/// in the same turn with no second ask in chat. His own request in his own
/// chat is his yes (`ToolContext::owner_request`), so the new employee gets
/// the whole drafted job as his, with no ceiling and no card.
#[tokio::test]
async fn an_owners_go_before_the_draft_creates_the_whole_job_at_once() {
    let c = chat(vec!["mail", "calendar"]).owner_turn();
    c.says_at("just create it now", Some(&json!({ db::OWNER_MARK: true }).to_string()), -5);
    let drafted = c
        .call("create_employee", json!({ "name": "invoice-chaser", "description": "Reads the inbox and books follow-ups." }))
        .await;
    assert!(!drafted.is_error, "{}", drafted.content);
    let draft_id = Chat::draft_of(&drafted);
    assert!(
        drafted.content.contains(&format!(
            "If the owner's latest message already told you to create it now (\"create it\", \"just do it\", \
             \"go ahead\"), call create_employee(draft_id: \"{draft_id}\") now and don't ask again"
        )),
        "the draft tells the model to create at once on the owner's go: {}",
        drafted.content
    );
    assert!(!drafted.content.contains("card"), "no card is promised before one exists: {}", drafted.content);
    let created = c.call("create_employee", json!({ "draft_id": draft_id })).await;
    assert!(!created.is_error, "{}", created.content);
    let id = c.agent_id("Invoice Chaser");
    assert_eq!(employee_caps(&c.store, &id), vec!["calendar", "mail"], "the whole drafted job");
    assert!(c.store.employee_ceiling(&id).unwrap().is_none(), "no ceiling");
    assert!(c.store.open_permission_asks(None).unwrap().is_empty(), "no card");
    assert!(created.content.contains("The owner agreed to this job"), "{}", created.content);
    assert!(!created.content.contains("card"), "{}", created.content);
}

/// Another employee's "yes", or one from Slack, Discord or a loop, is not
/// the owner's consent: the create stays the assistant's own, capped at what
/// it holds, with the rest on one card for the owner.
#[tokio::test]
async fn a_yes_from_anyone_but_the_owner_is_no_consent() {
    let c = chat(vec!["mail", "calendar"]);
    let drafted = c
        .call("create_employee", json!({ "name": "invoice-chaser", "description": "Reads the inbox and books follow-ups." }))
        .await;
    // Each as its door stores it: a coworker's or a channel's message that
    // starts a turn (no mark), and one queued into a running turn.
    c.says("yes", None);
    c.says(
        "yes, create it",
        Some(r#"{"arrivedMidTurn":true,"via":"slack"}"#),
    );
    c.says("yes", Some(r#"{"arrivedMidTurn":true,"via":"discord"}"#));
    c.says(
        "yes",
        Some(r#"{"arrivedMidTurn":true,"from":"coworker","coworker":"Ops"}"#),
    );
    c.says("yes", Some(r#"{"arrivedMidTurn":true,"via":"loop"}"#));
    let created = c
        .call(
            "create_employee",
            json!({ "draft_id": Chat::draft_of(&drafted) }),
        )
        .await;
    assert!(!created.is_error, "{}", created.content);
    let id = c.agent_id("Invoice Chaser");
    assert_eq!(
        employee_caps(&c.store, &id),
        vec!["mail"],
        "only what the creator holds"
    );
    assert!(
        resolve_grant(&c.store, &id, None).ceiling.is_some(),
        "it works under its creator"
    );
    assert!(
        created.content.contains("One approval card went to the owner"),
        "{}",
        created.content
    );
    assert!(
        c.store.employee_ceiling(&id).unwrap().is_some(),
        "the extras wait on the owner"
    );
}

#[tokio::test]
async fn job_edit_surfaces_only_added_needs() {
    let c = chat(vec!["mail", "calendar"]);
    let drafted = c.call("create_employee", json!({ "name": "order-support", "description": "x" })).await;
    c.owner_says("Yes");
    c.call("create_employee", json!({ "draft_id": Chat::draft_of(&drafted) })).await;
    let id = c.agent_id("Order Support");
    // The job today: mail only (the owner removed the calendar on its page).
    let calendar = c
        .store
        .permission_rules_in(&Scope::Employee(id.clone()))
        .unwrap()
        .into_iter()
        .find(|r| r.key == RuleKey::Capability("calendar".into()))
        .unwrap();
    c.store.remove_permission_rule(&calendar.id, &Writer::Owner).unwrap();
    assert_eq!(employee_caps(&c.store, &id), vec!["mail"]);

    // An edit that reads as mail + calendar asks only for the calendar.
    let edit = json!({
        "name": "Order Support",
        "description": "Replies to customers by email and puts follow-up calls on the calendar."
    });
    let drafted = c.call("update_employee", edit).await;
    assert!(!drafted.is_error, "{}", drafted.content);
    assert!(drafted.content.contains("\"Order Support will manage your calendar.\""), "{}", drafted.content);
    assert!(!drafted.content.contains("email"), "only what is new: {}", drafted.content);
    assert_eq!(drafted.payload.as_ref().unwrap()["adds"], json!(true));
    let unchanged = c.store.get_agent(&id).unwrap().unwrap();
    assert_eq!(unchanged.description, "x", "nothing changes before the yes");

    c.owner_says("Yes, add that.");
    let applied = c.call("update_employee", json!({ "draft_id": Chat::draft_of(&drafted) })).await;
    assert!(!applied.is_error, "{}", applied.content);
    assert_eq!(employee_caps(&c.store, &id), vec!["calendar", "mail"]);
    let calendar = c
        .store
        .permission_rules_in(&Scope::Employee(id.clone()))
        .unwrap()
        .into_iter()
        .find(|r| r.key == RuleKey::Capability("calendar".into()))
        .unwrap();
    assert_eq!(calendar.source, RuleSource::JobEdit);
    assert!(c.store.get_agent(&id).unwrap().unwrap().description.starts_with("Replies"));

    // An edit that adds nothing runs at once.
    let plain = c.call("update_employee", json!({ "name": "Order Support", "description": "Replies by email." })).await;
    assert!(!plain.is_error && plain.payload.is_none(), "{}", plain.content);
}

/// A blank employee, as the Employees page's blank create makes it: no
/// description, no instructions, no job.
fn blank(c: &Chat, name: &str) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    c.store.create_agent(&id, None, name, "", "", "{}", None, None).unwrap();
    id
}

/// correction-agent-update-description and -instructions: the owner dictates
/// a blank employee's new description or instructions word for word. His own
/// message is his consent (`ToolContext::owner_request`, the one rule the
/// permission check already applies): the edit lands in the one call, what
/// it adds to the job is granted as his, and nothing is drafted, asked again
/// or put on a card.
#[tokio::test]
async fn an_edit_the_owner_dictates_is_made_in_one_call() {
    let cases = [
        ("description", "Answers inbound calls for NeboAI and takes messages for the team.", "description updated"),
        (
            "instructions",
            "You answer inbound calls for NeboAI. Use only what is in local memory; never search the web. Never \
             transfer to a person. Take the caller's name, direct number and company, and say we will call back as \
             soon as possible.",
            "instructions (AGENT.md body) replaced",
        ),
    ];
    for (field, text, lands) in cases {
        let c = chat(vec!["telephony"]).owner_turn();
        let id = blank(&c, "front-desk-t1");
        let mut edit = json!({ "name": "front-desk-t1" });
        edit[field] = json!(text);
        let r = c.call("update_employee", edit).await;
        assert!(!r.is_error, "{field}: {}", r.content);
        assert!(r.payload.is_none(), "{field}: nothing is drafted: {}", r.content);
        assert!(r.content.contains(lands), "{field}: the result names what changed: {}", r.content);
        for second_step in ["draft_id", "Nothing is changed yet", "confirm", "card"] {
            assert!(!r.content.contains(second_step), "{field}: no second step ({second_step}): {}", r.content);
        }
        let row = c.store.get_agent(&id).unwrap().unwrap();
        match field {
            "description" => assert_eq!(row.description, text),
            _ => assert!(row.agent_md.contains(text), "{field}: {}", row.agent_md),
        }
        assert_eq!(employee_caps(&c.store, &id), vec!["telephony"], "{field}: the addition is granted");
        let sources: Vec<RuleSource> =
            c.store.permission_rules_in(&Scope::Employee(id)).unwrap().into_iter().map(|r| r.source).collect();
        assert_eq!(sources, vec![RuleSource::JobEdit], "{field}: as the owner's job edit");
        assert!(c.store.open_permission_asks(None).unwrap().is_empty(), "{field}: no card");
    }
}

/// The same edit in a run the owner didn't start with his request (an
/// employee's own work, a workflow, a coworker): it drafts, and nothing
/// changes before his yes. Made without it, the edit lands, the addition is
/// one card, and the result says a card went to the owner only because one
/// did.
#[tokio::test]
async fn an_edit_nobody_asked_for_drafts_and_its_card_is_named() {
    let c = chat(vec!["telephony"]);
    let id = blank(&c, "front-desk-t1");
    let edit = json!({ "name": "front-desk-t1", "description": "Answers inbound calls." });
    let drafted = c.call("update_employee", edit).await;
    assert!(!drafted.is_error, "{}", drafted.content);
    assert!(drafted.content.contains("Nothing is changed yet"), "{}", drafted.content);
    assert!(!drafted.content.contains("card"), "no card is promised before one exists: {}", drafted.content);
    assert_eq!(c.store.get_agent(&id).unwrap().unwrap().description, "", "nothing changes before the yes");
    assert!(c.store.open_permission_asks(None).unwrap().is_empty());

    let applied = c.call("update_employee", json!({ "draft_id": Chat::draft_of(&drafted) })).await;
    assert!(!applied.is_error, "{}", applied.content);
    assert_eq!(c.store.get_agent(&id).unwrap().unwrap().description, "Answers inbound calls.");
    assert!(employee_caps(&c.store, &id).is_empty(), "an employee never widens another");
    assert_eq!(c.store.open_permission_asks(None).unwrap().len(), 1, "the addition is one card");
    assert!(applied.content.contains("One approval card went to the owner"), "{}", applied.content);
}

#[test]
fn the_readers_answer_and_the_job_read_back() {
    // The reader's JSON answer, with prose around it or none at all.
    assert_eq!(parse_capabilities("Sure: {\"capabilities\": [\"mail\", \"web\"]}"), vec!["mail", "web"]);
    assert!(parse_capabilities("no json here").is_empty());
    // The job the create tool diffs an edit against is the owner's rules.
    let (_d, store) = store();
    let consent = Consent::new(Arc::new(Asks::new(store.clone())), Arc::new(Fixed(vec![])));
    grant_job(&store, "a", &needs(&["web"]), RuleSource::Owner).unwrap();
    assert_eq!(JobConsent::job_of(&consent, "a"), needs(&["web"]));
}

/// Records the description reader's request and answers with one key.
struct JobReaderProbe(std::sync::Mutex<Vec<ai::ChatRequest>>);

#[async_trait::async_trait]
impl ai::Provider for JobReaderProbe {
    fn id(&self) -> &str {
        "job-reader-probe"
    }
    async fn stream(&self, req: &ai::ChatRequest) -> Result<ai::EventReceiver, ai::ProviderError> {
        self.0.lock().unwrap().push(req.clone());
        let (tx, rx) = tokio::sync::mpsc::channel(2);
        tx.try_send(ai::StreamEvent::text(r#"{"capabilities": ["mail"]}"#)).unwrap();
        tx.try_send(ai::StreamEvent::done()).unwrap();
        Ok(rx)
    }
}

/// The reading of a job description names the employee it is for on its
/// model call, so Janus attributes it (`X-Agent-ID`).
#[tokio::test]
async fn reading_a_job_names_the_employee_it_is_for() {
    let probe = Arc::new(JobReaderProbe(Default::default()));
    let providers = Arc::new(tokio::sync::RwLock::new(vec![probe.clone() as Arc<dyn ai::Provider>]));
    let reader = AuxReader::new(providers);
    let src = JobSource {
        name: "Clerk",
        agent_id: "clerk-7",
        description: "Chases unpaid invoices by email",
        skills: &[],
        plugins: &[],
        workflows: &[],
        declared: None,
        installed: &[],
    };
    let needs = tools::needs::work_out_needs(&src, &reader).await;
    assert!(needs.capabilities.contains("mail"), "the reading was used: {needs:?}");
    let sent = probe.0.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].trace.purpose, "job_needs");
    assert_eq!(sent[0].trace.agent_id, "clerk-7");
}
