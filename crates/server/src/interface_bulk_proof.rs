//! The bulk pattern on a generic interface, proven through the real server:
//! a workflow's code steps read every page of `ledger.invoice.search` and
//! apply one `ledger.invoice.update` per row through whichever connected
//! plugin binds them, with each row's result, under the owner's approval
//! rules exactly as a model's call to the same operation meets them.
//!
//! The plugin is a stand-in with QuickBooks' own bindings for the four
//! operations (copied from `plugins/quickbooks/plugin.json`), whose binary
//! answers like the QuickBooks plugin does (`{"count", "items"}`) and pages
//! with `nextCursor` / `--cursor`.

use serde_json::{json, Value};
use types::permissions::{Effect, Rule, RuleKey, RuleSource, Scope, Writer};

use crate::staffed_proof::{Nebo, session};

const SLUG: &str = "books-standin";

/// QuickBooks' bindings for the operations the proof performs.
fn quickbooks_bindings() -> Value {
    json!({
        "ledger.invoice.search": "ledger invoice-search {customerId?:--customer-id} {status?:--status} {docNumber?:--doc-number}",
        "ledger.invoice.update": "ledger invoice-update --invoice-id {invoiceId} {txnDate?:--txn-date} {dueDate?:--due-date} {docNumber?:--doc-number} {memo?:--memo} {customerMemo?:--customer-memo} {terms?:--terms} {email?:--email}",
        "ledger.invoice.get": "ledger invoice-get {invoiceId}",
        "ledger.invoice.send": "ledger invoice-send {invoiceId} {sendTo?:--send-to}",
    })
}

/// Five open invoices over three pages; an update of invoice 1044 is
/// refused the way QuickBooks refuses one (exit 1, a reason on stderr).
/// Every call is appended to `calls.log` beside the binary.
const SCRIPT: &str = r#"#!/bin/sh
dir=$(cd "$(dirname "$0")" && pwd)
echo "$*" >> "$dir/calls.log"
case "$2" in
  invoice-search)
    cursor=""
    while [ $# -gt 0 ]; do [ "$1" = "--cursor" ] && cursor="$2"; shift; done
    case "$cursor" in
      "") echo '{"count":2,"items":[{"Id":"1041","DueDate":"2026-09-01"},{"Id":"1042","DueDate":"2026-12-01"}],"nextCursor":"3","note":"Full detail: <entity> get <Id>."}' ;;
      3) echo '{"count":2,"items":[{"Id":"1043","DueDate":"2026-09-15"},{"Id":"1044","DueDate":"2026-08-30"}],"nextCursor":"5"}' ;;
      5) echo '{"count":1,"items":[{"Id":"1045","DueDate":"2026-12-20"}]}' ;;
    esac ;;
  invoice-update)
    id=""; due=""
    while [ $# -gt 0 ]; do
      [ "$1" = "--invoice-id" ] && id="$2"
      [ "$1" = "--due-date" ] && due="$2"
      shift
    done
    if [ "$id" = "1044" ]; then echo "QuickBooks: invoice 1044 is closed" >&2; exit 1; fi
    echo "{\"Id\":\"$id\",\"DueDate\":\"$due\"}" ;;
  invoice-get) echo "{\"Id\":\"$3\",\"DueDate\":\"2026-10-31\"}" ;;
  invoice-send) echo "{\"Id\":\"$3\",\"EmailStatus\":\"EmailSent\"}" ;;
esac
"#;

/// The stand-in installed under the owner's plugins, and the operation tools
/// re-derived from what is connected — the plugin install door's last step.
async fn connect_standin(nebo: &Nebo) -> std::path::PathBuf {
    let dir = nebo.home.join("user").join("plugins").join(SLUG).join("0.1.0");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.json"),
        json!({
            "id": SLUG, "slug": SLUG, "name": "Books", "version": "0.1.0", "platforms": {},
            "interfaceBindings": quickbooks_bindings(),
        })
        .to_string(),
    )
    .unwrap();
    let bin = dir.join(SLUG);
    std::fs::write(&bin, SCRIPT).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    nebo.state.tools.refresh_plugin_tools().await;
    dir
}

async fn disconnect_standin(nebo: &Nebo) {
    let _ = std::fs::remove_dir_all(nebo.home.join("user").join("plugins").join(SLUG));
    nebo.state.tools.refresh_plugin_tools().await;
}

fn own_rule(nebo: &Nebo, agent: &str, key: RuleKey, effect: Effect) {
    let rule = Rule {
        id: uuid::Uuid::new_v4().to_string(),
        scope: Scope::Employee(agent.to_string()),
        key,
        field: None,
        effect,
        money: None,
        source: RuleSource::Owner,
        locked: false,
        created_at: 0,
    };
    nebo.store().write_permission_rule(&rule, &Writer::Owner).unwrap();
}

/// FETCH (operation) > MATCH (command, records on stdin) > APPLY (operation,
/// one update per decision) > SEND (operation, one send per decision, through
/// the provider it names: `fake-ledger` binds the send too) > VERIFY
/// (operation, one read per decision). MATCH moves every invoice due before
/// October to October 31.
fn bulk_workflow() -> workflow::parser::WorkflowDef {
    let matcher = "python3 -c 'import json,sys; rs=json.load(sys.stdin); \
        d=[{\"invoiceId\":r[\"Id\"],\"dueDate\":\"2026-10-31\",\"display\":\"Move invoice \"+r[\"Id\"]+\" due date to Oct 31\"} for r in rs if r[\"DueDate\"]<\"2026-10-01\"]; \
        print(json.dumps({\"decisions\":d,\"checks\":[{\"invoiceId\":x[\"invoiceId\"]} for x in d]}))'";
    workflow::parser::parse_workflow(
        &json!({
            "version": "1.0",
            "id": "bulk-proof",
            "name": "Bulk proof",
            "activities": [
                { "id": "fetch", "type": "operation", "params": { "operation": "ledger.invoice.search", "input": { "status": "open" } } },
                { "id": "match", "type": "command", "params": { "command": matcher, "stdin": "nodes.fetch.records" } },
                { "id": "apply", "type": "operation", "params": { "operation": "ledger.invoice.update", "rows": "nodes.match.decisions" } },
                { "id": "send", "type": "operation", "params": { "operation": "ledger.invoice.send", "input": { "provider": SLUG }, "rows": "nodes.match.checks" } },
                { "id": "verify", "type": "operation", "params": { "operation": "ledger.invoice.get", "rows": "nodes.match.checks" } },
            ],
            "connections": [
                { "from": "__trigger__", "to": "fetch" },
                { "from": "fetch", "to": "match" },
                { "from": "match", "to": "apply" },
                { "from": "apply", "to": "send" },
                { "from": "send", "to": "verify" },
                { "from": "verify", "to": "__emit__" },
            ],
        })
        .to_string(),
    )
    .unwrap()
}

/// Run the bulk workflow as `agent` on the production engine with the
/// registry roster a run is given; the run id and each step's output.
async fn run_bulk(nebo: &Nebo, agent: &str) -> (String, std::collections::HashMap<String, Value>) {
    let roster: Vec<Box<dyn tools::registry::DynTool>> = nebo
        .state
        .tools
        .list()
        .await
        .iter()
        .map(|td| Box::new(crate::workflow_manager::RegistryTool::new(td, nebo.state.tools.clone())) as Box<dyn tools::registry::DynTool>)
        .collect();
    let looper = agent::harness::workflow_turn::WorkflowTurns::new(nebo.state.harness.clone());
    let (run_id, _) = workflow::engine::execute_workflow(
        &bulk_workflow(),
        agent,
        "",
        false,
        json!({}),
        "manual",
        None,
        nebo.store(),
        None,
        &looper,
        &roster,
        None,
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        None,
    )
    .await
    .expect("the run completes");
    let outputs = nebo
        .store()
        .completed_activity_contents(&run_id)
        .unwrap()
        .into_iter()
        .map(|((id, _), content)| (id, serde_json::from_str(&content).unwrap_or(Value::String(content))))
        .collect();
    (run_id, outputs)
}

/// `interface-bulk-read-and-batch-write`: the five-step shape, written once
/// against `ledger`, runs on the connected plugin. FETCH follows the
/// plugin's pages to every record; MATCH reads them on stdin; APPLY and SEND
/// are one call per row, each meeting the owner's rules exactly as the seat's
/// own call would — a row the owner wants to be asked about is refused,
/// since nobody waits on a code step, and runs once he allows it — each
/// performed row in the write ledger under a key from the run, and a row the
/// plugin refuses is that row's result; VERIFY reads each one back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bulk_workflow_reads_every_page_and_writes_row_by_row_through_the_connected_plugin() {
    let nebo = session().await;
    let dir = connect_standin(&nebo).await;
    let calls = || std::fs::read_to_string(dir.join("calls.log")).unwrap_or_default();
    // An employee whose job is the ledger, as any ledger seat is hired.
    let name = format!("Books Clerk {}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    let agent = nebo.hire(&name, json!({ "requires": { "interfaces": ["ledger"] }, "workflows": {} })).await;
    own_rule(&nebo, &agent, RuleKey::Capability("shell".into()), Effect::Allow);
    // The owner wants to be asked before invoices go out.
    own_rule(&nebo, &agent, RuleKey::Operation("ledger.invoice.send".into()), Effect::Ask);

    // Every row meets the owner's rules as the seat's own call to the same
    // operation would: the reads and the updates run, and each send, which
    // the owner wants to be asked about, is refused — nobody waits on a code
    // step — before it reaches the plugin or the ledger.
    let (run_id, out) = run_bulk(&nebo, &agent).await;
    let fetch = &out["fetch"];
    assert_eq!(fetch["pages"], 3, "{fetch}");
    let ids: Vec<&str> = fetch["records"].as_array().unwrap().iter().map(|r| r["Id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["1041", "1042", "1043", "1044", "1045"]);
    assert!(calls().contains("ledger invoice-search --status open --cursor 3"), "{}", calls());
    assert_eq!(out["match"]["decisions"].as_array().unwrap().len(), 3, "{}", out["match"]);

    let apply = &out["apply"];
    assert_eq!((apply["succeeded"].as_u64(), apply["failed"].as_u64()), (Some(2), Some(1)), "{apply}");
    assert_eq!(apply["results"][0]["result"], json!({"Id": "1041", "DueDate": "2026-10-31"}));
    assert!(apply["results"][2]["error"].as_str().unwrap().contains("invoice 1044 is closed"), "{apply}");
    let log = calls();
    assert!(
        log.contains("ledger invoice-update --invoice-id 1041 --due-date 2026-10-31"),
        "QuickBooks' binding shapes the call: {log}"
    );
    assert!(!log.contains("clientKey") && !log.contains("--display"), "runtime fields stay with the runtime: {log}");
    let keys: Vec<String> = nebo.store().engine_effects_for_run(&run_id).unwrap().into_iter().map(|e| e.idem_key).collect();
    assert_eq!(keys.len(), 3, "one write-ledger row per update row, none for a refused send: {keys:?}");
    assert!(keys[0].ends_with(&format!("ledger.invoice.update:{run_id}:apply::0")), "{keys:?}");

    let send = &out["send"];
    assert_eq!((send["succeeded"].as_u64(), send["failed"].as_u64()), (Some(0), Some(3)), "{send}");
    assert!(send["results"][0]["error"].as_str().unwrap().contains("needs the owner's OK"), "{send}");
    assert!(!calls().contains("invoice-send"), "a refused row reached the plugin: {}", calls());
    assert_eq!(out["verify"]["succeeded"], 3, "{}", out["verify"]);

    // The owner allows the send: each row goes out, through the named plugin.
    own_rule(&nebo, &agent, RuleKey::Operation("ledger.invoice.send".into()), Effect::Allow);
    let (_, out) = run_bulk(&nebo, &agent).await;
    let send = &out["send"];
    assert_eq!((send["succeeded"].as_u64(), send["failed"].as_u64()), (Some(3), Some(0)), "{send}");
    assert_eq!(send["results"][1]["result"], json!({"Id": "1043", "EmailStatus": "EmailSent"}));
    assert!(calls().contains("ledger invoice-send 1044"), "{}", calls());

    disconnect_standin(&nebo).await;
}
