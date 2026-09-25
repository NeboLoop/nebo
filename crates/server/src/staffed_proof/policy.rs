//! Operations through the one permission check, on the real server: a law
//! denies, a reserved operation is the owner's, a seat's ceiling lands and
//! asks, an unknown declared operation fails closed, the company layer's
//! write is granted not channelled, and the catalogue decides what an
//! untrusted run may spend unasked.

use super::layers::company_pack;
use super::*;
use tools::Origin;
use types::permissions::{AskCase, Decision, Effect, RuleSource, Why};

/// The decision in the Approvals view's words.
fn access(d: &Decision) -> &'static str {
    match d {
        Decision::Allow { .. } => "always",
        Decision::Ask { .. } => "approval",
        Decision::Deny { .. } => "blocked",
    }
}

/// The Approvals view's row for one operation.
async fn operation_row(nebo: &Nebo, agent_id: &str, op: &str) -> Option<Value> {
    let ops = nebo.get_ok(&format!("/agents/{agent_id}/operations")).await;
    ops["operations"].as_array().unwrap().iter().find(|o| o["operation"] == op).cloned()
}

/// The owner answered this exact call: the context a resumed ask runs in.
fn answered(ctx: tools::ToolContext) -> tools::ToolContext {
    tools::ToolContext { answered_ask: Some("ask-owner".into()), ..ctx }
}

/// With the company pack applied, a seat's call on a law-denied operation is
/// refused from every origin, and the Approvals view says so; the owner's
/// answer to a grant on an owner-reserved operation is still refused —
/// "reserved to the owner" — and nothing is written. A grant on the law
/// itself is refused as locked. A grant inside the company's bounds on an
/// ordinary operation lands as standing authority and admits a call inside
/// its bound while asking for one outside it.
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

    // The law denies, from every origin, and the view agrees.
    let law = nebo.operation_rules(&ap)["ledger.billpayment.create"].clone();
    assert_eq!((law.effect, law.locked), (Effect::Deny, true));
    assert_eq!(law.source, RuleSource::Law { pack: "Payments".into() });
    for origin in [Origin::User, Origin::Workflow, Origin::System, Origin::Comm] {
        let d = nebo.decide(&ap, "ledger.billpayment.create", origin, None);
        assert!(matches!(&d, Decision::Deny { why: Why::Rule { rule_id }, .. } if *rule_id == law.id), "{origin:?}: {d:?}");
    }
    let row = operation_row(&nebo, &ap, "ledger.billpayment.create").await.expect("on the view");
    assert_eq!(row["effective"], "blocked");
    assert_eq!(row["locked"], true);

    // Only the owner widens: granting authority is an operation that asks,
    // so the call parks for the owner and writes nothing; and even with the
    // owner's answer a reserved operation stays the owner's.
    let gm_ctx = Nebo::ctx(&gm, Origin::User);
    let grant = json!({ "action": "grant", "agent": "Accounts Payable Clerk", "operation": "ledger.transfer.create", "bounds": { "max_amount_cents": 1000, "per_day_cents": 5000 } });
    let unasked = nebo.tool(&gm_ctx, "authority", grant.clone()).await;
    assert!(unasked.parked_ask.is_some(), "granting authority ran unasked: {}", unasked.content);
    let reserved = nebo.tool(&answered(gm_ctx.clone()), "authority", grant).await;
    assert!(reserved.is_error, "{}", reserved.content);
    assert!(reserved.content.contains("ledger.transfer.create is reserved to the owner"), "{}", reserved.content);
    assert!(reserved.content.contains("Only the owner can change the constitution"), "{}", reserved.content);
    let on_reserved = nebo.operation_rules(&ap)["ledger.transfer.create"].clone();
    assert_eq!((on_reserved.effect, on_reserved.locked), (Effect::Ask, true), "the reserved law is the owner's hand, unchanged");
    // Reserved is the owner's hand, not a block: a call on it asks.
    let d = nebo.decide(&ap, "ledger.transfer.create", Origin::Workflow, None);
    assert_eq!(access(&d), "approval", "{d:?}");

    // A grant on the law: refused, the rule is locked.
    let on_law = nebo
        .tool(
            &answered(gm_ctx.clone()),
            "authority",
            json!({ "action": "grant", "agent": "Accounts Payable Clerk", "operation": "ledger.billpayment.create", "bounds": { "max_amount_cents": 1000 } }),
        )
        .await;
    assert!(on_law.is_error, "{}", on_law.content);
    assert!(on_law.content.contains("fixed by a law"), "{}", on_law.content);
    assert_eq!(nebo.operation_rules(&ap)["ledger.billpayment.create"].effect, Effect::Deny, "the law stands");

    // A grant inside the company's bounds on an ordinary operation lands.
    let ok = nebo
        .tool(
            &answered(gm_ctx.clone()),
            "authority",
            json!({ "action": "grant", "agent": "Accounts Payable Clerk", "operation": "ledger.bill.create", "bounds": { "max_amount_cents": 100000, "per_day_cents": 400000, "per_day_count": 5 } }),
        )
        .await;
    assert!(!ok.is_error, "{}", ok.content);
    let granted = nebo.operation_rules(&ap)["ledger.bill.create"].clone();
    assert_eq!(granted.effect, Effect::Allow);
    assert_eq!(granted.money.as_ref().and_then(|m| m.per_action_cents), Some(100_000));
    assert!(matches!(granted.source, RuleSource::AllowAlways { .. }), "{:?}", granted.source);
    let inside = nebo.decide(&ap, "ledger.bill.create", Origin::Workflow, Some(50_000));
    assert!(matches!(&inside, Decision::Allow { why: Why::Rule { rule_id } } if *rule_id == granted.id), "{inside:?}");
    let outside = nebo.decide(&ap, "ledger.bill.create", Origin::Workflow, Some(150_000));
    assert!(
        matches!(&outside, Decision::Ask { case: AskCase::Money { cents: 150_000, limit_cents: Some(100_000) } }),
        "{outside:?}"
    );
    // And a grant past the company's per-operation figure is refused.
    let beyond = nebo
        .tool(
            &answered(gm_ctx),
            "authority",
            json!({ "action": "grant", "agent": "Accounts Payable Clerk", "operation": "ledger.expense.record", "bounds": { "max_amount_cents": 300000 } }),
        )
        .await;
    assert!(beyond.is_error && beyond.content.contains("exceeds the company's 250000"), "{}", beyond.content);
    nebo.clear_layers().await;
}

/// An employee package declaring a ceiling is installed; the declared
/// operation lands on the seat as an ask rule written by the package, shows
/// on the Approvals view, and asks in an unattended run. The owner authoring
/// a ceiling on the settings page lands through the same routine.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seats_published_ceiling_lands_and_gates() {
    let nebo = session().await;
    let seat = nebo
        .hire(
            "AR Specialist",
            json!({ "requires": { "interfaces": ["ledger"] }, "ceiling": { "ledger.invoice.void": "approval" }, "workflows": {} }),
        )
        .await;

    let rule = nebo.operation_rules(&seat).get("ledger.invoice.void").cloned().expect("the ceiling landed");
    assert_eq!(rule.effect, Effect::Ask);
    assert!(matches!(rule.source, RuleSource::Package { .. }), "{:?}", rule.source);
    assert!(!rule.locked, "a ceiling is the seat's declaration; the owner may still rule on it");
    let row = operation_row(&nebo, &seat, "ledger.invoice.void").await.expect("on the Approvals view");
    assert_eq!(row["override"], "approval");
    assert_eq!(row["effective"], "approval");
    let d = nebo.decide(&seat, "ledger.invoice.void", Origin::Workflow, None);
    assert!(matches!(&d, Decision::Ask { case: AskCase::AskRule { rule_id } } if *rule_id == rule.id), "{d:?}");
    // The seat's other gated operations run inside its job; the ceiling asks.
    assert_eq!(access(&nebo.decide(&seat, "ledger.invoice.update", Origin::Workflow, None)), "always");
    assert_eq!(operation_row(&nebo, &seat, "ledger.invoice.update").await.unwrap()["effective"], "always");

    // The owner authors a second ceiling entry on the settings page: same routine.
    nebo.put_ok(&format!("/agents/{seat}"), &json!({ "ceiling": { "ledger.invoice.void": "approval", "ledger.refund.create": "approval" } })).await;
    let authored = nebo.operation_rules(&seat);
    assert!(matches!(authored["ledger.refund.create"].source, RuleSource::Package { .. }));
    assert_eq!(authored["ledger.refund.create"].effect, Effect::Ask);
    assert_eq!(access(&nebo.decide(&seat, "ledger.refund.create", Origin::Workflow, None)), "approval");
    let detail = nebo.get_ok(&format!("/agents/{seat}")).await;
    assert_eq!(detail["ceiling"]["ledger.refund.create"], "approval", "{detail}");
}

/// A package built around a capability the catalogue never heard of: its
/// declared operation shows on the Approvals view and asks in an unattended
/// run — the owner rules on it one operation at a time. A seat nobody
/// configured calling a critical operation from a workflow, or a schedule,
/// asks — never "installation is the grant" — and so does any operation of
/// an interface it was never hired for (outside its job). A seat hired for
/// that interface runs a non-critical gated one from a trusted origin and
/// asks from an untrusted one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_declared_operation_fails_closed_and_a_critical_one_asks() {
    let nebo = session().await;
    let seat = nebo
        .hire("Widget Wrangler", json!({ "ceiling": { "widgets.thing.frob": "approval" }, "workflows": {} }))
        .await;
    assert!(!tools::interface_catalog::is_gated("widgets.thing.frob"), "the catalogue never heard of it");
    let row = operation_row(&nebo, &seat, "widgets.thing.frob").await.expect("declared, so on the view");
    assert_eq!(row["effective"], "approval");
    assert_eq!(access(&nebo.decide(&seat, "widgets.thing.frob", Origin::Workflow, None)), "approval");

    // The owner's own row for it is what loosens it — one operation at a time.
    nebo.put_ok(
        &format!("/entity-config/agent/{seat}"),
        &json!({ "operationPolicy": { "operations": { "widgets.thing.frob": "always" } } }),
    )
    .await;
    assert_eq!(access(&nebo.decide(&seat, "widgets.thing.frob", Origin::Workflow, None)), "always");

    // A seat nobody configured.
    let bare = nebo.hire("Bare Seat", json!({ "workflows": {} })).await;
    assert!(nebo.operation_rules(&bare).is_empty(), "nothing wrote a rule for a seat with no declaration");
    assert!(tools::interface_catalog::is_critical("ledger.billpayment.create"));
    for origin in [Origin::Workflow, Origin::System] {
        let d = nebo.decide(&bare, "ledger.billpayment.create", origin, None);
        assert_eq!(access(&d), "approval", "{origin:?}: a critical operation asks: {d:?}");
    }
    let d = nebo.decide(&bare, "ledger.invoice.update", Origin::Workflow, None);
    assert!(
        matches!(&d, Decision::Ask { case: AskCase::OutsideJob { capability } } if capability == "ledger"),
        "no hire bound the ledger: {d:?}"
    );
    // The owner binding the interface on the seat's page is the consent to it.
    nebo.put_ok(&format!("/agents/{bare}"), &json!({ "interfaces": ["ledger"] })).await;
    assert_eq!(access(&nebo.decide(&bare, "ledger.invoice.update", Origin::Workflow, None)), "always", "bound by the owner");
    let books = nebo.hire("Books Seat", json!({ "requires": { "interfaces": ["ledger"] }, "workflows": {} })).await;
    assert_eq!(access(&nebo.decide(&books, "ledger.invoice.update", Origin::Workflow, None)), "always", "inside the job");
    let d = nebo.decide(&books, "ledger.invoice.update", Origin::Comm, None);
    assert!(matches!(d, Decision::Ask { case: AskCase::UntrustedInput { .. } }), "an untrusted origin asks: {d:?}");
}

/// The pack tool declares which operation a call performs, so the check
/// never matches on a tool's name; writing the company layer is that
/// operation. For an ungranted seat it asks from any origin — critical. With
/// the owner's explicit allow it runs from a workflow and a schedule, and
/// asks over an untrusted origin. Under a company law it is denied
/// regardless, and the owner's allow cannot come back over the law.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn writing_the_company_layer_is_granted_not_channelled() {
    let nebo = session().await;
    nebo.clear_layers().await;
    let seat = nebo.hire("Layer Author", json!({ "workflows": {} })).await;
    const WRITE: &str = "layers.company.write";
    assert!(tools::interface_catalog::is_critical(WRITE));

    // The tool says what the call is; the registry relays it.
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

    // Ungranted: asks from every origin.
    assert!(nebo.operation_rules(&seat).is_empty());
    for origin in [Origin::User, Origin::Workflow, Origin::System, Origin::Comm, Origin::Mcp] {
        let d = nebo.decide(&seat, WRITE, origin, None);
        assert_eq!(access(&d), "approval", "{origin:?}: {d:?}");
    }
    let row = operation_row(&nebo, &seat, WRITE).await.expect("the runtime's own capability is on every seat's view");
    assert_eq!(row["effective"], "approval");
    assert_eq!(row["capability"], "layers");

    // The owner's explicit allow, on the employee's Approvals.
    nebo.put_ok(&format!("/entity-config/agent/{seat}"), &json!({ "operationPolicy": { "operations": { WRITE: "always" } } })).await;
    assert_eq!(access(&nebo.decide(&seat, WRITE, Origin::Workflow, None)), "always");
    assert_eq!(access(&nebo.decide(&seat, WRITE, Origin::System, None)), "always");
    let floored = nebo.decide(&seat, WRITE, Origin::Comm, None);
    assert!(matches!(floored, Decision::Ask { case: AskCase::UntrustedInput { .. } }), "{floored:?}");
    assert_eq!(access(&nebo.decide(&seat, WRITE, Origin::Mcp, None)), "approval");
    assert_eq!(operation_row(&nebo, &seat, WRITE).await.unwrap()["effective"], "always");

    // A company law on the write: denied regardless, and the allow is gone under it.
    let with_law = company_pack(&[("laws/layer.md", "---\nlaw: The company layer\nceiling: [\"layers.company.write\"]\n---\n\nNo seat rewrites the company layer unattended.\n")]);
    nebo.park_pack("acme", &with_law).await;
    assert_eq!(nebo.apply_layers().await["applied"], json!(["acme"]));
    for origin in [Origin::User, Origin::Workflow, Origin::System, Origin::Comm] {
        let d = nebo.decide(&seat, WRITE, origin, None);
        assert_eq!(access(&d), "blocked", "{origin:?}: {d:?}");
    }
    let law = nebo.operation_rules(&seat)[WRITE].clone();
    assert_eq!((law.effect, law.locked), (Effect::Deny, true));
    assert_eq!(law.source, RuleSource::Law { pack: "The company layer".into() });
    let row = operation_row(&nebo, &seat, WRITE).await.unwrap();
    assert_eq!((row["effective"].as_str(), row["locked"].as_bool()), (Some("blocked"), Some(true)), "{row}");
    // And the owner's allow cannot come back over the law.
    let (status, out) = nebo
        .put(&format!("/entity-config/agent/{seat}"), &json!({ "operationPolicy": { "operations": { WRITE: "always" } } }))
        .await;
    assert_ne!(status, 200, "an allow over a law was accepted: {out}");
    assert_eq!(nebo.operation_rules(&seat)[WRITE].effect, Effect::Deny);
    nebo.clear_layers().await;
}

/// The catalogue shipped in the binary names the gated operations: three
/// that were gated on paper only are listed on the Approvals view of a seat
/// that binds their capabilities, run inside its job for the owner's own
/// runs, and ask when someone else's words are in the run. An operation
/// absent from the catalogue with no declaration is not listed and runs; an
/// ungated read is not listed.
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
        assert_eq!(row["effective"], "always", "{row}");
        assert_eq!(access(&nebo.decide(&seat, op, Origin::Workflow, None)), "always", "{op}");
        let d = nebo.decide(&seat, op, Origin::Comm, None);
        assert!(matches!(d, Decision::Ask { case: AskCase::UntrustedInput { .. } }), "{op}: {d:?}");
    }
    // Absent from the catalogue, declared by nobody: not listed, and runs.
    assert!(!tools::interface_catalog::is_gated("widgets.thing.frob"));
    assert!(operation_row(&nebo, &seat, "widgets.thing.frob").await.is_none());
    assert_eq!(access(&nebo.decide(&seat, "widgets.thing.frob", Origin::Comm, None)), "always");
    // An ungated read is not gated and not listed.
    assert!(!tools::interface_catalog::is_gated("ledger.balance.get"));
    assert!(operation_row(&nebo, &seat, "ledger.balance.get").await.is_none());
    assert_eq!(access(&nebo.decide(&seat, "ledger.balance.get", Origin::Workflow, None)), "always");
    // A capability the seat does not bind is not on its view either.
    assert!(operation_row(&nebo, &seat, "mail.message.send").await.is_none());
}
