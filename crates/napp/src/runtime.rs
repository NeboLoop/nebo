use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::Command;
use tracing::{info, warn};

use crate::manifest::Manifest;
use crate::sandbox;
use crate::NappError;

/// A running tool process.
pub struct Process {
    pub tool_id: String,
    pub manifest: Manifest,
    pub pid: u32,
    pub sock_path: PathBuf,
    child: tokio::process::Child,
}

impl Process {
    /// Get the gRPC endpoint for this tool.
    pub fn grpc_endpoint(&self) -> String {
        format!("unix://{}", self.sock_path.display())
    }

    /// Check if the process is still alive.
    pub fn is_alive(&self) -> bool {
        #[cfg(unix)]
        {
            unsafe { libc::kill(self.pid as i32, 0) == 0 }
        }
        #[cfg(not(unix))]
        {
            true // Assume alive on non-Unix
        }
    }

    /// Stop the process gracefully.
    pub async fn stop(&mut self) {
        // Phase 1: SIGTERM
        #[cfg(unix)]
        {
            unsafe {
                libc::kill(-(self.pid as i32), libc::SIGTERM);
            }
        }
        #[cfg(not(unix))]
        {
            let _ = self.child.start_kill();
        }

        // Wait up to 2 seconds for graceful shutdown
        match tokio::time::timeout(Duration::from_secs(2), self.child.wait()).await {
            Ok(_) => {}
            Err(_) => {
                // Phase 2: Force kill
                let _ = self.child.kill().await;
                warn!(tool = self.tool_id.as_str(), "force killed tool");
            }
        }

        // Cleanup
        let _ = std::fs::remove_file(&self.sock_path);
        let pid_file = self.sock_path.with_extension("pid");
        let _ = std::fs::remove_file(&pid_file);

        info!(tool = self.tool_id.as_str(), "tool stopped");
    }
}

/// Runtime manages launching and stopping tool processes.
pub struct Runtime {
    _tools_dir: PathBuf,
}

impl Runtime {
    pub fn new(tools_dir: &Path) -> Self {
        Self {
            _tools_dir: tools_dir.to_path_buf(),
        }
    }

    /// Launch a tool from its directory.
    pub async fn launch(&self, tool_dir: &Path) -> Result<Process, NappError> {
        let manifest = Manifest::load(&tool_dir.join("manifest.json"))?;
        manifest.validate()?;

        // Find binary
        let binary = self.find_binary(tool_dir)?;

        // Validate binary
        sandbox::validate_binary(&binary, 500 * 1024 * 1024)?;

        // Socket path
        let sock_path = tool_dir.join(format!("{}.sock", manifest.id));

        // Clean up stale socket
        let _ = std::fs::remove_file(&sock_path);

        // Create data directory
        let data_dir = tool_dir.join("data");
        std::fs::create_dir_all(&data_dir)?;

        // Build sanitized environment
        let env = sandbox::sanitize_env(
            &manifest.id,
            &manifest.name,
            &manifest.version,
            &tool_dir.to_string_lossy(),
            &sock_path.to_string_lossy(),
            &data_dir.to_string_lossy(),
        );

        // Launch process
        let mut cmd = Command::new(&binary);
        cmd.env_clear();
        for (k, v) in &env {
            cmd.env(k, v);
        }
        cmd.current_dir(tool_dir);

        // Process group isolation on Unix
        #[cfg(unix)]
        {
            #[allow(unused_imports)]
            use std::os::unix::process::CommandExt;
            unsafe {
                cmd.pre_exec(|| {
                    libc::setpgid(0, 0);
                    Ok(())
                });
            }
        }

        let child = cmd
            .spawn()
            .map_err(|e| NappError::Runtime(format!("spawn tool: {}", e)))?;

        let pid = child.id().unwrap_or(0);

        // Write PID file
        let pid_file = sock_path.with_extension("pid");
        let _ = std::fs::write(&pid_file, pid.to_string());

        // Wait for socket to appear
        let timeout = Duration::from_secs(manifest.effective_startup_timeout() as u64);
        self.wait_for_socket(&sock_path, timeout).await?;

        // Health check: verify the socket is connectable
        if let Err(e) = self.health_check(&sock_path, Duration::from_secs(5)).await {
            warn!(
                tool = manifest.id.as_str(),
                error = %e,
                "health check failed after socket appeared (tool may use lazy init)"
            );
        }

        // Set socket permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o600));
        }

        info!(
            tool = manifest.id.as_str(),
            pid,
            "tool launched"
        );

        Ok(Process {
            tool_id: manifest.id.clone(),
            manifest,
            pid,
            sock_path,
            child,
        })
    }

    /// Find the binary in a tool directory.
    fn find_binary(&self, tool_dir: &Path) -> Result<PathBuf, NappError> {
        for name in &["binary", "app"] {
            let path = tool_dir.join(name);
            if path.exists() {
                return Ok(path);
            }
        }
        // Check tmp/
        let tmp = tool_dir.join("tmp");
        if tmp.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&tmp) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        return Ok(path);
                    }
                }
            }
        }
        Err(NappError::NotFound("no binary found".into()))
    }

    /// Check that the socket is connectable (basic health check).
    ///
    /// This is a best-effort check — tools may implement lazy init, so a failure
    /// here is logged as a warning but does not block launch.
    #[cfg(unix)]
    async fn health_check(&self, sock_path: &Path, timeout: Duration) -> Result<(), NappError> {
        match tokio::time::timeout(timeout, tokio::net::UnixStream::connect(sock_path)).await {
            Ok(Ok(_stream)) => Ok(()),
            Ok(Err(e)) => Err(NappError::Runtime(format!(
                "socket connect failed: {}",
                e
            ))),
            Err(_) => Err(NappError::Runtime(format!(
                "socket connect timed out after {}s",
                timeout.as_secs()
            ))),
        }
    }

    #[cfg(not(unix))]
    async fn health_check(&self, _sock_path: &Path, _timeout: Duration) -> Result<(), NappError> {
        // Unix sockets not available on Windows; skip health check
        Ok(())
    }

    /// Wait for the Unix domain socket to appear.
    async fn wait_for_socket(&self, sock_path: &Path, timeout: Duration) -> Result<(), NappError> {
        let start = std::time::Instant::now();
        let mut delay = Duration::from_millis(100);

        while start.elapsed() < timeout {
            if sock_path.exists() {
                return Ok(());
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_secs(1));
        }

        Err(NappError::Runtime(format!(
            "socket {} not created within {}s",
            sock_path.display(),
            timeout.as_secs()
        )))
    }

    /// Clean up stale processes from a previous run.
    pub fn cleanup_stale(&self, tool_dir: &Path) {
        let pid_files: Vec<_> = std::fs::read_dir(tool_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                e.path()
                    .extension()
                    .map(|ext| ext == "pid")
                    .unwrap_or(false)
            })
            .collect();

        for entry in pid_files {
            if let Ok(pid_str) = std::fs::read_to_string(entry.path()) {
                if let Ok(pid) = pid_str.trim().parse::<i32>() {
                    #[cfg(unix)]
                    {
                        unsafe {
                            if libc::kill(pid, 0) == 0 {
                                libc::kill(pid, libc::SIGTERM);
                                info!(pid, "killed stale tool process");
                            }
                        }
                    }
                }
            }
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
