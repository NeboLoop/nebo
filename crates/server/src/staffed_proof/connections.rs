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
    let emitted = nebo.tool(&ctx, "emit_event", json!({ "source": ANNOUNCEMENT, "payload": { "poId": "PO-1" } })).await;
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
    let emitted = nebo.tool(&bk_ctx, "emit_event", json!({ "source": "assignment.done", "payload": { "assignment_id": "a1" } })).await;
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
/// its operation tool in the server's registry, the call reaches the
/// binary as exactly the words the template shapes: `invoice send 1041
/// --send-to x@example.com` with `sendTo`, `invoice send 1041` without it,
/// and a call missing `invoiceId` is refused naming the field and the
/// tool, never run with an empty argument.
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
    // The customer it sends to is someone the seat already works with: a
    // first message to a new person would ask (case 2), which is not what
    // this proof is about.
    nebo.state.store.add_employee_counterparty(&seat, "x@example.com", "sent").unwrap();
    let ctx = Nebo::ctx(&seat, Origin::User);
    let argv = |content: &str| -> Vec<String> {
        content.lines().map(str::to_string).filter(|l| !l.is_empty()).collect()
    };

    let with = nebo
        .tool(&ctx, "ledger_invoice_send", json!({ "invoiceId": "1041", "sendTo": "x@example.com" }))
        .await;
    assert!(!with.is_error, "{}", with.content);
    assert!(with.content.contains("invoice\nsend\n1041\n--send-to\nx@example.com"), "exactly the shaped words: {}", with.content);
    assert!(!with.content.contains("--sendTo") && !with.content.contains("--invoiceId"), "a consumed field is not appended: {}", with.content);
    assert_eq!(argv(&with.content).iter().filter(|w| w.as_str() == "1041").count(), 1, "{}", with.content);

    let without = nebo
        .tool(&ctx, "ledger_invoice_send", json!({ "invoiceId": "1041" }))
        .await;
    assert!(!without.is_error, "{}", without.content);
    assert!(without.content.contains("invoice\nsend\n1041"), "{}", without.content);
    assert!(!without.content.contains("--send-to"), "an absent optional emits nothing: {}", without.content);

    let missing = nebo
        .tool(&ctx, "ledger_invoice_send", json!({ "sendTo": "x@example.com" }))
        .await;
    assert!(missing.is_error, "{}", missing.content);
    assert!(missing.content.contains("`invoiceId`") && missing.content.contains("ledger_invoice_send"), "{}", missing.content);
    assert!(!missing.content.contains("invoice\nsend"), "the binary never ran: {}", missing.content);

    // A plain binding is byte-for-byte as written and every field is a flag.
    let plain = nebo
        .tool(&ctx, "ledger_invoice_list", json!({ "limit": 5 }))
        .await;
    assert!(!plain.is_error, "{}", plain.content);
    assert!(plain.content.contains("invoice\nlist\n--limit\n5"), "{}", plain.content);
}

/// A chat parked on an install card resumes when the plugin lands by another
/// door. The card is the one find_plugins parks on, asked
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
            crate::chat_dispatch::announce_ask(&state, session_key, &event).await;
            (run, asking)
        }
    };
    let (_run, card) = park("agent:card-seat:main", SLUG).await;
    let (_other_run, other_card) = park("agent:card-seat:other", "some-other-plugin").await;
    assert!(nebo.state.run_registry.pending_ask_for_session("agent:card-seat:main").await.is_some());

    // The owner pastes the code into another chat.
    crate::codes::handle_code(&nebo.state, crate::codes::CodeType::Plugin, CODE, &crate::handlers::ws::EventOrigin::unclaimed("agent:card-seat:elsewhere")).await;
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

/// An install's events carry who asked: the client that pasted the code
/// (its socket's `client_id`) and the conversation it pasted it in, so only
/// that client opens the install surface. The owner hired from his phone and
/// came back to a desktop full of install dialogs (2026-09-27). The same
/// install arriving as a hire on the owner's account names no client, and
/// no client opens anything for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_install_carries_the_client_that_asked() {
    use crate::handlers::ws::EventOrigin;
    const CODE: &str = "PLUG-0R1G-0001";
    const SLUG: &str = "origin-ledger";
    let nebo = session().await;
    hub_offers_plugin(CODE, SLUG, "Origin Ledger");
    let profile = uuid::Uuid::new_v4().to_string();
    nebo.store()
        .create_auth_profile(&profile, "NeboAI", "neboai", "proof-token", None, None, 0, 1, Some("token"), None)
        .unwrap();

    // Every event about this code, as a desktop's socket hears it.
    let heard = |rx: &mut tokio::sync::broadcast::Receiver<crate::handlers::ws::HubEvent>| {
        let mut out = Vec::new();
        while let Ok(e) = rx.try_recv() {
            if e.payload["code"] == CODE {
                out.push((e.event_type, e.payload));
            }
        }
        out
    };

    let mut desktop = nebo.state.hub.subscribe();
    let phone = EventOrigin { client_id: Some("phone-page".to_string()), session_id: "agent:main:web".to_string(), platform: None };
    crate::codes::handle_code(&nebo.state, crate::codes::CodeType::Plugin, CODE, &phone).await;
    let events = heard(&mut desktop);
    let kinds: Vec<&str> = events.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(kinds, ["code_processing", "code_result"], "the install was announced and finished");
    for (kind, payload) in &events {
        assert_eq!(payload["client_id"], "phone-page", "{kind} names the client that asked");
        assert_eq!(payload["session_id"], "agent:main:web", "{kind} names the conversation");
    }
    assert_eq!(events[1].1["success"], true);

    // Again, as a hire on the owner's account: nobody here asked.
    let _ = nebo.state.plugin_store.remove(SLUG);
    let _ = nebo.store().delete_installed_plugin(SLUG);
    crate::codes::handle_code(
        &nebo.state,
        crate::codes::CodeType::Plugin,
        CODE,
        &EventOrigin::unclaimed("install-event-origin-ledger"),
    )
    .await;
    let events = heard(&mut desktop);
    assert_eq!(events.len(), 2);
    for (kind, payload) in &events {
        assert!(payload["client_id"].is_null(), "{kind} names no client");
        assert_eq!(payload["session_id"], "install-event-origin-ledger");
    }

    // Leave the shared server as it was found.
    let _ = nebo.state.plugin_store.remove(SLUG);
    let _ = nebo.store().delete_installed_plugin(SLUG);
    nebo.store().delete_auth_profile(&profile).unwrap();
}

/// A dependency cascade's progress carries the install it belongs to: a
/// collection the desktop's page redeems through `POST /codes` (its
/// `X-Nebo-Client` header) reports every `dep_*` step with that client, and
/// the same kind of install with no client behind it reports none. Only the
/// page that asked renders the rows; another client's cascade running at the
/// same moment never lands in its install modal. The cascade says it is
/// complete once, at its end: each level of the recursion used to say so,
/// and a hired employee's empty dependency list read as the whole cascade
/// being done.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cascade_carries_the_client_that_asked() {
    let nebo = session().await;
    let profile = uuid::Uuid::new_v4().to_string();
    nebo.store()
        .create_auth_profile(&profile, "NeboAI", "neboai", "proof-token", None, None, 0, 1, Some("token"), None)
        .unwrap();
    let job = json!({ "workflows": {} });
    hub_offers_agent("AGNT-CASC-0001", "cascade-desk", "Cascade Desk", job.clone());
    hub_offers_agent("AGNT-CASC-0002", "cascade-phone", "Cascade Phone", job);
    hub_offers_collection("COLL-CASC-0001", "cascade-desk-pack", "Desk Pack", &["AGNT-CASC-0001"]);
    hub_offers_collection("COLL-CASC-0002", "cascade-phone-pack", "Phone Pack", &["AGNT-CASC-0002"]);

    // The cascade's events as a desktop's socket hears them: kind and client.
    let cascade = |rx: &mut tokio::sync::broadcast::Receiver<crate::handlers::ws::HubEvent>| {
        let mut out = Vec::new();
        while let Ok(e) = rx.try_recv() {
            if e.event_type.starts_with("dep_") {
                out.push((e.event_type, e.payload["client_id"].clone()));
            }
        }
        out
    };
    let mut desktop = nebo.state.hub.subscribe();

    let sent = nebo
        .client
        .post(nebo.url("/codes"))
        .header(crate::handlers::ws::EventOrigin::CLIENT_HEADER, "desktop-page")
        .json(&json!({ "code": "COLL-CASC-0001" }))
        .send()
        .await
        .expect("POST /codes");
    assert_eq!(sent.status(), 200);
    let events = cascade(&mut desktop);
    let kinds: Vec<&str> = events.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(kinds, ["dep_cascade_start", "dep_started", "dep_installed", "dep_cascade_complete"]);
    for (kind, client) in &events {
        assert_eq!(client, "desktop-page", "{kind} names the page that asked");
    }

    // The same install with no client behind it (the phone app sends no name).
    nebo.post_ok("/codes", &json!({ "code": "COLL-CASC-0002" })).await;
    let events = cascade(&mut desktop);
    assert_eq!(events.len(), 4, "{events:?}");
    for (kind, client) in &events {
        assert!(client.is_null(), "{kind} names no client");
    }

    // Leave the shared server as it was found.
    for id in ["cascade-desk", "cascade-phone"] {
        let _ = nebo.delete(&format!("/agents/{id}")).await;
    }
    nebo.store().delete_auth_profile(&profile).unwrap();
}

/// An approval stays open past the turn that asked for it, for as long as
/// it can still be answered, and a client that connects later is shown it.
/// The employee suggests a goal through the real `suggest_goal` tool (its
/// answer waits on the owner however long that takes); the card is recorded
/// the way the chat pipeline records it; the turn ends through the real
/// `finish_turn`. The phone that was in the background all along connects
/// and gets the card, with the origin it was asked under. The owner's
/// decision closes it everywhere (`approval_resolved`, the goal is set) and
/// it is shown no more. A card nothing waits on any more (its waiter went
/// away) is not shown either: it has truly expired.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_open_approval_outlives_its_turn_and_reaches_a_client_that_connects_later() {
    use crate::handlers::ws::{EventOrigin, pending_approval_frames};
    let nebo = session().await;
    const REQUEST: &str = "toolu_goal_after_turn";
    const SESSION: &str = "agent:goal-seat:main";
    let shown = |frames: &[Value], id: &str| frames.iter().find(|f| f["data"]["request_id"] == id).cloned();

    // The employee suggests a goal in a turn the phone started.
    let (stream_tx, mut stream_rx) = tokio::sync::mpsc::channel(8);
    let mut ctx = Nebo::ctx("goal-seat", Origin::User);
    ctx.tool_call_id = REQUEST.to_string();
    ctx.stream_tx = Some(stream_tx);
    let r = nebo
        .tool(&ctx, "suggest_goal", json!({ "condition": "Every unpaid invoice has a reminder scheduled", "ask_owner": true }))
        .await;
    assert!(!r.is_error, "{}", r.content);
    let event = stream_rx.recv().await.expect("the card was asked");
    let tc = event.tool_call.expect("the card's call");
    let asked = EventOrigin { client_id: Some("socket-phone".to_string()), session_id: SESSION.to_string(), platform: None }
        .stamp(json!({ "request_id": tc.id, "tool": tc.name, "input": tc.input, "batch": null }));
    crate::chat_dispatch::record_approval_card(
        &nebo.state.approval_channels,
        &tc.id,
        tools::ApprovalCard {
            event: asked.clone(),
            session_key: SESSION.to_string(),
            agent_id: "goal-seat".to_string(),
            summary: "Agree on a goal".to_string(),
            since: 0,
        },
    )
    .await;

    // The turn ends.
    let run = nebo
        .state
        .run_registry
        .register(crate::run_registry::RegisterParams {
            session_key: SESSION.to_string(),
            entity_id: "goal-seat".to_string(),
            entity_name: "Goal Seat".to_string(),
            origin: "user".to_string(),
            channel: "web".to_string(),
            cancel_token: tokio_util::sync::CancellationToken::new(),
            parent_run_id: None,
        })
        .await;
    crate::chat_dispatch::finish_turn(
        &nebo.state,
        &run,
        crate::chat_dispatch::TurnEnd { payload: json!({ "session_id": SESSION }), artifacts: &[], control_stop: None },
    )
    .await;
    drop(run);

    // The phone connects later: it is shown the card, as it was asked.
    let frame = shown(&pending_approval_frames(&nebo.state).await, REQUEST).expect("the open card outlives its turn");
    assert_eq!(frame["type"], "approval_request");
    assert_eq!(frame["data"], asked, "shown as it was asked, origin included");

    // The owner approves it; every client hears so, and it is shown no more.
    let mut desktop = nebo.state.hub.subscribe();
    assert!(crate::chat_dispatch::answer_approval(&nebo.state, REQUEST, "once").await, "the answer reached the waiting suggestion");
    let resolved = std::iter::from_fn(|| desktop.try_recv().ok())
        .find(|e| e.event_type == "approval_resolved")
        .expect("every client hears the decision");
    assert_eq!(resolved.payload["request_id"], REQUEST);
    assert_eq!(resolved.payload["decision"], "once");
    assert!(shown(&pending_approval_frames(&nebo.state).await, REQUEST).is_none(), "a decided card is shown no more");

    // A card whose waiter went away has expired: not shown, and gone.
    let (answer, waiter) = tokio::sync::oneshot::channel();
    let mut expired = tools::PendingApproval::new(answer);
    expired.card = Some(tools::ApprovalCard { event: json!({ "request_id": "toolu_expired" }), session_key: SESSION.to_string(), agent_id: "goal-seat".to_string(), summary: String::new(), since: 0 });
    nebo.state.approval_channels.lock().await.insert("toolu_expired".to_string(), expired);
    drop(waiter);
    assert!(shown(&pending_approval_frames(&nebo.state).await, "toolu_expired").is_none(), "an expired card is not shown");
    assert!(!nebo.state.approval_channels.lock().await.contains_key("toolu_expired"));
}

/// An employee that names a plugin by its install code
/// (`requires.plugins: ["PLUG-…"]`, as marketplace packages do) reaches the
/// plugin's own `plugin__<slug>` tool: the install records the code it came
/// from, and the job's tools name the real tool. A qualified name maps to
/// the same tool; a code nothing here was installed from maps to none.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_plugin_required_by_its_code_is_its_own_tool() {
    use tools::plugin_tools::{plugin_slug_of, plugin_tool_name};
    const CODE: &str = "PLUG-C0DE-0001";
    const SLUG: &str = "coded-ledger";
    let nebo = session().await;
    hub_offers_plugin(CODE, SLUG, "Coded Ledger");
    let profile = uuid::Uuid::new_v4().to_string();
    nebo.store()
        .create_auth_profile(
            &profile,
            "NeboAI",
            "neboai",
            "proof-token",
            None,
            None,
            0,
            1,
            Some("token"),
            None,
        )
        .unwrap();
    crate::codes::handle_code(
        &nebo.state,
        crate::codes::CodeType::Plugin,
        CODE,
        &crate::handlers::ws::EventOrigin::unclaimed("agent:main:web"),
    )
    .await;
    assert!(
        nebo.state.plugin_store.resolve(SLUG, "*").is_some(),
        "the code installed the plugin"
    );

    assert_eq!(plugin_slug_of(nebo.store(), CODE).as_deref(), Some(SLUG));
    assert_eq!(
        plugin_slug_of(nebo.store(), &CODE.to_lowercase()).as_deref(),
        Some(SLUG)
    );
    assert_eq!(
        plugin_slug_of(nebo.store(), "@neboai/plugins/coded-ledger@^1").as_deref(),
        Some(SLUG)
    );
    assert_eq!(plugin_slug_of(nebo.store(), "PLUG-NEVR-0000"), None);

    let config =
        napp::agent::parse_agent_config(&json!({ "requires": { "plugins": [CODE] } }).to_string())
            .unwrap();
    let seat = tools::ActiveAgent {
        agent_id: "coded-seat".into(),
        name: "Coded Seat".into(),
        agent_md: String::new(),
        config: Some(config),
        channel_id: None,
        degraded: None,
        soul: None,
        rules: None,
    };
    let job = agent::harness::prompt::inputs::job_tools(
        &seat,
        None,
        &nebo.state.tools,
        nebo.store(),
        &std::collections::HashSet::new(),
    )
    .await;
    let tool = plugin_tool_name(SLUG);
    assert!(
        job.contains(&format!("- {tool}")),
        "the job names the real tool: {job}"
    );
    assert!(!job.contains(CODE), "{job}");
    assert!(
        nebo.state.tools.get_deferred_names().await.contains(&tool),
        "and the tool is there to load"
    );

    let _ = nebo.state.plugin_store.remove(SLUG);
    let _ = nebo.store().delete_installed_plugin(SLUG);
    nebo.state.tools.refresh_plugin_tools().await;
    nebo.store().delete_auth_profile(&profile).unwrap();
}

/// The hub echoes every redeem as a `tool_installed` install event, before
/// the redeem's own response — so the echo of a card tap reaches this
/// server while the tap's install is still running. The echo waits for that
/// install and finds the plugin present: one download, one install, and the
/// plugin never leaves the disk. Before this, the card's REST door was the
/// one door that did not claim its code, so the echo saw nothing in flight,
/// asked "installed?" too early, and ran the whole install again — a
/// second download, then a remove that left the plugin gone for the length
/// of the extraction (2026-09-26: one tap, three cycles, two such windows).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_hubs_echo_of_a_card_install_installs_nothing_twice() {
    const CODE: &str = "PLUG-ECH0-0001";
    const SLUG: &str = "echo-ledger";
    let nebo = session().await;
    hub_offers_plugin(CODE, SLUG, "Echo Ledger");
    let profile = uuid::Uuid::new_v4().to_string();
    nebo.store()
        .create_auth_profile(&profile, "NeboAI", "neboai", "proof-token", None, None, 0, 1, Some("token"), None)
        .unwrap();
    let before = napp_downloads_of(SLUG);
    // The tap's install stays in its download until the echo has arrived.
    let release = hub_holds_download(SLUG);

    // The card's tap: the REST door.
    let tap = {
        let state = nebo.state.clone();
        tokio::spawn(async move {
            crate::codes::submit_code(axum::extract::State(state), axum::http::HeaderMap::new(), axum::response::Json(json!({ "code": CODE })))
                .await
                .map(|body| body.0)
                .map_err(|(status, body)| format!("{status}: {}", body.error))
        })
    };
    nebo.wait_until(10, "the tap's install is in flight", || nebo.state.codes_in_flight.busy() || tap.is_finished())
        .await;
    if tap.is_finished() {
        let ended = tap.await.unwrap();
        panic!("the tap ended before its install was seen in flight: {ended:?}");
    }

    // The hub's echo of that tap's redeem, while the install runs: it waits.
    let echo = {
        let state = nebo.state.clone();
        tokio::spawn(async move {
            crate::handle_comm_install_event(
                &state,
                napp::InstallEvent {
                    event_type: "tool_installed".to_string(),
                    tool_id: hub_plugin_id(SLUG),
                    payload: json!({ "artifact_type": "plugin", "name": "Echo Ledger" }),
                },
            )
            .await
        })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!echo.is_finished(), "the echo waits for the install it echoes");
    assert_eq!(napp_downloads_of(SLUG) - before, 0, "nothing has been served yet");

    release();
    let tapped = tap.await.unwrap().unwrap();
    assert_eq!(tapped["success"], json!(true), "{tapped}");
    echo.await.unwrap().expect("the echo is handled");
    assert!(nebo.state.plugin_store.resolve(SLUG, "*").is_some(), "the tap installed the plugin");
    assert_eq!(napp_downloads_of(SLUG) - before, 1, "the echo downloaded nothing");
    assert!(!nebo.state.codes_in_flight.busy(), "and nothing is left installing");

    let _ = nebo.state.plugin_store.remove(SLUG);
    let _ = nebo.store().delete_installed_plugin(SLUG);
    nebo.state.tools.refresh_plugin_tools().await;
    nebo.store().delete_auth_profile(&profile).unwrap();
}

/// Two taps on one install card, the second while the first is installing,
/// are one install: the second is refused with 409 and the plain reason,
/// and the `.napp` is downloaded once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_taps_on_one_install_card_start_one_install() {
    const CODE: &str = "PLUG-TW1C-0001";
    const SLUG: &str = "twice-ledger";
    let nebo = session().await;
    hub_offers_plugin(CODE, SLUG, "Twice Ledger");
    let profile = uuid::Uuid::new_v4().to_string();
    nebo.store()
        .create_auth_profile(&profile, "NeboAI", "neboai", "proof-token", None, None, 0, 1, Some("token"), None)
        .unwrap();
    let before = napp_downloads_of(SLUG);
    // The first tap's install stays in its download until the second has tapped.
    let release = hub_holds_download(SLUG);

    let first = {
        let state = nebo.state.clone();
        tokio::spawn(async move {
            crate::codes::submit_code(axum::extract::State(state), axum::http::HeaderMap::new(), axum::response::Json(json!({ "code": CODE })))
                .await
        })
    };
    nebo.wait_until(10, "the first tap's install is in flight", || nebo.state.codes_in_flight.busy() || first.is_finished())
        .await;
    assert!(!first.is_finished(), "the first tap ended before its install was seen in flight");

    let second =
        crate::codes::submit_code(axum::extract::State(nebo.state.clone()), axum::http::HeaderMap::new(), axum::response::Json(json!({ "code": CODE })))
            .await;
    let (status, body) = second.err().expect("the second tap is refused");
    assert_eq!(status, axum::http::StatusCode::CONFLICT);
    assert_eq!(body.error, format!("{CODE} is already being installed."));

    release();
    let installed = first.await.unwrap().map_err(|(status, body)| format!("{status}: {}", body.error)).unwrap().0;
    assert_eq!(installed["success"], json!(true), "{installed}");
    assert!(nebo.state.plugin_store.resolve(SLUG, "*").is_some(), "the first tap installed the plugin");
    assert_eq!(napp_downloads_of(SLUG) - before, 1, "one download for two taps");

    let _ = nebo.state.plugin_store.remove(SLUG);
    let _ = nebo.store().delete_installed_plugin(SLUG);
    nebo.state.tools.refresh_plugin_tools().await;
    nebo.store().delete_auth_profile(&profile).unwrap();
}

/// A message of nothing but install codes installs them one after another,
/// in the order given, each through the one door a single code uses, and
/// the reply is a line per code as each finishes. The owner pasted a
/// connector code with a parenthesis on it and it went to the model as chat;
/// a numbered list with the punctuation a copy picks up reads as its codes,
/// once each. A code that fails is its own plain line, and the codes after it
/// still install: a connector this account can't see says so, and a code
/// nobody issued says it isn't valid, never NeboAI's raw answer. A list opens
/// no client's install surface (each code's setup would replace the last
/// one's). More than the most one message installs installs nothing and
/// says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_list_of_codes_installs_one_by_one_and_a_failure_is_its_own_line() {
    use crate::handlers::ws::{EventOrigin, HubEvent};
    const FIRST: &str = "PLUG-BTCH-0001";
    const HIDDEN: &str = "CONN-H1DE-0001";
    const GONE: &str = "PLUG-G0NE-0001";
    const LAST: &str = "PLUG-BTCH-0002";
    const SESSION: &str = "agent:main:web:code-list";
    let nebo = session().await;
    hub_offers_plugin(FIRST, "batch-ledger-a", "Batch Ledger A");
    hub_offers_plugin(LAST, "batch-ledger-b", "Batch Ledger B");
    hub_hides_connector(HIDDEN, "Hidden Desk");
    let profile = uuid::Uuid::new_v4().to_string();
    nebo.store()
        .create_auth_profile(&profile, "NeboAI", "neboai", "proof-token", None, None, 0, 1, Some("token"), None)
        .unwrap();

    let message = format!("1. {FIRST})\n2. `{HIDDEN}`,\n\u{2022} {GONE};\n- \"{LAST}\"\n{FIRST}.");
    let codes = crate::codes::detect_codes(&message).expect("a message of nothing but codes");
    let order: Vec<&str> = codes.iter().map(|(_, code)| *code).collect();
    assert_eq!(order, [FIRST, HIDDEN, GONE, LAST], "in the order given, each once");

    // What this conversation's socket hears.
    let heard = |rx: &mut tokio::sync::broadcast::Receiver<HubEvent>| {
        let mut out = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(e) if e.payload["session_id"] == SESSION => out.push(e),
                Ok(_) | Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {}
                Err(_) => break,
            }
        }
        out
    };
    let mut desktop = nebo.state.hub.subscribe();
    let asking = EventOrigin { client_id: Some("desk-page".to_string()), session_id: SESSION.to_string(), platform: None };
    let lines = crate::codes::handle_code_message(&nebo.state, &codes, &asking, "main").await;
    assert_eq!(
        lines,
        [
            "Installed plugin: Batch Ledger A",
            "Couldn't install CONN-H1DE-0001: it isn't available to your account. Ask its publisher to share it.",
            "Couldn't install PLUG-G0NE-0001: the code isn't valid.",
            "Installed plugin: Batch Ledger B",
        ]
    );
    assert!(nebo.state.plugin_store.resolve("batch-ledger-a", "*").is_some(), "the first code installed");
    assert!(nebo.state.plugin_store.resolve("batch-ledger-b", "*").is_some(), "the code after two failures installed");

    let events = heard(&mut desktop);
    let reply: Vec<&str> =
        events.iter().filter(|e| e.event_type == "chat_stream").filter_map(|e| e.payload["content"].as_str()).collect();
    assert_eq!(
        reply,
        [
            "Installing 4 codes, one at a time:\n\n",
            "- Installed plugin: Batch Ledger A\n",
            "- Couldn't install CONN-H1DE-0001: it isn't available to your account. Ask its publisher to share it.\n",
            "- Couldn't install PLUG-G0NE-0001: the code isn't valid.\n",
            "- Installed plugin: Batch Ledger B\n",
        ],
        "a line per code, as each finished"
    );
    assert_eq!(events.last().map(|e| e.event_type.as_str()), Some("chat_complete"), "then the reply is whole");
    for e in events.iter().filter(|e| e.event_type.starts_with("code_")) {
        assert!(e.payload["client_id"].is_null(), "a list opens no install surface: {}", e.payload);
    }
    let failed = events
        .iter()
        .find(|e| e.event_type == "code_result" && e.payload["code"] == HIDDEN)
        .expect("the hidden connector's result");
    assert_eq!(failed.payload["success"], false);
    assert_eq!(
        failed.payload["error"],
        "Couldn't install CONN-H1DE-0001: it isn't available to your account. Ask its publisher to share it."
    );

    // Too many for one message: nothing installs, and the reply says why.
    let many: Vec<String> = (0..=crate::codes::MAX_CODES_PER_MESSAGE).map(|i| format!("SKIL-CAP0-{i:04}")).collect();
    let many = many.join("\n");
    let many_codes = crate::codes::detect_codes(&many).expect("a long list is still a list");
    let lines = crate::codes::handle_code_message(&nebo.state, &many_codes, &asking, "main").await;
    assert_eq!(lines, ["That's 26 codes. I can install up to 25 at a time, so send them in smaller batches."]);
    assert!(
        !heard(&mut desktop).iter().any(|e| e.event_type == "code_processing"),
        "nothing started installing"
    );

    // Leave the shared server as it was found.
    for slug in ["batch-ledger-a", "batch-ledger-b"] {
        let _ = nebo.state.plugin_store.remove(slug);
        let _ = nebo.store().delete_installed_plugin(slug);
    }
    nebo.state.tools.refresh_plugin_tools().await;
    nebo.store().delete_auth_profile(&profile).unwrap();
}

/// A turn the owner started from the phone app hands its tools the app's
/// platform, and an install that needs payment reads there the way the app
/// is answered: "<name> isn't available to buy in the app.", with no
/// checkout link and no price, so the model has nothing to repeat. App
/// stores forbid pointing a buyer at a checkout outside the store. The same
/// call in a desktop turn still says where to pay.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_phone_turn_is_never_handed_a_checkout_link() {
    use crate::handlers::ws::HubEvent;
    const HIRE: &str = "AGNT-PAYD-0001";
    const SKILL: &str = "SKIL-PAYD-0001";
    let nebo = session().await;
    hub_sells(HIRE, "Paid Closer", "agent");
    hub_sells(SKILL, "Paid Writer", "skill");
    let profile = uuid::Uuid::new_v4().to_string();
    nebo.store()
        .create_auth_profile(&profile, "NeboAI", "neboai", "proof-token", None, None, 0, 1, Some("token"), None)
        .unwrap();
    let results = |rx: &mut tokio::sync::broadcast::Receiver<HubEvent>, code: &str| {
        let mut out = Vec::new();
        while let Ok(e) = rx.try_recv() {
            if e.event_type == "code_result" && e.payload["code"] == code {
                out.push(e.payload);
            }
        }
        out
    };
    let turn = |platform: Option<&str>| {
        let mut ctx = Nebo::ctx("", Origin::User);
        ctx.platform = platform.map(str::to_string);
        ctx
    };

    for (tool, code, name) in [("hire_employee", HIRE, "Paid Closer"), ("install_skill", SKILL, "Paid Writer")] {
        for phone in ["ios", "android"] {
            let mut desktop = nebo.state.hub.subscribe();
            let r = nebo.tool(&turn(Some(phone)), tool, json!({ "code": code })).await;
            assert!(!r.is_error, "{tool} from {phone}: {}", r.content);
            assert_eq!(r.content, format!("{name} isn't available to buy in the app."), "{tool} from {phone}");
            assert!(!r.content.contains("http"), "{tool} from {phone}: {}", r.content);
            let heard = results(&mut desktop, code);
            assert_eq!(heard.len(), 1, "{tool} from {phone}: {heard:?}");
            assert_eq!(heard[0]["message"], format!("{name} isn't available to buy in the app."));
            assert!(!heard[0].to_string().contains(&hub_checkout_url(code)), "{heard:?}");
        }

        // A desktop turn still gets where to pay.
        let r = nebo.tool(&turn(None), tool, json!({ "code": code })).await;
        assert!(!r.is_error, "{tool} on the desktop: {}", r.content);
        assert!(r.content.contains(&format!("Payment required: {}", hub_checkout_url(code))), "{tool} on the desktop: {}", r.content);
    }

    // Nothing was bought, so nothing was installed.
    assert!(nebo.store().get_agent("sold-AGNT-PAYD-0001").unwrap().is_none());
    nebo.store().delete_auth_profile(&profile).unwrap();
}

/// The first-party default plugins, offered by the hub stand-in under their
/// real codes, and the owner signed in to NeboAI (the profile to remove).
fn defaults_signed_in(nebo: &Nebo) -> (Vec<tools::default_artifacts::DefaultArtifact>, String) {
    let defaults: Vec<_> =
        tools::default_artifacts::DEFAULT_ARTIFACTS.iter().filter(|d| d.is_plugin()).copied().collect();
    for d in &defaults {
        hub_offers_plugin(d.code, d.slug, d.slug);
    }
    let profile = uuid::Uuid::new_v4().to_string();
    nebo.store()
        .create_auth_profile(&profile, "NeboAI", "neboai", "proof-token", None, None, 0, 1, Some("token"), None)
        .unwrap();
    (defaults, profile)
}

/// Take the defaults off this bot again, with the owner's removals and the
/// sign-in, so the next scenario starts from a fresh bot.
async fn forget_defaults(nebo: &Nebo, defaults: &[tools::default_artifacts::DefaultArtifact], profile: &str) {
    for d in defaults {
        let _ = nebo.state.plugin_store.remove(d.slug);
        let _ = std::fs::remove_dir_all(nebo.home.join("user").join("plugins").join(d.slug));
        let _ = nebo.store().delete_installed_plugin(d.slug);
    }
    nebo.state.tools.refresh_plugin_tools().await;
    crate::default_artifacts::forget_removals(nebo.store());
    nebo.store().delete_auth_profile(profile).unwrap();
}

/// Signed in, a pass installs every default this bot lacks, once each,
/// through the one code door; a default already on the bot is skipped (its
/// `.napp` is never fetched). The next connect's pass finds them all and
/// installs nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_defaults_install_once_after_sign_in_and_an_installed_one_is_skipped() {
    let nebo = session().await;
    let (defaults, profile) = defaults_signed_in(&nebo);
    let (kept, missing) = defaults.split_last().expect("default plugins");
    install_fake_plugin(&nebo.home, kept.slug, json!({}));
    nebo.wait_until(10, "the kept default resolves", || nebo.state.plugin_store.resolve(kept.slug, "*").is_some())
        .await;
    let before: Vec<usize> = defaults.iter().map(|d| napp_downloads_of(d.slug)).collect();

    crate::default_artifacts::spawn_pass(&nebo.state, defaults.clone(), &[], false, Duration::ZERO).expect("no pass was running").await.unwrap();
    for (d, before) in defaults.iter().zip(&before) {
        assert!(nebo.state.plugin_store.resolve(d.slug, "*").is_some(), "{} is installed", d.slug);
        let fetched = napp_downloads_of(d.slug) - before;
        assert_eq!(fetched, usize::from(missing.contains(d)), "{} fetched {fetched} times", d.slug);
    }
    assert!(!nebo.state.codes_in_flight.busy(), "nothing is left installing");
    let after: Vec<usize> = defaults.iter().map(|d| napp_downloads_of(d.slug)).collect();

    crate::default_artifacts::spawn_pass(&nebo.state, defaults.clone(), &[], false, Duration::ZERO).expect("the pass ended").await.unwrap();
    let again: Vec<usize> = defaults.iter().map(|d| napp_downloads_of(d.slug)).collect();
    assert_eq!(again, after, "the next pass installed nothing");

    forget_defaults(&nebo, &defaults, &profile).await;
}

/// A default the owner removes (the settings page's one removal door) is
/// recorded, and a pass never puts it back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_default_the_owner_removed_is_not_reinstalled_at_startup() {
    let nebo = session().await;
    let (defaults, profile) = defaults_signed_in(&nebo);
    let office = defaults[0];
    crate::default_artifacts::spawn_pass(&nebo.state, vec![office], &[], false, Duration::ZERO).expect("no pass was running").await.unwrap();
    assert!(nebo.state.plugin_store.resolve(office.slug, "*").is_some(), "the pass installed it");

    let (status, body) = nebo.delete(&format!("/plugins/{}", office.slug)).await;
    assert_eq!(status, 200, "{body}");
    assert!(nebo.state.plugin_store.resolve(office.slug, "*").is_none(), "the owner removed it");
    let fetched = napp_downloads_of(office.slug);

    crate::default_artifacts::spawn_pass(&nebo.state, vec![office], &[], false, Duration::ZERO).expect("the pass ended").await.unwrap();
    assert!(nebo.state.plugin_store.resolve(office.slug, "*").is_none(), "the pass left it removed");
    assert_eq!(napp_downloads_of(office.slug), fetched, "nothing was fetched for it");

    forget_defaults(&nebo, &defaults, &profile).await;
}

/// Every trigger at once installs a default once: a second pass asked for
/// while one runs starts nothing, a code door asked for it is refused while
/// the pass holds the code, and a request that needs it waits for that
/// install and then has it. Its `.napp` is fetched once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_triggers_install_a_default_once() {
    let nebo = session().await;
    let (defaults, profile) = defaults_signed_in(&nebo);
    let office = defaults[0];
    let before = napp_downloads_of(office.slug);
    let release = hub_holds_download(office.slug);

    let pass = crate::default_artifacts::spawn_pass(&nebo.state, vec![office], &[], false, Duration::ZERO).expect("no pass was running");
    nebo.wait_until(10, "the pass's install is in flight", || nebo.state.codes_in_flight.busy()).await;
    assert!(
        crate::default_artifacts::spawn_pass(&nebo.state, vec![office], &[], false, Duration::ZERO).is_none(),
        "a second trigger while a pass runs starts nothing"
    );
    let door = Arc::new(crate::channel_dispatch::CodeInstallerImpl::new(nebo.state.clone()));
    let refused = tools::CodeInstaller::install(door.as_ref(), office.code, tools::InstalledBy::Owner, None).await;
    assert!(refused.contains(tools::bot_tool::ALREADY_INSTALLING), "{refused}");
    let needed = {
        let door = door.clone();
        let plugins = nebo.state.plugin_store.clone();
        tokio::spawn(async move {
            tools::default_artifacts::install_on_request(door.as_ref(), &office, tools::InstalledBy::Owner, None, || {
                let plugins = plugins.clone();
                async move { plugins.resolve(office.slug, "*").is_some() }
            })
            .await
        })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;

    release();
    pass.await.unwrap();
    assert!(needed.await.unwrap(), "the request that needed it has it");
    assert_eq!(napp_downloads_of(office.slug) - before, 1, "one fetch for every trigger");

    forget_defaults(&nebo, &defaults, &profile).await;
}

/// A restored cloud bot has its plugins' records and data and not their
/// packages (its saved state leaves those out): its pass fetches each one
/// through the one plugin installer, and the data the plugin keeps — beside
/// its versions and where `NEBO_DATA_DIR` points — is untouched. A desktop's
/// pass leaves such a plugin alone; a pass with nothing to fetch ends at
/// once, whatever its spread.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restored_cloud_bot_fetches_the_plugin_packages_its_state_left_out() {
    const CODE: &str = "PLUG-REST-0RED";
    const SLUG: &str = "restored-index";
    let nebo = session().await;
    hub_offers_plugin(CODE, SLUG, "Restored Index");
    let profile = uuid::Uuid::new_v4().to_string();
    nebo.store()
        .create_auth_profile(&profile, "NeboAI", "neboai", "proof-token", None, None, 0, 1, Some("token"), None)
        .unwrap();
    crate::codes::handle_code(
        &nebo.state,
        crate::codes::CodeType::Plugin,
        CODE,
        &crate::handlers::ws::EventOrigin::unclaimed("restore-proof"),
    )
    .await;
    assert!(nebo.state.plugin_store.resolve(SLUG, "*").is_some(), "the code installed the plugin");

    // The plugin's own data, in both places a plugin keeps it.
    let slug_dir = nebo.state.plugin_store.plugins_dir().join(SLUG);
    std::fs::write(slug_dir.join("index.db"), b"the customer's whole index").unwrap();
    let data_dir = nebo.state.plugin_store.plugin_data_dir(SLUG);
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(data_dir.join("tokens.json"), b"{\"refresh\":\"r\"}").unwrap();

    // Restored from a state that left the package out.
    let package = napp::plugin::package_entries(&slug_dir);
    assert!(!package.is_empty(), "the install left a package");
    for name in &package {
        let p = slug_dir.join(name);
        if p.is_dir() { std::fs::remove_dir_all(p).unwrap() } else { std::fs::remove_file(p).unwrap() }
    }
    assert!(!nebo.state.plugin_store.package_on_disk(SLUG));
    let before = napp_downloads_of(SLUG);

    crate::default_artifacts::spawn_pass(&nebo.state, vec![], &[], false, Duration::ZERO).expect("no pass was running").await.unwrap();
    assert!(nebo.state.plugin_store.resolve(SLUG, "*").is_none(), "a desktop's pass fetches no package");
    assert_eq!(napp_downloads_of(SLUG), before);

    crate::default_artifacts::spawn_pass(&nebo.state, vec![], &[], true, Duration::ZERO).expect("no pass was running").await.unwrap();
    assert!(nebo.state.plugin_store.resolve(SLUG, "*").is_some(), "the cloud bot's pass fetched the package");
    assert_eq!(napp_downloads_of(SLUG) - before, 1, "fetched once");
    assert_eq!(std::fs::read(slug_dir.join("index.db")).unwrap(), b"the customer's whole index");
    assert_eq!(std::fs::read(data_dir.join("tokens.json")).unwrap(), b"{\"refresh\":\"r\"}");
    assert_eq!(nebo.store().plugin_slug_for_code(CODE).unwrap().as_deref(), Some(SLUG), "its record keeps its code");

    let idle = crate::default_artifacts::spawn_pass(&nebo.state, vec![], &[], false, Duration::from_secs(3600)).expect("the pass ended");
    tokio::time::timeout(Duration::from_secs(10), idle).await.expect("nothing to fetch, so no wait").unwrap();
    assert_eq!(napp_downloads_of(SLUG) - before, 1);

    let _ = nebo.state.plugin_store.remove(SLUG);
    let _ = std::fs::remove_dir_all(&slug_dir);
    let _ = std::fs::remove_dir_all(&data_dir);
    let _ = nebo.store().delete_installed_plugin(SLUG);
    nebo.state.tools.refresh_plugin_tools().await;
    nebo.store().delete_auth_profile(&profile).unwrap();
}

/// A restored cloud bot's settings came back with its state, and its
/// plugins' packages are fetched minutes after it starts. A plugin fetched
/// then runs with the owner's stored settings, the secret ones decrypted,
/// exactly as one present at startup does: before, it ran with no
/// credentials until the next restart (odoo, quickbooks on a restored bot).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_plugin_fetched_after_startup_runs_with_its_stored_settings() {
    const CODE: &str = "PLUG-REST-0SET";
    const SLUG: &str = "restored-ledger";
    let nebo = session().await;
    hub_offers_plugin(CODE, SLUG, "Restored Ledger");
    let profile = uuid::Uuid::new_v4().to_string();
    nebo.store()
        .create_auth_profile(&profile, "NeboAI", "neboai", "proof-token", None, None, 0, 1, Some("token"), None)
        .unwrap();
    crate::codes::handle_code(
        &nebo.state,
        crate::codes::CodeType::Plugin,
        CODE,
        &crate::handlers::ws::EventOrigin::unclaimed("restore-proof"),
    )
    .await;
    assert!(nebo.state.plugin_store.resolve(SLUG, "*").is_some(), "the code installed the plugin");

    // Restored: the settings are in the database as the owner saved them
    // (the key encrypted at rest), the package is not on disk, and the
    // process that starts holds nothing for the plugin.
    let sealed = auth::credential::encrypt("ledger-key-123").unwrap();
    nebo.store().upsert_plugin_setting_by_slug(SLUG, "LEDGER_API_KEY", &sealed, true).unwrap();
    nebo.store().upsert_plugin_setting_by_slug(SLUG, "LEDGER_REGION", "eu", false).unwrap();
    let slug_dir = nebo.state.plugin_store.plugins_dir().join(SLUG);
    for name in napp::plugin::package_entries(&slug_dir) {
        let p = slug_dir.join(name);
        if p.is_dir() { std::fs::remove_dir_all(p).unwrap() } else { std::fs::remove_file(p).unwrap() }
    }
    nebo.state.plugin_store.set_env_vars(SLUG, Default::default());
    assert!(nebo.state.plugin_store.auth_env_layers(SLUG).1.is_empty());

    crate::default_artifacts::spawn_pass(&nebo.state, vec![], &[], true, Duration::ZERO).expect("no pass was running").await.unwrap();
    assert!(nebo.state.plugin_store.resolve(SLUG, "*").is_some(), "the cloud bot's pass fetched the package");
    let env = nebo.state.plugin_store.resolved_auth_env(SLUG);
    assert_eq!(env.get("LEDGER_API_KEY").map(String::as_str), Some("ledger-key-123"), "the key, decrypted: {env:?}");
    assert_eq!(env.get("LEDGER_REGION").map(String::as_str), Some("eu"));

    let _ = nebo.state.plugin_store.remove(SLUG);
    let _ = std::fs::remove_dir_all(&slug_dir);
    let _ = nebo.store().delete_installed_plugin(SLUG);
    nebo.state.tools.refresh_plugin_tools().await;
    nebo.store().delete_auth_profile(&profile).unwrap();
}

/// A plugin copied in by hand after startup, with settings already stored
/// for it, gets them as a plugin present at startup does: the secret one
/// decrypted, and one its manifest declares secret but still stored in the
/// clear encrypted at rest first. With both in place it is ready.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_plugin_copied_in_after_startup_gets_its_stored_settings_sealed() {
    const SLUG: &str = "late-books";
    let nebo = session().await;
    nebo.store().upsert_installed_plugin(SLUG, "Late Books", "0.1.0", "", "", "", "unverified").unwrap();
    let sealed = auth::credential::encrypt("books-key-456").unwrap();
    nebo.store().upsert_plugin_setting_by_slug(SLUG, "BOOKS_API_KEY", &sealed, true).unwrap();
    nebo.store().upsert_plugin_setting_by_slug(SLUG, "BOOKS_TOKEN", "books-token-789", false).unwrap();

    let dir = nebo.home.join("user").join("plugins").join(SLUG).join("0.1.0");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.json"),
        json!({
            "id": SLUG, "slug": SLUG, "name": "Late Books", "version": "0.1.0", "platforms": {},
            "auth": { "type": "env", "env": { "BOOKS_API_KEY": "", "BOOKS_TOKEN": "" } },
        })
        .to_string(),
    )
    .unwrap();
    let bin = dir.join(SLUG);
    std::fs::write(&bin, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    nebo.wait_until(30, "the hot-loaded plugin to be ready", || nebo.state.plugin_store.is_ready(SLUG)).await;
    let env = nebo.state.plugin_store.resolved_auth_env(SLUG);
    assert_eq!(env.get("BOOKS_API_KEY").map(String::as_str), Some("books-key-456"), "{env:?}");
    assert_eq!(env.get("BOOKS_TOKEN").map(String::as_str), Some("books-token-789"), "{env:?}");
    let token = nebo
        .store()
        .list_plugin_settings_by_slug(SLUG)
        .unwrap()
        .into_iter()
        .find(|s| s.setting_key == "BOOKS_TOKEN")
        .unwrap();
    assert_eq!(token.is_secret, 1, "the declared secret is marked secret");
    assert!(auth::credential::is_encrypted(&token.setting_value), "and is no longer in the clear at rest");

    let _ = std::fs::remove_dir_all(nebo.home.join("user").join("plugins").join(SLUG));
    nebo.wait_until(30, "the removed plugin to leave", || nebo.state.plugin_store.resolve(SLUG, "*").is_none()).await;
    let _ = nebo.store().delete_installed_plugin(SLUG);
    nebo.state.tools.refresh_plugin_tools().await;
}

/// A cloud bot's pass waits out its spread before it fetches anything, and
/// a request that needs a default meanwhile does not wait for it: the safety
/// net installs it on the spot, once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cloud_pass_waits_its_spread_and_a_request_that_needs_a_default_does_not() {
    let nebo = session().await;
    let (defaults, profile) = defaults_signed_in(&nebo);
    let office = defaults[0];
    let before = napp_downloads_of(office.slug);

    let pass = crate::default_artifacts::spawn_pass(&nebo.state, vec![office], &[], true, Duration::from_secs(3600))
        .expect("no pass was running");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!pass.is_finished(), "the pass waits out its spread");
    assert!(nebo.state.plugin_store.resolve(office.slug, "*").is_none(), "nothing fetched yet");

    let door = crate::channel_dispatch::CodeInstallerImpl::new(nebo.state.clone());
    let plugins = nebo.state.plugin_store.clone();
    let landed = tokio::time::timeout(
        Duration::from_secs(60),
        tools::default_artifacts::install_on_request(&door, &office, tools::InstalledBy::Owner, None, || {
            let plugins = plugins.clone();
            async move { plugins.resolve(office.slug, "*").is_some() }
        }),
    )
    .await
    .expect("the request did not wait for the spread");
    assert!(landed, "the request that needed it has it");
    assert_eq!(napp_downloads_of(office.slug) - before, 1, "fetched once");
    assert!(!pass.is_finished(), "the pass is still waiting");

    pass.abort();
    let _ = pass.await;
    forget_defaults(&nebo, &defaults, &profile).await;
}
