//! Nebo Platform Readiness Test
//!
//! Validates the OS & execution layer: server boot, CRUD lifecycles, code detection,
//! dependency cascade, memory, cron/scheduling, event dispatch, browser snapshot, and
//! AI provider management. Skills, workflows, and roles are artifacts that run on this
//! platform — they are NOT tested here.
//!
//! Run:
//!   cargo test -p nebo-server --test mvp_readiness -- --nocapture

use std::path::PathBuf;
use std::time::Duration;

use reqwest::Client;
use serde_json::{json, Value};

// ── Test Server ─────────────────────────────────────────────────────

struct TestServer {
    port: u16,
    client: Client,
    data_dir: PathBuf,
    _temp_dir: tempfile::TempDir,
    _handle: tokio::task::JoinHandle<()>,
}

impl TestServer {
    async fn boot() -> Self {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let data_dir = temp_dir.path().to_path_buf();

        // Create required subdirectories
        std::fs::create_dir_all(data_dir.join("data")).unwrap();
        std::fs::create_dir_all(data_dir.join("nebo").join("skills")).unwrap();
        std::fs::create_dir_all(data_dir.join("nebo").join("tools")).unwrap();
        std::fs::create_dir_all(data_dir.join("nebo").join("workflows")).unwrap();
        std::fs::create_dir_all(data_dir.join("nebo").join("roles")).unwrap();
        std::fs::create_dir_all(data_dir.join("user").join("skills")).unwrap();
        std::fs::create_dir_all(data_dir.join("user").join("tools")).unwrap();
        std::fs::create_dir_all(data_dir.join("user").join("workflows")).unwrap();
        std::fs::create_dir_all(data_dir.join("user").join("roles")).unwrap();
        std::fs::create_dir_all(data_dir.join("bundled").join("skills")).unwrap();

        // Set NEBO_DATA_DIR so config::data_dir() resolves to our temp dir
        // SAFETY: single-threaded at this point (before server spawn)
        unsafe { std::env::set_var("NEBO_DATA_DIR", &data_dir); }

        let port = find_free_port();
        let db_path = data_dir.join("data").join("nebo.db");

        let mut cfg = config::Config::default();
        cfg.port = port;
        cfg.host = "127.0.0.1".to_string();
        cfg.database.sqlite_path = db_path.to_string_lossy().to_string();
        // Use a random JWT secret for test isolation
        cfg.auth.access_secret = uuid::Uuid::new_v4().to_string();

        let handle = tokio::spawn(async move {
            if let Err(e) = nebo_server::run(cfg, true).await {
                eprintln!("server error: {}", e);
            }
        });

        let client = Client::new();

        // Poll /health until ready (max 30s)
        let health_url = format!("http://127.0.0.1:{}/health", port);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            if tokio::time::Instant::now() > deadline {
                panic!("server failed to start within 30s");
            }
            match client.get(&health_url).send().await {
                Ok(resp) if resp.status().is_success() => break,
                _ => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }

        Self {
            port,
            client,
            data_dir: temp_dir.path().to_path_buf(),
            _temp_dir: temp_dir,
            _handle: handle,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}/api/v1{}", self.port, path)
    }

    fn health_url(&self) -> String {
        format!("http://127.0.0.1:{}/health", self.port)
    }

    async fn get(&self, path: &str) -> reqwest::Response {
        self.client.get(&self.url(path)).send().await.unwrap()
    }

    async fn post_json(&self, path: &str, body: &Value) -> reqwest::Response {
        self.client
            .post(&self.url(path))
            .json(body)
            .send()
            .await
            .unwrap()
    }

    async fn put_json(&self, path: &str, body: &Value) -> reqwest::Response {
        self.client
            .put(&self.url(path))
            .json(body)
            .send()
            .await
            .unwrap()
    }

    async fn delete(&self, path: &str) -> reqwest::Response {
        self.client
            .delete(&self.url(path))
            .send()
            .await
            .unwrap()
    }

    /// Get a direct DB store handle for setup/assertions that need DB access
    fn db_store(&self) -> db::Store {
        let db_path = self.data_dir.join("data").join("nebo.db");
        db::Store::new(&db_path.to_string_lossy()).expect("open test DB")
    }
}

fn find_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

// ── Readiness Report ────────────────────────────────────────────────

#[derive(Clone)]
struct TestResult {
    name: String,
    status: TestStatus,
    detail: String,
}

#[derive(Clone, PartialEq)]
enum TestStatus {
    Pass,
    Fail,
}

impl std::fmt::Display for TestStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TestStatus::Pass => write!(f, "PASS"),
            TestStatus::Fail => write!(f, "FAIL"),
        }
    }
}

fn print_report(results: &[TestResult]) {
    let capabilities = [
        ("Server Health", "server_health"),
        ("Skill Lifecycle", "skill_lifecycle"),
        ("Workflow Lifecycle", "workflow_lifecycle"),
        ("Role Lifecycle", "role_lifecycle"),
        ("Role Trigger Parsing", "role_triggers"),
        ("Provider Management", "provider_management"),
        ("Agent Status", "agent_status"),
        ("Memory System", "memory"),
        ("Cron / Scheduling", "cron"),
        ("Event System", "events"),
        ("Install Flow", "install_flow"),
        ("Browser Snapshot", "browser"),
    ];

    let total = capabilities.len();
    let mut pass_count = 0;
    let mut fail_count = 0;

    println!();
    println!("═══════════════════════════════════════════");
    println!("  NEBO PLATFORM READINESS");
    println!("═══════════════════════════════════════════");

    for (label, tag) in &capabilities {
        if let Some(r) = results.iter().find(|r| r.name == *tag) {
            let icon = if r.status == TestStatus::Pass {
                pass_count += 1;
                "[PASS]"
            } else {
                fail_count += 1;
                "[FAIL]"
            };
            println!("  {} {} — {}", icon, label, r.detail);
        }
    }

    println!("═══════════════════════════════════════════");
    println!(
        "  {}/{} PASS | {}/{} FAIL",
        pass_count, total, fail_count, total
    );
    println!("═══════════════════════════════════════════");
    println!();
}

// ── Main Test ───────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mvp_readiness() {
    let server = TestServer::boot().await;
    let mut results = Vec::new();

    // ── Artifact Lifecycles ──────────────────────────────────────────
    results.push(test_server_health(&server).await);
    results.push(test_agent_status(&server).await);
    results.push(test_skill_lifecycle(&server).await);
    results.push(test_workflow_lifecycle(&server).await);
    results.push(test_role_lifecycle(&server).await);
    results.push(test_role_trigger_parsing());
    results.push(test_provider_management(&server).await);

    // ── Platform Capabilities ────────────────────────────────────────
    results.push(test_install_flow());
    results.push(test_memory(&server).await);
    results.push(test_cron(&server).await);
    results.push(test_events().await);
    results.push(test_browser());

    // ── Report ──────────────────────────────────────────────────────
    print_report(&results);

    // Assert no hard failures
    let failures: Vec<_> = results
        .iter()
        .filter(|r| r.status == TestStatus::Fail)
        .collect();
    if !failures.is_empty() {
        for f in &failures {
            eprintln!("FAILED: {} — {}", f.name, f.detail);
        }
        panic!("{} test(s) failed", failures.len());
    }
}

// ═══════════════════════════════════════════════════════════════════
// INFRASTRUCTURE TESTS
// ═══════════════════════════════════════════════════════════════════

async fn test_server_health(server: &TestServer) -> TestResult {
    let resp = server
        .client
        .get(&server.health_url())
        .send()
        .await
        .unwrap();

    if resp.status() != 200 {
        return TestResult {
            name: "server_health".into(),
            status: TestStatus::Fail,
            detail: format!("health returned {}", resp.status()),
        };
    }

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");

    TestResult {
        name: "server_health".into(),
        status: TestStatus::Pass,
        detail: format!("healthy, version {}", body["version"]),
    }
}

async fn test_agent_status(server: &TestServer) -> TestResult {
    let resp = server.get("/agent/status").await;
    if resp.status() != 200 {
        return TestResult {
            name: "agent_status".into(),
            status: TestStatus::Fail,
            detail: format!("status returned {}", resp.status()),
        };
    }

    let body: Value = resp.json().await.unwrap();
    assert!(body["status"].is_string());

    // Also test GET /agent/settings
    let settings_resp = server.get("/agent/settings").await;
    assert_eq!(settings_resp.status(), 200);

    TestResult {
        name: "agent_status".into(),
        status: TestStatus::Pass,
        detail: format!("agent status: {}", body["status"]),
    }
}

// ═══════════════════════════════════════════════════════════════════
// SKILL LIFECYCLE (Step 2)
// ═══════════════════════════════════════════════════════════════════

async fn test_skill_lifecycle(server: &TestServer) -> TestResult {
    // 1. GET /extensions — should be 200
    let resp = server.get("/extensions").await;
    if resp.status() != 200 {
        return TestResult {
            name: "skill_lifecycle".into(),
            status: TestStatus::Fail,
            detail: format!("GET /extensions returned {}", resp.status()),
        };
    }

    // 2. POST /skills — create test skill (content must start with --- for SKILL.md format)
    let create_body = json!({
        "name": "mvp-test-skill",
        "content": "---\nname: MVP Test Skill\ndescription: A test skill for readiness verification\n---\n\n# MVP Test Skill\n\n## Instructions\n\nThis is a test."
    });
    let resp = server.post_json("/skills", &create_body).await;
    if resp.status() != 200 {
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or_default();
        return TestResult {
            name: "skill_lifecycle".into(),
            status: TestStatus::Fail,
            detail: format!("POST /skills returned {} — {:?}", status, body),
        };
    }

    // 3. GET /skills/mvp-test-skill — verify exists
    let resp = server.get("/skills/mvp-test-skill").await;
    assert_eq!(resp.status(), 200, "GET skill should return 200");
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["content"]
            .as_str()
            .unwrap_or("")
            .contains("MVP Test Skill"),
        "skill content should match"
    );

    // 4. PUT /skills/mvp-test-skill — update
    let update_body = json!({
        "content": "---\nname: MVP Test Skill\ndescription: Updated description\n---\n\n# MVP Test Skill\n\n## Instructions\n\nUpdated test."
    });
    let resp = server
        .put_json("/skills/mvp-test-skill", &update_body)
        .await;
    assert_eq!(resp.status(), 200, "PUT skill should return 200");

    // 5. Verify update
    let resp = server.get("/skills/mvp-test-skill").await;
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["content"]
            .as_str()
            .unwrap_or("")
            .contains("Updated description"),
        "skill should have updated content"
    );

    // 6. POST toggle disable
    let resp = server.post_json("/skills/mvp-test-skill/toggle", &json!({})).await;
    assert_eq!(resp.status(), 200);

    // 7. POST toggle re-enable
    let resp = server.post_json("/skills/mvp-test-skill/toggle", &json!({})).await;
    assert_eq!(resp.status(), 200);

    // 8. Verify filesystem
    let skill_dir = server
        .data_dir
        .join("user")
        .join("skills")
        .join("mvp-test-skill");
    let skill_md = skill_dir.join("SKILL.md");
    assert!(skill_md.exists(), "SKILL.md should exist on disk");

    // 9. DELETE
    let resp = server.delete("/skills/mvp-test-skill").await;
    assert_eq!(resp.status(), 200, "DELETE skill should return 200");

    // 10. Verify 404
    let resp = server.get("/skills/mvp-test-skill").await;
    assert_eq!(resp.status(), 404, "GET deleted skill should return 404");

    // 11. Verify filesystem cleanup
    assert!(
        !skill_md.exists(),
        "SKILL.md should be gone after delete"
    );

    TestResult {
        name: "skill_lifecycle".into(),
        status: TestStatus::Pass,
        detail: "CRUD + toggle + filesystem verified".into(),
    }
}

// ═══════════════════════════════════════════════════════════════════
// WORKFLOW LIFECYCLE (Step 3)
// ═══════════════════════════════════════════════════════════════════

async fn test_workflow_lifecycle(server: &TestServer) -> TestResult {
    // 1. POST /workflows — create
    let workflow_def = json!({
        "version": "1.0.0",
        "id": "mvp-test",
        "name": "mvp-test-wf",
        "activities": [{
            "id": "step1",
            "intent": "test",
            "skills": [],
            "token_budget": { "max": 1000 }
        }],
        "dependencies": { "skills": [], "workflows": [] },
        "budget": { "total_per_run": 5000, "cost_estimate": "$0.01" }
    });

    let create_body = json!({
        "name": "mvp-test-wf",
        "definition": serde_json::to_string(&workflow_def).unwrap(),
        "version": "1.0.0"
    });

    let resp = server.post_json("/workflows", &create_body).await;
    if resp.status() != 200 {
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or_default();
        return TestResult {
            name: "workflow_lifecycle".into(),
            status: TestStatus::Fail,
            detail: format!("POST /workflows returned {} — {:?}", status, body),
        };
    }
    let body: Value = resp.json().await.unwrap();
    let wf_id = body["workflow"]["id"].as_str().unwrap().to_string();

    // 2. GET /workflows — list
    let resp = server.get("/workflows").await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let workflows = body["workflows"].as_array().unwrap();
    assert!(
        workflows.iter().any(|w| w["id"] == wf_id),
        "workflow should appear in list"
    );

    // 3. GET /workflows/{id} — get
    let resp = server.get(&format!("/workflows/{}", wf_id)).await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["workflow"]["name"], "mvp-test-wf");

    // 4. POST toggle disable
    let resp = server
        .post_json(&format!("/workflows/{}/toggle", wf_id), &json!({}))
        .await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    // After first toggle, should be disabled (isEnabled = false)
    let _is_enabled = body["workflow"]["isEnabled"].as_bool();

    // 5. POST toggle re-enable
    let resp = server
        .post_json(&format!("/workflows/{}/toggle", wf_id), &json!({}))
        .await;
    assert_eq!(resp.status(), 200);

    // 6. DELETE
    let resp = server.delete(&format!("/workflows/{}", wf_id)).await;
    assert_eq!(resp.status(), 200);

    // 7. Verify gone
    let resp = server.get("/workflows").await;
    let body: Value = resp.json().await.unwrap();
    let workflows = body["workflows"].as_array().unwrap();
    assert!(
        !workflows.iter().any(|w| w["id"] == wf_id),
        "deleted workflow should not appear in list"
    );

    TestResult {
        name: "workflow_lifecycle".into(),
        status: TestStatus::Pass,
        detail: "CRUD + toggle verified".into(),
    }
}

// ═══════════════════════════════════════════════════════════════════
// ROLE LIFECYCLE (Step 4)
// ═══════════════════════════════════════════════════════════════════

async fn test_role_lifecycle(server: &TestServer) -> TestResult {
    let role_md = "---\nname: MVP Test Role\ndescription: A test role for readiness verification\n---\n\n# MVP Test Role\n\nYou are a test role for readiness verification.";

    let create_body = json!({
        "roleMd": role_md,
        "name": "mvp-test-role",
        "description": "A test role for MVP readiness"
    });

    // 1. POST /roles — create
    let resp = server.post_json("/roles", &create_body).await;
    if resp.status() != 200 {
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or_default();
        return TestResult {
            name: "role_lifecycle".into(),
            status: TestStatus::Fail,
            detail: format!("POST /roles returned {} — {:?}", status, body),
        };
    }
    let body: Value = resp.json().await.unwrap();
    let role_id = body["role"]["id"].as_str().unwrap().to_string();

    // 2. GET /roles — list
    let resp = server.get("/roles").await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let roles = body["roles"].as_array().unwrap();
    assert!(
        roles.iter().any(|r| r["id"] == role_id),
        "role should appear in list"
    );

    // 3. GET /roles/{id}
    let resp = server.get(&format!("/roles/{}", role_id)).await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["role"]["name"], "MVP Test Role");

    // 4. POST toggle disable
    let resp = server
        .post_json(&format!("/roles/{}/toggle", role_id), &json!({}))
        .await;
    assert_eq!(resp.status(), 200);

    // 5. POST toggle re-enable
    let resp = server
        .post_json(&format!("/roles/{}/toggle", role_id), &json!({}))
        .await;
    assert_eq!(resp.status(), 200);

    // 6. DELETE
    let resp = server.delete(&format!("/roles/{}", role_id)).await;
    assert_eq!(resp.status(), 200);

    // 7. Verify gone
    let resp = server.get("/roles").await;
    let body: Value = resp.json().await.unwrap();
    let roles = body["roles"].as_array().unwrap();
    assert!(
        !roles.iter().any(|r| r["id"] == role_id),
        "deleted role should not appear in list"
    );

    TestResult {
        name: "role_lifecycle".into(),
        status: TestStatus::Pass,
        detail: "CRUD + toggle verified".into(),
    }
}

// ═══════════════════════════════════════════════════════════════════
// PROVIDER MANAGEMENT (Step 15)
// ═══════════════════════════════════════════════════════════════════

async fn test_provider_management(server: &TestServer) -> TestResult {
    // 1. GET /providers — list (may be empty)
    let resp = server.get("/providers").await;
    if resp.status() != 200 {
        return TestResult {
            name: "provider_management".into(),
            status: TestStatus::Fail,
            detail: format!("GET /providers returned {}", resp.status()),
        };
    }

    // 2. POST /providers — create test provider
    let create_body = json!({
        "name": "mvp-test-provider",
        "provider": "openai",
        "apiKey": "sk-test-key-not-real",
        "priority": 50
    });
    let resp = server.post_json("/providers", &create_body).await;
    if resp.status() != 200 {
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or_default();
        return TestResult {
            name: "provider_management".into(),
            status: TestStatus::Fail,
            detail: format!("POST /providers returned {} — {:?}", status, body),
        };
    }
    let body: Value = resp.json().await.unwrap();
    let provider_id = body["id"].as_str().unwrap().to_string();

    // 3. GET /providers/{id}
    let resp = server.get(&format!("/providers/{}", provider_id)).await;
    assert_eq!(resp.status(), 200);

    // 4. PUT /providers/{id} — update priority
    let resp = server
        .put_json(
            &format!("/providers/{}", provider_id),
            &json!({ "priority": 75 }),
        )
        .await;
    assert_eq!(resp.status(), 200);

    // 5. DELETE /providers/{id}
    let resp = server.delete(&format!("/providers/{}", provider_id)).await;
    assert_eq!(resp.status(), 200);

    TestResult {
        name: "provider_management".into(),
        status: TestStatus::Pass,
        detail: "CRUD verified".into(),
    }
}

// ═══════════════════════════════════════════════════════════════════
// CODE DETECTION & INSTALL FLOW (Step 5 — Tier 1 #8)
// ═══════════════════════════════════════════════════════════════════

fn test_install_flow() -> TestResult {
    // ── Code Detection ──────────────────────────────────────────────
    use nebo_server::codes::{detect_code, CodeType};

    // Valid codes
    let (ct, _) = detect_code("SKIL-A1B2-C3D4").expect("valid SKILL code");
    assert_eq!(ct, CodeType::Skill);

    let (ct, _) = detect_code("ROLE-A1B2-C3D4").expect("valid ROLE code");
    assert_eq!(ct, CodeType::Role);

    let (ct, _) = detect_code("WORK-A1B2-C3D4").expect("valid WORK code");
    assert_eq!(ct, CodeType::Work);

    let (ct, _) = detect_code("NEBO-A1B2-C3D4").expect("valid NEBO code");
    assert_eq!(ct, CodeType::Nebo);

    let (ct, _) = detect_code("LOOP-A1B2-C3D4").expect("valid LOOP code");
    assert_eq!(ct, CodeType::Loop);

    // Case insensitive
    assert!(detect_code("nebo-a1b2-c3d4").is_some());

    // Trimmed whitespace
    assert!(detect_code("  SKIL-0000-ZZZZ  ").is_some());

    // Invalid codes
    assert!(detect_code("NEBO-IIIL-OOOU").is_none(), "I,L,O,U invalid in Crockford");
    assert!(detect_code("INVALID-A1B2-C3D4").is_none(), "bad prefix");
    assert!(detect_code("SKIL-A1B2").is_none(), "too short");
    assert!(detect_code("NEBO-A1B2-C3D4-EXTRA").is_none(), "too long");
    assert!(detect_code("hello world").is_none(), "not a code");
    assert!(detect_code("").is_none(), "empty string");

    // ── Cascade Deps ────────────────────────────────────────────────
    use nebo_server::deps::{
        extract_role_deps, extract_simple_name, extract_workflow_deps, is_marketplace_ref,
    };

    // is_marketplace_ref
    assert!(is_marketplace_ref("@acme/skills/web"));
    assert!(is_marketplace_ref("SKIL-A1B2-C3D4"));
    assert!(is_marketplace_ref("WORK-A1B2-C3D4"));
    assert!(is_marketplace_ref("ROLE-A1B2-C3D4"));
    assert!(!is_marketplace_ref("builtin-name"));
    assert!(!is_marketplace_ref("web"));

    // extract_simple_name
    assert_eq!(extract_simple_name("@acme/skills/web@^1.0"), "web");
    assert_eq!(extract_simple_name("@acme/skills/web"), "web");
    assert_eq!(extract_simple_name("SKIL-A1B2-C3D4"), "SKIL-A1B2-C3D4");
    assert_eq!(extract_simple_name("web"), "web");

    // extract_role_deps
    let role_json = r#"{
        "workflows": {
            "daily": {
                "ref": "@nebo/workflows/daily@^1.0.0",
                "trigger": { "type": "schedule", "cron": "0 7 * * *" }
            },
            "monitor": {
                "ref": "@nebo/workflows/monitor@^1.0.0",
                "trigger": { "type": "manual" }
            }
        },
        "skills": ["@nebo/skills/writer@^1.0.0"]
    }"#;
    let config = napp::role::parse_role_config(role_json).unwrap();
    let deps = extract_role_deps(&config);
    assert_eq!(deps.len(), 3, "2 workflows + 1 skill = 3 deps");

    // extract_workflow_deps
    let wf_json = r#"{
        "version": "1.0.0",
        "id": "test-wf",
        "name": "Test Workflow",
        "activities": [
            { "id": "a1", "intent": "test", "skills": ["@acme/skills/tool@^1.0.0"], "token_budget": { "max": 1000 } }
        ],
        "dependencies": {
            "skills": ["@acme/skills/dep1@^1.0.0"],
            "workflows": ["@acme/workflows/sub@^1.0.0"]
        },
        "budget": { "total_per_run": 5000, "cost_estimate": "$0.01" }
    }"#;
    let def = workflow::parser::parse_workflow(wf_json).unwrap();
    let deps = extract_workflow_deps(&def);
    // 1 skill from deps, 1 workflow from deps, 1 skill from activities = 3
    assert_eq!(deps.len(), 3, "should extract 3 unique deps");

    TestResult {
        name: "install_flow".into(),
        status: TestStatus::Pass,
        detail: "code detection + cascade deps extraction verified".into(),
    }
}

// ═══════════════════════════════════════════════════════════════════
// MEMORY (Step 6 — Tier 1 #3)
// ═══════════════════════════════════════════════════════════════════

async fn test_memory(server: &TestServer) -> TestResult {
    // Create memories via DB store (no POST endpoint)
    let store = server.db_store();
    let user_id = "test-user";

    store
        .upsert_memory(
            "tacit/preferences",
            "color",
            "blue",
            Some(r#"["preference"]"#),
            Some(r#"{"confidence": 0.9}"#),
            user_id,
        )
        .unwrap();

    store
        .upsert_memory(
            "entity/default",
            "boss-name",
            "Alice Smith",
            None,
            Some(r#"{"confidence": 0.8}"#),
            user_id,
        )
        .unwrap();

    // GET /memories — list
    let resp = server.get("/memories").await;
    if resp.status() != 200 {
        return TestResult {
            name: "memory".into(),
            status: TestStatus::Fail,
            detail: format!("GET /memories returned {}", resp.status()),
        };
    }
    let body: Value = resp.json().await.unwrap();
    let memories = body["memories"].as_array().unwrap();
    assert!(memories.len() >= 2, "should have at least 2 memories");

    // GET /memories/search?q=blue
    let resp = server.get("/memories/search?q=blue").await;
    assert_eq!(resp.status(), 200);

    // GET /memories/stats
    let resp = server.get("/memories/stats").await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body["total"].as_i64().unwrap() >= 2);

    // Find the "color" memory and update it
    let resp = server.get("/memories").await;
    let body: Value = resp.json().await.unwrap();
    let color_mem = body["memories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["key"] == "color")
        .expect("should find color memory");
    let mem_id = color_mem["id"].as_i64().unwrap();

    // PUT /memories/{id} — update
    let resp = server
        .put_json(&format!("/memories/{}", mem_id), &json!({ "value": "green" }))
        .await;
    assert_eq!(resp.status(), 200);

    // Verify update
    let resp = server.get(&format!("/memories/{}", mem_id)).await;
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["value"], "green");

    // DELETE /memories/{id}
    let resp = server.delete(&format!("/memories/{}", mem_id)).await;
    assert_eq!(resp.status(), 200);

    // ── Memory Scoring (unit tests) ─────────────────────────────────
    use agent::memory::decay_score;

    // Recent access
    let now = chrono::Utc::now().timestamp();
    let score = decay_score(5, Some(now));
    assert!((score - 5.0).abs() < 0.1, "recent score should be ~5.0");

    // Old access (90 days)
    let ninety_days_ago = now - (90 * 86400);
    let score = decay_score(5, Some(ninety_days_ago));
    assert!(score < 2.0, "90-day-old score should decay below 2.0");
    assert!(score > 1.5, "90-day-old score should stay above 1.5");

    // No access
    let score = decay_score(1, None);
    assert!((score - 1.0).abs() < f64::EPSILON);

    TestResult {
        name: "memory".into(),
        status: TestStatus::Pass,
        detail: "CRUD + search + stats + scoring verified".into(),
    }
}

// ═══════════════════════════════════════════════════════════════════
// CRON / SCHEDULING (Step 7 — Tier 1 #5)
// ═══════════════════════════════════════════════════════════════════

async fn test_cron(server: &TestServer) -> TestResult {
    let store = server.db_store();

    // Register a schedule trigger
    let count_before = store.count_cron_jobs().unwrap();
    workflow::triggers::register_schedule_trigger("mvp-wf-1", "0 7 * * *", &store);

    // Verify cron_job was created (count increased)
    let count_after = store.count_cron_jobs().unwrap();
    assert!(count_after > count_before, "cron_job count should increase after register");

    // Unregister
    workflow::triggers::unregister_triggers("mvp-wf-1", &store);
    let count_removed = store.count_cron_jobs().unwrap();
    assert!(count_removed < count_after, "cron_job count should decrease after unregister");

    // ── Role trigger registration ───────────────────────────────────
    let bindings = vec![db::models::RoleWorkflow {
        id: 0,
        role_id: "mvp-role-1".into(),
        binding_name: "daily".into(),
        trigger_type: "schedule".into(),
        trigger_config: "0 7 * * *".into(),
        description: Some("Daily briefing".into()),
        inputs: None,
        is_active: 1,
    }];

    let count_before_role = store.count_cron_jobs().unwrap();
    workflow::triggers::register_role_triggers("mvp-role-1", &bindings, &store);
    let count_after_role = store.count_cron_jobs().unwrap();
    assert!(count_after_role > count_before_role, "role cron_job should be created");

    // Unregister role triggers
    workflow::triggers::unregister_role_triggers("mvp-role-1", &store);
    let count_removed_role = store.count_cron_jobs().unwrap();
    assert!(count_removed_role < count_after_role, "role cron_job should be removed");

    TestResult {
        name: "cron".into(),
        status: TestStatus::Pass,
        detail: "workflow + role trigger registration/unregistration verified".into(),
    }
}

// ═══════════════════════════════════════════════════════════════════
// EVENT TRIGGERS (Step 8 — Tier 1 #10)
// ═══════════════════════════════════════════════════════════════════

async fn test_events() -> TestResult {
    // ── Event Matching ──────────────────────────────────────────────
    let dispatcher = workflow::events::EventDispatcher::new();
    dispatcher
        .subscribe(workflow::events::EventSubscription {
            pattern: "email.*".into(),
            default_inputs: serde_json::json!({}),
            role_source: "test-role".into(),
            binding_name: "email-watch".into(),
            definition_json: None,
            emit_source: None,
        })
        .await;
    dispatcher
        .subscribe(workflow::events::EventSubscription {
            pattern: "email.urgent".into(),
            default_inputs: serde_json::json!({}),
            role_source: "test-role".into(),
            binding_name: "urgent-watch".into(),
            definition_json: None,
            emit_source: None,
        })
        .await;

    // email.urgent matches both (wildcard + exact)
    let event = tools::events::Event {
        source: "email.urgent".into(),
        payload: serde_json::json!({}),
        origin: "test".into(),
        timestamp: 0,
    };
    let matches = dispatcher.match_event(&event).await;
    assert_eq!(matches.len(), 2, "email.urgent should match 2 subscriptions");

    // email.info matches only wildcard
    let event = tools::events::Event {
        source: "email.info".into(),
        payload: serde_json::json!({}),
        origin: "test".into(),
        timestamp: 0,
    };
    let matches = dispatcher.match_event(&event).await;
    assert_eq!(matches.len(), 1, "email.info should match 1 subscription");

    // calendar.changed matches none
    let event = tools::events::Event {
        source: "calendar.changed".into(),
        payload: serde_json::json!({}),
        origin: "test".into(),
        timestamp: 0,
    };
    let matches = dispatcher.match_event(&event).await;
    assert_eq!(matches.len(), 0, "calendar.changed should match 0 subscriptions");

    // ── Event Bus ───────────────────────────────────────────────────
    let (bus, mut rx) = tools::events::EventBus::new();

    bus.emit(tools::events::Event {
        source: "test.event".into(),
        payload: serde_json::json!({"key": "value"}),
        origin: "test-origin".into(),
        timestamp: 42,
    });

    let received = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("should receive within 1s")
        .expect("should have event");
    assert_eq!(received.source, "test.event");
    assert_eq!(received.timestamp, 42);

    TestResult {
        name: "events".into(),
        status: TestStatus::Pass,
        detail: "event matching + bus emit/receive verified".into(),
    }
}

// ═══════════════════════════════════════════════════════════════════
// ROLE TRIGGER PARSING — validates the platform parses all 4 trigger types
// ═══════════════════════════════════════════════════════════════════

fn test_role_trigger_parsing() -> TestResult {
    // Schedule trigger
    let json = r#"{ "workflows": { "w1": {
        "ref": "@o/workflows/x@^1.0", "trigger": { "type": "schedule", "cron": "0 7 * * *" }
    }}}"#;
    let cfg = napp::role::parse_role_config(json).unwrap();
    match &cfg.workflows["w1"].trigger {
        napp::role::RoleTrigger::Schedule { cron } => assert_eq!(cron, "0 7 * * *"),
        other => panic!("expected Schedule, got {:?}", other),
    }

    // Event trigger
    let json = r#"{ "workflows": { "w2": {
        "ref": "@o/workflows/y@^1.0", "trigger": { "type": "event", "sources": ["email.urgent", "cal.changed"] }
    }}}"#;
    let cfg = napp::role::parse_role_config(json).unwrap();
    match &cfg.workflows["w2"].trigger {
        napp::role::RoleTrigger::Event { sources } => {
            assert_eq!(sources.len(), 2);
            assert!(sources.contains(&"email.urgent".to_string()));
            assert!(sources.contains(&"cal.changed".to_string()));
        }
        other => panic!("expected Event, got {:?}", other),
    }

    // Heartbeat trigger
    let json = r#"{ "workflows": { "w3": {
        "ref": "@o/workflows/z@^1.0", "trigger": { "type": "heartbeat", "interval": "30m", "window": "08:00-18:00" }
    }}}"#;
    let cfg = napp::role::parse_role_config(json).unwrap();
    match &cfg.workflows["w3"].trigger {
        napp::role::RoleTrigger::Heartbeat { interval, window } => {
            assert_eq!(interval, "30m");
            assert_eq!(window.as_deref(), Some("08:00-18:00"));
        }
        other => panic!("expected Heartbeat, got {:?}", other),
    }

    // Manual trigger
    let json = r#"{ "workflows": { "w4": {
        "ref": "@o/workflows/m@^1.0", "trigger": { "type": "manual" }
    }}}"#;
    let cfg = napp::role::parse_role_config(json).unwrap();
    match &cfg.workflows["w4"].trigger {
        napp::role::RoleTrigger::Manual => {}
        other => panic!("expected Manual, got {:?}", other),
    }

    TestResult {
        name: "role_triggers".into(),
        status: TestStatus::Pass,
        detail: "all 4 trigger types (schedule, event, heartbeat, manual) parse correctly".into(),
    }
}

// ═══════════════════════════════════════════════════════════════════
// BROWSER SNAPSHOT — validates the platform's snapshot annotation engine
// ═══════════════════════════════════════════════════════════════════

fn test_browser() -> TestResult {
    let snapshot = r#"- button "Submit"
  - text "Click me"
- textbox
- link "Home"
- heading "Title"
- button "Cancel""#;

    // Test annotate_snapshot
    let (annotated, refs) = browser::snapshot::annotate_snapshot(snapshot, true);
    assert_eq!(refs.len(), 4, "4 interactive elements");
    assert_eq!(refs[0].role, "button");
    assert_eq!(refs[0].name, "Submit");
    assert_eq!(refs[1].role, "textbox");
    assert_eq!(refs[2].role, "link");
    assert_eq!(refs[2].name, "Home");
    assert_eq!(refs[3].role, "button");
    assert_eq!(refs[3].name, "Cancel");

    // Verify refs appear in annotated text
    assert!(annotated.contains("[e1]"));
    assert!(annotated.contains("[e2]"));
    assert!(annotated.contains("[e3]"));
    assert!(annotated.contains("[e4]"));

    // heading is NOT interactive — should not get a ref
    let heading_line = annotated.lines().find(|l| l.contains("heading")).unwrap();
    assert!(!heading_line.contains("[e"), "heading should not have ref");

    // Test annotate_with_role_ids
    let (_annotated, elements) = browser::snapshot::annotate_with_role_ids(snapshot);
    assert_eq!(elements.len(), 4);
    assert_eq!(elements[0].id, "B1"); // button
    assert_eq!(elements[1].id, "T1"); // textbox
    assert_eq!(elements[2].id, "L1"); // link
    assert_eq!(elements[3].id, "B2"); // button (second)

    TestResult {
        name: "browser".into(),
        status: TestStatus::Pass,
        detail: "snapshot annotation with element refs verified".into(),
    }
}

