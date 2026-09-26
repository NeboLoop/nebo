//! The app-sidecar proof: every way an app's sidecar can stop serving, each as
//! one deterministic scenario with a REAL process — the test sidecar
//! (`napp::test_sidecar`), killed, crashed, starved and removed — run by the
//! same `AppLifecycle` the proxy serves through, over a throwaway Nebo root.
//! Requests go through `AppLifecycle::serve`, the proxy's own path, to a
//! stand-in sidecar server in this process that the test sidecar relays to.
//!
//! Every scenario is listed as a fixture in `fixtures/app-sidecars/` and in
//! `suites/app-sidecars.yaml`; `make test-sidecar-proof` runs the suite through
//! `nebo-cli test run`, and `cargo test -p nebo-server --lib` runs the same
//! tests directly (CI's workspace run includes them).

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use napp::supervisor::{RestartPolicy, SidecarState};
use napp::test_sidecar::TestApp;

use crate::app_lifecycle::AppLifecycle;
use crate::handlers::ws::ClientHub;

mod scenarios;

/// The list the owner reads and the tests that run are one set, or this fails.
#[test]
fn every_proof_is_a_fixture_in_the_app_sidecars_suite_and_every_fixture_proves_something() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut proofs = std::collections::BTreeSet::new();
    let mut lines = include_str!("scenarios.rs").lines();
    while let Some(line) = lines.next() {
        if line.trim().starts_with("#[test]") || line.trim().starts_with("#[tokio::test") {
            let next = lines.next().unwrap_or("").trim();
            let name = next.trim_start_matches("async ").trim_start_matches("fn ").split('(').next().unwrap_or("");
            assert!(!name.is_empty(), "a test attribute with no fn under it");
            proofs.insert(format!("sidecar_proof::scenarios::{name}"));
        }
    }
    let suite = std::fs::read_to_string(root.join("suites/app-sidecars.yaml")).expect("suites/app-sidecars.yaml");
    let mut listed = std::collections::BTreeSet::new();
    for rel in suite.lines().filter_map(|l| l.trim().strip_prefix("- ")) {
        let path = root.join("suites").join(rel.trim());
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let proof = text
            .lines()
            .find_map(|l| l.strip_prefix("proof: "))
            .unwrap_or_else(|| panic!("{} names no proof", path.display()))
            .trim();
        assert!(proofs.contains(proof), "{} names a proof that does not exist: {proof}", path.display());
        assert!(listed.insert(proof.to_string()), "{proof} is listed twice");
    }
    let missing: Vec<_> = proofs.difference(&listed).collect();
    assert!(missing.is_empty(), "scenarios with no fixture in suites/app-sidecars.yaml: {missing:?}");
}

/// The supervisor's policy at test speed: the same shape, in milliseconds.
pub fn quick() -> RestartPolicy {
    RestartPolicy {
        first_delay: Duration::from_millis(50),
        max_delay: Duration::from_millis(400),
        healthy_after: Duration::from_secs(30),
        max_failures: 3,
        probe_every: Duration::from_millis(100),
        probe_misses: 2,
    }
}

/// An app whose sidecar is the test sidecar, supervised by a real
/// `AppLifecycle`, with a stand-in sidecar server answering what it relays.
pub struct World {
    pub app: TestApp,
    pub hub: Arc<ClientHub>,
    pub lifecycle: AppLifecycle,
    _upstream: tempfile::TempDir,
}

impl World {
    /// `setup` prepares the app (its mode, its program) before supervision begins.
    pub async fn new(id: &str, policy: RestartPolicy, setup: impl FnOnce(&TestApp)) -> Self {
        let app = TestApp::new(id);
        let upstream = tempfile::tempdir().expect("upstream dir");
        let sock = upstream.path().join("upstream.sock");
        serve_as_sidecar(&sock);
        app.set_upstream(&sock);
        setup(&app);
        let hub = Arc::new(ClientHub::new());
        let agent = db::models::Agent { id: id.to_string(), name: id.to_string(), is_app: Some(1), ..Default::default() };
        let skills = app.home.join("skills");
        let lifecycle = AppLifecycle::start(
            &agent,
            app.tool_dir.clone(),
            &app.home,
            hub.clone(),
            Arc::new(tools::Registry::new(run_everything())),
            Arc::new(tools::skills::Loader::new(skills.join("installed"), skills.join("user"))),
            0,
            policy,
        )
        .await;
        Self { app, hub, lifecycle, _upstream: upstream }
    }

    /// A GET through the proxy's path, answered by the stand-in server.
    pub async fn get(&self, path: &str) -> Result<String, crate::app_lifecycle::Unavailable> {
        let req = proto::HttpRequest {
            method: "GET".into(),
            path: path.into(),
            query: String::new(),
            headers: Default::default(),
            body: Vec::new(),
        };
        self.lifecycle.serve(req).await.map(|r| {
            assert_eq!(r.status_code, 200);
            String::from_utf8(r.body).expect("utf8")
        })
    }

    pub fn pid(&self) -> u32 {
        match self.lifecycle.state() {
            SidecarState::Running(l) => l.pid,
            other => panic!("expected running, got {other:?}"),
        }
    }

    /// Wait until the state satisfies `f`, or fail after `secs`.
    pub async fn until(&self, secs: u64, f: impl Fn(&SidecarState) -> bool) -> SidecarState {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
        loop {
            let s = self.lifecycle.state();
            if f(&s) {
                return s;
            }
            assert!(tokio::time::Instant::now() < deadline, "state never came: last {s:?}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// The permission check is the harness's; these proofs register no tool calls.
fn run_everything() -> Arc<dyn tools::PermissionGate> {
    struct RunEverything;
    #[async_trait::async_trait]
    impl tools::PermissionGate for RunEverything {
        async fn check(&self, _ctx: &tools::origin::ToolContext, _call: &tools::ResolvedCall<'_>) -> tools::GateVerdict {
            tools::GateVerdict::Run(types::permissions::Why::BasicWork)
        }
    }
    Arc::new(RunEverything)
}

/// A stand-in sidecar server on `sock`: answers every request with
/// "<method> <path>".
fn serve_as_sidecar(sock: &PathBuf) {
    struct Echo;
    #[tonic::async_trait]
    impl proto::ui_service_server::UiService for Echo {
        async fn health_check(
            &self,
            _req: tonic::Request<proto::HealthCheckRequest>,
        ) -> Result<tonic::Response<proto::HealthCheckResponse>, tonic::Status> {
            Err(tonic::Status::unimplemented("not used"))
        }
        async fn configure(
            &self,
            _req: tonic::Request<proto::SettingsMap>,
        ) -> Result<tonic::Response<proto::Empty>, tonic::Status> {
            Err(tonic::Status::unimplemented("not used"))
        }
        async fn handle_request(
            &self,
            req: tonic::Request<proto::HttpRequest>,
        ) -> Result<tonic::Response<proto::HttpResponse>, tonic::Status> {
            let r = req.into_inner();
            Ok(tonic::Response::new(proto::HttpResponse {
                status_code: 200,
                headers: Default::default(),
                body: format!("{} {}", r.method, r.path).into_bytes(),
            }))
        }
    }
    let listener = tokio::net::UnixListener::bind(sock).expect("bind the stand-in sidecar server");
    let incoming = futures::stream::unfold(listener, |l| async move {
        Some((l.accept().await.map(|(s, _)| s), l))
    });
    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(proto::ui_service_server::UiServiceServer::new(Echo))
            .serve_with_incoming(incoming),
    );
}
