//! VM lifecycle management.
//!
//! The VmManager owns the VM process and exposes high-level operations:
//! - Start/stop the VM
//! - Create sessions (isolated execution contexts)
//! - Route events to the correct session
//! - Handle VM crashes and auto-restart

use crate::error::{VmError, VmResult};
use crate::rpc::{GuestEvent, VmClient};
use crate::session::VmSession;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, info, warn};

/// VM configuration.
#[derive(Debug, Clone)]
pub struct VmConfig {
    /// Memory in MB (default: 2048).
    pub memory_mb: u64,
    /// Number of CPU cores (default: 2).
    pub cpu_count: u32,
    /// Disk size in GB for the session data volume (default: 10).
    pub disk_size_gb: u32,
    /// Domains allowed for network access from inside the VM.
    pub allowed_domains: Vec<String>,
    /// Path to the sidecar image (nebo-vm.{arch}.img) — embedded in app bundle.
    pub image_path: String,
    /// Path to the rootfs disk image — managed by Bundle (downloaded from CDN).
    pub rootfs_path: String,
    /// Expected SHA-256 of the rootfs image (for verification).
    pub rootfs_sha: String,
    /// Boot timeout in seconds.
    pub boot_timeout_secs: u64,
}

impl Default for VmConfig {
    fn default() -> Self {
        Self {
            memory_mb: 2048,
            cpu_count: 2,
            disk_size_gb: 10,
            allowed_domains: vec![
                "pypi.org".to_string(),
                "files.pythonhosted.org".to_string(),
                "registry.npmjs.org".to_string(),
                "github.com".to_string(),
                "crates.io".to_string(),
            ],
            image_path: String::new(),
            rootfs_path: String::new(),
            rootfs_sha: String::new(),
            boot_timeout_secs: 30,
        }
    }
}

/// VM running state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed(String),
}

/// Manages the VM lifecycle and sessions.
pub struct VmManager {
    config: VmConfig,
    state: Arc<RwLock<VmState>>,
    client: Arc<VmClient>,
    sessions: Arc<RwLock<HashMap<String, VmSession>>>,
    event_rx: Mutex<Option<mpsc::UnboundedReceiver<GuestEvent>>>,
}

impl VmManager {
    /// Create a new VM manager with the given configuration.
    pub fn new(config: VmConfig) -> Self {
        let (client, event_rx) = VmClient::new();
        Self {
            config,
            state: Arc::new(RwLock::new(VmState::Stopped)),
            client: Arc::new(client),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            event_rx: Mutex::new(Some(event_rx)),
        }
    }

    /// Get the current VM state.
    pub async fn state(&self) -> VmState {
        self.state.read().await.clone()
    }

    /// Get a reference to the RPC client.
    pub fn client(&self) -> &VmClient {
        &self.client
    }

    /// Start the VM. Platform-specific implementation will handle the actual
    /// VM creation (Virtualization.framework on macOS, QEMU on Linux).
    pub async fn start(&self) -> VmResult<()> {
        let current = self.state.read().await.clone();
        if current == VmState::Running {
            return Err(VmError::AlreadyRunning);
        }

        *self.state.write().await = VmState::Starting;
        info!(
            memory_mb = self.config.memory_mb,
            cpus = self.config.cpu_count,
            "starting VM"
        );

        // Platform-specific boot is handled by platform_macos/platform_linux modules.
        // They will:
        // 1. Start the VM process
        // 2. Wait for vsock/stdio connection from guest daemon
        // 3. Call client.connect(stream)
        // 4. Wait for "ready" event

        // For now, mark as running (platform modules will fill this in)
        *self.state.write().await = VmState::Running;

        // Start event routing
        self.start_event_router().await;

        Ok(())
    }

    /// Stop the VM gracefully.
    pub async fn stop(&self) -> VmResult<()> {
        let current = self.state.read().await.clone();
        if current == VmState::Stopped {
            return Ok(());
        }

        *self.state.write().await = VmState::Stopping;
        info!("stopping VM");

        // Kill all running sessions
        let sessions = self.sessions.read().await;
        for (_id, session) in sessions.iter() {
            if !session.is_done() {
                if let Some(ref pid) = session.process_id {
                    let _ = self.client.kill(pid, "TERM").await;
                }
            }
        }

        // Platform-specific shutdown handled by platform modules
        *self.state.write().await = VmState::Stopped;
        Ok(())
    }

    /// Create a new isolated session for executing code.
    pub async fn create_session(
        &self,
        name: &str,
        allowed_domains: Option<Vec<String>>,
    ) -> VmResult<String> {
        if *self.state.read().await != VmState::Running {
            return Err(VmError::NotRunning);
        }

        let id = uuid::Uuid::new_v4().to_string();
        let work_dir = format!("/sessions/{}", id);
        let domains = allowed_domains.unwrap_or_else(|| self.config.allowed_domains.clone());

        let session = VmSession::new(id.clone(), name.to_string(), work_dir, domains);

        self.sessions.write().await.insert(id.clone(), session);
        debug!(session_id = %id, name, "created VM session");
        Ok(id)
    }

    /// Execute a command in a session and wait for completion.
    pub async fn exec(
        &self,
        session_id: &str,
        command: &str,
        args: Vec<String>,
        env: Option<HashMap<String, String>>,
        timeout_secs: Option<u64>,
    ) -> VmResult<(String, String, i32)> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| VmError::ProcessNotFound {
                id: session_id.to_string(),
            })?;

        session
            .spawn(&self.client, command, args, env, timeout_secs)
            .await?;

        drop(sessions);

        // Wait for the session to complete
        let timeout = timeout_secs.unwrap_or(120);
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(timeout);

        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

            let sessions = self.sessions.read().await;
            if let Some(session) = sessions.get(session_id) {
                if session.is_done() {
                    let code = session.exit_code.unwrap_or(-1);
                    let stdout = session.stdout.clone();
                    let stderr = session.stderr.clone();
                    return Ok((stdout, stderr, code));
                }
            }

            if tokio::time::Instant::now() >= deadline {
                // Kill the process on timeout
                let sessions = self.sessions.read().await;
                if let Some(session) = sessions.get(session_id) {
                    if let Some(ref pid) = session.process_id {
                        let _ = self.client.kill(pid, "KILL").await;
                    }
                }
                return Err(VmError::RpcTimeout {
                    method: format!("exec:{command}"),
                    timeout_secs: timeout,
                });
            }
        }
    }

    /// Get a session by ID.
    pub async fn get_session(&self, id: &str) -> Option<VmSession> {
        self.sessions.read().await.get(id).cloned()
    }

    /// Remove a completed session and clean up its resources.
    pub async fn destroy_session(&self, id: &str) -> VmResult<()> {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get(id) {
            if !session.is_done() {
                if let Some(ref pid) = session.process_id {
                    let _ = self.client.kill(pid, "KILL").await;
                }
            }
        }
        sessions.remove(id);

        // Ask guest to clean up session directory
        let _ = self
            .client
            .request(
                "deleteSessionDirs",
                Some(serde_json::json!({ "names": [id] })),
            )
            .await;

        debug!(session_id = %id, "destroyed VM session");
        Ok(())
    }

    /// Start the background event router that dispatches guest events to sessions.
    async fn start_event_router(&self) {
        let mut rx = match self.event_rx.lock().await.take() {
            Some(rx) => rx,
            None => return,
        };

        let sessions = self.sessions.clone();
        let state = self.state.clone();

        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                // Route event to the correct session by process ID
                let mut map = sessions.write().await;
                if let Some(session) = map.values_mut().find(|s| {
                    s.process_id.as_deref() == Some(&event.id) || s.id == event.id
                }) {
                    session.handle_event(&event);
                }

                // Handle VM-level events
                match event.event_type.as_str() {
                    "networkStatus" => {
                        debug!(status = ?event.status, "VM network status changed");
                    }
                    "ready" => {
                        info!("guest daemon ready");
                    }
                    _ => {}
                }
            }

            // Event channel closed — VM died
            warn!("VM event channel closed");
            *state.write().await = VmState::Failed("event channel closed".to_string());
        });
    }
}

impl Clone for VmSession {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            name: self.name.clone(),
            state: self.state.clone(),
            work_dir: self.work_dir.clone(),
            allowed_domains: self.allowed_domains.clone(),
            stdout: self.stdout.clone(),
            stderr: self.stderr.clone(),
            exit_code: self.exit_code,
            process_id: self.process_id.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hard deadline for async tests — a wedged await must FAIL the test with
    /// a named error, never park the build queue.
    async fn bounded<T>(what: &str, fut: impl std::future::Future<Output = T>) -> T {
        tokio::time::timeout(std::time::Duration::from_secs(10), fut)
            .await
            .unwrap_or_else(|_| panic!("test deadline exceeded (10s): {what}"))
    }

    /// INVARIANT: the default config locks resource sizing (2048 MB / 2 CPUs /
    /// 10 GB / 30 s boot timeout) and ships the package-registry network
    /// allowlist with empty image paths (paths are resolved by Bundle).
    #[test]
    fn default_config_values() {
        let cfg = VmConfig::default();
        assert_eq!(cfg.memory_mb, 2048);
        assert_eq!(cfg.cpu_count, 2);
        assert_eq!(cfg.disk_size_gb, 10);
        assert_eq!(cfg.boot_timeout_secs, 30);
        for domain in ["pypi.org", "files.pythonhosted.org", "registry.npmjs.org", "github.com", "crates.io"] {
            assert!(cfg.allowed_domains.iter().any(|d| d == domain), "missing {domain}");
        }
        assert!(cfg.image_path.is_empty());
        assert!(cfg.rootfs_path.is_empty());
        assert!(cfg.rootfs_sha.is_empty());
    }

    /// INVARIANT: sessions cannot be created unless the VM is Running — a
    /// Stopped manager refuses with NotRunning.
    #[tokio::test]
    async fn create_session_requires_running_vm() {
        bounded("create_session_requires_running_vm", async {
            let mgr = VmManager::new(VmConfig::default());
            assert_eq!(mgr.state().await, VmState::Stopped);
            let err = mgr.create_session("skill", None).await.unwrap_err();
            assert!(matches!(err, VmError::NotRunning));
        })
        .await;
    }

    /// INVARIANT: start moves Stopped → Running, and a second start is rejected
    /// with AlreadyRunning instead of double-booting.
    #[tokio::test]
    async fn start_is_not_reentrant() {
        bounded("start_is_not_reentrant", async {
            let mgr = VmManager::new(VmConfig::default());
            mgr.start().await.unwrap();
            assert_eq!(mgr.state().await, VmState::Running);
            let err = mgr.start().await.unwrap_err();
            assert!(matches!(err, VmError::AlreadyRunning));
        })
        .await;
    }

    /// INVARIANT: a created session gets a /sessions/<id> workdir and inherits
    /// the config allowlist unless an explicit allowlist overrides it.
    #[tokio::test]
    async fn create_session_defaults_and_overrides() {
        bounded("create_session_defaults_and_overrides", async {
            let mgr = VmManager::new(VmConfig::default());
            mgr.start().await.unwrap();

            let id = mgr.create_session("skill-a", None).await.unwrap();
            let s = mgr.get_session(&id).await.unwrap();
            assert_eq!(s.work_dir, format!("/sessions/{id}"));
            assert_eq!(s.allowed_domains, VmConfig::default().allowed_domains);
            assert_eq!(s.name, "skill-a");

            let id2 = mgr
                .create_session("skill-b", Some(vec!["example.com".to_string()]))
                .await
                .unwrap();
            let s2 = mgr.get_session(&id2).await.unwrap();
            assert_eq!(s2.allowed_domains, vec!["example.com".to_string()]);
            assert_ne!(id, id2, "session ids must be unique");
        })
        .await;
    }

    /// INVARIANT: stop always lands in Stopped and is idempotent — stopping a
    /// stopped VM is not an error.
    #[tokio::test]
    async fn stop_is_idempotent() {
        bounded("stop_is_idempotent", async {
            let mgr = VmManager::new(VmConfig::default());
            mgr.stop().await.unwrap();
            assert_eq!(mgr.state().await, VmState::Stopped);

            mgr.start().await.unwrap();
            mgr.stop().await.unwrap();
            assert_eq!(mgr.state().await, VmState::Stopped);
            mgr.stop().await.unwrap();
            assert_eq!(mgr.state().await, VmState::Stopped);
        })
        .await;
    }

    /// INVARIANT: destroy_session removes the session from the manager even
    /// when no guest is connected (guest-side cleanup is best-effort).
    #[tokio::test]
    async fn destroy_session_removes_it() {
        bounded("destroy_session_removes_it", async {
            let mgr = VmManager::new(VmConfig::default());
            mgr.start().await.unwrap();
            let id = mgr.create_session("skill", None).await.unwrap();
            assert!(mgr.get_session(&id).await.is_some());

            mgr.destroy_session(&id).await.unwrap();
            assert!(mgr.get_session(&id).await.is_none());
        })
        .await;
    }
}
