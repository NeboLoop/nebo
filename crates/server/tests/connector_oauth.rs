//! Connectors end to end, against a real OAuth MCP server on loopback:
//! add one by its URL (the server says how it signs in), reconnect it from a
//! remote caller (the phone, through the tunnel), and finish the sign-in
//! through the hub's public callback relay, as the hub delivers it.
//!
//! Run:
//!   cargo test -p nebo-server --test connector_oauth

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::{Form, Json, Router};
use serde_json::{Value, json};

mod common;
use common::TestServer;

const PUBLIC_REDIRECT: &str = "https://neboai.com/oauth/plugin/callback";
const ACCESS_TOKEN: &str = "test-access-token";

/// What the fake OAuth MCP server saw.
#[derive(Default)]
struct Seen {
    registered_redirects: Vec<String>,
    exchanged_redirects: Vec<String>,
}

type Shared = Arc<Mutex<Seen>>;

/// A loopback MCP server that signs in with OAuth (RFC 8414 discovery,
/// Dynamic Client Registration, PKCE code exchange) and lists two tools only
/// to a caller holding its token. With `open`, it lists them to anyone.
async fn fake_server(open: bool) -> (String, Shared) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let seen: Shared = Arc::default();
    let meta_origin = origin.clone();
    let app = Router::new()
        .route(
            "/.well-known/oauth-authorization-server",
            axum::routing::get(move || {
                let o = meta_origin.clone();
                async move {
                    if open {
                        return StatusCode::NOT_FOUND.into_response();
                    }
                    Json(json!({
                        "issuer": o,
                        "authorization_endpoint": format!("{o}/authorize"),
                        "token_endpoint": format!("{o}/token"),
                        "registration_endpoint": format!("{o}/register"),
                    }))
                    .into_response()
                }
            }),
        )
        .route(
            "/register",
            axum::routing::post(|State(seen): State<Shared>, Json(body): Json<Value>| async move {
                let uris = body["redirect_uris"].as_array().cloned().unwrap_or_default();
                seen.lock().unwrap().registered_redirects.extend(uris.iter().filter_map(|u| u.as_str().map(String::from)));
                Json(json!({ "client_id": "client-1" }))
            }),
        )
        .route(
            "/token",
            axum::routing::post(
                |State(seen): State<Shared>, Form(form): Form<std::collections::HashMap<String, String>>| async move {
                    assert_eq!(form.get("grant_type").map(String::as_str), Some("authorization_code"));
                    assert_eq!(form.get("code").map(String::as_str), Some("the-code"));
                    assert!(form.get("code_verifier").is_some_and(|v| !v.is_empty()), "PKCE verifier sent");
                    seen.lock().unwrap().exchanged_redirects.push(form.get("redirect_uri").cloned().unwrap_or_default());
                    Json(json!({
                        "access_token": ACCESS_TOKEN,
                        "refresh_token": "test-refresh",
                        "expires_in": 3600,
                        "token_type": "Bearer",
                    }))
                },
            ),
        )
        .route(
            "/mcp",
            axum::routing::post(move |headers: HeaderMap, Json(req): Json<Value>| async move {
                let authed = headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v == format!("Bearer {ACCESS_TOKEN}"));
                if !open && !authed {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                Json(json!({
                    "jsonrpc": "2.0",
                    "id": req["id"],
                    "result": { "tools": [
                        { "name": "edge_nodes", "description": "Edge nodes: status, membership, and draining a node out of rotation.",
                          "inputSchema": { "type": "object" } },
                        { "name": "access_logs", "description": "Read the access logs.",
                          "inputSchema": { "type": "object" }, "annotations": { "readOnlyHint": true } },
                    ]},
                }))
                .into_response()
            }),
        )
        .with_state(seen.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("{origin}/mcp"), seen)
}

fn query_param(url: &str, key: &str) -> String {
    url::Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| panic!("no {key} in {url}"))
}

async fn json(resp: reqwest::Response) -> Value {
    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    assert!(status.is_success(), "{status}: {body}");
    body
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connector_added_by_url_signs_in_from_the_phone_through_the_public_callback() {
    if !common::in_own_process().await {
        return;
    }
    // A Nebo paired with NeboAI: the phone reaches it through its tunnel.
    let bot_id = "0b7f2c1a-1111-2222-3333-444455556666";
    std::fs::create_dir_all(config::data_dir().unwrap()).unwrap();
    config::write_bot_id(bot_id).unwrap();
    let server = TestServer::boot().await;
    let (mcp_url, seen) = fake_server(false).await;

    // Add custom connector: a name and a URL. The server says it signs in
    // with OAuth.
    let created = json(server.post_json("/integrations", &json!({
        "name": "Edge Ops", "serverUrl": mcp_url, "authType": "auto",
    })).await).await;
    let id = created["integration"]["id"].as_str().unwrap().to_string();
    assert_eq!(created["integration"]["authType"], "oauth");

    // Reconnect from the phone: the request arrives through the tunnel with
    // the browser's Host, and answers with the sign-in link.
    let resp = server
        .client
        .post(server.url(&format!("/integrations/{id}/reauthenticate")))
        .header("Host", "neboai.com")
        // The stamp the bot's tunnel puts on what it forwards.
        .header("x-nebo-tunnel-auth", comm::tunnel::tunnel_auth_secret())
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    let auth_url = json(resp).await["authUrl"].as_str().unwrap().to_string();
    // The sign-in returns to the hub's public callback, never to the tunnel
    // path (it needs a NeboAI session the sign-in browser doesn't carry).
    assert_eq!(query_param(&auth_url, "redirect_uri"), PUBLIC_REDIRECT);
    assert_eq!(seen.lock().unwrap().registered_redirects, [PUBLIC_REDIRECT], "DCR registers the same return");
    let state = query_param(&auth_url, "state");
    {
        use base64::Engine;
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&state).expect("hub-readable state");
        let v: Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(v["bot_id"], bot_id, "the hub picks this bot's tunnel from the state");
    }

    // The provider redirects to the hub, which relays the query into the
    // bot's tunnel at /plugins/oauth/relay.
    let relayed = server
        .get(&format!("/plugins/oauth/relay?code=the-code&state={}", urlencoding::encode(&state)))
        .await;
    assert_eq!(relayed.status(), 200, "{}", relayed.text().await.unwrap());
    assert_eq!(seen.lock().unwrap().exchanged_redirects, [PUBLIC_REDIRECT], "the exchange presents the same return");

    // Connected, with the server's tools.
    let row = json(server.get(&format!("/integrations/{id}")).await).await;
    assert_eq!(row["connectionStatus"], "connected", "{row}");
    assert_eq!(row["toolCount"], 2);

    // Its tools, with the rule keys their settings are stored under.
    let tools = json(server.get(&format!("/integrations/{id}/tools")).await).await;
    assert_eq!(tools["family"], "mcp__edge_ops__*");
    assert_eq!(tools["settable"], true);
    let listed: Vec<(&str, &str)> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["title"].as_str().unwrap(), t["key"].as_str().unwrap()))
        .collect();
    assert_eq!(listed, [("Access logs", "mcp__edge_ops__access_logs"), ("Edge nodes", "mcp__edge_ops__edge_nodes")]);
    assert_eq!(
        tools["tools"][1]["description"],
        "Edge nodes: status, membership, and draining a node out of rotation."
    );

    // A replayed callback finds no flow: refused, nothing changes.
    let replay = server
        .get(&format!("/plugins/oauth/relay?code=the-code&state={}", urlencoding::encode(&state)))
        .await;
    assert!(replay.status().is_client_error(), "replay: {}", replay.status());
    assert_eq!(seen.lock().unwrap().exchanged_redirects.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connector_that_needs_no_sign_in_is_added_as_open() {
    if !common::in_own_process().await {
        return;
    }
    let server = TestServer::boot().await;
    let (mcp_url, _) = fake_server(true).await;
    let created = json(server.post_json("/integrations", &json!({
        "name": "Open Docs", "serverUrl": mcp_url, "authType": "auto",
    })).await).await;
    assert_eq!(created["integration"]["authType"], "none");
    let id = created["integration"]["id"].as_str().unwrap();
    let connected = json(server.post_json(&format!("/integrations/{id}/connect"), &json!({})).await).await;
    assert_eq!(connected["success"], true, "{connected}");
    assert_eq!(connected["toolCount"], 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sign_in_on_this_machine_returns_to_its_own_callback() {
    if !common::in_own_process().await {
        return;
    }
    let server = TestServer::boot().await;
    let (mcp_url, _) = fake_server(false).await;
    let created = json(server.post_json("/integrations", &json!({
        "name": "Edge Ops", "serverUrl": mcp_url, "authType": "auto",
    })).await).await;
    let id = created["integration"]["id"].as_str().unwrap();
    let auth_url = json(server.get(&format!("/integrations/{id}/oauth-url")).await).await["authUrl"]
        .as_str()
        .unwrap()
        .to_string();
    let redirect = query_param(&auth_url, "redirect_uri");
    assert!(redirect.starts_with("http://localhost:"), "{redirect}");
    assert!(redirect.ends_with("/api/v1/integrations/oauth/callback"), "{redirect}");
}
