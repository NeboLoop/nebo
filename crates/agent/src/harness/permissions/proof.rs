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
    async fn capabilities_in(&self, _d: &str, _v: &[CapabilityTerm]) -> Vec<String> {
        panic!("a package's needs are read off its manifest, never by a model");
    }
}

/// The builder's description, read as the aux call would.
struct Reads(&'static [&'static str]);

#[async_trait::async_trait]
impl DescriptionReader for Reads {
    async fn capabilities_in(&self, _d: &str, _v: &[CapabilityTerm]) -> Vec<String> {
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
