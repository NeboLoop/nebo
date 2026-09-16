//! The one operation policy through the real server: a law blocks, a
//! reserved operation is the owner's, a seat's ceiling lands and gates, an
//! unknown declared operation fails closed, the company layer's write is a
//! granted operation, and the catalogue is the gate.

use super::layers::company_pack;
use super::*;
use tools::policy::{Bounds, DayCounters, OperationAccess, OperationParams, OperationPolicy, PolicyLayer};
use tools::Origin;

/// `decide` as the runner calls it for an unattended run: no amount, no
/// counters, the company policy as stored, the projection current.
fn decide(nebo: &Nebo, agent_id: &str, op: &str, origin: Origin) -> tools::policy::Decision {
    let company = nebo.company_policy();
    nebo.policy(agent_id).decide(op, origin, &OperationParams::default(), company.as_ref(), None, true)
}

/// The Approvals view's row for one operation.
async fn operation_row(nebo: &Nebo, agent_id: &str, op: &str) -> Option<Value> {
    let ops = nebo.get_ok(&format!("/agents/{agent_id}/operations")).await;
    ops["operations"].as_array().unwrap().iter().find(|o| o["operation"] == op).cloned()
}

/// With the company pack applied, a seat's call on a law-blocked operation
/// decides Blocked from every origin, and the Approvals view says so; on an
/// owner-reserved operation the General Manager's grant is refused by
/// `permits` — "reserved to the owner" — and the seat's policy is untouched.
/// A grant on the law itself is refused as locked. A grant inside the
/// company's bounds on an ordinary operation lands as standing authority and
/// admits a call inside its bound while refusing one outside it, naming the
/// bound.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_law_blocks_and_a_reserved_operation_is_the_owners() {
    let nebo = session().await;
    nebo.clear_layers().await;
    let ap = nebo.hire("Accounts Payable Clerk", json!({ "requires": { "interfaces": ["ledger"] }, "workflows": {} })).await;
    let gm = nebo.hire("Managing Director", json!({ "workflows": {} })).await;
    nebo.park_pack("acme", &company_pack(&[])).await;
    assert_eq!(nebo.apply_layers().await["applied"], json!(["acme"]));
    let company = nebo.company_policy().expect("the company layer is the policy");
    assert_eq!(company.reserved, vec!["ledger.transfer.create".to_string()]);

    // The law blocks, from every origin, and the view agrees.
    for origin in [Origin::User, Origin::Workflow, Origin::System, Origin::Comm] {
        let d = decide(&nebo, &ap, "ledger.billpayment.create", origin);
        assert_eq!(d.access, OperationAccess::Blocked, "{origin:?}: {d:?}");
        assert_eq!(d.layer, PolicyLayer::Law);
        assert_eq!(d.reason, "law:Payments");
    }
    let row = operation_row(&nebo, &ap, "ledger.billpayment.create").await.expect("on the view");
    assert_eq!(row["effective"], "blocked");
    assert_eq!(row["locked"], true);

    // The General Manager grants the reserved operation: refused by permits.
    let gm_ctx = Nebo::ctx(&gm, Origin::User);
    let grant = nebo
        .tool(
            &gm_ctx,
            "authority",
            json!({ "action": "grant", "agent": "Accounts Payable Clerk", "operation": "ledger.transfer.create", "bounds": { "max_amount_cents": 1000, "per_day_cents": 5000 } }),
        )
        .await;
    assert!(grant.is_error, "{}", grant.content);
    assert!(grant.content.contains("ledger.transfer.create is reserved to the owner"), "{}", grant.content);
    assert!(grant.content.contains("Only the owner can change the constitution"), "{}", grant.content);
    assert!(!nebo.policy(&ap).operations.contains_key("ledger.transfer.create"), "nothing was written");
    let rule = tools::policy::OperationRule {
        access: OperationAccess::Always,
        bounds: Some(Bounds { max_amount_cents: Some(1000), ..Default::default() }),
        ..Default::default()
    };
    assert!(matches!(company.permits("ledger.transfer.create", &rule), Err(tools::policy::PolicyError::Reserved(_))));
    // Reserved is the owner's hand, not a block: a call on it asks rather than refuses.
    let d = decide(&nebo, &ap, "ledger.transfer.create", Origin::Workflow);
    assert_eq!(d.access, OperationAccess::Approval, "{d:?}");

    // A grant on the law: refused, the rule is locked.
    let on_law = nebo
        .tool(
            &gm_ctx,
            "authority",
            json!({ "action": "grant", "agent": "Accounts Payable Clerk", "operation": "ledger.billpayment.create", "bounds": { "max_amount_cents": 1000 } }),
        )
        .await;
    assert!(on_law.is_error, "{}", on_law.content);
    assert!(on_law.content.contains("locked"), "{}", on_law.content);
    assert!(nebo.policy(&ap).operations["ledger.billpayment.create"].is_law(), "the law stands");

    // A grant inside the company's bounds on an ordinary operation lands.
    let ok = nebo
        .tool(
            &gm_ctx,
            "authority",
            json!({ "action": "grant", "agent": "Accounts Payable Clerk", "operation": "ledger.bill.create", "bounds": { "max_amount_cents": 100000, "per_day_cents": 400000, "per_day_count": 5 } }),
        )
        .await;
    assert!(!ok.is_error, "{}", ok.content);
    let granted = &nebo.policy(&ap).operations["ledger.bill.create"];
    assert!(granted.is_standing_grant());
    assert_eq!(granted.source.as_deref(), Some("general_manager"));
    let inside = nebo.policy(&ap).decide(
        "ledger.bill.create",
        Origin::Workflow,
        &OperationParams { amount_cents: Some(50_000), ..Default::default() },
        Some(&company),
        Some(&DayCounters::default()),
        true,
    );
    assert_eq!(inside.access, OperationAccess::Always, "{inside:?}");
    assert_eq!(inside.layer, PolicyLayer::StandingAuthority);
    let outside = nebo.policy(&ap).decide(
        "ledger.bill.create",
        Origin::Workflow,
        &OperationParams { amount_cents: Some(150_000), ..Default::default() },
        Some(&company),
        Some(&DayCounters::default()),
        true,
    );
    assert_eq!(outside.access, OperationAccess::Approval, "{outside:?}");
    assert_eq!(outside.reason, "amount 150000 exceeds the grant's 100000 per operation");
    // And a grant past the company's per-operation figure is refused by permits.
    let beyond = nebo
        .tool(
            &gm_ctx,
            "authority",
            json!({ "action": "grant", "agent": "Accounts Payable Clerk", "operation": "ledger.expense.record", "bounds": { "max_amount_cents": 300000 } }),
        )
        .await;
    assert!(beyond.is_error && beyond.content.contains("exceeds the company's 250000"), "{}", beyond.content);
    nebo.clear_layers().await;
}

/// An employee package declaring a ceiling is installed; the declared
/// operation lands on the seat's policy as an unlocked Approval sourced
/// `seat`, shows on the Approvals view, decides Approval in an unattended
/// run, and the employee-wide default set to Always loosens the seat's other
/// operations but never the ceiling. The owner authoring a ceiling on the
/// settings page lands through the same routine.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seats_published_ceiling_lands_and_gates() {
    let nebo = session().await;
    let seat = nebo
        .hire(
            "AR Specialist",
            json!({ "requires": { "interfaces": ["ledger"] }, "ceiling": { "ledger.invoice.void": "approval" }, "workflows": {} }),
        )
        .await;

    let policy = nebo.stored_policy(&seat).expect("install seeds the policy");
    let rule = policy.operations.get("ledger.invoice.void").expect("the ceiling landed");
    assert_eq!(rule.access, OperationAccess::Approval);
    assert_eq!(rule.source.as_deref(), Some("seat"));
    assert!(!rule.locked, "a ceiling is the seat's declaration; the owner may still grant it");
    assert!(!rule.may_grant(), "a declaration restricts, it never grants");
    let row = operation_row(&nebo, &seat, "ledger.invoice.void").await.expect("on the Approvals view");
    assert_eq!(row["override"], "approval");
    assert_eq!(row["effective"], "approval");
    let d = decide(&nebo, &seat, "ledger.invoice.void", Origin::Workflow);
    assert_eq!(d.access, OperationAccess::Approval, "{d:?}");
    assert!(d.reason.contains("declared by this employee's package"), "{d:?}");

    // The employee-wide default goes to Always: the other operations loosen,
    // the ceiling does not.
    let mut loosened = policy.clone();
    loosened.default = OperationAccess::Always;
    nebo.put_ok(&format!("/entity-config/agent/{seat}"), &json!({ "operationPolicy": loosened.to_json() })).await;
    assert_eq!(nebo.policy(&seat).default, OperationAccess::Always);
    let other = operation_row(&nebo, &seat, "ledger.invoice.update").await.expect("an ordinary gated operation");
    assert_eq!(other["effective"], "always", "the default is in force: {other}");
    let ceiling = operation_row(&nebo, &seat, "ledger.invoice.void").await.unwrap();
    assert_eq!(ceiling["effective"], "approval", "the default never loosens the ceiling: {ceiling}");
    let d = decide(&nebo, &seat, "ledger.invoice.void", Origin::Workflow);
    assert_eq!(d.access, OperationAccess::Approval, "{d:?}");
    let d = decide(&nebo, &seat, "ledger.invoice.update", Origin::Workflow);
    assert_eq!(d.access, OperationAccess::Always, "{d:?}");

    // The owner authors a second ceiling entry on the settings page: same routine.
    nebo.put_ok(&format!("/agents/{seat}"), &json!({ "ceiling": { "ledger.invoice.void": "approval", "ledger.refund.create": "approval" } })).await;
    let authored = nebo.policy(&seat);
    assert_eq!(authored.operations["ledger.refund.create"].source.as_deref(), Some("seat"));
    assert_eq!(authored.operations["ledger.refund.create"].access, OperationAccess::Approval);
    assert_eq!(authored.default, OperationAccess::Always, "the owner's default survives the re-apply");
    assert_eq!(decide(&nebo, &seat, "ledger.refund.create", Origin::Workflow).access, OperationAccess::Approval);
    let detail = nebo.get_ok(&format!("/agents/{seat}")).await;
    assert_eq!(detail["ceiling"]["ledger.refund.create"], "approval", "{detail}");
}

/// A package built around a capability the catalogue never heard of: its
/// declared operation shows on the Approvals view and decides Approval in an
/// unattended run, even with the employee-wide default at Always — the owner
/// rules on it one operation at a time. A seat with no policy at all calling
/// a critical operation from a workflow, or a schedule, is Approval — never
/// "installation is the grant" — while a non-critical one from a trusted
/// origin keeps that grant and an untrusted origin falls to the safe default.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_declared_operation_fails_closed_and_a_critical_one_asks() {
    let nebo = session().await;
    let seat = nebo
        .hire("Widget Wrangler", json!({ "ceiling": { "widgets.thing.frob": "approval" }, "workflows": {} }))
        .await;
    assert!(!tools::interface_catalog::is_gated("widgets.thing.frob"), "the catalogue never heard of it");
    let row = operation_row(&nebo, &seat, "widgets.thing.frob").await.expect("declared, so on the view");
    assert_eq!(row["effective"], "approval");
    assert_eq!(decide(&nebo, &seat, "widgets.thing.frob", Origin::Workflow).access, OperationAccess::Approval);

    let mut loosened = nebo.policy(&seat);
    loosened.default = OperationAccess::Always;
    nebo.put_ok(&format!("/entity-config/agent/{seat}"), &json!({ "operationPolicy": loosened.to_json() })).await;
    let d = decide(&nebo, &seat, "widgets.thing.frob", Origin::Workflow);
    assert_eq!(d.access, OperationAccess::Approval, "an unknown operation fails closed: {d:?}");
    assert_eq!(operation_row(&nebo, &seat, "widgets.thing.frob").await.unwrap()["effective"], "approval");
    // The owner's own row for it is what loosens it — one operation at a time.
    let mut owner = nebo.policy(&seat);
    owner.apply_edit("widgets.thing.frob", tools::policy::OperationRule::access(OperationAccess::Always)).unwrap();
    nebo.put_ok(&format!("/entity-config/agent/{seat}"), &json!({ "operationPolicy": owner.to_json() })).await;
    assert_eq!(decide(&nebo, &seat, "widgets.thing.frob", Origin::Workflow).access, OperationAccess::Always);

    // No policy at all: the shape the runner sees for a seat nobody configured.
    let bare = nebo.hire("Bare Seat", json!({ "workflows": {} })).await;
    assert!(nebo.stored_policy(&bare).is_none(), "nothing seeded a policy for a seat with no declaration");
    let gate = |op: &str, origin: Origin| {
        OperationPolicy::decide_optional(None, op, origin, &OperationParams::default(), None, None, true)
    };
    assert!(tools::interface_catalog::is_critical("ledger.billpayment.create"));
    let from_workflow = gate("ledger.billpayment.create", Origin::Workflow).expect("a critical operation is decided");
    assert_eq!(from_workflow.access, OperationAccess::Approval, "{from_workflow:?}");
    let from_schedule = gate("ledger.billpayment.create", Origin::System).expect("from a schedule too");
    assert_eq!(from_schedule.access, OperationAccess::Approval, "{from_schedule:?}");
    assert!(gate("ledger.invoice.update", Origin::Workflow).is_none(), "a non-critical operation keeps installation as the grant");
    assert_eq!(gate("ledger.invoice.update", Origin::Comm).map(|d| d.access), Some(OperationAccess::Approval), "an untrusted origin falls to the safe default");
}

/// The pack tool declares which operation a call performs, so the gate never
/// matches on a tool's name; writing the company layer is that operation.
/// For an ungranted seat it is Approval from any origin — critical, so with
/// no policy at all it still asks. With the owner's explicit grant it is
/// Always from a workflow and a schedule, and floored to Approval over an
/// untrusted origin. Under a company law it is Blocked regardless.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn writing_the_company_layer_is_granted_not_channelled() {
    let nebo = session().await;
    nebo.clear_layers().await;
    let seat = nebo.hire("Layer Author", json!({ "workflows": {} })).await;
    const WRITE: &str = "layers.company.write";
    assert!(tools::interface_catalog::is_critical(WRITE));

    // The tool says what the call is; the registry the runner asks relays it.
    let create = json!({ "action": "create", "layer": "company", "slug": "acme", "body": "How this company runs." });
    assert_eq!(nebo.state.tools.operation_performed("pack", &create).await.as_deref(), Some(WRITE));
    let src = tempfile::tempdir().unwrap();
    write_tree(&src.path().join("acme"), &company_pack(&[]));
    let add = json!({ "action": "add", "path": src.path().join("acme").to_string_lossy() });
    assert_eq!(nebo.state.tools.operation_performed("pack", &add).await.as_deref(), Some(WRITE));
    assert_eq!(nebo.state.tools.operation_performed("pack", &json!({ "action": "list" })).await, None);
    assert_eq!(
        nebo.state.tools.operation_performed("pack", &json!({ "action": "create", "layer": "industry", "slug": "roofing", "body": "x" })).await.as_deref(),
        Some("layers.industry.write")
    );

    // Ungranted: Approval from every origin, with or without a policy.
    assert!(nebo.stored_policy(&seat).is_none());
    for origin in [Origin::User, Origin::Workflow, Origin::System, Origin::Comm, Origin::Mcp] {
        let d = OperationPolicy::decide_optional(None, WRITE, origin, &OperationParams::default(), None, None, true)
            .unwrap_or_else(|| panic!("{origin:?}: a critical operation is decided even with no policy"));
        assert_eq!(d.access, OperationAccess::Approval, "{origin:?}: {d:?}");
    }
    let row = operation_row(&nebo, &seat, WRITE).await.expect("the runtime's own capability is on every seat's view");
    assert_eq!(row["effective"], "approval");
    assert_eq!(row["capability"], "layers");

    // The owner's explicit grant, on the employee's Approvals.
    let mut granted = OperationPolicy::default();
    granted.apply_edit(WRITE, tools::policy::OperationRule::access(OperationAccess::Always)).unwrap();
    nebo.put_ok(&format!("/entity-config/agent/{seat}"), &json!({ "operationPolicy": granted.to_json() })).await;
    assert_eq!(decide(&nebo, &seat, WRITE, Origin::Workflow).access, OperationAccess::Always);
    assert_eq!(decide(&nebo, &seat, WRITE, Origin::System).access, OperationAccess::Always);
    let floored = decide(&nebo, &seat, WRITE, Origin::Comm);
    assert_eq!((floored.access, floored.layer), (OperationAccess::Approval, PolicyLayer::OriginFloor), "{floored:?}");
    assert_eq!(decide(&nebo, &seat, WRITE, Origin::Mcp).access, OperationAccess::Approval);
    assert_eq!(operation_row(&nebo, &seat, WRITE).await.unwrap()["effective"], "always");

    // A company law on the write: Blocked regardless, and the grant is gone under it.
    let with_law = company_pack(&[("laws/layer.md", "---\nlaw: The company layer\nceiling: [\"layers.company.write\"]\n---\n\nNo seat rewrites the company layer unattended.\n")]);
    nebo.park_pack("acme", &with_law).await;
    assert_eq!(nebo.apply_layers().await["applied"], json!(["acme"]));
    for origin in [Origin::User, Origin::Workflow, Origin::System, Origin::Comm] {
        let d = decide(&nebo, &seat, WRITE, origin);
        assert_eq!((d.access, d.layer), (OperationAccess::Blocked, PolicyLayer::Law), "{origin:?}: {d:?}");
        assert_eq!(d.reason, "law:The company layer");
    }
    let row = operation_row(&nebo, &seat, WRITE).await.unwrap();
    assert_eq!((row["effective"].as_str(), row["locked"].as_bool()), (Some("blocked"), Some(true)), "{row}");
    // And the owner's grant cannot come back over the law.
    let mut regrant = nebo.policy(&seat);
    assert!(matches!(
        regrant.apply_edit(WRITE, tools::policy::OperationRule::access(OperationAccess::Always)),
        Err(tools::policy::PolicyError::Locked(_))
    ));
    nebo.clear_layers().await;
}

/// The catalogue shipped in the binary is the gate: three operations that
/// were gated on paper only decide as gated for a seat that binds their
/// capabilities and show on its Approvals view; an operation absent from the
/// catalogue with no declaration is not gated at all; an ungated read is not
/// listed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_catalogue_is_the_gate() {
    let nebo = session().await;
    let seat = nebo
        .hire("Coordinator", json!({ "requires": { "interfaces": ["ledger", "crm", "calendar"] }, "workflows": {} }))
        .await;
    // Formerly paper-only: none of these was in the compiled list of 46.
    let paper_only = ["ledger.invoice.void", "crm.record.merge", "calendar.event.cancel"];
    let gated = tools::interface_catalog::gated_operations();
    assert!(gated.len() > 46, "the catalogue, not the compiled list: {} gated", gated.len());
    for op in paper_only {
        assert!(tools::interface_catalog::is_gated(op), "{op} is gated by the catalogue");
        let row = operation_row(&nebo, &seat, op).await.unwrap_or_else(|| panic!("{op} is on the Approvals view"));
        assert_eq!(row["effective"], "approval", "{row}");
        let d = decide(&nebo, &seat, op, Origin::Workflow);
        assert_eq!(d.access, OperationAccess::Approval, "{op}: {d:?}");
        assert_ne!(d.reason, "not gated");
    }
    // Absent from the catalogue, declared by nobody: not gated.
    assert!(!tools::interface_catalog::is_gated("widgets.thing.frob"));
    assert!(operation_row(&nebo, &seat, "widgets.thing.frob").await.is_none());
    let d = decide(&nebo, &seat, "widgets.thing.frob", Origin::Comm);
    assert_eq!((d.access, d.reason.as_str()), (OperationAccess::Always, "not gated"), "{d:?}");
    // An ungated read is not gated and not listed.
    assert!(!tools::interface_catalog::is_gated("ledger.balance.get"));
    assert!(operation_row(&nebo, &seat, "ledger.balance.get").await.is_none());
    assert_eq!(decide(&nebo, &seat, "ledger.balance.get", Origin::Workflow).access, OperationAccess::Always);
    // A capability the seat does not bind is not on its view either.
    assert!(operation_row(&nebo, &seat, "mail.message.send").await.is_none());
}
