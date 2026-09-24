//! A drain stops the engine.
//!
//! Once a shutdown drain begins (`DRAINING`), the engine loop must not arm a
//! schedule, claim an event or deliver one: whatever is due waits in the
//! table for the next boot. A real server is booted, the engine is first
//! shown claiming an event (so the test can fail), then the drain starts and
//! an event enqueued during it must never be claimed.
//!
//! Its own test binary: `DRAINING` is process-wide, and setting it here must
//! not stop the engine under any other suite.
//!
//! Run:
//!   cargo test -p nebo-server --test drain -- --nocapture

use std::sync::atomic::Ordering;
use std::time::Duration;

use db::NewEvent;

mod common;
use common::TestServer;

/// The engine ticks every five seconds; three ticks is ample.
const WINDOW: Duration = Duration::from_secs(15);

fn enqueue(store: &db::Store, key: &str) -> i64 {
    match store
        .engine_enqueue_event(&NewEvent {
            kind: "signal",
            target_type: "run",
            target_id: &format!("email:{key}"),
            payload: "drain test",
            idem_key: key,
            durable: true,
            ..Default::default()
        })
        .expect("enqueue")
    {
        db::Enqueued::Inserted(id) => id,
        db::Enqueued::Duplicate => panic!("fresh key reported duplicate"),
    }
}

fn attempts(store: &db::Store, id: i64) -> i64 {
    store
        .engine_get_event(id)
        .expect("read event")
        .expect("event exists")
        .attempts
}

#[tokio::test(flavor = "multi_thread")]
async fn no_engine_tick_claims_an_event_once_draining_starts() {
    let server = TestServer::boot().await;
    let store = server.db_store();

    // Before the drain the engine claims a due event.
    let before = enqueue(&store, "before-drain");
    let deadline = tokio::time::Instant::now() + WINDOW;
    while attempts(&store, before) == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the engine never claimed an event before the drain"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // The drain begins. A pass already in flight is let finish, then an
    // event lands that no tick may touch.
    nebo_server::DRAINING.store(true, Ordering::Relaxed);
    tokio::time::sleep(Duration::from_secs(6)).await;
    let during = enqueue(&store, "during-drain");
    tokio::time::sleep(WINDOW).await;
    assert_eq!(
        attempts(&store, during),
        0,
        "an engine tick claimed an event after draining started"
    );
}
