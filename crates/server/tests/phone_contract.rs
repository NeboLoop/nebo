//! Calls the phone app makes to its bot, answered as the phone sends them:
//! the location heartbeat (`lib/api/phone_location.dart`), the borrow
//! picker's other computers (`BotApi.otherComputers`), and the bot's
//! Location in Bot settings (`BotApi.location` / `BotApi.setLocation`, the
//! same calls the web app's generated `getLocation` / `updateLocation` make),
//! and an employee's Identity page renaming a linked employee.
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

/// The Identity page's save (`_identityBody`, the web's `saveIdentity`) on a
/// linked employee: the owner's name is taken, and the link, not the name,
/// is what its team and delegation reach it by. Its persona stays on the
/// linked computer, and a blank name is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_linked_employee_is_renamed_from_its_identity_page() {
    let server = TestServer::boot().await;
    let store = server.db_store();
    // The row and brain a linked hire writes.
    let brain = "linked/bot-1/main";
    store
        .create_agent("linked-main", Some("linked"), "main", "OpenClaw agent", "---\nname: \"main\"\n---\n", "{}", None, None)
        .unwrap();
    store.upsert_entity_config("agent", "linked-main", &json!({ "modelPreference": brain })).unwrap();
    store.create_agent("chief", None, "Chief", "", "---\nname: Chief\n---\n", "{}", None, None).unwrap();
    let team = server
        .post_json(
            "/teams",
            &json!({ "name": "Research", "members": [{ "agentId": "main" }, { "agentId": "chief" }], "organizerAgentId": "chief" }),
        )
        .await;
    assert_eq!(team.status(), 200);
    let team_id = team.json::<Value>().await.unwrap()["team"]["id"].as_str().unwrap().to_string();

    let identity = |name: &str| {
        json!({ "name": name, "description": "OpenClaw agent", "color": "", "voice": "", "department": "", "reportsTo": "" })
    };
    let renamed = server.put_json("/agents/linked-main", &identity(" Scout ")).await;
    assert_eq!(renamed.status(), 200);
    assert_eq!(renamed.json::<Value>().await.unwrap()["agent"]["name"], "Scout");

    for refused in [identity("  "), json!({ "soul": "Loud." })] {
        assert_eq!(server.put_json("/agents/linked-main", &refused).await.status(), 400, "{refused}");
    }

    let roster = server.get("/agents").await.json::<Value>().await.unwrap();
    let row = roster["agents"].as_array().unwrap().iter().find(|a| a["id"] == "linked-main").cloned().unwrap();
    assert_eq!((row["name"].as_str(), row["kind"].as_str()), (Some("Scout"), Some("linked")));

    // Its team keeps it, and the new name reaches it: the lead is handed to
    // "Scout" by name.
    let lead = server.put_json(&format!("/teams/{team_id}"), &json!({ "organizerAgentId": "Scout" })).await;
    assert_eq!(lead.status(), 200);
    let team = lead.json::<Value>().await.unwrap()["team"].clone();
    assert_eq!(team["organizerAgentId"], "linked-main");
    assert!(team["members"].as_array().unwrap().iter().any(|m| m["agentId"] == "linked-main"));

    // A restart reopens the database and runs the manifest sync; the name is
    // the owner's, and the brain still names the same linked agent.
    let reopened = server.db_store();
    reopened.sync_agent_identity("linked-main", "main", "OpenClaw agent").unwrap();
    assert_eq!(reopened.get_agent("linked-main").unwrap().unwrap().name, "Scout");
    let config = reopened.get_entity_config("agent", "linked-main").unwrap().unwrap();
    assert_eq!(config.model_preference.as_deref(), Some(brain));
}
