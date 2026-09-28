//! Calls the phone app makes to its bot, answered as the phone sends them:
//! the location heartbeat (`lib/api/phone_location.dart`), the borrow
//! picker's other computers (`BotApi.otherComputers`), and the bot's
//! Location in Bot settings (`BotApi.location` / `BotApi.setLocation`, the
//! same calls the web app's generated `getLocation` / `updateLocation` make).
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_bots_location_round_trips() {
    let server = TestServer::boot().await;

    let unset = server.get("/agent/location").await;
    assert_eq!(unset.status(), 200);
    assert_eq!(unset.json::<Value>().await.unwrap(), json!({}), "no Location until one is set");

    // The phone: an address and the coordinates its geocoder found.
    let set = server
        .put_json(
            "/agent/location",
            &json!({ "label": "  50 W Broadway, Salt Lake City  ", "latitude": 40.7625, "longitude": -111.8937 }),
        )
        .await;
    assert_eq!(set.status(), 200);
    let office = json!({ "location": { "label": "50 W Broadway, Salt Lake City", "latitude": 40.7625, "longitude": -111.8937 } });
    assert_eq!(set.json::<Value>().await.unwrap(), office);
    assert_eq!(server.get("/agent/location").await.json::<Value>().await.unwrap(), office, "it is kept");

    // Half a coordinate pair, or one off the globe, is refused and changes nothing.
    for bad in [
        json!({ "label": "Somewhere", "latitude": 40.0 }),
        json!({ "label": "Somewhere", "latitude": 91.0, "longitude": 0.0 }),
    ] {
        assert_eq!(server.put_json("/agent/location", &bad).await.status(), 400, "{bad}");
    }
    assert_eq!(server.get("/agent/location").await.json::<Value>().await.unwrap(), office);

    // The web: a typed address with no geocoder drops the old coordinates.
    let typed = server.put_json("/agent/location", &json!({ "label": "1 Main St, Provo" })).await;
    assert_eq!(typed.json::<Value>().await.unwrap(), json!({ "location": { "label": "1 Main St, Provo" } }));

    // An empty label clears it.
    let cleared = server.put_json("/agent/location", &json!({ "label": "" })).await;
    assert_eq!(cleared.status(), 200);
    assert_eq!(server.get("/agent/location").await.json::<Value>().await.unwrap(), json!({}));
}
