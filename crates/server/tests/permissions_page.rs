//! The Permissions page as the desktop and the phone call it: one three-way
//! switch (Allow, Ask, Off) per choice, written through
//! `PUT /permissions/company` and `PUT /agents/{id}/permissions` with
//! `set: {id, value}`, and the MCP screen's old tool-permission routes gone.
//!
//! Run:
//!   cargo test -p nebo-server --test permissions_page

use serde_json::{Value, json};

mod common;
use common::TestServer;

fn switch<'a>(page: &'a Value, id: &str) -> &'a Value {
    page["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .chain(page["specific"].as_array().unwrap())
        .find(|s| s["id"] == id)
        .unwrap_or_else(|| panic!("no switch {id} in {page}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_permissions_page_is_one_switch_through_one_api() {
    let server = TestServer::boot().await;

    // The page's shape: switches and plain lines, none of the old lists.
    let page: Value = server.get("/permissions/company").await.json().await.unwrap();
    let mut keys: Vec<&str> = page.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(
        keys,
        ["alwaysAsks", "capabilities", "companyMode", "fixed", "folders", "groups", "mode", "modeFromCompany", "money", "specific"]
    );

    // Each state round-trips, on the company page and an employee's.
    for path in ["/permissions/company", "/agents/assistant/permissions"] {
        for value in ["allow", "ask", "deny"] {
            let put = server.put_json(path, &json!({ "set": { "id": "capability:web", "value": value } })).await;
            assert_eq!(put.status(), 200, "{path} {value}");
            let answered: Value = put.json().await.unwrap();
            let got: Value = server.get(path).await.json().await.unwrap();
            for page in [&answered, &got] {
                let web = switch(page, "capability:web");
                assert_eq!((web["value"].as_str(), web["inherited"].as_bool()), (Some(value), Some(false)), "{path} {value}");
            }
        }
        let cleared = server.put_json(path, &json!({ "set": { "id": "capability:web", "value": "inherit" } })).await;
        assert_eq!(cleared.status(), 200);
    }

    // A company Off binds the employee: shown locked, and refused there.
    server.put_json("/permissions/company", &json!({ "set": { "id": "capability:shell", "value": "deny" } })).await;
    let employee: Value = server.get("/agents/assistant/permissions").await.json().await.unwrap();
    assert_eq!(switch(&employee, "capability:shell")["locked"], true);
    let refused = server
        .put_json("/agents/assistant/permissions", &json!({ "set": { "id": "capability:shell", "value": "allow" } }))
        .await;
    assert_eq!(refused.status(), 400, "no employee setting undoes a company Off");

    // The MCP screen's own tool-permission routes are gone: its access is set
    // on this page. (An unknown GET gets the app's page, never an answer.)
    server
        .db_store()
        .create_mcp_integration("crm-1", "Acme CRM", "http", Some("https://mcp.example.com"), "none", None, None)
        .unwrap();
    for gone in [
        server.get("/integrations/crm-1/tool-permissions").await,
        server.put_json("/integrations/crm-1/tool-permissions", &json!({ "default": "allow" })).await,
    ] {
        let status = gone.status();
        let json = gone.headers().get("content-type").is_some_and(|c| c.to_str().unwrap_or("").contains("json"));
        assert!(!status.is_success() || !json, "{status} answered as a route");
    }
}
