use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::handlers::ws::ClientHub;
use tools::sidecar_tool::{SidecarActionTool, SidecarCaller, SidecarResponse, SidecarToolDef};
use types::NeboError;

/// gRPC-based caller that routes through the sidecar's UIService.HandleRequest.
struct GrpcSidecarCaller {
    sock_path: PathBuf,
}

#[cfg(unix)]
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
        let method = method.to_string();
        let path = path.to_string();
        let query = query.to_string();
        let body = body.to_vec();
        let sock = self.sock_path.clone();

        Box::pin(async move {
            let channel =
                tonic::transport::Endpoint::from_static("http://[::]:50051")
                    .connect_with_connector_lazy(tower::service_fn(
                        move |_: tonic::transport::Uri| {
                            let sock = sock.clone();
                            async move {
                                tokio::net::UnixStream::connect(sock)
                                    .await
                                    .map(hyper_util::rt::TokioIo::new)
                            }
                        },
                    ));

            let mut client = proto::ui_service_client::UiServiceClient::new(channel);
            let req = proto::HttpRequest {
                method,
                path,
                query,
                headers: HashMap::new(),
                body,
            };

            match client.handle_request(req).await {
                Ok(resp) => {
                    let inner = resp.into_inner();
                    Ok(SidecarResponse {
                        status_code: inner.status_code,
                        body: inner.body,
                    })
                }
                Err(e) => Err(format!("gRPC call failed: {}", e)),
            }
        })
    }
}

#[cfg(not(unix))]
impl SidecarCaller for GrpcSidecarCaller {
    fn call(
        &self,
        _method: &str,
        _path: &str,
        _query: &str,
        _body: &[u8],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<SidecarResponse, String>> + Send + '_>,
    > {
        Box::pin(async { Err("gRPC sidecar requires Unix sockets".to_string()) })
    }
}

pub struct AppLifecycle {
    agent_id: String,
    tool_dir: PathBuf,
    runtime: Arc<napp::Runtime>,
    supervisor: Arc<napp::supervisor::Supervisor>,
    process: Arc<Mutex<Option<napp::Process>>>,
    cancel: CancellationToken,
    hub: Arc<ClientHub>,
    registry: Arc<tools::Registry>,
    skill_loader: Arc<tools::skills::Loader>,
    /// Per-launch auth token for this app's sidecar.
    app_token: Arc<tokio::sync::RwLock<String>>,
    /// Cached manifest permissions for API-level enforcement.
    permissions: Arc<tokio::sync::RwLock<Vec<String>>>,
    /// Names of skills loaded for this app (for cleanup on shutdown).
    loaded_skill_names: Vec<String>,
    /// Server port for NEBO_API_URL injection into sidecar env.
    api_port: u16,
}

impl AppLifecycle {
    pub fn new(
        agent_id: String,
        tool_dir: PathBuf,
        hub: Arc<ClientHub>,
        registry: Arc<tools::Registry>,
        skill_loader: Arc<tools::skills::Loader>,
        api_port: u16,
    ) -> Self {
        let runtime_root = tool_dir
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let runtime = Arc::new(napp::Runtime::new(&runtime_root));
        Self {
            agent_id,
            tool_dir,
            runtime,
            supervisor: Arc::new(napp::supervisor::Supervisor::new()),
            process: Arc::new(Mutex::new(None)),
            cancel: CancellationToken::new(),
            hub,
            registry,
            skill_loader,
            app_token: Arc::new(tokio::sync::RwLock::new(String::new())),
            permissions: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            loaded_skill_names: Vec::new(),
            api_port,
        }
    }

    pub async fn launch(&mut self) -> Result<(), NeboError> {
        self.runtime.cleanup_stale(&self.tool_dir);
        let process = self
            .runtime
            .launch(&self.tool_dir, self.api_port)
            .await
            .map_err(|e| NeboError::Internal(format!("launch app sidecar: {e}")))?;
        let sock_path = process.sock_path.clone();
        *self.app_token.write().await = process.app_token.clone();
        *self.permissions.write().await = process.manifest.permissions.clone();
        *self.process.lock().await = Some(process);
        self.supervisor.watch(&self.agent_id).await;
        self.hub.broadcast(
            "app_started",
            serde_json::json!({
                "agentId": self.agent_id,
                "sockPath": sock_path,
            }),
        );
        self.discover_tools(&sock_path).await;
        self.loaded_skill_names = self.skill_loader.load_app_skills(&self.tool_dir).await;
        self.spawn_health_checker();
        Ok(())
    }

    /// Get the current app token for API authentication validation.
    pub async fn app_token(&self) -> String {
        self.app_token.read().await.clone()
    }

    /// Check if this app has a specific permission declared in its manifest.
    ///
    /// Supports exact match, prefix match, and wildcard (`network:*`).
    pub async fn has_permission(&self, perm: &str) -> bool {
        let perms = self.permissions.read().await;
        let prefix = perm.split(':').next().unwrap_or("");
        perms.iter().any(|p| {
            p == perm
                || p == &format!("{}:*", prefix)
                || (p.ends_with(':') && perm.starts_with(p.as_str()))
        })
    }

    pub async fn shutdown(&mut self) -> Result<(), NeboError> {
        self.cancel.cancel();
        self.supervisor.unwatch(&self.agent_id).await;
        let mut process = self.process.lock().await.take();
        if let Some(ref mut process) = process {
            process.stop().await;
        }
        // Unregister all sidecar tools and skills for this agent
        self.registry.unregister_agent_tools(&self.agent_id).await;
        self.skill_loader.unload_skills(&self.loaded_skill_names).await;
        self.hub.broadcast(
            "app_stopped",
            serde_json::json!({ "agentId": self.agent_id }),
        );
        Ok(())
    }

    /// Register sidecar tools declared in agent.json.
    ///
    /// Follows the same filesystem-based pattern as skills and plugins —
    /// tool definitions live in agent.json, not behind an HTTP endpoint.
    async fn discover_tools(&self, sock_path: &Path) {
        let caller: Arc<dyn SidecarCaller> = Arc::new(GrpcSidecarCaller {
            sock_path: sock_path.to_path_buf(),
        });

        let agent_root = self.tool_dir.parent().unwrap_or(&self.tool_dir);
        let defs = match read_tool_defs_from_config(agent_root, &self.agent_id) {
            Some(d) => d,
            None => return,
        };

        let count = defs.len();
        for def in defs {
            let tool = SidecarActionTool::new(def, caller.clone());
            self.registry
                .register_for_agent(&self.agent_id, Box::new(tool))
                .await;
        }
        info!(
            agent = %self.agent_id,
            tools = count,
            "registered sidecar tools from agent.json"
        );
    }

    fn spawn_health_checker(&self) {
        let agent_id = self.agent_id.clone();
        let tool_dir = self.tool_dir.clone();
        let process_slot = self.process.clone();
        let runtime = self.runtime.clone();
        let supervisor = self.supervisor.clone();
        let cancel = self.cancel.clone();
        let hub = self.hub.clone();
        let registry = self.registry.clone();
        let app_token = self.app_token.clone();
        let permissions = self.permissions.clone();
        let api_port = self.api_port;

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(supervisor.check_interval());
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = interval.tick() => {}
                }
                let (dead, binary_changed) = {
                    let guard = process_slot.lock().await;
                    match guard.as_ref() {
                        Some(process) => (!process.is_alive(), process.binary_changed()),
                        None => (false, false),
                    }
                };

                // Binary changed on disk → hot-restart (not a crash, no backoff)
                if binary_changed && !dead {
                    info!(agent = %agent_id, "binary changed on disk, restarting sidecar");
                    // Unregister old tools before restart
                    registry.unregister_agent_tools(&agent_id).await;
                    {
                        let mut guard = process_slot.lock().await;
                        if let Some(ref mut p) = *guard {
                            p.stop().await;
                        }
                        *guard = None;
                    }
                    match runtime.launch(&tool_dir, api_port).await {
                        Ok(process) => {
                            let sock_path = process.sock_path.clone();
                            *app_token.write().await = process.app_token.clone();
                            *permissions.write().await = process.manifest.permissions.clone();
                            *process_slot.lock().await = Some(process);

                            // Re-register tools from agent.json
                            let caller: Arc<dyn SidecarCaller> = Arc::new(GrpcSidecarCaller {
                                sock_path: sock_path.clone(),
                            });
                            let agent_root = tool_dir.parent().unwrap_or(&tool_dir);
                            if let Some(defs) = read_tool_defs_from_config(agent_root, &agent_id) {
                                for def in defs {
                                    let tool = SidecarActionTool::new(def, caller.clone());
                                    registry.register_for_agent(&agent_id, Box::new(tool)).await;
                                }
                            }

                            hub.broadcast(
                                "app_restarted",
                                serde_json::json!({
                                    "agentId": agent_id,
                                    "reason": "binary_changed",
                                }),
                            );
                        }
                        Err(e) => {
                            warn!(agent = %agent_id, error = %e, "failed to restart sidecar after binary change");
                        }
                    }
                    continue;
                }

                if !dead {
                    continue;
                }

                // Process crashed — use supervisor backoff/limits
                hub.broadcast("app_crashed", serde_json::json!({ "agentId": agent_id }));
                if !supervisor.should_restart(&agent_id).await {
                    continue;
                }
                supervisor.record_restart(&agent_id).await;
                match runtime.launch(&tool_dir, api_port).await {
                    Ok(process) => {
                        *app_token.write().await = process.app_token.clone();
                        *permissions.write().await = process.manifest.permissions.clone();
                        *process_slot.lock().await = Some(process);
                        let restart_count = supervisor.restart_count(&agent_id).await;
                        hub.broadcast(
                            "app_restarted",
                            serde_json::json!({
                                "agentId": agent_id,
                                "restartCount": restart_count,
                            }),
                        );
                    }
                    Err(e) => {
                        warn!(agent = %agent_id, error = %e, "failed to restart app sidecar");
                    }
                }
            }
        });
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
