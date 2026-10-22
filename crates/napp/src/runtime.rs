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
/// <NEBO_SIDECAR_TOKEN>`. Nebo reads the address from this line, connects only
/// to a loopback address, and sends that token on every call.
///
/// Two tokens, one direction each, both per launch: `NEBO_SIDECAR_TOKEN`
/// opens the sidecar to Nebo, `NEBO_APP_TOKEN` opens the app's Nebo routes to
/// the sidecar. Nebo never sends `NEBO_APP_TOKEN` anywhere, so a process that
/// takes over a sidecar's port learns only a token that opens nothing in Nebo.
pub const LISTENING: &str = "nebo-app-listening ";

/// The request header carrying a health probe's nonce, and the response header
/// carrying the sidecar's proof (see [`proof`]).
const CHALLENGE: &str = "x-nebo-challenge";
const PROOF: &str = "x-nebo-proof";

/// How long a health probe waits for the sidecar's answer.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The longest line of a sidecar's stdout Nebo holds at once; a longer one is
/// logged in pieces of this size.
const MAX_LINE: u64 = 64 * 1024;

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
    /// to Nebo's API with it. Only ever accepted by Nebo, never sent by it.
    pub app_token: String,
    /// Per-launch token, passed as NEBO_SIDECAR_TOKEN: Nebo authenticates to
    /// the sidecar with it on every call. It opens nothing in Nebo.
    pub sidecar_token: String,
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

        // Per-launch tokens, one for each direction.
        let app_token = generate_token();
        env.push(("NEBO_APP_TOKEN".into(), app_token.clone()));
        let sidecar_token = generate_token();
        env.push(("NEBO_SIDECAR_TOKEN".into(), sidecar_token.clone()));

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
            sidecar_token,
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

        // Health check: the same challenge the supervisor probes with. A
        // sidecar that answers without proving itself is refused at once; one
        // that does not answer yet may be initializing lazily, so that is only
        // logged and the supervisor's probes decide whether it is alive.
        match probe(process.addr, &process.sidecar_token, &process.app_token).await {
            Health::Proven => {}
            Health::Unreachable => warn!(
                tool = process.manifest.id.as_str(),
                addr = %process.addr,
                "health check failed after the sidecar announced its address (tool may use lazy init)"
            ),
            Health::Unproven(why) => {
                process.stop().await;
                return Err(NappError::Runtime(format!("{} ({why})", Health::UNPROVEN)));
            }
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
/// closes its stdout, so a sidecar never blocks on a full pipe, and holds at
/// most [`MAX_LINE`] bytes of a line at once.
async fn relay_stdout(
    stdout: impl tokio::io::AsyncRead + Unpin,
    mut log: impl tokio::io::AsyncWrite + Unpin,
    announce: oneshot::Sender<String>,
) {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    let mut announce = Some(announce);
    let mut reader = BufReader::new(stdout);
    let mut line = Vec::new();
    loop {
        line.clear();
        match (&mut reader).take(MAX_LINE).read_until(b'\n', &mut line).await {
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

/// The sidecar's UI service at `addr`, ready for calls.
fn client(addr: SocketAddr) -> Result<proto::ui_service_client::UiServiceClient<tonic::transport::Channel>, tonic::Status> {
    let channel = tonic::transport::Endpoint::from_shared(format!("http://{addr}"))
        .map_err(|e| tonic::Status::internal(format!("sidecar address {addr}: {e}")))?
        .connect_lazy();
    Ok(proto::ui_service_client::UiServiceClient::new(channel).max_decoding_message_size(32 * 1024 * 1024))
}

/// `msg` as a call carrying the launch's sidecar token.
fn authorized<T>(sidecar_token: &str, msg: T) -> Result<tonic::Request<T>, tonic::Status> {
    let mut req = tonic::Request::new(msg);
    let bearer = format!("Bearer {sidecar_token}")
        .parse()
        .map_err(|_| tonic::Status::internal("the sidecar token is not a header value"))?;
    req.metadata_mut().insert("authorization", bearer);
    Ok(req)
}

/// One `UIService.HandleRequest` call to the sidecar at `addr`, carrying its
/// sidecar token. The one way Nebo talks to a sidecar: the app proxy and the
/// sidecar tools both come through it ([`crate::supervisor::Launched::handle_request`]).
pub(crate) async fn handle_request(
    addr: SocketAddr,
    sidecar_token: &str,
    req: proto::HttpRequest,
) -> Result<proto::HttpResponse, tonic::Status> {
    client(addr)?.handle_request(authorized(sidecar_token, req)?).await.map(|r| r.into_inner())
}

/// What a health probe found at a sidecar's address.
#[derive(Debug, PartialEq)]
pub enum Health {
    /// It answered and proved it holds this launch's `NEBO_APP_TOKEN`.
    Proven,
    /// Nothing answered: not listening, not speaking gRPC, or too slow.
    Unreachable,
    /// Something answered but did not prove it is this app's sidecar: no
    /// proof, a wrong one, or the sidecar token refused. Never retried.
    Unproven(String),
}

impl Health {
    pub const UNPROVEN: &'static str = "answered its health check without proving it is this app's sidecar";
}

/// The health proof for one probe: `HMAC-SHA256(key = NEBO_APP_TOKEN,
/// nonce || NEBO_SIDECAR_TOKEN || port as 2 bytes big-endian)`. Only the
/// sidecar Nebo launched holds `NEBO_APP_TOKEN` — Nebo never sends it — so a
/// process that took the sidecar's port and saw the sidecar token on Nebo's
/// calls still cannot compute it. The port binds the proof to the address the
/// sidecar announced.
pub fn proof(app_token: &str, nonce: &[u8], sidecar_token: &str, port: u16) -> [u8; 32] {
    use hmac::Mac;
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(app_token.as_bytes()).expect("HMAC takes a key of any length");
    mac.update(nonce);
    mac.update(sidecar_token.as_bytes());
    mac.update(&port.to_be_bytes());
    mac.finalize().into_bytes().into()
}

/// The health probe, at launch, on the supervisor's timer and on revive.
/// A bare connection proves nothing — if the sidecar stopped listening,
/// another process may hold its port. Nebo calls `UIService.HealthCheck` with
/// the sidecar token and a fresh nonce ([`CHALLENGE`]), and the answer must
/// carry the [`proof`] for it ([`PROOF`]), checked in constant time.
pub async fn probe(addr: SocketAddr, sidecar_token: &str, app_token: &str) -> Health {
    use hmac::Mac;
    use rand::Rng;
    let nonce: [u8; 32] = rand::thread_rng().r#gen();
    let call = async {
        let mut req = authorized(sidecar_token, proto::HealthCheckRequest {})?;
        let challenge = hex::encode(nonce).parse().map_err(|_| tonic::Status::internal("challenge header"))?;
        req.metadata_mut().insert(CHALLENGE, challenge);
        client(addr)?.health_check(req).await
    };
    let answer = match tokio::time::timeout(PROBE_TIMEOUT, call).await {
        Err(_) => return Health::Unreachable,
        Ok(Err(status)) if unreachable(&status) => return Health::Unreachable,
        // Whatever holds the address answered with a status of its own, any
        // status: the sidecar answers its probe with a proof, never a refusal.
        Ok(Err(status)) => return Health::Unproven(format!("{}: {}", status.code(), status.message())),
        Ok(Ok(answer)) => answer,
    };
    let Some(presented) = answer.metadata().get(PROOF).and_then(|v| v.to_str().ok()).and_then(|v| hex::decode(v).ok()) else {
        return Health::Unproven("no proof".into());
    };
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(app_token.as_bytes()).expect("HMAC takes a key of any length");
    mac.update(&nonce);
    mac.update(sidecar_token.as_bytes());
    mac.update(&addr.port().to_be_bytes());
    match mac.verify_slice(&presented) {
        Ok(()) => Health::Proven,
        Err(_) => Health::Unproven("a wrong proof".into()),
    }
}

/// Whether a failed probe found nothing to answer it: the connection could
/// not be made, or it broke before a gRPC answer came back. tonic keeps the
/// transport's own error as the status's source then; a status the peer sent
/// has none. Only this is a miss the supervisor may wait out: a status the
/// peer chose (`UNAVAILABLE` included) is an answer without a proof.
fn unreachable(status: &tonic::Status) -> bool {
    std::error::Error::source(status).is_some()
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

    /// A sidecar that writes one endless line can't make Nebo hold it all:
    /// it is logged in pieces of at most MAX_LINE, and the announce after it
    /// still lands.
    #[tokio::test]
    async fn a_huge_line_is_logged_in_bounded_pieces() {
        let huge = vec![b'x'; (MAX_LINE as usize) * 3 + 10];
        let mut out = huge.clone();
        out.extend_from_slice(b"\nnebo-app-listening 127.0.0.1:5000\n");
        let mut log = Vec::new();
        let (tx, rx) = oneshot::channel();
        relay_stdout(&out[..], &mut log, tx).await;
        assert_eq!(rx.await.unwrap(), "127.0.0.1:5000");
        let pieces: Vec<&[u8]> = log.split(|b| *b == b'\n').filter(|p| !p.is_empty()).collect();
        assert!(pieces.iter().all(|p| p.len() as u64 <= MAX_LINE), "every piece is bounded");
        assert_eq!(pieces.iter().map(|p| p.len()).sum::<usize>(), huge.len() + "nebo-app-listening 127.0.0.1:5000".len());
    }

    /// The vector every SDK checks its own proof against: the formula is one
    /// contract across Nebo and the Rust, Go and C SDKs.
    #[test]
    fn the_proof_matches_the_shared_vector() {
        let nonce: Vec<u8> = (0u8..32).collect();
        let p = proof(
            "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210",
            &nonce,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            54321,
        );
        assert_eq!(hex::encode(p), "f67ca363caac14eb7f345df8c7548d11a40334c55d255e30ec02339265ed8db8");
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
#[cfg(test)]
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

    /// A probe tells nothing-answered from something-answered. A closed port
    /// is a miss the supervisor may wait out; a server that answers the
    /// probe with a status of its own, `UNAVAILABLE` included, is unproven
    /// and is never sent another request.
    #[tokio::test]
    async fn a_status_the_peer_sent_is_unproven_and_a_closed_port_is_unreachable() {
        use proto::ui_service_server::{UiService, UiServiceServer};
        struct Refuses;
        #[tonic::async_trait]
        impl UiService for Refuses {
            async fn health_check(
                &self,
                _: tonic::Request<proto::HealthCheckRequest>,
            ) -> Result<tonic::Response<proto::HealthCheckResponse>, tonic::Status> {
                Err(tonic::Status::unavailable("busy, try later"))
            }
            async fn configure(&self, _: tonic::Request<proto::SettingsMap>) -> Result<tonic::Response<proto::Empty>, tonic::Status> {
                Ok(tonic::Response::new(proto::Empty {}))
            }
            async fn handle_request(
                &self,
                _: tonic::Request<proto::HttpRequest>,
            ) -> Result<tonic::Response<proto::HttpResponse>, tonic::Status> {
                Err(tonic::Status::unavailable("busy"))
            }
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let incoming = futures::stream::unfold(listener, |l| async move { Some((l.accept().await.map(|(s, _)| s), l)) });
        tokio::spawn(tonic::transport::Server::builder().add_service(UiServiceServer::new(Refuses)).serve_with_incoming(incoming));
        match probe(addr, "sidecar-token", "app-token").await {
            Health::Unproven(why) => assert!(why.contains("busy, try later"), "{why}"),
            other => panic!("a peer's own UNAVAILABLE must be unproven, got {other:?}"),
        }

        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        assert_eq!(probe(closed, "sidecar-token", "app-token").await, Health::Unreachable);
    }
}
