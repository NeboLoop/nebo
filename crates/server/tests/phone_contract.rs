//! Calls the phone app makes to its bot, answered as the phone sends them:
//! the location heartbeat (`lib/api/phone_location.dart`) and the borrow
//! picker's other computers (`BotApi.otherComputers`).
//!
//! Run:
//!   cargo test -p nebo-server --test phone_contract

use serde_json::{Value, json};

mod common;
use common::TestServer;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_phones_calls_are_answered() {
    let server = TestServer::boot().await;

    // The heartbeat: a reading shared with one employee, then the revocation
    // the phone sends with no coordinates.
    let now = chrono::Utc::now().timestamp();
    let shared = server
        .put_json(
            "/phone/location",
            &json!({
                "accountId": "owner-account",
                "deviceId": "0a1b2c",
                "revision": 1,
                "agentIds": ["assistant"],
                "latitude": 40.7608,
                "longitude": -111.891,
                "accuracyMetres": 12.5,
                "takenAt": now,
            }),
        )
        .await;
    assert_eq!(
        shared.status(),
        200,
        "the phone's location reading is taken"
    );
    assert_eq!(shared.json::<Value>().await.unwrap()["accepted"], true);
    let revoked = server
        .put_json(
            "/phone/location",
            &json!({ "accountId": "owner-account", "deviceId": "0a1b2c", "revision": 2, "agentIds": [] }),
        )
        .await;
    assert_eq!(revoked.status(), 200, "a revocation is taken");
    let stale = server
        .put_json(
            "/phone/location",
            &json!({
                "accountId": "owner-account", "deviceId": "0a1b2c", "revision": 3, "agentIds": ["assistant"],
                "latitude": 40.7608, "longitude": -111.891, "accuracyMetres": 12.5, "takenAt": now - 3600,
            }),
        )
        .await;
    assert_eq!(stale.status(), 400, "an hour-old reading is refused");

    // The borrow picker: an unlinked Nebo has no other computers.
    let others = server.get("/teams/other-computers").await;
    assert_eq!(others.status(), 200);
    assert_eq!(
        others.json::<Value>().await.unwrap(),
        json!({ "computers": [] })
    );
}
