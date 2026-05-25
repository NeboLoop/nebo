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
        }
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
    pub fn build_env(&self) -> Vec<(String, String)> {
        let mut env: Vec<(String, String)> = sanitized_env();

        // Permission filtering on the sanitized base
        if self.enforce_permissions {
            if let Some(manifest) = self.plugin_store.get_manifest(&self.slug) {
                if let Some(ref perms) = manifest.permissions {
                    if !perms.env_allow.is_empty() {
                        let allow: std::collections::HashSet<&str> =
                            perms.env_allow.iter().map(|s| s.as_str()).collect();
                        env.retain(|(k, _)| allow.contains(k.as_str()));
                    }
                    if !perms.env_deny.is_empty() {
                        let deny: std::collections::HashSet<&str> =
                            perms.env_deny.iter().map(|s| s.as_str()).collect();
                        env.retain(|(k, _)| !deny.contains(k.as_str()));
                    }
                }
            }
        }

        // Plugin binary env var (e.g., GWS_BIN=/path/to/gws)
        env.push((
            crate::plugin::plugin_env_var(&self.slug),
            self.binary_path.to_string_lossy().into_owned(),
        ));

        // Plugin data directory
        let plugin_data = self.plugin_store.plugin_data_dir(&self.slug);
        if let Err(e) = std::fs::create_dir_all(&plugin_data) {
            tracing::warn!(plugin = %self.slug, error = %e, "failed to create plugin data directory");
        }
        let data_str = plugin_data.to_string_lossy().into_owned();
        env.push(("NEBO_PLUGIN_DATA".into(), data_str.clone()));
        env.push((
            crate::plugin::plugin_data_env_var(&self.slug),
            data_str,
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

        // Auth env vars
        for (k, v) in self.plugin_store.resolved_auth_env(&self.slug) {
            env.push((k, v));
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

        env
    }

    /// Build a Command with env_clear + all env vars + shlex-parsed args.
    /// `kill_on_drop` is set so plugin sidecars die with their parent rather
    /// than orphaning during nebo restart/crash.
    pub fn command(&self, args_str: &str) -> tokio::process::Command {
        let args = shlex::split(args_str)
            .unwrap_or_else(|| args_str.split_whitespace().map(String::from).collect());

        let mut cmd = tokio::process::Command::new(&self.binary_path);
        cmd.args(&args);
        cmd.env_clear();
        for (k, v) in self.build_env() {
            cmd.env(k, v);
        }
        cmd.kill_on_drop(true);
        cmd
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
