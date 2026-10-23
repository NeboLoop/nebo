use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::plugin::PluginStore;

/// Environment variables that can be exploited for code injection.
const DANGEROUS_ENV_VARS: &[&str] = &[
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "DYLD_FRAMEWORK_PATH",
    "IFS",
    "CDPATH",
    "BASH_ENV",
    "ENV",
    "PROMPT_COMMAND",
    "SHELLOPTS",
    "BASHOPTS",
    "GLOBIGNORE",
    "PYTHONSTARTUP",
    "PYTHONPATH",
    "RUBYOPT",
    "RUBYLIB",
    "PERL5OPT",
    "PERL5LIB",
    "PERL5DB",
    "NODE_OPTIONS",
];

/// Why a plugin launch failed. Distinguishes "never started" from "ran too long"
/// from "we lost the child", so callers can report the real cause instead of
/// flattening every failure into one string.
#[derive(Debug)]
pub enum LaunchError {
    /// The process could not be started at all.
    Spawn(std::io::Error),
    /// Exceeded its timeout; the child has been killed.
    TimedOut { after: Duration },
    /// Started, but waiting on it failed.
    Wait(std::io::Error),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LaunchError::Spawn(e) => write!(f, "failed to start: {e}"),
            LaunchError::TimedOut { after } => {
                write!(f, "timed out after {}s", after.as_secs())
            }
            LaunchError::Wait(e) => write!(f, "failed while running: {e}"),
        }
    }
}

impl std::error::Error for LaunchError {}

/// Shlex-split a plugin command string into argv, with a whitespace fallback
/// for strings shlex rejects. The ONE splitting rule for plugin commands —
/// callers that need "command string plus extra args" split here and append.
pub fn split_command(args_str: &str) -> Vec<String> {
    shlex::split(args_str)
        .unwrap_or_else(|| args_str.split_whitespace().map(String::from).collect())
}

/// How the app finds the Chromium-family browser it draws with (Chrome,
/// Chromium, Edge, Brave: one that paints, unlike the headless Obscura).
/// Registered once at startup by the app (nebo-browser's `find_chrome`).
static CHROMIUM_RESOLVER: OnceLock<fn() -> Option<PathBuf>> = OnceLock::new();
static CHROMIUM: OnceLock<Option<String>> = OnceLock::new();

/// Register how plugins' `NEBO_CHROMIUM` is found (called once at startup).
pub fn set_chromium_resolver(find: fn() -> Option<PathBuf>) {
    let _ = CHROMIUM_RESOLVER.set(find);
}

/// `NEBO_CHROMIUM` for a plugin: the one set in Nebo's own environment
/// wins, else the browser `find` locates; none when neither has one.
pub fn pick_chromium(own: Option<String>, find: impl FnOnce() -> Option<PathBuf>) -> Option<String> {
    match own.filter(|v| !v.trim().is_empty()) {
        Some(v) => Some(v),
        None => find().map(|p| p.to_string_lossy().into_owned()),
    }
}

/// The browser plugins are handed as `NEBO_CHROMIUM` (looked up once).
fn chromium() -> Option<String> {
    let own = std::env::var("NEBO_CHROMIUM").ok();
    // Until the app registers its resolver, nothing is cached.
    let Some(find) = CHROMIUM_RESOLVER.get().copied() else { return pick_chromium(own, || None) };
    CHROMIUM.get_or_init(|| pick_chromium(own, find)).clone()
}

/// Return a sanitized copy of the current process environment,
/// stripping dangerous loader/shell injection vars, and the install key
/// (`NEBO_MCP_API_KEY`): it proves the owner's own client to Nebo's local
/// API, and a process Nebo starts gets a credential of its own, scoped to
/// what it calls (`plugin_base_env`, an app's `NEBO_APP_TOKEN`), never it.
pub fn sanitized_env() -> Vec<(String, String)> {
    let dangerous: std::collections::HashSet<&str> = DANGEROUS_ENV_VARS.iter().copied().collect();

    std::env::vars()
        .filter(|(k, _)| {
            let upper = k.to_uppercase();
            if dangerous.contains(upper.as_str()) || upper == "NEBO_MCP_API_KEY" {
                return false;
            }
            if upper.starts_with("BASH_FUNC_")
                || upper.starts_with("LD_")
                || upper.starts_with("DYLD_")
            {
                return false;
            }
            true
        })
        .collect()
}

/// Builder for plugin process environment and commands.
///
/// Encapsulates the env setup pattern shared across all plugin spawn sites:
/// env_clear, sanitized env, plugin binary/data vars, PATH, auth, and
/// optional deps/home/agent-config/permissions.
pub struct PluginRuntime {
    slug: String,
    binary_path: PathBuf,
    plugin_store: Arc<PluginStore>,
    include_deps: bool,
    include_home: bool,
    agent_config: Option<HashMap<String, String>>,
    enforce_permissions: bool,
    /// Caller-supplied env applied LAST, so it wins over everything else.
    /// Carries per-invocation context (channel ids, per-agent account dirs)
    /// that callers used to set by hand-rolling their own Command.
    extra_env: Vec<(String, String)>,
    /// Where [`run_capture_args`](Self::run_capture_args) sends each line
    /// the plugin writes (stdout and stderr), as it writes it: a caller
    /// showing a long command's progress. The whole output is captured
    /// either way.
    output: Option<tokio::sync::mpsc::UnboundedSender<String>>,
}

impl PluginRuntime {
    pub fn new(slug: &str, binary_path: PathBuf, plugin_store: Arc<PluginStore>) -> Self {
        Self {
            slug: slug.to_string(),
            binary_path,
            plugin_store,
            include_deps: false,
            include_home: false,
            agent_config: None,
            enforce_permissions: false,
            extra_env: Vec::new(),
            output: None,
        }
    }

    /// Add one env var to this invocation. Applied after all other sources, so
    /// it overrides them — that ordering is what per-agent account isolation
    /// depends on.
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_env.push((key.into(), value.into()));
        self
    }

    pub fn with_deps(mut self) -> Self {
        self.include_deps = true;
        self
    }

    pub fn with_home(mut self) -> Self {
        self.include_home = true;
        self
    }

    pub fn with_agent_config(mut self, cfg: HashMap<String, String>) -> Self {
        self.agent_config = Some(cfg);
        self
    }

    pub fn with_permissions(mut self) -> Self {
        self.enforce_permissions = true;
        self
    }

    /// Send each line the plugin writes to `lines` as it is written
    /// ([`PluginRuntime::output`]).
    pub fn with_output(mut self, lines: tokio::sync::mpsc::UnboundedSender<String>) -> Self {
        self.output = Some(lines);
        self
    }

    /// Build the full set of env vars for this plugin invocation.
    ///
    /// Duplicate keys resolve last-wins when applied to the Command, so vec
    /// order IS the precedence order: manifest auth defaults < inherited
    /// (sanitized) process env < stored user auth values < caller extra_env.
    pub fn build_env(&self) -> Vec<(String, String)> {
        let (manifest_auth, stored_auth) = self.plugin_store.auth_env_layers(&self.slug);
        let mut inherited = sanitized_env();

        // Permission filtering on the sanitized base only — the plugin's own
        // declared auth env is never stripped by its own allow/deny lists.
        if self.enforce_permissions {
            if let Some(manifest) = self.plugin_store.get_manifest(&self.slug) {
                if let Some(ref perms) = manifest.permissions {
                    if !perms.env_allow.is_empty() {
                        let allow: std::collections::HashSet<&str> =
                            perms.env_allow.iter().map(|s| s.as_str()).collect();
                        inherited.retain(|(k, _)| allow.contains(k.as_str()));
                    }
                    if !perms.env_deny.is_empty() {
                        let deny: std::collections::HashSet<&str> =
                            perms.env_deny.iter().map(|s| s.as_str()).collect();
                        inherited.retain(|(k, _)| !deny.contains(k.as_str()));
                    }
                }
            }
        }

        // Manifest auth DEFAULTS go first so anything inherited from the host
        // process overrides them (e.g. a cloud pod's Web-type OAuth client);
        // user-stored auth values are pushed later and win over both.
        let mut env: Vec<(String, String)> = manifest_auth.into_iter().collect();
        env.append(&mut inherited);

        // Plugin binary env var (e.g., GWS_BIN=/path/to/gws)
        env.push((
            crate::plugin::plugin_env_var(&self.slug),
            self.binary_path.to_string_lossy().into_owned(),
        ));

        // Per-artifact persistent data directory (non-versioned, slug-keyed).
        // ONE canonical name across plugins, apps, and skills: NEBO_DATA_DIR.
        // Pushed after the permission-filter pass above so it's never stripped,
        // and after sanitized_env so it overrides any inherited NEBO_DATA_DIR
        // (the deprecated root override) with this plugin's own data dir.
        let plugin_data = self.plugin_store.plugin_data_dir(&self.slug);
        if let Err(e) = std::fs::create_dir_all(&plugin_data) {
            tracing::warn!(plugin = %self.slug, error = %e, "failed to create plugin data directory");
        }
        env.push((
            "NEBO_DATA_DIR".into(),
            plugin_data.to_string_lossy().into_owned(),
        ));

        // Dependency plugin binary vars
        if self.include_deps {
            for dep in self.plugin_store.get_dependencies(&self.slug) {
                if let Some(dep_path) = self.plugin_store.resolve(&dep.name, &dep.version) {
                    env.push((
                        crate::plugin::plugin_env_var(&dep.name),
                        dep_path.to_string_lossy().into_owned(),
                    ));
                }
            }
        }

        // Augmented PATH
        env.push(("PATH".into(), self.plugin_store.path_with_plugins()));

        // The browser Nebo draws with, for plugins that render web pages
        // (e.g. Nebo Media's HTML motion templates). After the permission
        // filter so an env allowlist never hides it; absent when none is found.
        if let Some(browser) = chromium() {
            env.push(("NEBO_CHROMIUM".into(), browser));
        }

        // User-stored auth values (plugin settings) — the top auth layer.
        for (k, v) in stored_auth {
            if !v.is_empty() {
                env.push((k, v));
            }
        }
        // Owner's NeboAI sign-in for first-party plugins (`auth.type: neboai`).
        env.extend(self.plugin_store.neboai_auth_env(&self.slug));

        // HOME preservation for credential lookups
        if self.include_home {
            if let Ok(home) = std::env::var("HOME") {
                env.push(("HOME".into(), home));
            }
        }

        // Per-agent config overrides
        if let Some(ref cfg) = self.agent_config {
            for (k, v) in cfg {
                env.push((k.clone(), v.clone()));
            }
        }

        // Caller-supplied env LAST so it wins — per-agent account directories
        // must override any global value of the same key.
        for (k, v) in &self.extra_env {
            env.push((k.clone(), v.clone()));
        }

        env
    }

    /// Build a Command with env_clear + all env vars + shlex-parsed args.
    /// `kill_on_drop` is set so plugin sidecars die with their parent rather
    /// than orphaning during nebo restart/crash.
    pub fn command(&self, args_str: &str) -> tokio::process::Command {
        self.command_args(&split_command(args_str))
    }

    /// Same as [`command`], for callers that already hold split arguments.
    ///
    /// Prefer this whenever the args were built programmatically: round-tripping
    /// them through a string and re-splitting mangles anything containing spaces
    /// or quotes.
    pub fn command_args(&self, args: &[String]) -> tokio::process::Command {
        let mut cmd = command::new::<tokio::process::Command>(&self.binary_path, command::Console::Hidden);
        cmd.args(args);
        cmd.env_clear();
        for (k, v) in self.build_env() {
            cmd.env(k, v);
        }
        // Run in the plugin's non-versioned data dir so a relative-path write
        // (e.g. a DB at ./plugin.sqlite) lands in persistent storage that
        // survives upgrades, not the version dir that gets wiped. Same data dir
        // exported as NEBO_DATA_DIR; the binary is resolved by absolute path.
        let data_dir = self.plugin_store.plugin_data_dir(&self.slug);
        let _ = std::fs::create_dir_all(&data_dir);
        cmd.current_dir(&data_dir);
        cmd.kill_on_drop(true);
        cmd
    }

    /// Run a plugin command to completion and capture its output.
    ///
    /// THE canonical one-shot launch. Every caller that needs a plugin's stdout
    /// must use this rather than building its own `Command`, because the three
    /// guarantees below have to hold together and had drifted apart across
    /// eight separate spawn sites:
    ///
    /// 1. **The timeout is mandatory.** There is deliberately no unbounded
    ///    variant — a plugin that hangs on a network call must not hang Nebo.
    /// 2. **The child is killed when the timeout fires.** `command()` sets
    ///    `kill_on_drop`, so dropping the wait future reaps the process. Without
    ///    it, tokio leaves the child running: it survives Nebo, reparents to
    ///    launchd/init, and never exits. One customer box accumulated 330 such
    ///    orphans in 30 hours until it ran out of file descriptors and every
    ///    outbound request began failing.
    /// 3. **The pid is tracked for its whole life**, including the timeout path,
    ///    so the reaper never SIGKILLs a live probe and shutdown never leaves
    ///    strays behind.
    pub async fn run_capture(
        &self,
        args_str: &str,
        requested_timeout: Duration,
    ) -> Result<std::process::Output, LaunchError> {
        self.run_capture_args(&split_command(args_str), requested_timeout).await
    }

    /// Same as [`run_capture`], for callers that already hold split arguments.
    pub async fn run_capture_args(
        &self,
        args: &[String],
        requested_timeout: Duration,
    ) -> Result<std::process::Output, LaunchError> {
        let timeout = self.effective_timeout(requested_timeout);
        let mut cmd = self.command_args(args);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = spawn_with_etxtbsy_retry(&mut cmd).await?;
        let pid = child.id();
        if let Some(p) = pid {
            crate::child_guard::register_child(p);
        }
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let result = tokio::time::timeout(timeout, async {
            // Both pipes are read while the process runs, as
            // `wait_with_output` reads them: a full pipe would stall it.
            let (status, stdout, stderr) =
                tokio::join!(child.wait(), read_pipe(stdout, self.output.clone()), read_pipe(stderr, self.output.clone()));
            Ok::<_, std::io::Error>(std::process::Output {
                status: status?,
                stdout: stdout?,
                stderr: stderr?,
            })
        })
        .await;
        if let Some(p) = pid {
            crate::child_guard::unregister_child(p);
        }

        match result {
            Ok(Ok(output)) => Ok(output),
            Ok(Err(e)) => Err(LaunchError::Wait(e)),
            // The child is dropped when this returns, which triggers kill_on_drop.
            Err(_) => Err(LaunchError::TimedOut { after: timeout }),
        }
    }

    /// Spawn a long-lived plugin process (channel bridge, sidecar) with stdin,
    /// stdout and stderr piped, and register it with the child guard.
    ///
    /// The caller owns the returned `Child` for the process's lifetime — dropping
    /// it kills the process (`kill_on_drop`), which is what makes bridges die with
    /// Nebo instead of orphaning. Callers MUST `unregister_child` when the process
    /// exits so the guard's pid set does not grow without bound.
    pub fn spawn_streaming(&self, args: &[String]) -> Result<tokio::process::Child, LaunchError> {
        let mut cmd = self.command_args(args);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let child = spawn_with_etxtbsy_retry_blocking(&mut cmd)?;
        if let Some(p) = child.id() {
            crate::child_guard::register_child(p);
        }
        Ok(child)
    }

    /// Resolve effective timeout: min(caller_timeout, manifest max_timeout_seconds).
    pub fn effective_timeout(&self, requested: Duration) -> Duration {
        if !self.enforce_permissions {
            return requested;
        }
        let max = self
            .plugin_store
            .get_manifest(&self.slug)
            .and_then(|m| m.permissions)
            .map(|p| p.max_timeout_seconds)
            .unwrap_or(300);
        requested.min(Duration::from_secs(max))
    }
}

/// The most of one output pipe a run keeps. The rest is read and let go, so
/// the plugin never stalls on a full pipe, and the output says it was cut.
const CAPTURE_MAX: usize = 32 * 1024 * 1024;

/// The longest progress line sent on; a longer one is cut.
const LINE_MAX: usize = 4096;

/// What a plugin's output may carry back to whoever reads it (an employee's
/// model, an app's page): never the credential in its `NEBO_LOCAL_URL`
/// ([`crate::plugin::plugin_local_token`]: one for every plugin, for the
/// life of the process). A plugin that prints the address (an HTTP client's
/// error names it) shows it without the credential.
pub fn without_local_credential(bytes: &[u8]) -> Vec<u8> {
    let token = crate::plugin::plugin_local_token().as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while let Some(at) = rest.windows(token.len()).position(|w| w == token) {
        out.extend_from_slice(&rest[..at]);
        out.extend_from_slice(b"<credential>");
        rest = &rest[at + token.len()..];
    }
    out.extend_from_slice(rest);
    out
}

/// Read one of a plugin's output pipes to its end, sending each line to
/// `lines` as it arrives. A carriage return ends a line too: a progress bar
/// redraws its line with one. Keeps at most [`CAPTURE_MAX`] bytes, and what
/// it keeps and sends carries no local credential
/// ([`without_local_credential`]): taken out before any cut, and a cut
/// that would end on the start of one drops that start.
async fn read_pipe(
    pipe: Option<impl tokio::io::AsyncRead + Unpin>,
    lines: Option<tokio::sync::mpsc::UnboundedSender<String>>,
) -> std::io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut all = Vec::new();
    let Some(mut pipe) = pipe else { return Ok(all) };
    let mut buf = [0u8; 8192];
    let mut line_start = 0;
    let mut cut = false;
    loop {
        let n = pipe.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        if cut {
            continue;
        }
        // Only the new bytes are searched for a line's end.
        let scan_from = all.len();
        if all.len() + n > CAPTURE_MAX {
            all.extend_from_slice(&buf[..CAPTURE_MAX - all.len()]);
            cut = true;
        } else {
            all.extend_from_slice(&buf[..n]);
        }
        if let Some(tx) = &lines {
            let mut from = scan_from;
            while let Some(end) = all[from..].iter().position(|b| *b == b'\n' || *b == b'\r') {
                send_line(tx, &all[line_start..from + end], false);
                line_start = from + end + 1;
                from = line_start;
            }
        }
    }
    if let Some(tx) = &lines {
        send_line(tx, &all[line_start..], cut);
    }
    let mut kept = without_local_credential(&all);
    if cut {
        drop_credential_start(&mut kept);
        kept.extend_from_slice(format!("\n[Output cut: only the first {} MB were kept.]\n", CAPTURE_MAX / (1024 * 1024)).as_bytes());
    }
    Ok(kept)
}

/// Drop a tail of `bytes` that is the start of the local credential: what a
/// cut through the credential would leave.
fn drop_credential_start(bytes: &mut Vec<u8>) {
    let token = crate::plugin::plugin_local_token().as_bytes();
    if let Some(k) = (1..token.len()).rev().find(|&k| bytes.ends_with(&token[..k])) {
        bytes.truncate(bytes.len() - k);
    }
}

/// Send one line: the credential taken out of the whole line first, then
/// cut to [`LINE_MAX`]. `cut_short`: the line ends where the capture was
/// cut, so a start of the credential there is dropped too.
fn send_line(tx: &tokio::sync::mpsc::UnboundedSender<String>, bytes: &[u8], cut_short: bool) {
    let mut bytes = without_local_credential(bytes);
    if cut_short {
        drop_credential_start(&mut bytes);
    }
    bytes.truncate(LINE_MAX);
    let line = String::from_utf8_lossy(&bytes);
    let line = line.trim_end();
    if !line.is_empty() {
        let _ = tx.send(line.to_string());
    }
}

/// Spawn with a brief retry on ETXTBSY ("Text file busy").
///
/// The classic Unix fork/exec race: any other thread's fork (a parallel
/// plugin spawn, an install writing a different binary) briefly inherits
/// this binary's write-fd between its fork and exec — Rust opens files
/// CLOEXEC, so the window is real but milliseconds wide. Exec'ing inside
/// that window fails ETXTBSY even though nothing is actually writing the
/// file. Retrying for a moment is the standard remedy; failing a plugin
/// launch over a fd inherited for a millisecond is not.
async fn spawn_with_etxtbsy_retry(
    cmd: &mut tokio::process::Command,
) -> Result<tokio::process::Child, LaunchError> {
    let mut delay = std::time::Duration::from_millis(10);
    for _ in 0..6 {
        match cmd.spawn() {
            Ok(child) => return Ok(child),
            Err(e) if e.raw_os_error() == Some(26) => {
                tokio::time::sleep(delay).await;
                delay *= 2; // 10..320ms, ~630ms total
            }
            Err(e) => return Err(LaunchError::Spawn(e)),
        }
    }
    cmd.spawn().map_err(LaunchError::Spawn)
}

/// The sync twin for `spawn_streaming`, whose callers are not async. The
/// blocking sleeps are bounded (~630ms worst case) and only ever taken while
/// the fork/exec race is actually in progress — the common path is one spawn.
fn spawn_with_etxtbsy_retry_blocking(
    cmd: &mut tokio::process::Command,
) -> Result<tokio::process::Child, LaunchError> {
    let mut delay = std::time::Duration::from_millis(10);
    for _ in 0..6 {
        match cmd.spawn() {
            Ok(child) => return Ok(child),
            Err(e) if e.raw_os_error() == Some(26) => {
                std::thread::sleep(delay);
                delay *= 2;
            }
            Err(e) => return Err(LaunchError::Spawn(e)),
        }
    }
    cmd.spawn().map_err(LaunchError::Spawn)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A caller that asks for the output line by line gets each line as the
    /// plugin writes it (a progress bar's carriage returns included), and
    /// the whole output is still captured.
    #[tokio::test]
    async fn each_line_is_sent_as_it_is_written_and_the_output_is_still_whole() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(PluginStore::new(tmp.path().join("plugins"), tmp.path().join("user"), None));
        let bin = tmp.path().join("progress");
        std::fs::write(&bin, "#!/bin/sh\nprintf 'step 1\\r'\nsleep 0.3\nprintf 'step 2\\n'\necho warn >&2\nprintf 'done'\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let run = PluginRuntime::new("progress", bin, store).with_output(tx);
        let mut task = tokio::spawn(async move { run.run_capture_args(&[], Duration::from_secs(60)).await });
        let first = tokio::time::timeout(Duration::from_secs(30), rx.recv()).await.unwrap();
        assert_eq!(first.as_deref(), Some("step 1"), "the first line arrives before the plugin ends");
        assert!(tokio::time::timeout(Duration::from_millis(100), &mut task).await.is_err(), "the plugin is still running");

        let output = task.await.unwrap().unwrap();
        let mut rest = Vec::new();
        while let Some(line) = rx.recv().await {
            rest.push(line);
        }
        rest.sort();
        assert_eq!(rest, ["done", "step 2", "warn"]);
        assert_eq!(String::from_utf8_lossy(&output.stdout), "step 1\rstep 2\ndone");
        assert_eq!(String::from_utf8_lossy(&output.stderr), "warn\n");
    }

    /// The plugins' local credential never comes back in what a plugin
    /// prints, whole or line by line.
    #[tokio::test]
    async fn the_local_credential_never_comes_back_in_the_output() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(PluginStore::new(tmp.path().join("plugins"), tmp.path().join("user"), None));
        let bin = tmp.path().join("leaky");
        let token = crate::plugin::plugin_local_token();
        std::fs::write(&bin, format!("#!/bin/sh\necho \"error sending request for url (http://127.0.0.1:1/k/{token}/x)\"\necho {token} >&2\n")).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let output = PluginRuntime::new("leaky", bin, store).with_output(tx).run_capture_args(&[], Duration::from_secs(60)).await.unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout, "error sending request for url (http://127.0.0.1:1/k/<credential>/x)\n");
        assert_eq!(String::from_utf8_lossy(&output.stderr), "<credential>\n");
        while let Some(line) = rx.recv().await {
            assert!(!line.contains(token), "{line}");
        }
    }

    /// A line cut to its most, or an output cut at the capture's end, never
    /// leaves part of the credential where the cut fell.
    #[test]
    fn a_cut_never_leaves_part_of_the_credential() {
        let token = crate::plugin::plugin_local_token();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let line = format!("{}{token}/x", "q".repeat(LINE_MAX - 2));
        send_line(&tx, line.as_bytes(), false);
        let sent = rx.try_recv().unwrap();
        assert!(sent.len() <= LINE_MAX && !sent.contains(&token[..2]), "{}", &sent[sent.len() - 20..]);

        send_line(&tx, format!("fetch: {}", &token[..60]).as_bytes(), true);
        assert_eq!(rx.try_recv().unwrap(), "fetch:");
        let mut kept = format!("ok {}", &token[..63]).into_bytes();
        drop_credential_start(&mut kept);
        assert_eq!(kept, b"ok ");
    }
}
