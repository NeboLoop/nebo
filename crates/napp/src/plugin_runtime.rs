use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
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

/// Return a sanitized copy of the current process environment,
/// stripping dangerous loader/shell injection vars.
pub fn sanitized_env() -> Vec<(String, String)> {
    let dangerous: std::collections::HashSet<&str> = DANGEROUS_ENV_VARS.iter().copied().collect();

    std::env::vars()
        .filter(|(k, _)| {
            let upper = k.to_uppercase();
            if dangerous.contains(upper.as_str()) {
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

        // User-stored auth values (plugin settings) — the top auth layer.
        for (k, v) in stored_auth {
            if !v.is_empty() {
                env.push((k, v));
            }
        }

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
        let mut cmd = tokio::process::Command::new(&self.binary_path);
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
        // No console window on Windows. Lived at every call site before this;
        // a caller that forgot it flashed a black box at the user on every
        // plugin invocation.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
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

        let child = spawn_with_etxtbsy_retry(&mut cmd).await?;
        let pid = child.id();
        if let Some(p) = pid {
            crate::child_guard::register_child(p);
        }
        let result = tokio::time::timeout(timeout, child.wait_with_output()).await;
        if let Some(p) = pid {
            crate::child_guard::unregister_child(p);
        }

        match result {
            Ok(Ok(output)) => Ok(output),
            Ok(Err(e)) => Err(LaunchError::Wait(e)),
            // The wait future is dropped here, which triggers kill_on_drop.
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
