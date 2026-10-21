use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::oneshot;
use tracing::{info, warn};

use crate::NappError;
use crate::manifest::Manifest;
use crate::sandbox;

/// The line a sidecar prints on its stdout once it is listening, followed by
/// the loopback address it bound: `nebo-app-listening 127.0.0.1:54321`.
///
/// The transport, on every OS: the sidecar binds `127.0.0.1:0` (the OS picks
/// a free port, so no other process can be waiting on it), serves gRPC there,
/// and refuses every call that does not carry `authorization: Bearer
/// <NEBO_APP_TOKEN>`. Nebo reads the address from this line, connects only to
/// a loopback address, and sends the token on every call.
pub const LISTENING: &str = "nebo-app-listening ";

/// A running tool process. Owned by its [`crate::supervisor::Supervisor`],
/// which is the only thing that launches, watches and restarts one.
pub struct Process {
    pub tool_id: String,
    pub manifest: Manifest,
    pub pid: u32,
    /// The loopback address it serves on, as it announced ([`LISTENING`]).
    pub addr: SocketAddr,
    pub binary_path: PathBuf,
    /// Where the process's stdout and stderr go (`$NEBO_DATA_DIR/sidecar.log`).
    pub log_path: PathBuf,
    /// Per-launch token, passed as NEBO_APP_TOKEN: the sidecar authenticates
    /// to Nebo's API with it, and Nebo authenticates to the sidecar with it.
    pub app_token: String,
    /// `<tool dir>/<app id>.pid`, so a Nebo that died without stopping the
    /// process can retire it at its next start ([`Runtime::cleanup_stale`]).
    pid_file: PathBuf,
    binary_mtime: std::time::SystemTime,
    child: tokio::process::Child,
    /// The write end of the process's stdin, held apart from `child` for the
    /// process's whole life: `Child::wait` closes a stdin it still holds, and
    /// a sidecar reads that EOF as "Nebo is gone" and exits.
    _stdin: Option<tokio::process::ChildStdin>,
}

impl Process {
    /// Get the gRPC endpoint for this tool.
    pub fn grpc_endpoint(&self) -> String {
        format!("http://{}", self.addr)
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

    /// Wait for the process to exit, then clean up after it. Returns why it
    /// ended: its exit status and the last lines it wrote to its log. The
    /// exit is awaited on the child handle itself, so it is never missed —
    /// a zombie still answers `kill(pid, 0)`, which is how an exit used to go
    /// unnoticed.
    pub async fn exited(&mut self) -> String {
        let status = self.child.wait().await;
        self.cleanup();
        let status = match status {
            Ok(s) => describe_exit(s),
            Err(e) => format!("could not be waited on: {e}"),
        };
        match log_tail(&self.log_path, 10) {
            Some(tail) => format!("{status}; last output:\n{tail}"),
            None => status,
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

        self.cleanup();
        info!(tool = self.tool_id.as_str(), "tool stopped");
    }

    /// Remove what the process leaves behind once it is gone: its pid file
    /// and its signal-handler registration.
    fn cleanup(&self) {
        let _ = std::fs::remove_file(&self.pid_file);
        crate::child_guard::unregister_child(self.pid);
    }
}

/// An exit status in words: the code, or the signal that ended it.
fn describe_exit(status: std::process::ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return format!("killed by signal {sig}");
        }
    }
    match status.code() {
        Some(code) => format!("exited with code {code}"),
        None => "exited".to_string(),
    }
}

/// The last `lines` non-empty lines of a log file, read from its end.
fn log_tail(path: &Path, lines: usize) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(8 * 1024);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let tail: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail = &tail[tail.len().saturating_sub(lines)..];
    (!tail.is_empty()).then(|| tail.join("\n"))
}

/// Runtime launches and stops one sidecar process.
pub struct Runtime {
    /// The persistent data directory ([`crate::app_data::data_dir`]):
    /// its `NEBO_DATA_DIR`, working directory and `sidecar.log`.
    data_dir: PathBuf,
}

impl Runtime {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    /// Launch a tool from its directory.
    pub async fn launch(
        &self,
        tool_dir: &Path,
        api_port: u16,
    ) -> Result<Process, NappError> {
        // A manifest that cannot be read or is invalid is a damaged package:
        // reported as a manifest error so the supervisor treats it as
        // permanent instead of retrying it.
        let manifest = Manifest::load(&tool_dir.join("manifest.json"))
            .and_then(|m| m.validate().map(|()| m))
            .map_err(|e| match e {
                NappError::Manifest(m) => NappError::Manifest(m),
                other => NappError::Manifest(other.to_string()),
            })?;

        // Find binary and snapshot its mtime for change detection
        let binary = sidecar_binary(tool_dir)
            .ok_or_else(|| NappError::NotFound("no binary found".into()))?;
        let canonical = std::fs::canonicalize(&binary).unwrap_or(binary.clone());
        let binary_mtime = std::fs::metadata(&canonical)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

        // Validate binary
        sandbox::validate_binary(&binary, 500 * 1024 * 1024)?;

        // Its own data directory in appdata/ (physically separated from code)
        let data_dir = &self.data_dir;
        std::fs::create_dir_all(data_dir)?;

        // Build sanitized environment
        let mut env = sandbox::sanitize_env(
            &manifest.id,
            &manifest.name,
            &manifest.version,
            &tool_dir.to_string_lossy(),
            &data_dir.to_string_lossy(),
            api_port,
        );

        // Per-launch auth token for API authentication
        let app_token = generate_token();
        env.push(("NEBO_APP_TOKEN".into(), app_token.clone()));

        // Launch process
        let mut cmd = command::new::<Command>(&binary, command::Console::Hidden);
        cmd.env_clear();
        for (k, v) in &env {
            cmd.env(k, v);
        }
        // Run in the non-versioned data dir, not the versioned code dir: a
        // sidecar that writes a DB to a relative path (./app.db) then lands in
        // persistent storage that survives updates, instead of the install dir
        // that gets wiped. Bundled code/resources are reached via NEBO_APP_DIR.
        cmd.current_dir(data_dir);
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

        // stderr goes to a dedicated log file in the data directory; stdout
        // is read for the LISTENING line and copied to the same log.
        let log_path = data_dir.join("sidecar.log");
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|e| NappError::Runtime(format!("open sidecar log: {}", e)))?;
        let stdout_log = log_file
            .try_clone()
            .map_err(|e| NappError::Runtime(format!("clone log fd: {}", e)))?;
        cmd.stdout(std::process::Stdio::piped());
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

        // Reap any pre-existing instance of this sidecar binary — an orphan
        // would keep serving an old launch's state beside the new one.
        crate::child_guard::reap_existing_for(&binary);

        let mut child = cmd.spawn().map_err(|e| {
            // A binary the OS refuses to execute (wrong architecture, not
            // executable, gone between the check and the spawn) will refuse
            // again: permanent. Anything else may pass on a retry.
            let refused = e.kind() == std::io::ErrorKind::PermissionDenied
                || e.kind() == std::io::ErrorKind::NotFound
                || matches!(e.raw_os_error(), Some(libc::ENOEXEC))
                || cfg!(target_os = "macos") && e.raw_os_error() == Some(86); // EBADARCH
            if refused {
                NappError::Sandbox(format!("the system will not run it: {e}"))
            } else {
                NappError::Runtime(format!("spawn tool: {}", e))
            }
        })?;

        let pid = child.id().unwrap_or(0);

        // Track for signal-handler cleanup. Unregistered when the process's
        // exit is awaited or it is stopped (`Process::cleanup`).
        crate::child_guard::register_child(pid);

        // Write PID file
        let pid_file = tool_dir.join(format!("{}.pid", manifest.id));
        let _ = std::fs::write(&pid_file, pid.to_string());

        let (announce, announced) = oneshot::channel();
        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(relay_stdout(stdout, tokio::fs::File::from_std(stdout_log), announce));
        }

        let stdin = child.stdin.take();
        let mut process = Process {
            tool_id: manifest.id.clone(),
            manifest,
            pid,
            // Known once the process announces it (below); never handed out before.
            addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            binary_path: binary,
            log_path,
            app_token,
            pid_file,
            binary_mtime,
            child,
            _stdin: stdin,
        };

        // Wait for the process to announce where it listens — or to die
        // first, which is reported with its exit status instead of a bare
        // timeout.
        let timeout = Duration::from_secs(process.manifest.effective_startup_timeout() as u64);
        let exited = |why: String| NappError::Runtime(format!("exited before it was ready: {why}"));
        let announced = tokio::select! {
            r = tokio::time::timeout(timeout, announced) => r,
            why = process.exited() => return Err(exited(why)),
        };
        let listening = match announced {
            Ok(Ok(line)) => parse_listening(&line),
            // Its stdout closed without the line: it is almost always exiting,
            // and its exit status is the reason worth showing.
            Ok(Err(_)) => match tokio::time::timeout(Duration::from_secs(2), process.exited()).await {
                Ok(why) => return Err(exited(why)),
                Err(_) => Err(NappError::Runtime(format!(
                    "closed its output without printing \"{}<address>\"",
                    LISTENING
                ))),
            },
            Err(_) => Err(NappError::Runtime(format!(
                "did not print \"{}<address>\" within {}s",
                LISTENING,
                timeout.as_secs()
            ))),
        };
        match listening {
            Ok(addr) => process.addr = addr,
            Err(e) => {
                process.stop().await;
                return Err(e);
            }
        }

        // Health check: verify the address accepts a connection. Best effort:
        // a sidecar may initialize lazily, so a failure is only logged.
        let health = tokio::time::timeout(Duration::from_secs(5), tokio::net::TcpStream::connect(process.addr)).await;
        if !matches!(health, Ok(Ok(_))) {
            warn!(
                tool = process.manifest.id.as_str(),
                addr = %process.addr,
                "health check failed after the sidecar announced its address (tool may use lazy init)"
            );
        }

        info!(tool = process.manifest.id.as_str(), pid, addr = %process.addr, "tool launched");

        Ok(process)
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

/// The address in a sidecar's [`LISTENING`] line. Only a loopback address is
/// accepted: a sidecar is never reachable from another computer.
fn parse_listening(line: &str) -> Result<SocketAddr, NappError> {
    let addr: SocketAddr = line
        .parse()
        .map_err(|e| NappError::Runtime(format!("announced \"{line}\", which is not an address: {e}")))?;
    if !addr.ip().is_loopback() || addr.port() == 0 {
        return Err(NappError::Runtime(format!(
            "announced {addr}; a sidecar must listen on a loopback address (127.0.0.1)"
        )));
    }
    Ok(addr)
}

/// Copy the sidecar's stdout into its log, line by line, and hand the address
/// of the first [`LISTENING`] line to `announce`. Reads until the process
/// closes its stdout, so a sidecar never blocks on a full pipe.
async fn relay_stdout(
    stdout: tokio::process::ChildStdout,
    mut log: tokio::fs::File,
    announce: oneshot::Sender<String>,
) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let mut announce = Some(announce);
    let mut reader = BufReader::new(stdout);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if let Some(addr) = String::from_utf8_lossy(&line).trim_end().strip_prefix(LISTENING) {
            if let Some(tx) = announce.take() {
                let _ = tx.send(addr.trim().to_string());
            }
        }
        if !line.ends_with(b"\n") {
            line.push(b'\n');
        }
        let _ = log.write_all(&line).await;
        let _ = log.flush().await;
    }
}

/// The program a tool directory runs, if it has one — the ONE rule for
/// where a sidecar binary lives. The agent loader records an app's binary with
/// it and the runtime launches with it, so "this app has a program" means the
/// same thing everywhere. An app with no program (a UI-only app) is not an
/// error: there is simply nothing to run.
pub fn sidecar_binary(tool_dir: &Path) -> Option<PathBuf> {
    for name in &["binary", "app"] {
        let path = tool_dir.join(name);
        if path.exists() {
            return Some(path);
        }
    }
    // Check tmp/
    let tmp = tool_dir.join("tmp");
    if tmp.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&tmp) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    return Some(path);
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
                    return Some(path);
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
                                return Some(path);
                            }
                        }
                    }
                    #[cfg(not(unix))]
                    {
                        return Some(path);
                    }
                }
            }
        }
    }
    None
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

    /// No computer is ever exposed: an address another computer could reach
    /// is refused, whatever the sidecar announces.
    #[test]
    fn only_a_loopback_address_is_accepted() {
        assert_eq!(parse_listening("127.0.0.1:5000").unwrap(), "127.0.0.1:5000".parse().unwrap());
        for line in ["0.0.0.0:5000", "192.168.1.20:5000", "[::]:5000", "127.0.0.1:0", "/tmp/app.sock"] {
            assert!(parse_listening(line).is_err(), "{line} must be refused");
        }
    }
}

/// The fixture sidecar, launched for real.
#[cfg(all(test, unix))]
mod process_tests {
    use super::*;
    use crate::test_sidecar::{self, TestApp};

    async fn launch(app: &TestApp) -> Process {
        let data_dir = crate::app_data::data_dir(&app.home, crate::app_data::DataKind::App, &app.id).expect("app id");
        Runtime::new(data_dir).launch(&app.tool_dir, 0).await.expect("launch")
    }

    /// Nebo holds the write end of the sidecar's stdin for the sidecar's
    /// whole life. When it closes — Nebo gone by any means, SIGKILL included
    /// — the sidecar exits on its own.
    #[tokio::test]
    async fn the_sidecar_exits_when_its_stdin_closes() {
        let app = TestApp::new("stdin-eof");
        let mut process = launch(&app).await;
        assert!(tokio::net::TcpStream::connect(process.addr).await.is_ok(), "it serves");
        drop(process._stdin.take());
        let why = tokio::time::timeout(Duration::from_secs(10), process.exited()).await.expect("it exits on its own");
        assert!(why.starts_with("exited with code 0"), "{why}");
        assert!(tokio::net::TcpStream::connect(process.addr).await.is_err(), "nothing listens");
    }

    #[tokio::test]
    async fn the_sidecar_exits_when_nebo_stops_it() {
        let app = TestApp::new("stopped");
        let mut process = launch(&app).await;
        let (pid, addr) = (process.pid, process.addr);
        process.stop().await;
        assert!(!test_sidecar::exists(pid), "the process is gone");
        assert!(tokio::net::TcpStream::connect(addr).await.is_err(), "nothing listens");
        assert!(!process.pid_file.exists(), "its pid file is removed");
    }
}
