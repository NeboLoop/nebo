//! A cloud bot parks only when it is idle, and parking is the graceful drain.
//!
//! A real server is booted in server mode (a cloud bot) and asked to park
//! the way the hub asks — through `comm::residency`, which the gateway's
//! RESIDENCY frame feeds. It declines while its last work is too recent and
//! while its next timer is too close; asked again when neither holds, it
//! parks: deliveries are held (whatever arrives waits in the hub mailbox)
//! and the server drains and stops.
//!
//! Its own test binary: server mode, the held deliveries and the drain are
//! process-wide.
//!
//! Run:
//!   cargo test -p nebo-server --test residency -- --nocapture

use std::time::Duration;

use comm::residency::{Decision, Passivate};
use db::NewEvent;

mod common;
use common::TestServer;

fn ask(idle_for: u64, min_next_wake: u64) -> Passivate {
    Passivate {
        idle_for: Duration::from_secs(idle_for),
        min_next_wake: Duration::from_secs(min_next_wake),
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cloud_bot_parks_only_when_idle_and_parking_is_the_drain() {
    // SAFETY: set before the server (and any other thread) starts.
    unsafe {
        std::env::set_var("NEBO_SERVER_MODE", "1");
    }
    let server = TestServer::boot().await;
    let store = server.db_store();

    // Just booted: its last work (the start) is seconds old.
    match comm::residency::request(ask(3600, 0)).await.unwrap() {
        Decision::Busy(why) => assert!(why.contains("last work ended"), "{why}"),
        Decision::Parking => panic!("parked though it started seconds ago"),
    }

    // A timer a minute out is closer than the ten minutes asked for.
    let timer = match store
        .engine_enqueue_event(&NewEvent {
            kind: "timer",
            target_type: "binding",
            target_id: "residency-test",
            idem_key: "residency-test-timer",
            due_at: Some(now() + 60),
            ..Default::default()
        })
        .expect("enqueue timer")
    {
        db::Enqueued::Inserted(id) => id,
        db::Enqueued::Duplicate => unreachable!(),
    };
    match comm::residency::request(ask(0, 600)).await.unwrap() {
        Decision::Busy(why) => assert!(why.contains("next timer"), "{why}"),
        Decision::Parking => panic!("parked with a timer a minute away"),
    }
    assert!(
        !comm::residency::holding(),
        "a bot that declined still takes deliveries"
    );
    assert_eq!(
        reqwest::get(server.health_url())
            .await
            .map(|r| r.status().is_success())
            .ok(),
        Some(true),
        "a bot that declined keeps serving"
    );

    // Nothing near and idle long enough: it parks.
    store
        .engine_complete_event(timer, now())
        .expect("complete timer");
    match comm::residency::request(ask(0, 600)).await.unwrap() {
        Decision::Parking => {}
        Decision::Busy(why) => panic!("declined to park: {why}"),
    }
    assert!(
        comm::residency::holding(),
        "deliveries are held from the moment it parks"
    );

    // Parking is the drain: the server stops.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        if reqwest::get(server.health_url()).await.is_err() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the server did not drain after parking"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // A second ask while parking is never a refusal.
    assert_eq!(
        comm::residency::request(ask(0, 600)).await.unwrap(),
        Decision::Parking
    );
}
