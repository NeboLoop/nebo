//! One seat announces, the seat waiting for it runs.
//!
//! A company that runs on its own is nothing but these handoffs, so this test
//! drives the whole event path the product uses — the real emit tool, the real
//! event bus, the real dispatcher, and a subscription written with the address
//! an employee package actually writes — and asserts the waiting seat's
//! workflow starts.
//!
//! Deliberately NOT a unit test of the address formatter: a formatter test is
//! exactly what passed while no cross-seat handoff in the catalogue could
//! connect.

use std::pin::Pin;
use std::sync::{Arc, Mutex};

use tools::events::{Event, EventBus};
use tools::origin::{Origin, ToolContext};
use tools::registry::DynTool;
use tools::workflows::{WorkflowInfo, WorkflowManager, WorkflowRunInfo};
use nebo_workflow::events::{EventDispatcher, EventSubscription};

/// What a triggered run was asked to do. The dispatcher's only observable
/// effect, and the thing a handoff either produces or does not.
#[derive(Debug, Clone)]
struct StartedRun {
    agent_id: String,
    trigger_type: String,
    trigger_detail: Option<String>,
    emit_sources: Vec<String>,
    event_source: String,
    producer: Option<String>,
}

/// Records the runs the dispatcher starts. Everything else is unreachable in
/// this path and says so.
#[derive(Default)]
struct RecordingManager {
    started: Mutex<Vec<StartedRun>>,
}

impl RecordingManager {
    fn started(&self) -> Vec<StartedRun> {
        self.started.lock().unwrap().clone()
    }
}

fn unreachable_fut<'a, T: Send + 'a>(what: &'static str) -> Pin<Box<dyn Future<Output = T> + Send + 'a>> {
    Box::pin(async move { panic!("the event path never calls {what}") })
}

impl WorkflowManager for RecordingManager {
    fn list<'a>(&'a self, _: &'a str) -> Pin<Box<dyn Future<Output = Vec<WorkflowInfo>> + Send + 'a>> {
        Box::pin(async { Vec::new() })
    }
    fn install<'a>(&'a self, _: &'a str) -> Pin<Box<dyn Future<Output = Result<WorkflowInfo, String>> + Send + 'a>> {
        unreachable_fut("install")
    }
    fn uninstall<'a>(&'a self, _: &'a str) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        unreachable_fut("uninstall")
    }
    fn resolve<'a>(&'a self, _: &'a str, _: &'a str) -> Pin<Box<dyn Future<Output = Result<WorkflowInfo, String>> + Send + 'a>> {
        unreachable_fut("resolve")
    }
    fn resolve_agent<'a>(&'a self, _: &'a str) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        unreachable_fut("resolve_agent")
    }
    fn run<'a>(&'a self, _: &'a str, _: serde_json::Value, _: &'a str) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        unreachable_fut("run")
    }
    fn run_status<'a>(&'a self, _: &'a str) -> Pin<Box<dyn Future<Output = Result<WorkflowRunInfo, String>> + Send + 'a>> {
        unreachable_fut("run_status")
    }
    fn list_runs<'a>(&'a self, _: &'a str, _: i64) -> Pin<Box<dyn Future<Output = Vec<WorkflowRunInfo>> + Send + 'a>> {
        Box::pin(async { Vec::new() })
    }
    fn toggle<'a>(&'a self, _: &'a str) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + 'a>> {
        unreachable_fut("toggle")
    }
    fn create<'a>(&'a self, _: &'a str, _: &'a str, _: &'a str) -> Pin<Box<dyn Future<Output = Result<WorkflowInfo, String>> + Send + 'a>> {
        unreachable_fut("create")
    }
    fn update<'a>(&'a self, _: &'a str, _: &'a str, _: &'a str) -> Pin<Box<dyn Future<Output = Result<WorkflowInfo, String>> + Send + 'a>> {
        unreachable_fut("update")
    }
    fn delete<'a>(&'a self, _: &'a str, _: &'a str) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        unreachable_fut("delete")
    }
    fn cancel<'a>(&'a self, _: &'a str) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        unreachable_fut("cancel")
    }

    fn run_inline<'a>(
        &'a self,
        _definition_json: String,
        inputs: serde_json::Value,
        trigger_type: &'a str,
        trigger_detail: Option<String>,
        agent_id: &'a str,
        emit_sources: Vec<String>,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        let started = StartedRun {
            agent_id: agent_id.to_string(),
            trigger_type: trigger_type.to_string(),
            trigger_detail,
            emit_sources,
            event_source: inputs["_event_source"].as_str().unwrap_or_default().to_string(),
            producer: inputs["_event_payload"]["producer"].as_str().map(str::to_string),
        };
        self.started.lock().unwrap().push(started);
        Box::pin(async { Ok(uuid::Uuid::new_v4().to_string()) })
    }
}

/// A fresh store with the named seats hired, returning their ids in order.
fn store_with_seats(names: &[&str]) -> (Arc<db::Store>, Vec<String>) {
    let path = std::env::temp_dir().join(format!("nebo-handoff-{}.db", uuid::Uuid::new_v4()));
    let store = Arc::new(db::Store::new(&path.to_string_lossy()).expect("store"));
    let ids = names
        .iter()
        .map(|name| {
            let id = uuid::Uuid::new_v4().to_string();
            store
                .create_agent(&id, None, name, "", "", "", None, None)
                .expect("hire the seat");
            id
        })
        .collect();
    (store, ids)
}

/// The one inline definition a triggered binding needs; the recording manager
/// never parses it.
const DEF: &str = r#"{"activities":[{"id":"work","intent":"do the work"}]}"#;

/// Raise an event the way the product does: the real emit tool, stamped with
/// the producing seat, executed against the real bus.
async fn seat_emits(bus: &EventBus, producer_slug: &str, agent_id: &str, source: &str) {
    let tool = tools::EmitTool::new(bus.clone()).with_producer(producer_slug);
    let ctx = ToolContext::new(Origin::Workflow)
        .with_session(format!("agent:{agent_id}:workflow:run-1"), "s1");
    let result = tool
        .execute_dyn(&ctx, serde_json::json!({"source": source, "payload": {"poId": "PO-1"}}))
        .await;
    assert!(!result.is_error, "emit failed: {}", result.content);
}

/// Wait for the dispatcher to start the runs an event should have started.
async fn runs_after(manager: &Arc<RecordingManager>, at_least: usize) -> Vec<StartedRun> {
    for _ in 0..200 {
        let started = manager.started();
        if started.len() >= at_least {
            return started;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!(
        "the announcement never reached the waiting seat: expected {at_least} run(s), saw {:?}",
        manager.started()
    );
}

/// THE showstopper. The Inventory Manager announces a reorder with the address
/// its package declares; the Procurement Coordinator, subscribed to exactly
/// that string, runs.
#[tokio::test]
async fn one_seats_announcement_reaches_the_seat_waiting_for_it() {
    let (store, ids) = store_with_seats(&["Inventory Manager", "Procurement Coordinator"]);
    let (inventory, procurement) = (ids[0].clone(), ids[1].clone());

    // The two halves of the seam, verbatim from the packages: the producer's
    // `emit` and the consumer's `trigger.sources` entry are the same string.
    let announcement = "operations.inventory-manager.reorder-needed";
    let procurement_emit = "operations.procurement-coordinator.po-issued";

    let dispatcher = Arc::new(EventDispatcher::new());
    dispatcher
        .subscribe(EventSubscription {
            pattern: announcement.to_string(),
            default_inputs: serde_json::json!({}),
            agent_source: procurement.clone(),
            binding_name: "requisition-intake".into(),
            definition_json: Some(DEF.into()),
            // The consumer's own announcement, addressed by the ONE function.
            emit_sources: vec![nebo_workflow::events::emit_source_for(
                "Procurement Coordinator",
                procurement_emit,
            )],
            case: None,
        })
        .await;

    let manager = Arc::new(RecordingManager::default());
    let (bus, rx) = EventBus::new();
    dispatcher
        .clone()
        .spawn(rx, manager.clone() as Arc<dyn WorkflowManager>, store.clone());

    seat_emits(
        &bus,
        &db::agent_slug("Inventory Manager"),
        &inventory,
        // What the terminal node is told to announce — built by the one function
        // from the producing seat and the package's declared event.
        &nebo_workflow::events::emit_source_for("Inventory Manager", announcement),
    )
    .await;

    let started = runs_after(&manager, 1).await;
    assert_eq!(started.len(), 1, "exactly one waiting seat runs");
    let run = &started[0];
    assert_eq!(run.agent_id, procurement, "the Procurement Coordinator ran");
    assert_eq!(run.trigger_type, "event");
    assert_eq!(run.trigger_detail.as_deref(), Some("requisition-intake:operations.inventory-manager.reorder-needed"));
    // The address that travelled is the address the package wrote — not a
    // slug-prefixed variant of it.
    assert_eq!(run.event_source, announcement);
    assert_eq!(run.producer.as_deref(), Some("inventory-manager"), "the announcement says who spoke");
    // And what this seat will announce in turn is its package's address, once.
    assert_eq!(run.emit_sources, [procurement_emit]);
}

/// A registered company event is bare on both sides: a seat announces
/// `assignment.done`, a subscription to the bare name hears it, and the payload
/// names the seat that spoke.
#[tokio::test]
async fn a_bare_company_event_matches_a_bare_subscription() {
    let (store, ids) = store_with_seats(&["Bookkeeper", "General Manager"]);
    let (bookkeeper, manager_seat) = (ids[0].clone(), ids[1].clone());

    let dispatcher = Arc::new(EventDispatcher::new());
    dispatcher
        .subscribe(EventSubscription {
            pattern: "assignment.done".into(),
            default_inputs: serde_json::json!({}),
            agent_source: manager_seat.clone(),
            binding_name: "close-the-loop".into(),
            definition_json: Some(DEF.into()),
            emit_sources: Vec::new(),
            case: None,
        })
        .await;

    let manager = Arc::new(RecordingManager::default());
    let (bus, rx) = EventBus::new();
    dispatcher
        .clone()
        .spawn(rx, manager.clone() as Arc<dyn WorkflowManager>, store.clone());

    seat_emits(&bus, "bookkeeper", &bookkeeper, "assignment.done").await;

    let started = runs_after(&manager, 1).await;
    assert_eq!(started[0].agent_id, manager_seat);
    assert_eq!(started[0].event_source, "assignment.done", "a company event stays bare");
    assert_eq!(started[0].producer.as_deref(), Some("bookkeeper"));
}

/// The chat emit tool — one shared instance for every employee — addresses an
/// event by the seat whose session raised it, and reaches the same subscriber.
#[tokio::test]
async fn an_event_raised_from_chat_is_addressed_by_the_seat_that_raised_it() {
    let (store, ids) = store_with_seats(&["Inventory Manager", "Procurement Coordinator"]);
    let (inventory, procurement) = (ids[0].clone(), ids[1].clone());

    let dispatcher = Arc::new(EventDispatcher::new());
    dispatcher
        .subscribe(EventSubscription {
            pattern: "inventory-manager.reorder-needed".into(),
            default_inputs: serde_json::json!({}),
            agent_source: procurement.clone(),
            binding_name: "requisition-intake".into(),
            definition_json: Some(DEF.into()),
            emit_sources: Vec::new(),
            case: None,
        })
        .await;

    let manager = Arc::new(RecordingManager::default());
    let (bus, rx) = EventBus::new();
    dispatcher
        .clone()
        .spawn(rx, manager.clone() as Arc<dyn WorkflowManager>, store.clone());

    // No producer passed in: the tool reads the seat off the run's session key,
    // exactly as the one registered in the chat tool registry does.
    let tool = tools::EmitTool::new(bus.clone()).with_session_producer(store.clone());
    let ctx = ToolContext::new(Origin::User).with_session(format!("agent:{inventory}:main"), "s1");
    let result = tool
        .execute_dyn(&ctx, serde_json::json!({"source": "reorder-needed"}))
        .await;
    assert!(!result.is_error, "emit failed: {}", result.content);

    let started = runs_after(&manager, 1).await;
    assert_eq!(started[0].event_source, "inventory-manager.reorder-needed");
    assert_eq!(started[0].producer.as_deref(), Some("inventory-manager"));
}

/// A seat's slug appears in an address exactly once, whatever the address
/// already says. The bug this test exists for built
/// `procurement-coordinator.operations.procurement-coordinator.po-issued`,
/// which no subscription in the catalogue can match.
#[tokio::test]
async fn a_doubled_slug_never_appears_in_a_source() {
    let (bus, mut rx) = EventBus::new();
    let seats: &[(&str, &[&str])] = &[
        (
            "Procurement Coordinator",
            &[
                "operations.procurement-coordinator.po-issued",
                "operations.procurement-coordinator.exception",
                // Already runtime-namespaced, and passed through again.
                "procurement-coordinator.po-late",
            ],
        ),
        ("Bookkeeper", &["accounting.bookkeeper.month-closed", "briefing.ready"]),
    ];

    for (name, addresses) in seats {
        let slug = db::agent_slug(name);
        for address in *addresses {
            seat_emits(&bus, &slug, "agent-1", &nebo_workflow::events::emit_source_for(name, address)).await;
            let Event { source, .. } = rx.recv().await.expect("the bus carried the event");
            assert_eq!(
                source.split('.').filter(|segment| *segment == slug).count(),
                1,
                "{slug} appears more than once in {source}"
            );
            // An address that already names the seat travels exactly as written.
            if address.split('.').any(|segment| segment == slug) {
                assert_eq!(&source, address);
            } else {
                assert_eq!(source, format!("{slug}.{address}"));
            }
        }
    }
}
