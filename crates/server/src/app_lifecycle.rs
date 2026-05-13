use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::handlers::ws::ClientHub;
use types::NeboError;

pub struct AppLifecycle {
    agent_id: String,
    tool_dir: PathBuf,
    runtime: Arc<napp::Runtime>,
    supervisor: Arc<napp::supervisor::Supervisor>,
    process: Arc<Mutex<Option<napp::Process>>>,
    cancel: CancellationToken,
    hub: Arc<ClientHub>,
}

impl AppLifecycle {
    pub fn new(agent_id: String, tool_dir: PathBuf, hub: Arc<ClientHub>) -> Self {
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
        }
    }

    pub async fn launch(&mut self) -> Result<(), NeboError> {
        self.runtime.cleanup_stale(&self.tool_dir);
        let process = self
            .runtime
            .launch(&self.tool_dir)
            .await
            .map_err(|e| NeboError::Internal(format!("launch app sidecar: {e}")))?;
        let sock_path = process.sock_path.clone();
        *self.process.lock().await = Some(process);
        self.supervisor.watch(&self.agent_id).await;
        self.hub.broadcast(
            "app_started",
            serde_json::json!({
                "agentId": self.agent_id,
                "sockPath": sock_path,
            }),
        );
        self.spawn_health_checker();
        Ok(())
    }

    pub async fn shutdown(&mut self) -> Result<(), NeboError> {
        self.cancel.cancel();
        self.supervisor.unwatch(&self.agent_id).await;
        let mut process = self.process.lock().await.take();
        if let Some(ref mut process) = process {
            process.stop().await;
        }
        self.hub.broadcast(
            "app_stopped",
            serde_json::json!({ "agentId": self.agent_id }),
        );
        Ok(())
    }

    fn spawn_health_checker(&self) {
        let agent_id = self.agent_id.clone();
        let tool_dir = self.tool_dir.clone();
        let process_slot = self.process.clone();
        let runtime = self.runtime.clone();
        let supervisor = self.supervisor.clone();
        let cancel = self.cancel.clone();
        let hub = self.hub.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(supervisor.check_interval());
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = interval.tick() => {}
                }
                let should_restart = {
                    let guard = process_slot.lock().await;
                    match guard.as_ref() {
                        Some(process) => !process.is_alive(),
                        None => false,
                    }
                };
                if !should_restart {
                    continue;
                }

                hub.broadcast("app_crashed", serde_json::json!({ "agentId": agent_id }));
                if !supervisor.should_restart(&agent_id).await {
                    continue;
                }
                supervisor.record_restart(&agent_id).await;
                match runtime.launch(&tool_dir).await {
                    Ok(process) => {
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
