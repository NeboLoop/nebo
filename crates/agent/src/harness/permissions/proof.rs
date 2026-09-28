//! Deterministic proofs for the permission fixtures (`fixtures/permissions/`,
//! `suites/permissions.yaml`): the scenarios whose facts are mechanical run
//! here, with no model and no server. A fixture's `proof:` names the test
//! (`harness::permissions::proof::<name>`); `nebo-cli test run` runs it in
//! this crate.

use std::sync::Arc;

use tools::needs::{self, CapabilityTerm, DeclaredNeeds, DescriptionReader, JobSource};
use tools::{Origin, ToolContext};
use types::permissions::{AskCase, CallEffects, Decision, Effect, RuleKey, RuleSource, Scope, Target};

use super::consent::grant_job;
use super::{decide, resolve_grant, CheckCx};

fn store() -> (tempfile::TempDir, Arc<db::Store>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(db::Store::new(&dir.path().join("proof.db").to_string_lossy()).unwrap());
    (dir, store)
}

fn decision(store: &db::Store, agent: &str, capability: &str) -> Decision {
    let grant = resolve_grant(store, agent, None);
    let ctx = ToolContext {
        origin: Origin::User,
        session_key: format!("agent:{agent}:web"),
        grant: Some(Arc::new(grant.clone())),
        ..Default::default()
    };
    let t = Target {
        tool: "probe".into(),
        key: format!("{capability}_call"),
        operation: None,
        capability: Some(capability.into()),
        field: None,
        subject: None,
        read_only: false,
        effects: CallEffects::unknown(),
    };
    let input = serde_json::json!({});
    decide(&CheckCx { ctx: &ctx, input: &input, grant: &grant, store }, &t)
}

fn job(store: &db::Store, agent: &str) -> Vec<String> {
    let mut caps: Vec<String> = store
        .permission_rules_in(&Scope::Employee(agent.to_string()))
        .unwrap()
        .into_iter()
        .filter(|r| r.effect == Effect::Allow)
        .filter_map(|r| match r.key {
            RuleKey::Capability(c) => Some(c),
            _ => None,
        })
        .collect();
    caps.sort();
    caps
}

struct NoModel;

#[async_trait::async_trait]
impl DescriptionReader for NoModel {
    async fn capabilities_in(&self, _d: &str, _v: &[CapabilityTerm], _a: &str) -> Vec<String> {
        panic!("a package's needs are read off its manifest, never by a model");
    }
}

/// The builder's description, read as the aux call would.
struct Reads(&'static [&'static str]);

#[async_trait::async_trait]
impl DescriptionReader for Reads {
    async fn capabilities_in(&self, _d: &str, _v: &[CapabilityTerm], _a: &str) -> Vec<String> {
        self.0.iter().map(|s| s.to_string()).collect()
    }
}

/// `hire-one-line-grants-the-job`: a packaged employee's declared needs are
/// read mechanically, rendered as one plain line above Hire, and the tap
/// grants exactly those as standing allow rules for that employee.
#[tokio::test]
async fn hire_one_line_grants_the_job() {
    let (_d, store) = store();
    let manifest = r#"{
        "requires": { "interfaces": ["telephony", "calendar"] },
        "workflows": {
            "new-mail": { "trigger": { "type": "watch", "plugin": "mail", "event": "email.new" }, "activities": [] }
        }
    }"#;
    let config = napp::agent::parse_agent_config(manifest).unwrap();
    let declared = DeclaredNeeds::of(&config);
    let src = JobSource {
        name: "Receptionist",
        agent_id: "",
        description: "Answers calls, runs errands on the web, and anything else that comes up.",
        skills: &[],
        plugins: &[],
        workflows: &[],
        declared: Some(&declared),
        installed: &[],
    };
    let needs = needs::work_out_needs(&src, &NoModel).await;
    assert_eq!(
        needs::consent_line("Receptionist", &needs),
        "Receptionist will manage your calendar, read and send email, and answer your calls."
    );
    grant_job(&store, "receptionist", &needs, RuleSource::Hire { package: "receptionist".into() }).unwrap();
    assert_eq!(job(&store, "receptionist"), vec!["calendar", "mail", "telephony"]);
    for r in store.permission_rules_in(&Scope::Employee("receptionist".into())).unwrap() {
        assert_eq!(r.source, RuleSource::Hire { package: "receptionist".into() });
    }
    // Nothing else: no company rule, no rule for another employee, and the
    // description's words (the web) never became part of the job.
    assert!(store.permission_rules_in(&Scope::Company).unwrap().is_empty());
    assert!(job(&store, "front-desk").is_empty());
    for granted in ["telephony", "calendar", "mail"] {
        assert!(matches!(decision(&store, "receptionist", granted), Decision::Allow { .. }), "{granted}");
    }
    for undeclared in ["web", "shell", "file"] {
        assert!(
            matches!(decision(&store, "receptionist", undeclared), Decision::Ask { case: AskCase::OutsideJob { .. } }),
            "{undeclared} was never declared, so it is outside the job"
        );
    }
}

/// `builder-removed-capability-not-granted`: the builder shows the worked-out
/// needs above Create, each removable; a removed capability is not granted
/// and every other one is.
#[tokio::test]
async fn builder_removed_capability_not_granted() {
    let (_d, store) = store();
    let src = JobSource {
        name: "Lead Finder",
        agent_id: "",
        description: "Researches new leads online, emails them, and books intro calls.",
        skills: &[],
        plugins: &[],
        workflows: &[],
        declared: None,
        installed: &[],
    };
    let drafted = needs::work_out_needs(&src, &Reads(&["web", "mail", "calendar"])).await;
    assert_eq!(drafted.capabilities.len(), 3);
    // The owner removes the web chip before tapping Create.
    let kept = drafted.without(&["web".to_string()]);
    assert_eq!(
        needs::consent_line("Lead Finder", &kept),
        "Lead Finder will manage your calendar and read and send email."
    );
    grant_job(&store, "lead-finder", &kept, RuleSource::Created { draft_id: "d1".into() }).unwrap();
    assert_eq!(job(&store, "lead-finder"), vec!["calendar", "mail"]);
    assert!(matches!(decision(&store, "lead-finder", "web"), Decision::Ask { case: AskCase::OutsideJob { .. } }));
    assert!(matches!(decision(&store, "lead-finder", "mail"), Decision::Allow { .. }));
}

/// `company-deny-beats-employee-allow`: a deny from any scope decides. The
/// company turns the
/// shell off; an employee's own allow (its job, or an "Allow always") never
/// turns it back on, and an answer the owner gave once never runs it.
#[tokio::test]
async fn company_deny_beats_employee_allow() {
    let (_d, store) = store();
    let rule = |scope: Scope, effect: Effect, source: RuleSource| types::permissions::Rule {
        id: uuid::Uuid::new_v4().to_string(),
        scope,
        key: RuleKey::Capability("shell".into()),
        field: None,
        effect,
        money: None,
        source,
        locked: false,
        created_at: 0,
    };
    let owner = types::permissions::Writer::Owner;
    store.write_permission_rule(&rule(Scope::Employee("dev".into()), Effect::Allow, RuleSource::Owner), &owner).unwrap();
    assert!(matches!(decision(&store, "dev", "shell"), Decision::Allow { .. }), "the employee's job runs");
    store.write_permission_rule(&rule(Scope::Company, Effect::Deny, RuleSource::Owner), &owner).unwrap();
    assert!(matches!(decision(&store, "dev", "shell"), Decision::Deny { .. }), "the company deny binds the employee");
    store
        .write_permission_rule(&rule(Scope::Employee("dev".into()), Effect::Allow, RuleSource::AllowAlways { ask_id: "a1".into() }), &owner)
        .unwrap();
    assert!(matches!(decision(&store, "dev", "shell"), Decision::Deny { .. }), "an Allow always doesn't undo it");
    // An ask the owner answered once is not a way around it either.
    let grant = resolve_grant(&store, "dev", None);
    let ctx = ToolContext {
        origin: Origin::User,
        session_key: "agent:dev:web".into(),
        grant: Some(Arc::new(grant.clone())),
        answered_ask: Some("a2".into()),
        ..Default::default()
    };
    let t = Target {
        tool: "probe".into(),
        key: "shell_call".into(),
        operation: None,
        capability: Some("shell".into()),
        field: None,
        subject: None,
        read_only: false,
        effects: CallEffects::unknown(),
    };
    let input = serde_json::json!({});
    assert!(matches!(decide(&CheckCx { ctx: &ctx, input: &input, grant: &grant, store: &store }, &t), Decision::Deny { .. }));
}

/// `a-tools-own-setting-outranks-its-default`: on the Permissions page a
/// connected server's default covers its tools, and a tool's own switch
/// overrides it whichever way it points. The permission check agrees: a
/// tool set to Allow runs under a server set to Ask or Off, a tool set to
/// Off is refused under a server set to Allow, and an employee's own Allow
/// still doesn't undo a company Off.
#[tokio::test]
async fn a_tools_own_setting_outranks_its_default() {
    let (_d, store) = store();
    let owner = types::permissions::Writer::Owner;
    let put = |scope: Scope, key: &str, effect: Effect| {
        let rule = types::permissions::Rule {
            id: uuid::Uuid::new_v4().to_string(),
            scope,
            key: RuleKey::Tool(key.into()),
            field: None,
            effect,
            money: None,
            source: RuleSource::Owner,
            locked: false,
            created_at: 0,
        };
        store.write_permission_rule(&rule, &owner).unwrap();
    };
    let call = |agent: &str, tool: &str| {
        let grant = resolve_grant(&store, agent, None);
        let ctx = ToolContext {
            origin: Origin::User,
            session_key: format!("agent:{agent}:web"),
            grant: Some(Arc::new(grant.clone())),
            ..Default::default()
        };
        let t = Target {
            tool: tool.into(),
            key: tool.into(),
            operation: None,
            capability: None,
            field: None,
            subject: None,
            read_only: false,
            effects: CallEffects::unknown(),
        };
        let input = serde_json::json!({});
        decide(&CheckCx { ctx: &ctx, input: &input, grant: &grant, store: &store }, &t)
    };
    let (lookup, purge) = ("mcp__crm__lookup", "mcp__crm__purge");

    put(Scope::Company, "mcp__crm__*", Effect::Ask);
    assert!(matches!(call("dev", lookup), Decision::Ask { .. }), "the server's default asks");
    put(Scope::Company, lookup, Effect::Allow);
    assert!(matches!(call("dev", lookup), Decision::Allow { .. }), "the tool's own Allow runs it");
    assert!(matches!(call("dev", purge), Decision::Ask { .. }), "the others keep the default");
    put(Scope::Company, "mcp__crm__*", Effect::Deny);
    assert!(matches!(call("dev", lookup), Decision::Allow { .. }), "its own Allow outranks a server Off");
    assert!(matches!(call("dev", purge), Decision::Deny { .. }));
    put(Scope::Company, "mcp__crm__*", Effect::Allow);
    put(Scope::Company, purge, Effect::Deny);
    assert!(matches!(call("dev", purge), Decision::Deny { .. }), "its own Off outranks a server Allow");
    put(Scope::Employee("dev".into()), purge, Effect::Allow);
    assert!(matches!(call("dev", purge), Decision::Deny { .. }), "an employee's Allow doesn't undo a company Off");
}

/// A server on this computer that answers every request with a page titled
/// "Example Domain", and counts the connections it took.
fn page_server() -> (u16, Arc<std::sync::atomic::AtomicUsize>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = hits.clone();
    std::thread::spawn(move || {
        for mut conn in listener.incoming().flatten() {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut buf = [0u8; 1024];
            let _ = conn.read(&mut buf);
            let body = "<html><title>Example Domain</title></html>";
            let _ = write!(conn, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        }
    });
    (port, hits)
}

/// `web-off-shell-reaches-no-network` (helper-cannot-exceed-parent, E8): an
/// employee whose web access the owner turned off can't reach the network
/// through the shell either, and neither can its helpers, whether the
/// refusal is the employee's own rule or only the grant a helper runs under.
/// On 2026-09-26 a helper refused `browser_open` and `fetch_url` ran `curl`
/// through run_command and got the page, in all three runs. The same
/// command from an employee with web access reaches the page, so the
/// command and the server are real.
#[tokio::test]
async fn web_off_shell_reaches_no_network() {
    use std::sync::atomic::Ordering;
    let (_d, store) = store();
    let owner = types::permissions::Writer::Owner;
    let rule = |scope: Scope, cap: &str, effect: Effect| types::permissions::Rule {
        id: uuid::Uuid::new_v4().to_string(),
        scope,
        key: RuleKey::Capability(cap.into()),
        field: None,
        effect,
        money: None,
        source: RuleSource::Owner,
        locked: false,
        created_at: 0,
    };
    // Every job has the shell and the web; the owner turned web off for the clerk.
    for cap in ["shell", "web"] {
        store.write_permission_rule(&rule(Scope::Company, cap, Effect::Allow), &owner).unwrap();
    }
    store.write_permission_rule(&rule(Scope::Employee("clerk".into()), "web", Effect::Deny), &owner).unwrap();
    let registry = tools::Registry::new(Arc::new(super::Check::new(store.clone())));
    registry.register_defaults().await;
    let (port, hits) = page_server();
    let fetch = serde_json::json!({ "command": format!("curl -s -m 5 http://127.0.0.1:{port}/"), "description": "Fetch the page" });

    let clerk = resolve_grant(&store, "clerk", None);
    let run = |key: &str, grant: types::permissions::Grant, door: types::permissions::Door| ToolContext {
        origin: Origin::System,
        session_key: key.into(),
        session_id: key.replace(':', "_"),
        door,
        grant: Some(Arc::new(grant)),
        ..Default::default()
    };
    // The clerk's own run, its helper as delegation seats one (its grant,
    // under it as the ceiling), and a helper whose own rules would allow the
    // web but whose employee's don't.
    let mut helper = clerk.clone();
    helper.ceiling = Some(types::permissions::Ceiling::Parent { grant: Box::new(clerk.clone()) });
    let mut narrowed = resolve_grant(&store, "", None);
    narrowed.ceiling = Some(types::permissions::Ceiling::Parent { grant: Box::new(clerk.clone()) });
    use types::permissions::Door;
    for (who, ctx) in [
        ("the clerk", run("agent:clerk:web", clerk.clone(), Door::Chat)),
        ("the clerk's helper", run("subagent:agent:clerk:web:sa-1", helper, Door::Helper)),
        ("a helper under the clerk's grant", run("subagent:agent:clerk:web:sa-2", narrowed, Door::Helper)),
    ] {
        let r = registry.execute(&ctx, "run_command", fetch.clone()).await;
        assert!(!r.content.contains("Example Domain"), "{who} reached the page: {}", r.content);
        assert!(r.content.contains("web access is off"), "{who} was not told why: {}", r.content);
        assert!(r.content.contains("No other tool, helper or coworker gets around it"), "{who}: another route stays open: {}", r.content);
    }
    assert_eq!(hits.load(Ordering::SeqCst), 0, "a web-off run reached the server");

    // An employee with web access runs the same command and gets the page.
    let analyst = run("agent:analyst:web", resolve_grant(&store, "analyst", None), Door::Chat);
    let r = registry.execute(&analyst, "run_command", fetch).await;
    assert!(r.content.contains("Example Domain"), "the command itself works: {}", r.content);
    assert!(!r.content.contains("web access is off"), "{}", r.content);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}
