use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::Command;
use tracing::{info, warn};

use crate::NappError;
use crate::manifest::Manifest;
use crate::sandbox;

/// A running tool process.
pub struct Process {
    pub tool_id: String,
    pub manifest: Manifest,
    pub pid: u32,
    pub sock_path: PathBuf,
    pub binary_path: PathBuf,
    /// Per-launch auth token passed as NEBO_APP_TOKEN env var.
    pub app_token: String,
    binary_mtime: std::time::SystemTime,
    child: tokio::process::Child,
}

impl Process {
    /// Get the gRPC endpoint for this tool.
    pub fn grpc_endpoint(&self) -> String {
        format!("unix://{}", self.sock_path.display())
    }

    /// Check if the binary on disk has changed since launch.
    ///
    /// Follows symlinks so a rebuild of the target binary is detected even
    /// when the tool directory uses a symlink (e.g. `bin/brief → sidecar/target/release/brief-sidecar`).
    pub fn binary_changed(&self) -> bool {
        let path = std::fs::canonicalize(&self.binary_path).unwrap_or(self.binary_path.clone());
        match std::fs::metadata(&path).and_then(|m| m.modified()) {
            Ok(mtime) => mtime != self.binary_mtime,
            Err(_) => false,
        }
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
    pub async fn launch(
        &self,
        tool_dir: &Path,
        api_port: u16,
    ) -> Result<Process, NappError> {
        let manifest = Manifest::load(&tool_dir.join("manifest.json"))?;
        manifest.validate()?;

        // Find binary and snapshot its mtime for change detection
        let binary = self.find_binary(tool_dir)?;
        let canonical = std::fs::canonicalize(&binary).unwrap_or(binary.clone());
        let binary_mtime = std::fs::metadata(&canonical)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

        // Validate binary
        sandbox::validate_binary(&binary, 500 * 1024 * 1024)?;

        // Socket path
        let sock_path = tool_dir.join(format!("{}.sock", manifest.id));

        // Clean up stale socket
        let _ = std::fs::remove_file(&sock_path);

        // Create data directory in appdata/ (physically separated from code)
        let artifact_slug = tool_dir
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or(&manifest.name);
        let artifact_type = match manifest.artifact_type.as_str() {
            "agent" => "agents",
            _ => "plugins",
        };
        let data_dir = config::appdata_dir()
            .map(|d| d.join(artifact_type).join(artifact_slug))
            .unwrap_or_else(|_| tool_dir.join("data"));
        std::fs::create_dir_all(&data_dir)?;

        // Build sanitized environment
        let mut env = sandbox::sanitize_env(
            &manifest.id,
            &manifest.name,
            &manifest.version,
            &tool_dir.to_string_lossy(),
            &sock_path.to_string_lossy(),
            &data_dir.to_string_lossy(),
            api_port,
        );

        // Per-launch auth token for API authentication
        let app_token = generate_token();
        env.push(("NEBO_APP_TOKEN".into(), app_token.clone()));

        // Launch process
        let mut cmd = Command::new(&binary);
        cmd.env_clear();
        for (k, v) in &env {
            cmd.env(k, v);
        }
        // Run in the non-versioned data dir, not the versioned code dir: a
        // sidecar that writes a DB to a relative path (./app.db) then lands in
        // persistent storage that survives updates, instead of the install dir
        // that gets wiped. Bundled code/resources are reached via NEBO_APP_DIR.
        cmd.current_dir(&data_dir);
        // SIGKILL the sidecar when its Child handle is dropped (nebo exit,
        // hot-reload restart, panic unwind, task cancellation). Without this,
        // sidecars stay alive after nebo dies, holding sockets and ports.
        cmd.kill_on_drop(true);
        // Pipe stdin and hold the write end open for the sidecar's whole life
        // (it lives inside the retained `Process.child`). When nebo dies by ANY
        // means — including SIGKILL, which no signal handler can catch — the OS
        // closes this pipe and the sidecar's stdin reaches EOF. App sidecars
        // watch stdin and self-exit on EOF, which is the only parent-death
        // signal that survives SIGKILL. kill_on_drop covers the graceful path;
        // this covers the kill -9 window kill_on_drop can't.
        cmd.stdin(std::process::Stdio::piped());

        // Redirect stdout/stderr to dedicated log file in the data directory
        let log_path = data_dir.join("sidecar.log");
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|e| NappError::Runtime(format!("open sidecar log: {}", e)))?;
        let stderr_file = log_file
            .try_clone()
            .map_err(|e| NappError::Runtime(format!("clone log fd: {}", e)))?;
        cmd.stdout(std::process::Stdio::from(stderr_file));
        cmd.stderr(std::process::Stdio::from(log_file));

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

        // Reap any pre-existing instance of this sidecar binary — orphans
        // would hold the same Unix socket and produce silent failures.
        crate::child_guard::reap_existing_for(&binary);

        let child = cmd
            .spawn()
            .map_err(|e| NappError::Runtime(format!("spawn tool: {}", e)))?;

        let pid = child.id().unwrap_or(0);

        // Track for signal-handler cleanup. Unregistered when the supervisor
        // notices the sidecar exited (see supervisor.rs / lifecycle code).
        crate::child_guard::register_child(pid);

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

        info!(tool = manifest.id.as_str(), pid, "tool launched");

        Ok(Process {
            tool_id: manifest.id.clone(),
            manifest,
            pid,
            sock_path,
            binary_path: binary,
            app_token,
            binary_mtime,
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
        // App packages may place their sidecar in bin/.
        let bin = tool_dir.join("bin");
        if bin.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&bin) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        return Ok(path);
                    }
                }
            }
        }
        // Dev-built sidecars live in sidecar/target/release/.
        let sidecar_release = tool_dir.join("sidecar/target/release");
        if sidecar_release.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&sidecar_release) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() && path.extension().is_none() {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            if let Ok(meta) = path.metadata() {
                                if meta.permissions().mode() & 0o111 != 0 {
                                    return Ok(path);
                                }
                            }
                        }
                        #[cfg(not(unix))]
                        {
                            return Ok(path);
                        }
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
            Ok(Err(e)) => Err(NappError::Runtime(format!("socket connect failed: {}", e))),
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
            #[cfg(unix)]
            if let Ok(pid_str) = std::fs::read_to_string(entry.path()) {
                if let Ok(pid) = pid_str.trim().parse::<i32>() {
                    unsafe {
                        if libc::kill(pid, 0) == 0 {
                            libc::kill(pid, libc::SIGTERM);
                            info!(pid, "killed stale tool process");
                        }
                    }
                }
            }
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Generate a random 32-byte hex token for per-launch app authentication.
fn generate_token() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: [u8; 32] = rng.r#gen();
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_token_format() {
        let token = generate_token();
        assert_eq!(token.len(), 64); // 32 bytes = 64 hex chars
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_generate_token_unique() {
        let t1 = generate_token();
        let t2 = generate_token();
        assert_ne!(t1, t2);
    }
}
