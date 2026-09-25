//! Every stored shape that names tools, from fixtures, moved once.

use std::path::Path;

use serde_json::Value;
use types::permissions::{Effect, Rule, RuleField, RuleKey, RuleSource, Scope, Writer};

use super::*;

const AGENT_JSON: &str = include_str!("fixtures/agent.json");
const AGENT_MD: &str = include_str!("fixtures/AGENT.md");
const SKILL_MD: &str = include_str!("fixtures/SKILL.md");
const HOOKS: &str = include_str!("fixtures/hooks.yaml");
const BINDING: &str = include_str!("fixtures/binding.json");
const WORKFLOW: &str = include_str!("fixtures/workflow.json");
const CRON: &str = r#"Check the run using work(action="status", resource="engagement-desk", agent="Social Media Manager"). Report the result."#;

fn store(data: &Path) -> db::Store {
    std::fs::create_dir_all(data.join("data")).unwrap();
    db::Store::new(&data.join("data").join("nebo.db").to_string_lossy()).unwrap()
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// No old call is written in `text`.
fn no_old_call(text: &str) {
    let r = rename_map::rewrite_text(text);
    assert!(r.moved.is_empty(), "old calls remain: {:?}", r.moved);
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect()
}

/// Everything the conversion touches, as an install has it before the
/// upgrade: an employee (its row and its package), a binding, a workflow,
/// a scheduled job, an API key, a live run, a queued task, a plugin's
/// skill, a signed manifest, and a project's hooks file.
struct Install {
    _dir: tempfile::TempDir,
    data: std::path::PathBuf,
    project: std::path::PathBuf,
    store: db::Store,
}

fn install() -> Install {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().to_path_buf();
    let store = store(&data);
    let frontmatter: Value = serde_json::from_str(AGENT_JSON).unwrap();
    store
        .create_agent("dm", Some("agent"), "Direct Mail Specialist", "Mails", AGENT_MD, &frontmatter.to_string(), None, None)
        .unwrap();
    store
        .upsert_agent_workflow("dm", "inventory-alerts", "heartbeat", "30m", None, None, None, Some(BINDING.trim()), None, false)
        .unwrap();
    store
        .create_workflow("wf-1", None, "weekly-report", "1.0", WORKFLOW.trim(), Some(r#"Post with team(action: "post", team: "ops", text: "out")"#), None)
        .unwrap();
    store
        .create_cron_job("check-run", "0 * * * * *", "", "agent", Some(CRON), None, None, true, Some("dm"), None)
        .unwrap();
    let grants: Vec<String> = ["agent:memory", "web", "os:calendar", "plugin:shopify", "recall"].map(String::from).to_vec();
    store.create_api_key("key-1", "Front desk", "hash", "nk_1", "dm", &[], &grants).unwrap();
    store
        .engine_create_run(&db::NewRun {
            id: "run-live",
            kind: "workflow",
            session_key: "agent:dm:workflow:run-live",
            agent_id: "dm",
            lane: "main",
            parent_run_id: None,
            definition: Some(WORKFLOW.trim()),
            inputs: None,
            external_ref: None,
        })
        .unwrap();
    store
        .create_pending_task("task-1", "run", "agent:dm:main", None, r#"Search with web(resource: "search", action: "search", query: "rates")"#, None, None, None, 0, None)
        .unwrap();

    write(&data.join("user/agents/Direct Mail Specialist/AGENT.md"), AGENT_MD);
    write(&data.join("user/agents/Direct Mail Specialist/agent.json"), AGENT_JSON);
    write(&data.join("nebo/plugins/gmail/0.1.7/skills/gmail-shared/SKILL.md"), SKILL_MD);
    write(&data.join("nebo/plugins/gmail/0.1.7/manifest.json"), r#"{"usage": "plugin(resource: \"gmail\", action: \"exec\", command: \"doctor\")"}"#);

    let project = data.join("projects/site");
    std::fs::create_dir_all(project.join(".git")).unwrap();
    write(&project.join(".nebo/hooks.yaml"), HOOKS);
    std::fs::create_dir_all(project.join("src")).unwrap();
    let folder = Rule {
        id: "folder-1".into(),
        scope: Scope::Employee("dm".into()),
        key: RuleKey::Capability("file".into()),
        field: Some(RuleField::Folder(project.join("src"))),
        effect: Effect::Allow,
        money: None,
        source: RuleSource::Owner,
        locked: false,
        created_at: 0,
    };
    store.write_permission_rule(&folder, &Writer::Owner).unwrap();
    Install { _dir: dir, data, project, store }
}

#[test]
fn every_stored_shape_moves_onto_the_current_tool_set() {
    let i = install();
    let report = upgrade(&i.store, &i.data).unwrap().expect("the first start converts");
    assert!(!report.changes.is_empty());

    // The employee's row: its instructions and its declared lists.
    let agent = i.store.get_agent("dm").unwrap().unwrap();
    assert!(agent.agent_md.contains(r#"plugin__rentcast(command: "properties search --address \"1 Main St, Springfield\" --radius 1")"#), "{}", agent.agent_md);
    assert!(agent.agent_md.contains(r#"read_skill_file(name: "neighbor-mail-blasts", path: "reference/actions.md")"#), "{}", agent.agent_md);
    assert!(agent.agent_md.contains(r#"remember(key: "mailed/spring", value: "done")"#));
    assert!(agent.agent_md.contains(r#"os(resource: "calendar", action: "today")"#), "os still does the calendar");
    no_old_call(&agent.agent_md);
    let fm: Value = serde_json::from_str(&agent.frontmatter).unwrap();
    let requires = strings(&fm["requires"]["tools"]);
    for name in ["search_web", "fetch_url", "find_plugins", "read_plugin_events", "run_workflow", "workflow_status", "read_file"] {
        assert!(requires.iter().any(|n| n == name), "requires.tools has {name}: {requires:?}");
    }
    assert!(!requires.iter().any(|n| ["web", "plugin", "work"].contains(&n.as_str()) || n.contains(['*', ':'])), "{requires:?}");
    let outreach = strings(&fm["scopes"]["outreach"]["tools"]);
    assert!(outreach.iter().any(|n| n == "send_loop_message") && outreach.iter().any(|n| n == "list_teams"), "{outreach:?}");
    assert_eq!(strings(&fm["app"]["permissions"]), ["network:*", "tool:emit_event"]);

    let send = &fm["workflows"]["mail-blast"]["activities"][0];
    assert_eq!(send["steps"][0], r#"Run plugin__stannp(command: "letters send --campaign spring")"#);
    assert_eq!(send["steps"][1], r#"Close it with update_task(task_id: "t1", status: "completed")"#);
    assert_eq!(strings(&send["requires_tools"]), ["plugin__stannp"], "the successor the step calls");
    let scope = strings(&send["tools"]);
    for name in ["find_plugins", "plugin__stannp", "send_loop_message"] {
        assert!(scope.iter().any(|n| n == name), "the step's scope has {name}: {scope:?}");
    }
    let research = &fm["workflows"]["mail-blast"]["activities"][1];
    assert_eq!(strings(&research["requiresTools"]), ["search_web"], "message names none it calls: dropped");
    assert!(report.unmoved.iter().any(|u| u.before == "message" && u.place.ends_with("requiresTools")));

    // A call-tree intent's grant, both spellings.
    let billing = &fm["workflows"]["front-desk"]["activities"][1]["params"]["tools"];
    assert_eq!(billing, "remember, recall, forget, search_web, os:calendar");
    let booking = strings(&fm["workflows"]["front-desk"]["activities"][2]["params"]["tools"]);
    assert_eq!(
        booking,
        [
            "run_workflow:weekly-report",
            "workflow_status:weekly-report",
            "list_workflow_runs:weekly-report",
            "set_workflow_enabled:weekly-report",
            "read_plugin_events:calendly",
            "plugin__calendly",
        ]
    );

    // The package on disk moves the same way.
    let disk_md = read(&i.data.join("user/agents/Direct Mail Specialist/AGENT.md"));
    assert_eq!(disk_md, agent.agent_md);
    let disk_json: Value = serde_json::from_str(&read(&i.data.join("user/agents/Direct Mail Specialist/agent.json"))).unwrap();
    assert_eq!(disk_json, fm);

    // The binding row (the owner's two workflows).
    let binding = i.store.list_agent_workflows("dm").unwrap().into_iter().find(|b| b.binding_name == "inventory-alerts").unwrap();
    let activities = binding.activities.clone().unwrap();
    assert_eq!(
        activities[0]["steps"][0],
        "plugin__shopify(command: 'products list --store example.myshopify.com') to get all products"
    );
    assert_eq!(activities[1]["steps"][0], "Call it via social_queue_get()");

    // The workflow, its skill, the live run's definition.
    let wf = i.store.get_workflow("wf-1").unwrap().unwrap();
    let def: Value = serde_json::from_str(&wf.definition).unwrap();
    assert_eq!(def["activities"][0]["steps"][0], r#"team_messages(team: "ops")"#);
    assert_eq!(def["activities"][0]["steps"][1], r#"send_loop_message(channel_id: "c1", text: "Weekly report is out")"#);
    assert!(strings(&def["activities"][0]["tools"]).iter().any(|n| n == "team_messages"));
    assert_eq!(wf.skill_md.as_deref(), Some(r#"Post with send_message(to: "ops", message: "out")"#));
    let run = i.store.engine_get_run("run-live").unwrap().unwrap();
    assert_eq!(serde_json::from_str::<Value>(run.definition.as_deref().unwrap()).unwrap(), def);

    // The scheduled job, the API key, the queued task.
    let job = i.store.list_cron_jobs(100, 0).unwrap().into_iter().find(|j| j.name == "check-run").unwrap();
    assert_eq!(
        job.message.as_deref(),
        Some(r#"Check the run using workflow_status(workflow="engagement-desk", employee="Social Media Manager"). Report the result."#)
    );
    let key = i.store.list_api_keys_for_agent("dm").unwrap().into_iter().find(|k| k.id == "key-1").unwrap();
    for name in ["remember", "recall", "forget", "search_web", "fetch_url", "os:calendar", "read_plugin_events:shopify", "plugin__shopify"] {
        assert!(key.tools.iter().any(|t| t == name), "the key grants {name}: {:?}", key.tools);
    }
    assert!(!key.tools.iter().any(|t| ["web", "agent:memory", "plugin:shopify"].contains(&t.as_str())), "{:?}", key.tools);
    assert_eq!(key.tools.iter().filter(|t| *t == "recall").count(), 1, "each grant once");
    let task = i.store.get_pending_task("task-1").unwrap().unwrap();
    assert_eq!(task.prompt, r#"Search with search_web(query: "rates")"#);

    // A plugin's skill: exec, help; `list` has no successor and is reported.
    let skill = read(&i.data.join("nebo/plugins/gmail/0.1.7/skills/gmail-shared/SKILL.md"));
    assert!(skill.contains(r#"plugin__gmail(command: "<service> <action> [flags]")"#), "{skill}");
    assert!(skill.contains(r#"plugin__gmail(command: "drafts --help")"#), "{skill}");
    assert!(skill.contains(r#"plugin(action: "list")"#));
    assert!(report.unmoved.iter().any(|u| u.before == r#"plugin(action: "list")"#));
    // A signed manifest is never touched.
    assert!(read(&i.data.join("nebo/plugins/gmail/0.1.7/manifest.json")).contains("plugin(resource"));

    // The project's hooks file.
    let hooks: serde_yaml::Value = serde_yaml::from_str(&read(&i.project.join(".nebo/hooks.yaml"))).unwrap();
    let post = hooks["post_tool"].as_sequence().unwrap();
    let fmt = &post[0];
    assert_eq!(fmt["tool"], serde_yaml::to_value(["write_file", "edit_file"]).unwrap());
    assert!(fmt.get("resource").is_none() && fmt.get("action").is_none());
    assert_eq!(post[2]["name"], "calendar-log-os", "a kept tool's filters stay on it");
    assert_eq!(post[2]["tool"], serde_yaml::to_value(["os"]).unwrap());
    assert!(post[1]["tool"].as_sequence().unwrap().iter().any(|t| t == "read_file"));
    assert_eq!(post[3]["name"], "untouched");
    assert_eq!(hooks["pre_tool"][0]["tool"], serde_yaml::to_value(["run_command"]).unwrap());

    // Once: the next start converts nothing, and a second pass over what it
    // wrote finds nothing to move.
    assert!(upgrade(&i.store, &i.data).unwrap().is_none());
    let mut again = Report::default();
    for place in db::tool_naming_places() {
        for cell in i.store.tool_naming_cells(place).unwrap() {
            assert_eq!(rewrite_cell(place, &cell.value, place, &mut again), None, "{place}: {}", cell.value);
        }
    }
    assert!(again.changes.is_empty(), "{:?}", again.changes);
}

/// A run parked on the old approval card, with the old call it would run on
/// approval: after the upgrade it waits on an ask for the moved call, and
/// the old card is resolved.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_parked_on_the_old_approval_card_waits_on_an_ask() {
    let dir = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(self::store(dir.path()));
    store.create_agent("dm", Some("agent"), "Direct Mail Specialist", "Mails", "", "{}", None, None).unwrap();
    // An employee that asks before it changes anything: the moved call asks.
    store.set_permission_mode(&Scope::Employee("dm".into()), types::permissions::Mode::Ask).unwrap();
    store
        .engine_create_run(&db::NewRun {
            id: "run-1",
            kind: "workflow",
            session_key: "agent:dm:workflow:run-1",
            agent_id: "dm",
            lane: "main",
            ..Default::default()
        })
        .unwrap();
    let old_call = serde_json::json!({
        "id": "call-1",
        "name": "agent",
        "input": {"resource": "memory", "action": "store", "key": "inventory/cooldown", "value": "7"},
    });
    store
        .create_workflow_suspension("run-1", "dm", "inventory-alerts", "check", "", Some(2), "[]", &old_call.to_string(), "", "Remember the cooldown")
        .unwrap();

    let check = std::sync::Arc::new(agent::Check::new(store.clone()));
    let registry = tools::Registry::new(check.clone());
    registry.register_all(store.clone(), tools::new_handle()).await;
    let resolved = std::sync::Mutex::new(Vec::<String>::new());
    let resolve = |id: &str| resolved.lock().unwrap().push(id.to_string());
    convert_parked_approvals(&store, &registry, check.as_ref(), resolve).await.unwrap();

    let asks = store.open_permission_asks(None).unwrap();
    assert_eq!(asks.len(), 1, "one ask for the parked run");
    assert_eq!(asks[0].run_id.as_deref(), Some("run-1"));
    let call: Value = serde_json::from_str(&asks[0].call).unwrap();
    assert_eq!(call["name"], "remember");
    let parked = store.get_workflow_suspension("run-1").unwrap().unwrap();
    let pending: Value = serde_json::from_str(&parked.6).unwrap();
    assert_eq!(pending["name"], "remember", "the approved run resumes with the current call");
    assert_eq!(pending["input"], serde_json::json!({"key": "inventory/cooldown", "value": "7"}));
    assert_eq!(*resolved.lock().unwrap(), ["wf-approval:run-1"]);

    // Once.
    let resolve_again = |id: &str| panic!("converted twice: {id}");
    convert_parked_approvals(&store, &registry, check.as_ref(), resolve_again).await.unwrap();
    assert_eq!(store.open_permission_asks(None).unwrap().len(), 1);
}
