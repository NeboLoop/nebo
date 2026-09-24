//! The connections: a hand-off between seats through the real bus, and a
//! plugin call shaped by its binding.

use super::*;
use tools::Origin;

/// The event runs the dispatcher started for a seat, oldest first.
fn event_runs(nebo: &Nebo, agent_id: &str) -> Vec<db::models::WorkflowRun> {
    nebo.store()
        .list_workflow_runs(&types::keyparser::agent_workflow_id(agent_id), 50, 0)
        .unwrap()
        .into_iter()
        .filter(|r| r.trigger_type == "event")
        .collect()
}

/// The one activity a triggered binding needs.
fn work() -> Value {
    json!([{ "id": "work", "type": "custom", "intent": "do the work" }])
}

/// One seat emits the address its package writes; a second seat, hired with
/// a subscription to that exact string, starts: the run the real dispatcher
/// opened carries the address untouched, the producer stamped, and the
/// consumer's own announcement addressed once. A bare company event matches
/// a bare subscription and names who spoke. A seat's slug never appears
/// twice in a source.
///
/// The emit is the real `emit` tool in the server's registry, called with a
/// run's context of the producing seat, the way the runner calls it; the
/// runs that start fail for want of a model, but the rows they open are the
/// dispatcher's decision and stand.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hand_off_connects() {
    let nebo = session().await;
    const ANNOUNCEMENT: &str = "operations.inventory-manager.reorder-needed";
    const PROCUREMENT_EMIT: &str = "operations.procurement-coordinator.po-issued";
    let inventory = nebo
        .hire(
            "Inventory Manager",
            json!({ "workflows": { "reorder-check": {
                "trigger": { "type": "schedule", "cron": "0 8 * * 1" },
                "emit": ANNOUNCEMENT,
                "activities": work()
            } } }),
        )
        .await;
    let procurement = nebo
        .hire(
            "Procurement Coordinator",
            json!({ "workflows": { "requisition-intake": {
                "trigger": { "type": "event", "sources": [ANNOUNCEMENT] },
                "emit": PROCUREMENT_EMIT,
                "activities": work()
            } } }),
        )
        .await;
    let gm = nebo
        .hire(
            "General Manager",
            json!({ "workflows": { "close-the-loop": {
                "trigger": { "type": "event", "sources": ["assignment.done"] },
                "activities": work()
            } } }),
        )
        .await;
    for id in [&inventory, &procurement, &gm] {
        nebo.activate(id).await;
    }
    assert!(event_runs(&nebo, &procurement).is_empty());

    // The producer announces, from its own run, the name its package declares.
    let ctx = tools::ToolContext::new(Origin::Workflow).with_session(format!("agent:{inventory}:workflow:run-1"), "s1");
    let emitted = nebo.tool(&ctx, "emit", json!({ "source": ANNOUNCEMENT, "payload": { "poId": "PO-1" } })).await;
    assert!(!emitted.is_error, "{}", emitted.content);

    nebo.wait_until(15, "the waiting seat to start", || !event_runs(&nebo, &procurement).is_empty()).await;
    let runs = event_runs(&nebo, &procurement);
    assert_eq!(runs.len(), 1, "exactly one waiting seat runs once: {runs:?}");
    let run = &runs[0];
    assert_eq!(run.trigger_detail.as_deref(), Some(&format!("requisition-intake:{ANNOUNCEMENT}")[..]));
    let inputs: Value = serde_json::from_str(run.inputs.as_deref().unwrap()).unwrap();
    assert_eq!(inputs["_event_source"], ANNOUNCEMENT, "the address travels as the package wrote it: {inputs}");
    assert_eq!(inputs["_event_payload"]["producer"], "inventory-manager", "the announcement says who spoke: {inputs}");
    assert_eq!(inputs["_event_payload"]["poId"], "PO-1");
    assert!(event_runs(&nebo, &inventory).is_empty(), "the producer does not hear itself");
    assert!(event_runs(&nebo, &gm).is_empty(), "a subscriber to another name hears nothing");

    // A bare company event matches a bare subscription.
    let bk = nebo.hire("Bookkeeper", json!({ "workflows": {} })).await;
    nebo.activate(&bk).await;
    let bk_ctx = tools::ToolContext::new(Origin::Workflow).with_session(format!("agent:{bk}:workflow:run-2"), "s2");
    let emitted = nebo.tool(&bk_ctx, "emit", json!({ "source": "assignment.done", "payload": { "assignment_id": "a1" } })).await;
    assert!(!emitted.is_error, "{}", emitted.content);
    nebo.wait_until(15, "the manager to hear the company event", || !event_runs(&nebo, &gm).is_empty()).await;
    let heard = &event_runs(&nebo, &gm)[0];
    let inputs: Value = serde_json::from_str(heard.inputs.as_deref().unwrap()).unwrap();
    assert_eq!(inputs["_event_source"], "assignment.done", "a company event stays bare: {inputs}");
    assert_eq!(inputs["_event_payload"]["producer"], "bookkeeper");
    assert_eq!(event_runs(&nebo, &procurement).len(), 1, "the procurement seat did not hear it");

    // No doubled slug, whatever the address already says.
    for (name, address, expect) in [
        ("Inventory Manager", ANNOUNCEMENT, ANNOUNCEMENT.to_string()),
        ("Inventory Manager", "inventory-manager.low-stock", "inventory-manager.low-stock".to_string()),
        ("Inventory Manager", "low-stock", "inventory-manager.low-stock".to_string()),
        ("Bookkeeper", "assignment.done", "assignment.done".to_string()),
    ] {
        let source = workflow::events::emit_source_for(name, address);
        assert_eq!(source, expect);
        assert_eq!(source.split('.').filter(|s| *s == db::agent_slug(name)).count(), if expect.starts_with("assignment") { 0 } else { 1 }, "{source}");
    }
}

/// A fake ledger plugin binds `ledger.invoice.send` to a template. Through
/// the plugin tool in the server's registry, the port call reaches the
/// binary as exactly the words the template shapes: `invoice send 1041
/// --send-to x@example.com` with `sendTo`, `invoice send 1041` without it,
/// and a call missing `invoiceId` is an error naming the field and the
/// operation, never an empty argument.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_template_binding_shapes_the_call() {
    // The fake plugin was installed before the server's first scan
    // (`prepare_all`) with the template binding.
    let nebo = session().await;
    assert!(nebo.state.plugin_store.is_ready(FAKE_LEDGER), "the plugin is installed and ready");
    let seat = nebo.hire("Billing Clerk", json!({ "requires": { "interfaces": ["ledger"] }, "workflows": {} })).await;
    // Sending an invoice forms a contract: it asks unless the owner allowed
    // it for this seat, which is what the shaping below runs under.
    nebo.put_ok(
        &format!("/entity-config/agent/{seat}"),
        &json!({ "operationPolicy": { "operations": { "ledger.invoice.send": "always" } } }),
    )
    .await;
    let ctx = Nebo::ctx(&seat, Origin::User);
    let argv = |content: &str| -> Vec<String> {
        content.lines().map(str::to_string).filter(|l| !l.is_empty()).collect()
    };

    let with = nebo
        .tool(&ctx, "plugin", json!({ "operation": "ledger.invoice.send", "input": { "invoiceId": "1041", "sendTo": "x@example.com" } }))
        .await;
    assert!(!with.is_error, "{}", with.content);
    assert!(with.content.contains("invoice\nsend\n1041\n--send-to\nx@example.com"), "exactly the shaped words: {}", with.content);
    assert!(!with.content.contains("--sendTo") && !with.content.contains("--invoiceId"), "a consumed field is not appended: {}", with.content);
    assert_eq!(argv(&with.content).iter().filter(|w| w.as_str() == "1041").count(), 1, "{}", with.content);

    let without = nebo
        .tool(&ctx, "plugin", json!({ "operation": "ledger.invoice.send", "input": { "invoiceId": "1041" } }))
        .await;
    assert!(!without.is_error, "{}", without.content);
    assert!(without.content.contains("invoice\nsend\n1041"), "{}", without.content);
    assert!(!without.content.contains("--send-to"), "an absent optional emits nothing: {}", without.content);

    let missing = nebo
        .tool(&ctx, "plugin", json!({ "operation": "ledger.invoice.send", "input": { "sendTo": "x@example.com" } }))
        .await;
    assert!(missing.is_error, "{}", missing.content);
    assert!(missing.content.contains("'invoiceId'") && missing.content.contains("ledger.invoice.send"), "{}", missing.content);
    assert!(!missing.content.contains("invoice\nsend"), "the binary never ran: {}", missing.content);

    // A plain binding is byte-for-byte as written and every field is a flag.
    let plain = nebo
        .tool(&ctx, "plugin", json!({ "operation": "ledger.invoice.list", "input": { "limit": 5 } }))
        .await;
    assert!(!plain.is_error, "{}", plain.content);
    assert!(plain.content.contains("invoice\nlist\n--limit\n5"), "{}", plain.content);
}

/// A chat parked on an install card resumes when the plugin lands by another
/// door. The card is the one the plugin tool's discover parks on, asked
/// through the real `ask_user` on the server's ask channels and announced the
/// way the chat pipeline announces it; the owner then pastes the card's code
/// into another chat, and `codes::handle_code` installs it from the hub
/// stand-in. The parked call reads "installed" — the answer the card's own
/// button gives — and the run holds no question any more. A card offering a
/// different plugin stays parked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_install_by_another_door_answers_the_install_card() {
    use tools::plugin_tool::{install_card_widget, INSTALL_CARD_INSTALLED};

    const CODE: &str = "PLUG-CARD-0001";
    const SLUG: &str = "card-office";
    let nebo = session().await;
    hub_offers_plugin(CODE, SLUG, "Card Office");
    // Paired with the stand-in hub for this scenario only.
    let profile = uuid::Uuid::new_v4().to_string();
    nebo.store()
        .create_auth_profile(&profile, "NeboAI", "neboai", "proof-token", None, None, 0, 1, Some("token"), None)
        .unwrap();

    // Two chats, each parked on a card: one for the plugin, one for another.
    let park = |session_key: &'static str, slug: &'static str| {
        let state = nebo.state.clone();
        async move {
            let run = state
                .run_registry
                .register(crate::run_registry::RegisterParams {
                    session_key: session_key.to_string(),
                    entity_id: "card-seat".to_string(),
                    entity_name: "Card Seat".to_string(),
                    origin: "user".to_string(),
                    channel: "web".to_string(),
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                    parent_run_id: None,
                })
                .await;
            let (stream_tx, mut stream_rx) = tokio::sync::mpsc::channel(8);
            let mut ctx = tools::ToolContext::new(Origin::User).with_session(session_key.to_string(), "s1");
            ctx.stream_tx = Some(stream_tx);
            ctx.ask_channels = Some(state.ask_channels.clone());
            let asking = tokio::spawn(async move {
                ctx.ask_user("Install it on the card.", install_card_widget("PLUG-ANY", slug, slug, "")).await
            });
            let event = stream_rx.recv().await.expect("the card was asked");
            crate::chat_dispatch::announce_ask(&state.hub, &state.run_registry, session_key, &event).await;
            (run, asking)
        }
    };
    let (_run, card) = park("agent:card-seat:main", SLUG).await;
    let (_other_run, other_card) = park("agent:card-seat:other", "some-other-plugin").await;
    assert!(nebo.state.run_registry.pending_ask_for_session("agent:card-seat:main").await.is_some());

    // The owner pastes the code into another chat.
    crate::codes::handle_code(&nebo.state, crate::codes::CodeType::Plugin, CODE, "agent:card-seat:elsewhere").await;
    assert!(nebo.state.plugin_store.resolve(SLUG, "*").is_some(), "the code installed the plugin");

    let answer = tokio::time::timeout(Duration::from_secs(10), card)
        .await
        .expect("the parked call resumed")
        .unwrap();
    assert_eq!(answer.as_deref(), Some(INSTALL_CARD_INSTALLED));
    assert!(nebo.state.run_registry.pending_ask_for_session("agent:card-seat:main").await.is_none(), "the run holds no question");

    assert!(!other_card.is_finished(), "a card for another plugin stays parked");
    assert!(nebo.state.run_registry.pending_ask_for_session("agent:card-seat:other").await.is_some());

    // Leave the shared server as it was found.
    other_card.abort();
    let _ = nebo.state.plugin_store.remove(SLUG);
    let _ = nebo.store().delete_installed_plugin(SLUG);
    nebo.store().delete_auth_profile(&profile).unwrap();
}
