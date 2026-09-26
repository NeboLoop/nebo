//! An app's sidecar, run under its one supervisor (`napp::supervisor`).
//!
//! Every way an app sidecar starts goes through [`start`]: boot, activation,
//! an update, and the first request to an app nobody started. Every way it is
//! brought back goes through the supervisor: a crash, a process that stops
//! answering, a request that cannot reach it, the app's "Try again". There is
//! no second launcher.
//!
//! The server's part is what follows each state change: the app's tools are
//! registered against the running sidecar, the per-launch token and manifest
//! permissions are read from it, and every change is broadcast as
//! `sidecar_state` (plus the documented `app_started` / `app_crashed` /
//! `app_restarted` / `app_stopped`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use napp::supervisor::{Launched, RestartPolicy, SidecarState, Supervisor};
use tracing::{info, warn};

use crate::handlers::ws::ClientHub;
use tools::sidecar_tool::{SidecarActionTool, SidecarCaller, SidecarResponse, SidecarToolDef};

/// How long a request waits for a sidecar that is starting or restarting.
pub(crate) const REQUEST_WAIT: Duration = Duration::from_secs(15);

/// One `UIService.HandleRequest` call to the sidecar on `sock`. The one way
/// Nebo talks to a sidecar: the proxy and the sidecar tools both call it.
#[cfg(unix)]
pub(crate) async fn handle_request(
    sock: &Path,
    req: proto::HttpRequest,
) -> Result<proto::HttpResponse, tonic::Status> {
    let sock = sock.to_path_buf();
    let channel = tonic::transport::Endpoint::from_static("http://[::]:50051")
        .connect_with_connector_lazy(tower::service_fn(move |_: tonic::transport::Uri| {
            let sock = sock.clone();
            async move {
                tokio::net::UnixStream::connect(sock)
                    .await
                    .map(hyper_util::rt::TokioIo::new)
            }
        }));
    let mut client = proto::ui_service_client::UiServiceClient::new(channel)
        .max_decoding_message_size(32 * 1024 * 1024);
    client.handle_request(req).await.map(|r| r.into_inner())
}

#[cfg(not(unix))]
pub(crate) async fn handle_request(
    _sock: &Path,
    _req: proto::HttpRequest,
) -> Result<proto::HttpResponse, tonic::Status> {
    Err(tonic::Status::unavailable("sidecars require Unix sockets"))
}

/// gRPC-based caller that routes through the sidecar's UIService.HandleRequest.
struct GrpcSidecarCaller {
    sock_path: PathBuf,
}

impl SidecarCaller for GrpcSidecarCaller {
    fn call(
        &self,
        method: &str,
        path: &str,
        query: &str,
        body: &[u8],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<SidecarResponse, String>> + Send + '_>,
    > {
        let req = proto::HttpRequest {
            method: method.to_string(),
            path: path.to_string(),
            query: query.to_string(),
            headers: Default::default(),
            body: body.to_vec(),
        };
        Box::pin(async move {
            match handle_request(&self.sock_path, req).await {
                Ok(inner) => Ok(SidecarResponse { status_code: inner.status_code, body: inner.body }),
                Err(e) => Err(format!("gRPC call failed: {}", e)),
            }
        })
    }
}

/// Why a request could not be served by the sidecar, for the app to show.
#[derive(Debug)]
pub(crate) struct Unavailable {
    pub state: SidecarState,
}

impl Unavailable {
    /// The sentence the app shows: the real reason, never a generic
    /// "could not be reached".
    pub fn message(&self, app: &str) -> String {
        match &self.state {
            SidecarState::Starting => format!("{app} is starting."),
            SidecarState::Restarting { .. } => format!("{app} is restarting."),
            SidecarState::Failed { permanent: true, reason } => {
                format!("{app} can't run on this computer: {reason}. Reinstall {app} to fix it.")
            }
            SidecarState::Failed { reason, .. } => format!("{app} stopped: {reason}"),
            SidecarState::Running(_) => format!("{app} did not answer."),
            SidecarState::Off => format!("{app} is turned off."),
        }
    }
}

pub struct AppLifecycle {
    agent_id: String,
    supervisor: Supervisor,
    /// Follows every state change: tools, broadcasts, the last launch. Ends
    /// on `Off`, after unregistering the app's tools.
    follower: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// The most recent launch, for its manifest permissions.
    latest: Arc<tokio::sync::RwLock<Option<Arc<Launched>>>>,
    skill_loader: Arc<tools::skills::Loader>,
    /// Names of skills loaded for this app (for cleanup on shutdown).
    loaded_skill_names: Vec<String>,
}

impl AppLifecycle {
    /// Put an app's sidecar under its supervisor. Returns once supervision has
    /// begun; the launch itself is published as state (see [`Self::settled`]).
    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        agent: &db::models::Agent,
        tool_dir: PathBuf,
        home: &Path,
        hub: Arc<ClientHub>,
        registry: Arc<tools::Registry>,
        skill_loader: Arc<tools::skills::Loader>,
        api_port: u16,
        policy: RestartPolicy,
    ) -> Self {
        let runtime = Arc::new(napp::Runtime::new(home));
        let supervisor = Supervisor::start(runtime, tool_dir.clone(), api_port, policy);
        let latest = Arc::new(tokio::sync::RwLock::new(None));
        let follower = tokio::spawn(follow(
            agent.id.clone(),
            tools::sidecar_tool::app_slug(&agent.name),
            tool_dir.clone(),
            supervisor.subscribe(),
            hub,
            registry,
            latest.clone(),
        ));
        let loaded_skill_names = skill_loader.load_app_skills(&tool_dir).await;
        Self {
            agent_id: agent.id.clone(),
            supervisor,
            follower: tokio::sync::Mutex::new(Some(follower)),
            latest,
            skill_loader,
            loaded_skill_names,
        }
    }

    pub fn state(&self) -> SidecarState {
        self.supervisor.state()
    }

    /// Wait up to `wait` for the launch in flight to settle.
    pub async fn settled(&self, wait: Duration) -> SidecarState {
        self.supervisor.settled(wait).await
    }

    /// Bring the sidecar up now — "Try again", or a request that could not
    /// reach it. The supervisor's own restart path; never a second launcher.
    pub async fn revive(&self, wait: Duration) -> SidecarState {
        self.supervisor.revive(wait).await
    }

    /// The running sidecar's token for API authentication; empty when it is
    /// not running, so a dead launch's token authenticates nothing.
    pub async fn app_token(&self) -> String {
        match self.supervisor.state() {
            SidecarState::Running(l) => l.app_token.clone(),
            _ => String::new(),
        }
    }

    /// Check if this app has a specific permission declared in its manifest.
    ///
    /// Supports exact match, prefix match, and wildcard (`network:*`).
    pub async fn has_permission(&self, perm: &str) -> bool {
        let latest = self.latest.read().await;
        let Some(launched) = latest.as_ref() else {
            return false;
        };
        let prefix = perm.split(':').next().unwrap_or("");
        launched.manifest.permissions.iter().any(|p| {
            p == perm
                || p == &format!("{}:*", prefix)
                || (p.ends_with(':') && perm.starts_with(p.as_str()))
        })
    }

    /// The running sidecar, waiting for one that is starting or restarting.
    /// A failed or stopped sidecar is reported at once, never waited on.
    pub(crate) async fn ready(&self, wait: Duration) -> Result<Arc<Launched>, Unavailable> {
        let state = match self.supervisor.state() {
            SidecarState::Starting | SidecarState::Restarting { .. } => self.supervisor.settled(wait).await,
            other => other,
        };
        match state {
            SidecarState::Running(l) => Ok(l),
            state => Err(Unavailable { state }),
        }
    }

    /// Serve one request through the sidecar. A connection the sidecar does
    /// not accept revives it through the supervisor, and a request that is
    /// safe to repeat (GET, HEAD) is sent again to the relaunched one.
    pub(crate) async fn serve(&self, req: proto::HttpRequest) -> Result<proto::HttpResponse, Unavailable> {
        let launched = self.ready(REQUEST_WAIT).await?;
        match handle_request(&launched.sock_path, req.clone()).await {
            Ok(resp) => Ok(resp),
            Err(e) => {
                warn!(agent = %self.agent_id, error = %e, "sidecar did not answer — reviving it");
                let state = self.supervisor.revive(REQUEST_WAIT).await;
                let SidecarState::Running(again) = state else {
                    return Err(Unavailable { state });
                };
                let repeatable = matches!(req.method.as_str(), "GET" | "HEAD");
                if !repeatable {
                    return Err(Unavailable { state: SidecarState::Running(again) });
                }
                handle_request(&again.sock_path, req).await.map_err(|e| {
                    warn!(agent = %self.agent_id, error = %e, "sidecar did not answer after reviving");
                    Unavailable { state: SidecarState::Running(again.clone()) }
                })
            }
        }
    }

    /// Stop the sidecar for good (Nebo exiting, the app deactivated or
    /// replaced). Not a crash: nothing restarts it.
    pub async fn shutdown(&self) {
        self.supervisor.shutdown().await;
        if let Some(follower) = self.follower.lock().await.take() {
            let _ = follower.await;
        }
        self.skill_loader.unload_skills(&self.loaded_skill_names).await;
    }
}

/// Follow a sidecar's state: register its tools each time it runs, remember
/// the launch, and broadcast every change.
async fn follow(
    agent_id: String,
    app: String,
    tool_dir: PathBuf,
    mut rx: tokio::sync::watch::Receiver<SidecarState>,
    hub: Arc<ClientHub>,
    registry: Arc<tools::Registry>,
    latest: Arc<tokio::sync::RwLock<Option<Arc<Launched>>>>,
) {
    let mut launches: u32 = 0;
    let mut was_running = false;
    loop {
        let state = rx.borrow_and_update().clone();
        let mut event = state.wire();
        event["agentId"] = serde_json::json!(agent_id);
        hub.broadcast("sidecar_state", event);
        match &state {
            SidecarState::Running(launched) => {
                launches += 1;
                *latest.write().await = Some(launched.clone());
                register_tools(&agent_id, &app, &tool_dir, &launched.sock_path, &registry).await;
                if launches == 1 {
                    hub.broadcast(
                        "app_started",
                        serde_json::json!({ "agentId": agent_id, "sockPath": launched.sock_path }),
                    );
                } else {
                    hub.broadcast(
                        "app_restarted",
                        serde_json::json!({ "agentId": agent_id, "restartCount": launches - 1 }),
                    );
                }
                was_running = true;
            }
            SidecarState::Restarting { .. } | SidecarState::Failed { .. } if was_running => {
                hub.broadcast("app_crashed", serde_json::json!({ "agentId": agent_id }));
                was_running = false;
            }
            SidecarState::Off => {
                registry.unregister_agent_tools(&agent_id).await;
                hub.broadcast("app_stopped", serde_json::json!({ "agentId": agent_id }));
                return;
            }
            _ => {}
        }
        if rx.changed().await.is_err() {
            registry.unregister_agent_tools(&agent_id).await;
            return;
        }
    }
}

/// Register the sidecar tools declared in agent.json against the socket.
///
/// Follows the same filesystem-based pattern as skills and plugins — tool
/// definitions live in agent.json, not behind an HTTP endpoint. Re-read on
/// every launch, so a rebuilt app's changed tools take effect.
async fn register_tools(agent_id: &str, app: &str, tool_dir: &Path, sock: &Path, registry: &tools::Registry) {
    registry.unregister_agent_tools(agent_id).await;
    let Some(defs) = read_tool_defs_from_config(tool_dir, agent_id) else {
        return;
    };
    let caller: Arc<dyn SidecarCaller> = Arc::new(GrpcSidecarCaller { sock_path: sock.to_path_buf() });
    let count = defs.len();
    for def in defs {
        registry
            .register_for_agent(agent_id, Box::new(SidecarActionTool::new(app, def, caller.clone())))
            .await;
    }
    info!(agent = %agent_id, tools = count, "registered sidecar tools from agent.json");
}

/// Whether an app has a program to supervise. An app that recorded one at
/// install, or has one on disk now (the one rule, `napp::runtime::sidecar_binary`),
/// does; a UI-only app does not, and nothing is launched for it.
pub(crate) fn has_sidecar(agent: &db::models::Agent, tool_dir: &Path) -> bool {
    agent.app_binary_path.as_deref().is_some_and(|p| !p.is_empty())
        || napp::runtime::sidecar_binary(tool_dir).is_some()
}

/// Start an app's sidecar under its supervisor. The ONE way an app sidecar
/// starts — boot, activation, an update and an on-request start all call this.
/// With `replace`, a sidecar the app already has is stopped and started anew
/// (activation, an update); without it, a running one is kept (a request).
/// Returns the lifecycle, or `None` for an app with no program to run.
pub(crate) async fn start(
    state: &crate::state::AppState,
    agent: &db::models::Agent,
    replace: bool,
) -> Option<Arc<AppLifecycle>> {
    if !replace {
        if let Some(lc) = state.app_lifecycles.read().await.get(&agent.id) {
            return Some(lc.clone());
        }
    }
    let tool_dir = crate::handlers::agents::app_tool_dir(agent)?;
    if !has_sidecar(agent, &tool_dir) {
        return None;
    }
    let home = match config::data_dir() {
        Ok(h) => h,
        Err(e) => {
            warn!(agent = %agent.id, error = %e, "no data directory — app sidecar not started");
            return None;
        }
    };
    let mut lifecycles = state.app_lifecycles.write().await;
    // Decided again under the lock: two first requests start it once.
    if let Some(existing) = lifecycles.get(&agent.id) {
        if !replace {
            return Some(existing.clone());
        }
    }
    if let Some(old) = lifecycles.remove(&agent.id) {
        old.shutdown().await;
    }
    let lifecycle = Arc::new(
        AppLifecycle::start(
            agent,
            tool_dir,
            &home,
            state.hub.clone(),
            state.tools.clone(),
            state.skill_loader.clone(),
            state.config.port,
            RestartPolicy::default(),
        )
        .await,
    );
    lifecycles.insert(agent.id.clone(), lifecycle.clone());
    Some(lifecycle)
}

/// Stop an app's sidecar for good (deactivation). Not a crash.
pub(crate) async fn stop(state: &crate::state::AppState, agent_id: &str) {
    let removed = state.app_lifecycles.write().await.remove(agent_id);
    if let Some(lifecycle) = removed {
        lifecycle.shutdown().await;
    }
}

/// Replace a running app's sidecar with the one in its current (post-update)
/// tool dir. No-op if the app isn't running (the next request starts it from
/// the new dir).
pub(crate) async fn relaunch(state: &crate::state::AppState, agent: &db::models::Agent) {
    if !state.app_lifecycles.read().await.contains_key(&agent.id) {
        return;
    }
    if start(state, agent, true).await.is_none() {
        stop(state, &agent.id).await;
        warn!(agent = %agent.id, "app has no program after update — not relaunched");
    }
}

/// Read tool definitions from agent.json in the agent directory.
/// Returns None if no agent.json or no tools declared.
fn read_tool_defs_from_config(agent_root: &Path, agent_id: &str) -> Option<Vec<SidecarToolDef>> {
    let path = agent_root.join("agent.json");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            info!(agent = %agent_id, reason = %e, "no agent.json, skipping tool registration");
            return None;
        }
    };
    match napp::agent::parse_agent_config(&content) {
        Ok(config) if !config.tools.is_empty() => {
            let defs = config
                .tools
                .into_iter()
                .map(|t| SidecarToolDef {
                    name: t.name,
                    description: t.description,
                    method: t.method,
                    path: t.path,
                    input_schema: t.input_schema,
                })
                .collect();
            Some(defs)
        }
        Ok(_) => {
            info!(agent = %agent_id, "no tools declared in agent.json");
            None
        }
        Err(e) => {
            warn!(agent = %agent_id, error = %e, "failed to parse agent.json");
            None
        }
    }
}
