//! Plugin primitive — managed binaries downloaded once, shared across skills.
//!
//! Skills declare plugin dependencies in SKILL.md frontmatter:
//! ```yaml
//! plugins:
//!   - name: gws
//!     version: ">=1.2.0"
//! ```
//!
//! Plugins are downloaded from NeboAI, verified (SHA256 + ED25519), and stored at
//! `<data_dir>/nebo/plugins/<slug>/<version>/`. User-provided plugins live at
//! `<data_dir>/user/plugins/<slug>/<version>/` and override installed versions.
//! Multiple skills can share the same plugin binary. Scripts access the binary via
//! `{SLUG}_BIN` environment variable.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use crate::NappError;
use crate::signing::SigningKeyProvider;

// ── Types ───────────────────────────────────────────────────────────

/// Plugin manifest stored locally at `<data_dir>/nebo/plugins/<slug>/<version>/plugin.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    /// NeboAI artifact ID.
    pub id: String,
    /// URL-safe slug — matches skill's `plugins[].name`.
    pub slug: String,
    /// Human-readable display name.
    pub name: String,
    /// Semver version string.
    pub version: String,
    /// Brief description.
    #[serde(default)]
    pub description: String,
    /// Publisher name.
    #[serde(default)]
    pub author: String,
    /// Platform-specific binary entries keyed by platform key (e.g., "darwin-arm64").
    pub platforms: HashMap<String, PlatformBinary>,
    /// ED25519 signing key ID used to sign binaries.
    #[serde(default)]
    pub signing_key_id: String,
    /// Custom env var name override. Defaults to `{SLUG}_BIN`.
    #[serde(default)]
    pub env_var: String,
    /// Optional authentication configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<PluginAuth>,
    /// Optional event capabilities — events this plugin can produce via watch processes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<PluginEventDef>>,
    /// Plugin-to-plugin dependencies (e.g., digest depends on ffmpeg).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<PluginDependency>,
    /// Structured capability declarations (tools, hooks, commands, routes, providers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<PluginCapabilities>,
    /// Optional permissions manifest — declares env access, network, and timeout caps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<PluginPermissions>,
    /// Plugin category for discovery routing (e.g., "payments", "communication", "developer").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub category: String,
    /// Trigger keywords for search matching (e.g., ["payment", "invoice", "billing"]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<String>,
    /// Channel bridge capability. When present, this plugin can act as a
    /// bidirectional messaging bridge (e.g., Slack, Discord, Telegram).
    /// The user enables it per-agent in Settings → Plugins → Channel Routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<PluginChannel>,
}

/// Channel bridge declaration in plugin.json.
///
/// Tells Nebo this plugin can bridge external messaging platforms.
/// The binary runs as a persistent subprocess: inbound messages on stdout (NDJSON),
/// outbound replies on stdin (NDJSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginChannel {
    /// CLI command appended to plugin binary (e.g., "bridge --listen").
    pub command: String,
    /// Human-readable channel name (e.g., "Slack", "Discord").
    #[serde(default)]
    pub name: String,
    /// Description shown in settings UI.
    #[serde(default)]
    pub description: String,
    /// Seconds to wait before restarting on crash (default: 5).
    #[serde(default = "default_channel_restart_delay")]
    pub restart_delay_secs: u64,
    /// When true, one bridge process is shared across all agents.
    /// Incoming messages are routed to agents by name. Each agent's
    /// replies are posted with its own display identity.
    #[serde(default)]
    pub shared: bool,
}

fn default_channel_restart_delay() -> u64 {
    5
}

/// Authentication configuration for a plugin binary.
///
/// Plugins that require credentials (e.g., OAuth for Google Workspace) declare
/// their auth requirements here. Nebo runs the plugin's auth commands and injects
/// the publisher-provided env vars (client_id, client_secret, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginAuth {
    /// Auth type identifier (e.g., "oauth_cli").
    #[serde(rename = "type")]
    pub auth_type: String,
    /// Environment variables to inject before running auth commands.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// CLI subcommands (appended to plugin binary path).
    #[serde(default)]
    pub commands: PluginAuthCommands,
    /// Human-readable label for the auth button (e.g., "Google Account").
    #[serde(default)]
    pub label: String,
    /// Description shown to user during auth step.
    #[serde(default)]
    pub description: String,
    /// Optional help info shown in configuration modals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<ArtifactHelp>,
}

/// Help content that any artifact (plugin, skill, agent, MCP) can declare.
///
/// Displayed in configuration modals to guide users through setup.
/// At least one of `url` or `text` should be provided.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactHelp {
    /// Link to external documentation (e.g., API key creation page).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Label for the URL link (e.g., "Get your API key").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_label: Option<String>,
    /// Inline help text (markdown). Shown directly in the modal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// CLI commands for plugin authentication lifecycle.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginAuthCommands {
    /// Subcommand to trigger authentication (e.g., "auth login").
    #[serde(default)]
    pub login: String,
    /// Subcommand to check auth status, must return JSON (e.g., "auth status").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Subcommand to clear credentials (e.g., "auth logout").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logout: Option<String>,
}

/// An event type that a plugin can produce via a long-running watch process.
///
/// Plugins that monitor external services (e.g., Gmail for new emails) declare
/// their event types here. Each event has a name, the CLI command to start the
/// watcher, and whether the watcher multiplexes multiple event types on stdout.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginEventDef {
    /// Event name, e.g. "email.new". Prefixed with plugin slug at runtime → "gws.email.new".
    pub name: String,
    /// Human-readable description of what triggers this event.
    #[serde(default)]
    pub description: String,
    /// CLI arguments passed to the plugin binary to start the watcher
    /// (e.g. "gmail +watch --format ndjson"). Supports `{{key}}` template
    /// substitution from agent input values.
    pub command: String,
    /// If true, the watcher process may output lines with an `"event"` field
    /// to multiplex multiple event types. Default: false (single event type).
    #[serde(default)]
    pub multiplexed: bool,
}

/// A plugin-to-plugin dependency declared in plugin.json.
///
/// Same shape as the skill `PluginDependency` in the tools crate, using the
/// same JSON field names so both skill YAML and plugin.json parse identically.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDependency {
    /// Dependency plugin slug.
    pub name: String,
    /// Semver version range. Defaults to `"*"`.
    #[serde(default = "default_dep_version")]
    pub version: String,
    /// If true, the parent plugin loads even without this dep.
    #[serde(default)]
    pub optional: bool,
}

fn default_dep_version() -> String {
    "*".to_string()
}

// ── Structured Capabilities (Phase 1) ───────────────────────────────

/// Structured capability declarations for a plugin.
///
/// Plugins can declare tools, hooks, commands, routes, and providers
/// in their manifest. All are executed out-of-process via the plugin binary.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCapabilities {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<PluginToolDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hooks: Vec<PluginHookDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<PluginCommandDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<PluginRouteDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<PluginProviderDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config_schema: Vec<PluginConfigField>,
}

/// A configuration field declared by a plugin.
///
/// Rendered as a settings form in the UI. Values stored in `plugin_settings`,
/// injected as env vars on plugin execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginConfigField {
    /// Env var name (e.g., "MAX_RESULTS").
    pub key: String,
    /// Display label.
    pub label: String,
    /// Help text.
    #[serde(default)]
    pub description: String,
    /// Field type: "string", "number", "boolean", "select".
    #[serde(default = "default_string_type")]
    pub field_type: String,
    /// Default value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Whether the field must be set.
    #[serde(default)]
    pub required: bool,
    /// If true, stored encrypted via is_secret in plugin_settings.
    #[serde(default)]
    pub secret: bool,
    /// Options for "select" type fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
}

fn default_string_type() -> String {
    "string".to_string()
}

/// Permissions manifest for a plugin.
///
/// Declares what env vars the plugin may read, whether it needs network access,
/// and the maximum execution timeout. Enforced at tool execution time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginPermissions {
    /// Env vars the plugin is allowed to read. Empty = all allowed.
    #[serde(default)]
    pub env_allow: Vec<String>,
    /// Env vars always stripped before execution.
    #[serde(default)]
    pub env_deny: Vec<String>,
    /// Whether the plugin needs network access (informational for now).
    #[serde(default)]
    pub network: bool,
    /// Maximum timeout in seconds for any tool execution. Default: 300.
    #[serde(default = "default_max_timeout")]
    pub max_timeout_seconds: u64,
}

fn default_max_timeout() -> u64 {
    300
}

/// A structured tool exposed by a plugin, backed by a CLI subcommand.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginToolDef {
    /// Tool name as exposed to the agent (e.g., "gws.gmail.triage").
    pub name: String,
    /// Human-readable description for the model.
    pub description: String,
    /// CLI arguments passed to the plugin binary (e.g., "gmail +triage").
    pub command: String,
    /// JSON Schema for typed input. If absent, a generic object schema is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<serde_json::Value>,
    /// Whether this tool requires user approval before execution.
    #[serde(default = "default_true")]
    pub approval: bool,
    /// Maximum execution time in seconds.
    #[serde(default = "default_120")]
    pub timeout_seconds: u64,
}

/// A lifecycle hook contributed by a plugin, executed out-of-process.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginHookDef {
    /// Hook point name (must be in VALID_HOOKS).
    pub hook: String,
    /// "filter" (can modify payload) or "action" (fire-and-forget). Default: "action".
    #[serde(default = "default_action")]
    pub hook_type: String,
    /// Priority — lower runs first. Default: 100.
    #[serde(default = "default_priority")]
    pub priority: i32,
    /// CLI subcommand for the hook handler.
    pub command: String,
    /// Timeout in milliseconds. Default: 500.
    #[serde(default = "default_500")]
    pub timeout_ms: u64,
}

/// A slash/app command contributed by a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCommandDef {
    /// Command name (e.g., "/gmail" or "gmail").
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// CLI subcommand to execute.
    pub command: String,
    /// If true, register as a slash command in chat.
    #[serde(default)]
    pub slash: bool,
}

/// An HTTP route contributed by a plugin, proxied through the catch-all handler.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginRouteDef {
    /// Route path (e.g., "/gws/oauth/callback").
    pub path: String,
    /// HTTP method (GET, POST, etc.).
    pub method: String,
    /// CLI subcommand that handles the request.
    pub command: String,
    /// Auth requirement: "public" or "jwt". Default: "jwt".
    #[serde(default = "default_jwt")]
    pub auth: String,
}

/// A provider adapter contributed by a plugin (model, speech, image, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginProviderDef {
    /// Provider ID (e.g., "openrouter").
    pub id: String,
    /// Display name.
    pub display_name: String,
    /// Provider type: "model", "speech", "image", etc.
    pub provider_type: String,
    /// CLI subcommand to list available models (JSON output).
    pub models_command: String,
    /// CLI subcommand for streaming chat (NDJSON on stdout).
    pub chat_command: String,
    /// CLI subcommand for auth setup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_command: Option<String>,
}

fn default_true() -> bool {
    true
}
fn default_120() -> u64 {
    120
}
fn default_500() -> u64 {
    500
}
fn default_action() -> String {
    "action".to_string()
}
fn default_priority() -> i32 {
    100
}
fn default_jwt() -> String {
    "jwt".to_string()
}

/// Binary artifact for a specific platform.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformBinary {
    /// Binary filename (e.g., "gws" or "gws.exe").
    pub binary_name: String,
    /// SHA256 hex hash of the binary.
    pub sha256: String,
    /// ED25519 signature (base64).
    pub signature: String,
    /// File size in bytes.
    pub size: u64,
    /// Download URL for the binary.
    pub download_url: String,
}

// ── Validation ──────────────────────────────────────────────────────

fn is_valid_slug_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'
}

impl PluginManifest {
    /// Returns true if this plugin declares tool capabilities (connector pattern).
    pub fn is_connector(&self) -> bool {
        self.capabilities
            .as_ref()
            .map(|c| !c.tools.is_empty())
            .unwrap_or(false)
    }

    /// Number of declared tool capabilities.
    pub fn tool_count(&self) -> usize {
        self.capabilities
            .as_ref()
            .map(|c| c.tools.len())
            .unwrap_or(0)
    }

    /// Validate manifest fields beyond serde deserialization.
    ///
    /// Checks slug format, semver validity, platform entries, binary name safety,
    /// and auth/event field consistency. Same slug rules as `Skill::validate()`.
    pub fn validate(&self) -> Result<(), NappError> {
        // Slug: non-empty, lowercase alphanumeric + hyphens, no leading/trailing hyphens, max 64
        if self.slug.is_empty() {
            return Err(NappError::PluginValidation("slug is required".into()));
        }
        if self.slug.len() > 64 {
            return Err(NappError::PluginValidation(format!(
                "slug exceeds 64 characters: {}",
                self.slug.len()
            )));
        }
        if self.slug.starts_with('-') || self.slug.ends_with('-') {
            return Err(NappError::PluginValidation(
                "slug must not start or end with a hyphen".into(),
            ));
        }
        if self.slug.contains("--") {
            return Err(NappError::PluginValidation(
                "slug must not contain consecutive hyphens".into(),
            ));
        }
        if !self.slug.chars().all(is_valid_slug_char) {
            return Err(NappError::PluginValidation(
                "slug must contain only lowercase letters, digits, and hyphens".into(),
            ));
        }

        // Version: valid semver
        if semver::Version::parse(&self.version).is_err() {
            return Err(NappError::PluginValidation(format!(
                "invalid semver version: '{}'",
                self.version
            )));
        }

        // Platforms: at least one entry
        if self.platforms.is_empty() {
            return Err(NappError::PluginValidation(
                "at least one platform entry is required".into(),
            ));
        }

        // Binary names: no path separators, no "..", no empty
        for (platform_key, pb) in &self.platforms {
            if pb.binary_name.is_empty() {
                return Err(NappError::PluginValidation(format!(
                    "binary_name is empty for platform '{}'",
                    platform_key
                )));
            }
            if pb.binary_name.contains('/') || pb.binary_name.contains('\\') {
                return Err(NappError::PluginValidation(format!(
                    "binary_name contains path separator for platform '{}': '{}'",
                    platform_key, pb.binary_name
                )));
            }
            if pb.binary_name.contains("..") {
                return Err(NappError::PluginValidation(format!(
                    "binary_name contains path traversal for platform '{}': '{}'",
                    platform_key, pb.binary_name
                )));
            }
        }

        // Auth: login command must be non-empty for interactive auth types (oauth_cli, etc.)
        // Env-only auth types (auth_type == "env") use env vars and don't need a login command.
        if let Some(ref auth) = self.auth {
            if auth.commands.login.is_empty() && auth.auth_type != "env" {
                return Err(NappError::PluginValidation(
                    "auth.commands.login is required when auth is declared (unless auth type is 'env')".into(),
                ));
            }
        }

        // Events: name and command must be non-empty, name must not contain path separators
        if let Some(ref events) = self.events {
            for event in events {
                if event.name.is_empty() {
                    return Err(NappError::PluginValidation("event name is required".into()));
                }
                if event.name.contains('/') || event.name.contains('\\') {
                    return Err(NappError::PluginValidation(format!(
                        "event name contains path separator: '{}'",
                        event.name
                    )));
                }
                if event.command.is_empty() {
                    return Err(NappError::PluginValidation(format!(
                        "event command is required for event '{}'",
                        event.name
                    )));
                }
            }
        }

        Ok(())
    }
}

// ── PluginStore ─────────────────────────────────────────────────────

/// Manages downloaded plugin binaries.
///
/// Lives in the napp crate alongside Registry — shares `SigningKeyProvider` and
/// version resolution infrastructure. Scans two directories:
/// - `installed_dir` (`<data_dir>/nebo/plugins/`) — marketplace downloads
/// - `user_dir` (`<data_dir>/user/plugins/`) — user-provided, overrides installed
pub struct PluginStore {
    /// Marketplace plugin storage: `<data_dir>/nebo/plugins/`.
    installed_dir: PathBuf,
    /// User plugin storage: `<data_dir>/user/plugins/`.
    user_dir: PathBuf,
    /// ED25519 signing key provider for signature verification.
    signing_key: Option<Arc<SigningKeyProvider>>,
    /// Cached manifests keyed by `slug:version`.
    manifests: Arc<tokio::sync::RwLock<HashMap<String, PluginManifest>>>,
    /// Prevents concurrent downloads of the same plugin slug.
    downloading: Arc<tokio::sync::Mutex<HashSet<String>>>,
    /// In-memory diagnostic log for plugin health tracking.
    diagnostics: Arc<std::sync::RwLock<Vec<PluginDiagnostic>>>,
    /// In-memory auth status per slug: `true` = authenticated, `false` = needs auth.
    /// Populated once at startup; updated on login/logout events.
    /// Uses std::sync::RwLock because writes never span .await points and sync
    /// reads are needed by PluginTool.description() (DynTool is a sync trait).
    auth_cache: Arc<std::sync::RwLock<HashMap<String, bool>>>,
    /// In-memory cache of stored env var values per slug.
    /// Populated from DB at startup; updated when user saves plugin config.
    env_cache: Arc<std::sync::RwLock<HashMap<String, HashMap<String, String>>>>,
}

/// A diagnostic entry for plugin health tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDiagnostic {
    pub slug: String,
    pub level: String,
    pub phase: String,
    pub message: String,
    pub timestamp: i64,
}

impl PluginStore {
    pub fn new(
        installed_dir: PathBuf,
        user_dir: PathBuf,
        signing_key: Option<Arc<SigningKeyProvider>>,
    ) -> Self {
        Self {
            installed_dir,
            user_dir,
            signing_key,
            manifests: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            downloading: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
            diagnostics: Arc::new(std::sync::RwLock::new(Vec::new())),
            auth_cache: Arc::new(std::sync::RwLock::new(HashMap::new())),
            env_cache: Arc::new(std::sync::RwLock::new(HashMap::new())),
        }
    }

    // ── Auth cache ──────────────────────────────────────────────────

    /// Populate auth cache at startup: runs auth-status for every plugin that has
    /// an auth config with a status command, in parallel.
    pub async fn refresh_auth_cache(&self) {
        let installed = self.list_installed();
        let mut seen = HashSet::new();

        // Collect only plugins that have auth + status command
        let slugs_with_auth: Vec<String> = installed
            .into_iter()
            .filter_map(|(slug, _, _, _)| {
                if !seen.insert(slug.clone()) {
                    return None;
                }
                let manifest = self.get_manifest(&slug)?;
                let auth = manifest.auth?;
                auth.commands.status.as_ref()?;
                Some(slug)
            })
            .collect();

        if slugs_with_auth.is_empty() {
            return;
        }

        let path_env = self.path_with_plugins();
        let futures: Vec<_> = slugs_with_auth
            .iter()
            .map(|slug| {
                let slug = slug.clone();
                let path_env = path_env.clone();
                let store = self;
                async move {
                    let result = run_auth_status_check(store, &slug, &path_env).await;
                    (slug, result)
                }
            })
            .collect();

        let results = futures::future::join_all(futures).await;
        let mut cache = self.auth_cache.write().unwrap();
        for (slug, authed) in results {
            cache.insert(slug, authed);
        }
        info!("auth cache populated: {} plugins checked", cache.len());
    }

    /// Update a single plugin's auth status (call after login/logout).
    pub async fn update_auth_status(&self, slug: &str) {
        let path_env = self.path_with_plugins();
        let authed = run_auth_status_check(self, slug, &path_env).await;
        self.auth_cache.write().unwrap().insert(slug.to_string(), authed);
    }

    /// Check auth status for a single plugin on first access. Caches the result.
    /// Subsequent calls for the same slug return the cached value immediately.
    pub async fn check_auth_lazy(&self, slug: &str) -> bool {
        // Return cached if available
        if let Some(status) = self.auth_cache.read().unwrap().get(slug) {
            return *status;
        }
        // First access: run the check, cache it
        let path_env = self.path_with_plugins();
        let status = run_auth_status_check(self, slug, &path_env).await;
        self.auth_cache.write().unwrap().insert(slug.to_string(), status);
        status
    }

    /// Get plugins that need auth (authenticated = false). Pure in-memory read.
    pub async fn plugins_needing_auth(&self) -> Vec<(String, PluginAuth)> {
        let cache = self.auth_cache.read().unwrap();
        let mut result = Vec::new();
        for (slug, authed) in cache.iter() {
            if !authed {
                if let Some(manifest) = self.get_manifest(slug) {
                    if let Some(auth) = manifest.auth {
                        result.push((slug.clone(), auth));
                    }
                }
            }
        }
        result
    }

    /// Record a diagnostic event for a plugin.
    pub fn record_diagnostic(&self, slug: &str, level: &str, phase: &str, message: &str) {
        let diag = PluginDiagnostic {
            slug: slug.to_string(),
            level: level.to_string(),
            phase: phase.to_string(),
            message: message.to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
        };
        if let Ok(mut diags) = self.diagnostics.write() {
            // Cap at 1000 entries to prevent unbounded growth
            if diags.len() >= 1000 {
                diags.drain(..100);
            }
            diags.push(diag);
        }
    }

    /// Get diagnostics for a specific plugin.
    pub fn get_diagnostics(&self, slug: &str) -> Vec<PluginDiagnostic> {
        self.diagnostics
            .read()
            .map(|d| d.iter().filter(|e| e.slug == slug).cloned().collect())
            .unwrap_or_default()
    }

    // ── Env var cache ─────────────────────────────────────────────

    /// Store a user-provided env var value for a plugin.
    pub fn set_env_var(&self, slug: &str, key: &str, value: &str) {
        if let Ok(mut cache) = self.env_cache.write() {
            cache
                .entry(slug.to_string())
                .or_default()
                .insert(key.to_string(), value.to_string());
        }
    }

    /// Bulk-set env var values for a plugin (e.g., from DB at startup).
    pub fn set_env_vars(&self, slug: &str, vars: HashMap<String, String>) {
        if let Ok(mut cache) = self.env_cache.write() {
            cache.insert(slug.to_string(), vars);
        }
    }

    /// Get resolved auth env vars for a plugin: stored values override manifest defaults.
    pub fn resolved_auth_env(&self, slug: &str) -> HashMap<String, String> {
        let manifest_env = self
            .get_manifest(slug)
            .and_then(|m| m.auth)
            .map(|a| a.env)
            .unwrap_or_default();

        let stored = self
            .env_cache
            .read()
            .ok()
            .and_then(|c| c.get(slug).cloned())
            .unwrap_or_default();

        let mut resolved = manifest_env;
        for (k, v) in stored {
            if !v.is_empty() {
                resolved.insert(k, v);
            }
        }
        resolved
    }

    // ── Readiness ──────────────────────────────────────────────────

    /// Check if a plugin is ready to execute based on its auth/config prerequisites.
    /// Evaluates existing caches — no new state, no DB calls.
    pub fn is_ready(&self, slug: &str) -> bool {
        let manifest = match self.get_manifest(slug) {
            Some(m) => m,
            None => return false,
        };

        if let Some(ref auth) = manifest.auth {
            if auth.commands.status.is_some() {
                let authed = self
                    .auth_cache
                    .read()
                    .unwrap()
                    .get(slug)
                    .copied()
                    .unwrap_or(false);
                if !authed {
                    return false;
                }
            }
            if auth.auth_type == "env" {
                let stored = self
                    .env_cache
                    .read()
                    .ok()
                    .and_then(|c| c.get(slug).cloned())
                    .unwrap_or_default();
                if !auth.env.keys().all(|k| {
                    stored.get(k).map(|v| !v.is_empty()).unwrap_or(false)
                }) {
                    return false;
                }
            }
        }

        if let Some(ref caps) = manifest.capabilities {
            for field in &caps.config_schema {
                if field.required {
                    let has_value = self
                        .env_cache
                        .read()
                        .ok()
                        .and_then(|c| {
                            c.get(slug)
                                .and_then(|vars| vars.get(&field.key).cloned())
                        })
                        .map(|v| !v.is_empty())
                        .unwrap_or(false);
                    if !has_value {
                        return false;
                    }
                }
            }
        }

        true
    }

    /// Root directory for installed (marketplace) plugin storage.
    pub fn plugins_dir(&self) -> &Path {
        &self.installed_dir
    }

    /// Root directory for user-provided plugin storage (overrides marketplace).
    pub fn user_plugins_dir(&self) -> &Path {
        &self.user_dir
    }

    /// Resolve a plugin binary path from local storage only. Non-async.
    ///
    /// Checks user directory first (override), then installed directory.
    /// Scans `<dir>/<slug>/` for version directories, matches the
    /// semver range, and returns the binary path if found.
    pub fn resolve(&self, slug: &str, version_range: &str) -> Option<PathBuf> {
        // User dir takes priority
        if let Some(path) = self.resolve_in_dir(&self.user_dir, slug, version_range) {
            return Some(path);
        }
        self.resolve_in_dir(&self.installed_dir, slug, version_range)
    }

    /// Returns the persistent data directory for a plugin: `<data_dir>/plugins/data/<slug>/`.
    ///
    /// This directory is NOT inside the versioned plugin cache, so it survives plugin
    /// updates. Plugins use it for OAuth tokens, cached responses, user preferences, etc.
    ///
    /// The path is derived from `installed_dir` (`<data_dir>/nebo/plugins/`) by going
    /// Persistent data directory for a plugin, separated from the code tree.
    /// Returns `~/.nebo/appdata/plugins/<slug>/` — survives all upgrades.
    pub fn plugin_data_dir(&self, slug: &str) -> PathBuf {
        config::appdata_dir()
            .map(|d| d.join("plugins").join(slug))
            .unwrap_or_else(|_| {
                // Fallback: derive from installed_dir parent
                let data_dir = self
                    .installed_dir
                    .parent()
                    .and_then(|p| p.parent())
                    .unwrap_or(&self.installed_dir);
                data_dir.join("plugins").join("data").join(slug)
            })
    }

    /// Resolve a plugin binary within a single root directory.
    fn resolve_in_dir(&self, root: &Path, slug: &str, version_range: &str) -> Option<PathBuf> {
        let slug_dir = root.join(slug);
        if !slug_dir.exists() {
            return None;
        }

        let req = if version_range.is_empty() || version_range == "*" {
            None
        } else {
            match semver::VersionReq::parse(version_range) {
                Ok(r) => Some(r),
                Err(_) => return None,
            }
        };

        // Try flat layout first: plugin.json at slug root (dev repos / symlinks)
        let flat_manifest = slug_dir.join("plugin.json");
        if flat_manifest.exists() {
            if let Some((version, binary_path)) =
                self.try_load_flat_plugin(&slug_dir, &flat_manifest)
            {
                if req.as_ref().map_or(true, |r| r.matches(&version)) {
                    return Some(binary_path);
                }
            }
        }

        let mut best: Option<(semver::Version, PathBuf)> = None;

        let entries = match std::fs::read_dir(&slug_dir) {
            Ok(e) => e,
            Err(_) => return None,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };

            let version = match semver::Version::parse(dir_name) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Check version range
            if let Some(ref req) = req {
                if !req.matches(&version) {
                    continue;
                }
            }

            // Check for quarantine marker
            if path.join(".quarantined").exists() {
                continue;
            }

            // Check for binary — read manifest to find binary_name, or scan for executable
            let binary_path = self.find_binary_in_version_dir(&path);
            if binary_path.is_none() {
                continue;
            }

            match &best {
                Some((current_best, _)) if &version <= current_best => {}
                _ => {
                    best = Some((version, binary_path.unwrap()));
                }
            }
        }

        best.map(|(_, path)| path)
    }

    /// Ensure a plugin is installed, downloading from NeboAI if missing.
    ///
    /// Deduplicates concurrent downloads via the `downloading` mutex.
    /// The `download_fn` callback queries NeboAI for the manifest and binary bytes.
    pub async fn ensure<F, Fut>(
        &self,
        slug: &str,
        version_range: &str,
        download_fn: F,
    ) -> Result<PathBuf, NappError>
    where
        F: FnOnce(String, String) -> Fut,
        Fut: std::future::Future<Output = Result<(PluginManifest, Vec<u8>), NappError>>,
    {
        // Fast path: already installed locally
        if let Some(path) = self.resolve(slug, version_range) {
            return Ok(path);
        }

        // Dedup concurrent downloads for the same slug
        {
            let mut downloading = self.downloading.lock().await;
            if downloading.contains(slug) {
                // Another task is downloading this plugin — wait and retry
                drop(downloading);
                // Simple retry loop: wait briefly, then check local storage
                for _ in 0..30 {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    if let Some(path) = self.resolve(slug, version_range) {
                        return Ok(path);
                    }
                }
                return Err(NappError::PluginDownloadFailed(format!(
                    "timed out waiting for concurrent download of plugin '{}'",
                    slug
                )));
            }
            downloading.insert(slug.to_string());
        }

        // Download and install
        let result = self
            .download_and_install(slug, version_range, download_fn)
            .await;

        // Release download lock
        {
            let mut downloading = self.downloading.lock().await;
            downloading.remove(slug);
        }

        result
    }

    /// Download, verify, and install a plugin binary.
    async fn download_and_install<F, Fut>(
        &self,
        slug: &str,
        _version_range: &str,
        download_fn: F,
    ) -> Result<PathBuf, NappError>
    where
        F: FnOnce(String, String) -> Fut,
        Fut: std::future::Future<Output = Result<(PluginManifest, Vec<u8>), NappError>>,
    {
        let platform = current_platform_key();

        let (manifest, binary_data) = download_fn(slug.to_string(), platform.clone()).await?;

        // Validate manifest before proceeding
        manifest.validate()?;

        // Find the platform binary entry
        let platform_binary = manifest.platforms.get(&platform).ok_or_else(|| {
            NappError::PluginPlatformUnavailable {
                plugin: slug.to_string(),
                platform: platform.clone(),
            }
        })?;

        // Verify SHA256 hash
        let mut hasher = Sha256::new();
        hasher.update(&binary_data);
        let actual_hash = hex::encode(hasher.finalize());
        if actual_hash != platform_binary.sha256 {
            return Err(NappError::PluginDownloadFailed(format!(
                "SHA256 mismatch for plugin '{}': expected {}, got {}",
                slug, platform_binary.sha256, actual_hash
            )));
        }

        // Verify ED25519 signature if signing key is available
        if let Some(ref signing_key) = self.signing_key {
            match signing_key.get_key().await {
                Ok(verifying_key) => {
                    use base64::Engine;
                    use ed25519_dalek::{Signature, Verifier};

                    let sig_bytes = base64::engine::general_purpose::STANDARD
                        .decode(&platform_binary.signature)
                        .map_err(|e| {
                            NappError::Signing(format!("decode plugin signature: {}", e))
                        })?;
                    let signature = Signature::from_slice(&sig_bytes).map_err(|e| {
                        NappError::Signing(format!("invalid plugin signature: {}", e))
                    })?;
                    verifying_key
                        .verify(&binary_data, &signature)
                        .map_err(|_| {
                            NappError::Signing(format!(
                                "plugin '{}' signature verification failed",
                                slug
                            ))
                        })?;
                    debug!(plugin = slug, "ED25519 signature verified");
                }
                Err(e) => {
                    warn!(plugin = slug, error = %e, "could not fetch signing key, skipping signature verification");
                }
            }
        }

        // Store binary on disk (always in installed_dir — marketplace downloads)
        let version_dir = self.installed_dir.join(slug).join(&manifest.version);
        std::fs::create_dir_all(&version_dir)?;

        let binary_path = version_dir.join(&platform_binary.binary_name);
        std::fs::write(&binary_path, &binary_data)?;

        // Set executable permission on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755))?;
        }

        // Write manifest for future reference
        let manifest_path = version_dir.join("plugin.json");
        let manifest_json = serde_json::to_string_pretty(&manifest)?;
        std::fs::write(&manifest_path, manifest_json)?;

        // Cache manifest in memory
        {
            let cache_key = format!("{}:{}", slug, manifest.version);
            let mut manifests = self.manifests.write().await;
            manifests.insert(cache_key, manifest.clone());
        }

        info!(
            plugin = slug,
            version = %manifest.version,
            platform = %platform,
            path = %binary_path.display(),
            size = binary_data.len(),
            "installed plugin binary"
        );

        Ok(binary_path)
    }

    /// Install a plugin from a .napp archive containing binary + plugin.json + PLUGIN.md + skills/.
    ///
    /// Stores the .napp archive at `<installed_dir>/<slug>/<version>.napp`, then
    /// extracts alongside (into `<version>/`) — same pattern as agent install.
    /// Reads plugin.json for metadata, verifies binary integrity (SHA256 + ED25519).
    pub async fn install_from_napp(
        &self,
        slug: &str,
        version: &str,
        napp_data: &[u8],
    ) -> Result<PathBuf, NappError> {
        // Dedup concurrent installs
        {
            let mut downloading = self.downloading.lock().await;
            if downloading.contains(slug) {
                drop(downloading);
                for _ in 0..30 {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    if let Some(path) = self.resolve(slug, "*") {
                        return Ok(path);
                    }
                }
                return Err(NappError::PluginDownloadFailed(format!(
                    "timed out waiting for concurrent install of plugin '{}'",
                    slug
                )));
            }
            downloading.insert(slug.to_string());
        }

        let result = self.install_from_napp_inner(slug, version, napp_data).await;

        // Release install lock
        {
            let mut downloading = self.downloading.lock().await;
            downloading.remove(slug);
        }

        result
    }

    /// Inner implementation of .napp-based plugin install.
    ///
    /// Uses a safe stage-verify-swap pattern so the old version is never deleted
    /// until the new one is fully extracted and verified (like an iOS app update).
    ///
    /// Flow:
    ///   1. Save .napp archive to disk
    ///   2. Extract to <version>.staging/ directory
    ///   3. Verify manifest, binary, SHA256, ED25519
    ///   4. Only after verification: swap staging into place, back up old version
    ///   5. Clean up old backups
    async fn install_from_napp_inner(
        &self,
        slug: &str,
        version: &str,
        napp_data: &[u8],
    ) -> Result<PathBuf, NappError> {
        let plugin_dir = self.installed_dir.join(slug);
        std::fs::create_dir_all(&plugin_dir)?;

        // 1. Save .napp archive
        let napp_path = plugin_dir.join(format!("{version}.napp"));
        std::fs::write(&napp_path, napp_data)?;
        info!(plugin = slug, path = %napp_path.display(), size = napp_data.len(), "stored sealed .napp");

        // 2. Extract to staging dir (never touches the live version dir)
        let staging_dir = plugin_dir.join(format!("{version}.staging"));
        if staging_dir.exists() {
            std::fs::remove_dir_all(&staging_dir)?;
        }
        crate::reader::extract_all(&napp_path, &staging_dir)?;
        info!(plugin = slug, dir = %staging_dir.display(), "extracted .napp to staging");

        // 3. Read and validate plugin.json from staging
        let plugin_json_path = staging_dir.join("plugin.json");
        let plugin_manifest: Option<PluginManifest> = if plugin_json_path.exists() {
            let data = std::fs::read_to_string(&plugin_json_path)?;
            match serde_json::from_str(&data) {
                Ok(m) => Some(m),
                Err(e) => {
                    warn!(plugin = slug, error = %e, "failed to parse plugin.json");
                    None
                }
            }
        } else {
            None
        };

        if let Some(ref pm) = plugin_manifest {
            pm.validate()?;
        }

        // Resolve effective version (e.g., "latest" -> real semver from manifest)
        let effective_version = if let Some(ref pm) = plugin_manifest {
            if semver::Version::parse(version).is_err()
                && semver::Version::parse(&pm.version).is_ok()
            {
                pm.version.clone()
            } else {
                version.to_string()
            }
        } else {
            version.to_string()
        };

        // If effective version differs, rename staging dir + .napp to match
        let mut staging = staging_dir;
        if effective_version != version {
            let new_staging = plugin_dir.join(format!("{effective_version}.staging"));
            let new_napp_path = plugin_dir.join(format!("{effective_version}.napp"));
            if new_staging.exists() {
                std::fs::remove_dir_all(&new_staging)?;
            }
            std::fs::rename(&staging, &new_staging)?;
            std::fs::rename(&napp_path, &new_napp_path)?;
            staging = new_staging;
            info!(plugin = slug, from = %version, to = %effective_version, "resolved version from manifest");
        }

        // Find binary in staging and make executable
        let binary_name = self
            .find_binary_in_version_dir(&staging)
            .ok_or_else(|| NappError::Extraction("no binary found in extracted .napp".into()))?
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let staging_binary = staging.join(&binary_name);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&staging_binary, std::fs::Permissions::from_mode(0o755))?;
        }

        // Verify SHA256 + ED25519 against staging binary (before touching live dir)
        if let Some(ref pm) = plugin_manifest {
            let platform = current_platform_key();
            if let Some(pb) = pm.platforms.get(&platform) {
                let binary_data = std::fs::read(&staging_binary)?;

                // SHA256
                let mut hasher = Sha256::new();
                hasher.update(&binary_data);
                let actual_hash = hex::encode(hasher.finalize());
                if actual_hash != pb.sha256 {
                    // Verification failed — clean up staging, keep old version intact
                    let _ = std::fs::remove_dir_all(&staging);
                    self.record_diagnostic(slug, "error", "verify", "SHA256 mismatch");
                    return Err(NappError::PluginDownloadFailed(format!(
                        "SHA256 mismatch for plugin '{}': expected {}, got {}",
                        slug, pb.sha256, actual_hash
                    )));
                }

                // ED25519
                if let Some(ref signing_key) = self.signing_key {
                    match signing_key.get_key().await {
                        Ok(verifying_key) => {
                            use base64::Engine;
                            use ed25519_dalek::{Signature, Verifier};

                            let sig_bytes = base64::engine::general_purpose::STANDARD
                                .decode(&pb.signature)
                                .map_err(|e| {
                                    NappError::Signing(format!("decode plugin signature: {}", e))
                                })?;
                            let signature = Signature::from_slice(&sig_bytes).map_err(|e| {
                                NappError::Signing(format!("invalid plugin signature: {}", e))
                            })?;
                            verifying_key
                                .verify(&binary_data, &signature)
                                .map_err(|_| {
                                    // Verification failed — clean up staging, keep old version
                                    let _ = std::fs::remove_dir_all(&staging);
                                    NappError::Signing(format!(
                                        "plugin '{}' signature verification failed",
                                        slug
                                    ))
                                })?;
                            debug!(plugin = slug, "ED25519 signature verified (.napp)");
                        }
                        Err(e) => {
                            warn!(plugin = slug, error = %e, "could not fetch signing key, skipping signature verification");
                        }
                    }
                }
            }

            // Cache manifest in memory
            let cache_key = format!("{}:{}", slug, effective_version);
            let mut manifests = self.manifests.write().await;
            manifests.insert(cache_key, pm.clone());
        }

        // 4. Verification passed — safe to swap staging into place.
        let final_dir = plugin_dir.join(&effective_version);
        if final_dir.exists() {
            // Back up old version instead of deleting
            let prev_dir = plugin_dir.join(format!("{effective_version}.prev"));
            let _ = std::fs::remove_dir_all(&prev_dir); // clean up any leftover backup
            std::fs::rename(&final_dir, &prev_dir)?;
            info!(plugin = slug, version = %effective_version, "backed up previous version");
        }
        std::fs::rename(&staging, &final_dir)?;

        // 5. Clean up old backups and stale version dirs (keep only the current version)
        if let Ok(entries) = std::fs::read_dir(&plugin_dir) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                // Keep the current version dir and .napp
                if fname == effective_version || fname == format!("{effective_version}.napp") {
                    continue;
                }
                // Remove .prev backups, .staging leftovers, and old version dirs
                if fname.ends_with(".prev") || fname.ends_with(".staging") {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }

        let binary_path = final_dir.join(&binary_name);
        info!(
            plugin = slug,
            version = %effective_version,
            path = %binary_path.display(),
            "installed plugin from .napp"
        );

        self.record_diagnostic(
            slug,
            "info",
            "install",
            &format!("installed v{}", effective_version),
        );

        Ok(binary_path)
    }

    /// Verify binary integrity against cached manifest.
    pub fn verify_integrity(&self, slug: &str, version: &str) -> Result<(), NappError> {
        // Check user dir first, then installed dir
        let user_dir = self.user_dir.join(slug).join(version);
        let version_dir = if user_dir.exists() {
            user_dir
        } else {
            self.installed_dir.join(slug).join(version)
        };
        let manifest_path = version_dir.join("plugin.json");

        let manifest_data = std::fs::read_to_string(&manifest_path).map_err(|e| {
            NappError::PluginNotFound(format!("manifest for {}@{}: {}", slug, version, e))
        })?;
        let manifest: PluginManifest = serde_json::from_str(&manifest_data)?;

        let platform = current_platform_key();
        let platform_binary = manifest.platforms.get(&platform).ok_or_else(|| {
            NappError::PluginPlatformUnavailable {
                plugin: slug.to_string(),
                platform,
            }
        })?;

        let binary_path = version_dir.join(&platform_binary.binary_name);
        let binary_data = std::fs::read(&binary_path).map_err(|e| {
            NappError::PluginNotFound(format!("binary for {}@{}: {}", slug, version, e))
        })?;

        let mut hasher = Sha256::new();
        hasher.update(&binary_data);
        let actual_hash = hex::encode(hasher.finalize());

        if actual_hash != platform_binary.sha256 {
            return Err(NappError::Signing(format!(
                "integrity check failed for {}@{}: expected {}, got {}",
                slug, version, platform_binary.sha256, actual_hash
            )));
        }

        Ok(())
    }

    /// List all installed plugins as `(slug, version, binary_path, source)`.
    ///
    /// Scans both user and installed directories. User plugins override
    /// installed plugins when the same slug+version exists in both.
    pub fn list_installed(&self) -> Vec<(String, semver::Version, PathBuf, &'static str)> {
        let mut results = Vec::new();
        let mut seen = HashSet::new();

        // User dir first — takes priority
        self.collect_from_dir(&self.user_dir, "user", &mut results, &mut seen);
        // Installed dir — skips slug+version already seen in user
        self.collect_from_dir(&self.installed_dir, "installed", &mut results, &mut seen);

        results.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
        results
    }

    /// Scan a single root directory for plugins.
    ///
    /// Supports two layouts:
    /// 1. **Versioned** (marketplace): `<root>/<slug>/<version>/plugin.json + binary`
    /// 2. **Flat** (dev repos / symlinks): `<root>/<slug>/plugin.json + target/release/<binary>`
    fn collect_from_dir(
        &self,
        root: &Path,
        source: &'static str,
        results: &mut Vec<(String, semver::Version, PathBuf, &'static str)>,
        seen: &mut HashSet<(String, String)>,
    ) {
        let entries = match std::fs::read_dir(root) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let slug_path = entry.path();
            if !slug_path.is_dir() {
                continue;
            }
            let slug = match slug_path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            // Try flat layout first: plugin.json at slug root (dev repos / symlinks)
            let flat_manifest = slug_path.join("plugin.json");
            if flat_manifest.exists() {
                if let Some((version, binary_path)) =
                    self.try_load_flat_plugin(&slug_path, &flat_manifest)
                {
                    let key = (slug.clone(), version.to_string());
                    if seen.insert(key) {
                        results.push((slug, version, binary_path, source));
                    }
                    continue; // Flat layout found — skip version subdirectory scan
                }
            }

            // Versioned layout: <slug>/<version>/plugin.json + binary
            let version_entries = match std::fs::read_dir(&slug_path) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for ver_entry in version_entries.flatten() {
                let ver_path = ver_entry.path();
                if !ver_path.is_dir() {
                    continue;
                }
                if ver_path.join(".quarantined").exists() {
                    continue;
                }

                let ver_name = match ver_path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n,
                    None => continue,
                };
                let version = match semver::Version::parse(ver_name) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                let key = (slug.clone(), ver_name.to_string());
                if !seen.insert(key) {
                    continue; // Already seen from higher-priority dir
                }

                if let Some(binary_path) = self.find_binary_in_version_dir(&ver_path) {
                    results.push((slug.clone(), version, binary_path, source));
                }
            }
        }
    }

    /// Try to load a plugin from a flat (dev repo) layout.
    ///
    /// Reads version from plugin.json, then looks for the binary in:
    /// 1. The slug directory itself (via `find_binary_in_version_dir`)
    /// 2. `target/release/<binary_name>` (Rust dev repos)
    /// 3. `target/debug/<binary_name>` (Rust dev repos, debug build)
    fn try_load_flat_plugin(
        &self,
        slug_dir: &Path,
        manifest_path: &Path,
    ) -> Option<(semver::Version, PathBuf)> {
        let data = std::fs::read_to_string(manifest_path).ok()?;
        let manifest: PluginManifest = serde_json::from_str(&data).ok()?;
        let version = semver::Version::parse(&manifest.version).ok()?;
        let platform = current_platform_key();

        // Check if parent directory has version subdirs — if so, this isn't flat layout
        // (it's a versioned layout with plugin.json at the wrong level)
        if slug_dir
            .read_dir()
            .ok()
            .map(|entries| {
                entries.flatten().any(|e| {
                    e.path().is_dir()
                        && e.file_name()
                            .to_str()
                            .and_then(|n| semver::Version::parse(n).ok())
                            .is_some()
                })
            })
            .unwrap_or(false)
        {
            return None; // Has version subdirs — not flat layout
        }

        let binary_name = manifest
            .platforms
            .get(&platform)
            .map(|pb| pb.binary_name.as_str())
            .unwrap_or(&manifest.slug);

        // 1. Binary next to plugin.json
        let direct = slug_dir.join(binary_name);
        if direct.is_file() {
            return Some((version, direct));
        }

        // 2. Rust target/release
        let release = slug_dir.join("target").join("release").join(binary_name);
        if release.is_file() {
            return Some((version, release));
        }

        // 3. Rust target/debug
        let debug = slug_dir.join("target").join("debug").join(binary_name);
        if debug.is_file() {
            return Some((version, debug));
        }

        // 4. Fallback: any executable in the slug directory
        if let Some(binary_path) = self.find_binary_in_version_dir(slug_dir) {
            return Some((version, binary_path));
        }

        debug!(
            slug = %manifest.slug,
            dir = %slug_dir.display(),
            "flat plugin found but no binary for platform {platform}"
        );
        None
    }

    /// Build env var pairs for all installed (non-quarantined) plugins.
    ///
    /// Returns `Vec<(env_name, binary_path)>` — e.g., `("GWS_BIN", "/path/to/gws")`.
    /// For plugins with multiple versions, picks the highest semver
    /// (`list_installed` sorts by slug asc, version desc — first per slug wins).
    pub fn build_env_map(&self) -> Vec<(String, String)> {
        let installed = self.list_installed();
        let mut seen = std::collections::HashSet::new();
        let mut result = Vec::new();
        for (slug, _version, binary_path, _source) in installed {
            if seen.insert(slug.clone()) {
                result.push((
                    plugin_env_var(&slug),
                    binary_path.to_string_lossy().into_owned(),
                ));
            }
        }
        result
    }

    /// Build a PATH string that prepends all installed plugin directories
    /// to the system PATH. This ensures plugin binaries can find themselves
    /// and sibling binaries when spawned as subprocesses.
    pub fn path_with_plugins(&self) -> String {
        let installed = self.list_installed();
        let mut dirs = std::collections::HashSet::new();
        let mut prefix_parts = Vec::new();
        for (_slug, _version, binary_path, _source) in &installed {
            if let Some(dir) = binary_path.parent() {
                if dirs.insert(dir.to_path_buf()) {
                    prefix_parts.push(dir.to_string_lossy().into_owned());
                }
            }
        }
        let system_path = std::env::var("PATH").unwrap_or_default();
        if prefix_parts.is_empty() {
            system_path
        } else {
            let sep = if cfg!(windows) { ";" } else { ":" };
            prefix_parts.push(system_path);
            prefix_parts.join(sep)
        }
    }

    /// Remove plugin versions not referenced by any of the given slugs.
    ///
    /// Takes a snapshot of referenced slugs to avoid lock coupling with skill loader.
    pub fn garbage_collect(&self, referenced_slugs: &HashSet<String>) -> Vec<String> {
        let mut removed = Vec::new();

        // GC both directories
        for root in [&self.installed_dir, &self.user_dir] {
            let entries = match std::fs::read_dir(root) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for entry in entries.flatten() {
                let slug_path = entry.path();
                if !slug_path.is_dir() {
                    continue;
                }
                let slug = match slug_path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };

                if !referenced_slugs.contains(&slug) {
                    if let Err(e) = std::fs::remove_dir_all(&slug_path) {
                        warn!(slug = %slug, error = %e, "failed to garbage collect plugin");
                    } else {
                        info!(slug = %slug, "garbage collected unreferenced plugin");
                        removed.push(slug);
                    }
                }
            }
        }

        removed
    }

    /// Remove a plugin entirely (all versions). Checks both user and installed dirs.
    pub fn remove(&self, slug: &str) -> Result<(), NappError> {
        let mut found = false;
        // Remove from user dir if present
        let user_slug_dir = self.user_dir.join(slug);
        if user_slug_dir.exists() {
            std::fs::remove_dir_all(&user_slug_dir)?;
            found = true;
        }
        // Remove from installed dir if present
        let installed_slug_dir = self.installed_dir.join(slug);
        if installed_slug_dir.exists() {
            std::fs::remove_dir_all(&installed_slug_dir)?;
            found = true;
        }
        if !found {
            return Err(NappError::PluginNotFound(slug.to_string()));
        }
        info!(slug = slug, "removed plugin");
        Ok(())
    }

    /// Quarantine a plugin version (delete binary, write `.quarantined` marker).
    pub fn quarantine(&self, slug: &str, version: &str, reason: &str) {
        // Find which dir contains this version
        let user_ver = self.user_dir.join(slug).join(version);
        let version_dir = if user_ver.exists() {
            user_ver
        } else {
            self.installed_dir.join(slug).join(version)
        };
        if !version_dir.exists() {
            return;
        }

        // Write quarantine marker
        let marker = version_dir.join(".quarantined");
        let _ = std::fs::write(&marker, reason);

        // Remove the binary (preserve manifest for investigation)
        if let Some(binary_path) = self.find_binary_in_version_dir(&version_dir) {
            let _ = std::fs::remove_file(&binary_path);
        }

        warn!(
            plugin = slug,
            version = version,
            reason = reason,
            "quarantined plugin"
        );
    }

    /// Read the manifest for a plugin's latest installed version.
    /// Checks user dir first (override), then installed dir.
    pub fn get_manifest(&self, slug: &str) -> Option<PluginManifest> {
        // User dir takes priority
        if let Some(m) = self.get_manifest_from_dir(&self.user_dir, slug) {
            return Some(m);
        }
        self.get_manifest_from_dir(&self.installed_dir, slug)
    }

    /// Read manifest from a single root directory.
    fn get_manifest_from_dir(&self, root: &Path, slug: &str) -> Option<PluginManifest> {
        let slug_dir = root.join(slug);
        if !slug_dir.exists() {
            return None;
        }

        // Try flat layout first: plugin.json at slug root (dev repos / symlinks)
        let flat_manifest = slug_dir.join("plugin.json");
        if flat_manifest.exists() {
            let data = std::fs::read_to_string(&flat_manifest).ok()?;
            return serde_json::from_str(&data).ok();
        }

        // Find the latest version directory
        let mut best: Option<(semver::Version, PathBuf)> = None;
        let entries = std::fs::read_dir(&slug_dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || path.join(".quarantined").exists() {
                continue;
            }
            let ver_name = path.file_name()?.to_str()?;
            let version = semver::Version::parse(ver_name).ok()?;
            match &best {
                Some((current, _)) if &version <= current => {}
                _ => best = Some((version, path)),
            }
        }

        let (_, version_dir) = best?;
        let manifest_path = version_dir.join("plugin.json");
        let data = std::fs::read_to_string(&manifest_path).ok()?;
        serde_json::from_str(&data).ok()
    }

    /// Get binary path and auth config for a plugin, if auth is declared.
    pub fn get_auth_info(&self, slug: &str) -> Option<(PathBuf, PluginAuth)> {
        let binary_path = self.resolve(slug, "*")?;
        let manifest = self.get_manifest(slug)?;
        let auth = manifest.auth?;
        Some((binary_path, auth))
    }

    /// Get plugin-to-plugin dependencies from the manifest.
    pub fn get_dependencies(&self, slug: &str) -> Vec<PluginDependency> {
        self.get_manifest(slug)
            .map(|m| m.dependencies)
            .unwrap_or_default()
    }

    /// Ensure all non-optional plugin dependencies are installed.
    ///
    /// Iterates `manifest.dependencies`, resolves each, and calls the provided
    /// download function for any that are missing. Returns the slugs that were
    /// actually installed (not those already present).
    pub async fn ensure_deps<F, Fut>(
        &self,
        manifest: &PluginManifest,
        download_fn: F,
    ) -> Result<Vec<String>, NappError>
    where
        F: Fn(String, String) -> Fut + Clone,
        Fut: std::future::Future<Output = Result<(PluginManifest, Vec<u8>), NappError>>,
    {
        let mut installed = Vec::new();
        for dep in &manifest.dependencies {
            if dep.optional {
                continue;
            }
            // Already resolved locally? Skip.
            if self.resolve(&dep.name, &dep.version).is_some() {
                continue;
            }
            info!(
                parent = %manifest.slug,
                dep = %dep.name,
                version = %dep.version,
                "installing plugin dependency"
            );
            match self
                .ensure(&dep.name, &dep.version, download_fn.clone())
                .await
            {
                Ok(_) => installed.push(dep.name.clone()),
                Err(e) => {
                    warn!(
                        parent = %manifest.slug,
                        dep = %dep.name,
                        error = %e,
                        "failed to install plugin dependency"
                    );
                    return Err(e);
                }
            }
        }
        Ok(installed)
    }

    /// Get event definitions for a plugin, if declared in its manifest.
    pub fn get_events(&self, slug: &str) -> Option<Vec<PluginEventDef>> {
        let manifest = self.get_manifest(slug)?;
        manifest.events
    }

    /// Look up a specific event definition by plugin slug and event name.
    pub fn resolve_event(&self, slug: &str, event_name: &str) -> Option<PluginEventDef> {
        let events = self.get_events(slug)?;
        events.into_iter().find(|e| e.name == event_name)
    }

    /// Get the channel bridge definition for a plugin, if declared.
    pub fn get_channel_def(&self, slug: &str) -> Option<PluginChannel> {
        let manifest = self.get_manifest(slug)?;
        manifest.channel
    }

    /// List help docs from a plugin's `help/` directory.
    ///
    /// Returns `(filename, content)` pairs for all `.md` files found.
    pub fn list_help_docs(&self, slug: &str) -> Vec<(String, String)> {
        let binary = match self.resolve(slug, "*") {
            Some(p) => p,
            None => return Vec::new(),
        };
        let plugin_dir = match binary.parent() {
            Some(d) => d,
            None => return Vec::new(),
        };
        let help_dir = plugin_dir.join("help");
        if !help_dir.is_dir() {
            return Vec::new();
        }
        let mut docs = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&help_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "md") {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        let name = path
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();
                        docs.push((name, content));
                    }
                }
            }
        }
        docs.sort_by(|a, b| a.0.cmp(&b.0));
        docs
    }

    /// Find a binary in a version directory by reading plugin.json or scanning for executables.
    fn find_binary_in_version_dir(&self, version_dir: &Path) -> Option<PathBuf> {
        // Try plugin.json first
        let manifest_path = version_dir.join("plugin.json");
        if manifest_path.exists() {
            if let Ok(data) = std::fs::read_to_string(&manifest_path) {
                if let Ok(manifest) = serde_json::from_str::<PluginManifest>(&data) {
                    let platform = current_platform_key();
                    if let Some(pb) = manifest.platforms.get(&platform) {
                        let binary_path = version_dir.join(&pb.binary_name);
                        if binary_path.is_file() {
                            return Some(binary_path);
                        }
                    }
                }
            }
        }

        // Fallback: find first executable file
        let entries = std::fs::read_dir(version_dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            // Skip metadata files
            let name = path.file_name()?.to_str()?;
            if name == "plugin.json" || name.starts_with('.') {
                continue;
            }
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
                // On Windows, check for common executable extensions
                if name.ends_with(".exe") || name.ends_with(".bat") || name.ends_with(".cmd") {
                    return Some(path);
                }
            }
        }

        None
    }

    /// Start watching for filesystem changes in plugin directories.
    ///
    /// Re-scans on file changes and emits diff events for added/removed plugins.
    /// Mirrors `AgentLoader::watch()`.
    pub fn watch(
        &self,
    ) -> (
        tokio::task::JoinHandle<()>,
        tokio::sync::mpsc::Receiver<PluginFsEvent>,
    ) {
        let installed_dir = self.installed_dir.clone();
        let user_dir = self.user_dir.clone();
        let (event_tx, event_rx) = tokio::sync::mpsc::channel::<PluginFsEvent>(32);

        // Snapshot current state for diffing
        let initial: HashMap<String, PathBuf> = self
            .list_installed()
            .into_iter()
            .map(|(slug, _ver, path, _src)| (slug, path))
            .collect();
        let prev = Arc::new(tokio::sync::RwLock::new(initial));

        let store_installed_dir = self.installed_dir.clone();
        let store_user_dir = self.user_dir.clone();

        let handle = tokio::spawn(async move {
            use notify::{Event, EventKind, RecursiveMode, Watcher};
            use tokio::sync::mpsc;

            let (tx, mut rx) = mpsc::channel::<notify::Result<Event>>(32);

            let mut watcher = match notify::RecommendedWatcher::new(
                move |res| {
                    let _ = tx.blocking_send(res);
                },
                notify::Config::default()
                    .with_poll_interval(std::time::Duration::from_secs(2)),
            ) {
                Ok(w) => w,
                Err(e) => {
                    warn!(error = %e, "failed to create filesystem watcher for plugins");
                    return;
                }
            };

            if user_dir.exists() {
                if let Err(e) = watcher.watch(&user_dir, RecursiveMode::Recursive) {
                    warn!(error = %e, dir = %user_dir.display(), "failed to watch user plugins dir");
                }
            }

            if installed_dir.exists() {
                if let Err(e) = watcher.watch(&installed_dir, RecursiveMode::Recursive) {
                    warn!(error = %e, dir = %installed_dir.display(), "failed to watch installed plugins dir");
                }
            }

            let mut last_reload = std::time::Instant::now();
            let debounce = std::time::Duration::from_secs(2);

            while let Some(result) = rx.recv().await {
                match result {
                    Ok(event) => {
                        let dominated = matches!(
                            event.kind,
                            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                        );
                        if !dominated {
                            continue;
                        }

                        let relevant = event.paths.iter().any(|p| {
                            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                            name == "plugin.json"
                                || name == "PLUGIN.md"
                                || name.ends_with(".napp")
                                // New symlink/directory added directly under watched dir
                                || (matches!(event.kind, EventKind::Create(_))
                                    && (p.parent() == Some(user_dir.as_path())
                                        || p.parent() == Some(installed_dir.as_path()))
                                    && p.is_dir())
                                // Binary rebuilt in target/release or target/debug
                                || p.ancestors().any(|a| {
                                    a.file_name()
                                        .and_then(|n| n.to_str())
                                        .map(|n| n == "release" || n == "debug")
                                        .unwrap_or(false)
                                })
                        });
                        if !relevant {
                            continue;
                        }

                        if last_reload.elapsed() < debounce {
                            continue;
                        }
                        last_reload = std::time::Instant::now();

                        debug!("plugins directory changed, re-scanning");

                        // Re-scan both directories using the same logic as list_installed
                        let tmp_store = PluginStore::new(
                            store_installed_dir.clone(),
                            store_user_dir.clone(),
                            None,
                        );
                        let current: HashMap<String, PathBuf> = tmp_store
                            .list_installed()
                            .into_iter()
                            .map(|(slug, _ver, path, _src)| (slug, path))
                            .collect();

                        // Diff against previous snapshot
                        {
                            let old = prev.read().await;
                            for (slug, path) in &current {
                                if !old.contains_key(slug) {
                                    let _ = event_tx
                                        .send(PluginFsEvent::Added {
                                            slug: slug.clone(),
                                            binary_path: path.clone(),
                                        })
                                        .await;
                                } else if old.get(slug) != Some(path) {
                                    let _ = event_tx
                                        .send(PluginFsEvent::Changed {
                                            slug: slug.clone(),
                                            binary_path: path.clone(),
                                        })
                                        .await;
                                }
                            }
                            for slug in old.keys() {
                                if !current.contains_key(slug) {
                                    let _ = event_tx
                                        .send(PluginFsEvent::Removed {
                                            slug: slug.clone(),
                                        })
                                        .await;
                                }
                            }
                        }

                        let count = current.len();
                        *prev.write().await = current;
                        info!(count, "re-scanned plugins after filesystem change");
                    }
                    Err(e) => {
                        warn!(error = %e, "filesystem watch error (plugins)");
                    }
                }
            }
        });

        (handle, event_rx)
    }
}

/// Filesystem change event emitted by `PluginStore::watch()`.
#[derive(Debug, Clone)]
pub enum PluginFsEvent {
    Added { slug: String, binary_path: PathBuf },
    Changed { slug: String, binary_path: PathBuf },
    Removed { slug: String },
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Detect the current platform key matching NeboAI conventions.
///
/// Returns e.g., "darwin-arm64", "linux-amd64", "windows-amd64".
pub fn current_platform_key() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => other,
    };
    format!("{}-{}", os, arch)
}

/// Derive the environment variable name for a plugin binary path.
///
/// `gws` → `GWS_BIN`, `my-tool` → `MY_TOOL_BIN`.
pub fn plugin_env_var(slug: &str) -> String {
    format!("{}_BIN", slug.to_uppercase().replace('-', "_"))
}

/// Returns the `{SLUG}_DATA` environment variable name for a plugin's persistent data directory.
///
/// Example: `nebo-office` → `NEBO_OFFICE_DATA`.
pub fn plugin_data_env_var(slug: &str) -> String {
    format!("{}_DATA", slug.to_uppercase().replace('-', "_"))
}

/// Run a single plugin's auth status command. Returns `true` if authenticated.
async fn run_auth_status_check(store: &PluginStore, slug: &str, path_env: &str) -> bool {
    let Some((binary_path, auth)) = store.get_auth_info(slug) else {
        return true; // no auth config → treat as authenticated
    };
    let Some(status_cmd) = auth.commands.status.as_deref() else {
        return true; // no status command → treat as authenticated
    };
    let resolved_env = store.resolved_auth_env(slug);
    let args: Vec<&str> = status_cmd.split_whitespace().collect();
    let mut cmd = tokio::process::Command::new(&binary_path);
    cmd.args(&args);
    cmd.env("PATH", path_env);
    for (key, value) in &resolved_env {
        cmd.env(key, value);
    }
    match cmd.output().await {
        Ok(output) => output.status.success(),
        Err(_) => false,
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_platform_key() {
        let key = current_platform_key();
        // Should be non-empty and contain a dash
        assert!(key.contains('-'), "platform key should be os-arch: {}", key);
    }

    #[test]
    fn test_plugin_env_var() {
        assert_eq!(plugin_env_var("gws"), "GWS_BIN");
        assert_eq!(plugin_env_var("my-tool"), "MY_TOOL_BIN");
        assert_eq!(plugin_env_var("ffmpeg"), "FFMPEG_BIN");
    }

    #[test]
    fn test_resolve_empty_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user_dir = tmp.path().join("user");
        std::fs::create_dir_all(&user_dir).unwrap();
        let store = PluginStore::new(tmp.path().to_path_buf(), user_dir, None);
        assert!(store.resolve("nonexistent", "*").is_none());
    }

    #[test]
    fn test_resolve_with_installed_plugin() {
        let tmp = tempfile::TempDir::new().unwrap();
        let plugins_dir = tmp.path();

        // Create a plugin version directory with a binary
        let version_dir = plugins_dir.join("gws").join("1.2.0");
        std::fs::create_dir_all(&version_dir).unwrap();
        let binary_path = version_dir.join("gws");
        std::fs::write(&binary_path, b"fake binary").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let user_plugins_dir = tmp.path().join("user_plugins");
        std::fs::create_dir_all(&user_plugins_dir).unwrap();
        let store = PluginStore::new(plugins_dir.to_path_buf(), user_plugins_dir, None);
        let result = store.resolve("gws", "*");
        assert!(result.is_some());
        assert!(result.unwrap().ends_with("gws"));
    }

    #[test]
    fn test_resolve_semver_range() {
        let tmp = tempfile::TempDir::new().unwrap();
        let plugins_dir = tmp.path();

        // Create multiple versions
        for version in &["1.0.0", "1.2.0", "2.0.0"] {
            let version_dir = plugins_dir.join("gws").join(version);
            std::fs::create_dir_all(&version_dir).unwrap();
            let binary_path = version_dir.join("gws");
            std::fs::write(&binary_path, b"fake binary").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755))
                    .unwrap();
            }
        }

        let user_plugins_dir = tmp.path().join("user_plugins");
        std::fs::create_dir_all(&user_plugins_dir).unwrap();
        let store = PluginStore::new(plugins_dir.to_path_buf(), user_plugins_dir, None);

        // ^1.0.0 should resolve to 1.2.0 (not 2.0.0)
        let result = store.resolve("gws", "^1.0.0");
        assert!(result.is_some());
        let path = result.unwrap();
        assert!(
            path.to_string_lossy().contains("1.2.0"),
            "expected 1.2.0 but got {}",
            path.display()
        );

        // >=2.0.0 should resolve to 2.0.0
        let result = store.resolve("gws", ">=2.0.0");
        assert!(result.is_some());
        assert!(result.unwrap().to_string_lossy().contains("2.0.0"));
    }

    #[test]
    fn test_resolve_skips_quarantined() {
        let tmp = tempfile::TempDir::new().unwrap();
        let plugins_dir = tmp.path();

        let version_dir = plugins_dir.join("gws").join("1.0.0");
        std::fs::create_dir_all(&version_dir).unwrap();
        let binary_path = version_dir.join("gws");
        std::fs::write(&binary_path, b"fake binary").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        // Quarantine it
        std::fs::write(version_dir.join(".quarantined"), "test reason").unwrap();

        let user_plugins_dir = tmp.path().join("user_plugins");
        std::fs::create_dir_all(&user_plugins_dir).unwrap();
        let store = PluginStore::new(plugins_dir.to_path_buf(), user_plugins_dir, None);
        assert!(store.resolve("gws", "*").is_none());
    }

    #[test]
    fn test_list_installed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let plugins_dir = tmp.path();

        for (slug, version) in &[("gws", "1.0.0"), ("gws", "1.2.0"), ("ffmpeg", "5.0.0")] {
            let version_dir = plugins_dir.join(slug).join(version);
            std::fs::create_dir_all(&version_dir).unwrap();
            let binary_path = version_dir.join(slug);
            std::fs::write(&binary_path, b"fake binary").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755))
                    .unwrap();
            }
        }

        let user_plugins_dir = tmp.path().join("user_plugins");
        std::fs::create_dir_all(&user_plugins_dir).unwrap();
        let store = PluginStore::new(plugins_dir.to_path_buf(), user_plugins_dir, None);
        let installed = store.list_installed();
        assert_eq!(installed.len(), 3);
    }

    #[test]
    fn test_garbage_collect() {
        let tmp = tempfile::TempDir::new().unwrap();
        let plugins_dir = tmp.path();

        for slug in &["gws", "ffmpeg", "orphan"] {
            let version_dir = plugins_dir.join(slug).join("1.0.0");
            std::fs::create_dir_all(&version_dir).unwrap();
            std::fs::write(version_dir.join(slug), b"fake").unwrap();
        }

        // Use a separate temp dir for user_plugins to avoid GC seeing it as a slug
        let user_tmp = tempfile::TempDir::new().unwrap();
        let user_plugins_dir = user_tmp.path().to_path_buf();
        let store = PluginStore::new(plugins_dir.to_path_buf(), user_plugins_dir, None);
        let referenced: HashSet<String> = ["gws", "ffmpeg"].iter().map(|s| s.to_string()).collect();
        let removed = store.garbage_collect(&referenced);
        assert_eq!(removed, vec!["orphan"]);
        assert!(!plugins_dir.join("orphan").exists());
        assert!(plugins_dir.join("gws").exists());
    }

    #[test]
    fn test_quarantine() {
        let tmp = tempfile::TempDir::new().unwrap();
        let plugins_dir = tmp.path();

        let version_dir = plugins_dir.join("bad-plugin").join("1.0.0");
        std::fs::create_dir_all(&version_dir).unwrap();
        let binary_path = version_dir.join("bad-plugin");
        std::fs::write(&binary_path, b"malicious").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let user_plugins_dir = tmp.path().join("user_plugins");
        std::fs::create_dir_all(&user_plugins_dir).unwrap();
        let store = PluginStore::new(plugins_dir.to_path_buf(), user_plugins_dir, None);
        store.quarantine("bad-plugin", "1.0.0", "revoked by NeboAI");

        // Binary should be removed, marker should exist
        assert!(!binary_path.exists());
        assert!(version_dir.join(".quarantined").exists());

        // resolve should skip quarantined
        assert!(store.resolve("bad-plugin", "*").is_none());
    }

    #[test]
    fn test_verify_integrity_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user_dir = tmp.path().join("user");
        std::fs::create_dir_all(&user_dir).unwrap();
        let store = PluginStore::new(tmp.path().to_path_buf(), user_dir, None);
        assert!(store.verify_integrity("nonexistent", "1.0.0").is_err());
    }

    #[test]
    fn test_manifest_serde() {
        let manifest = PluginManifest {
            id: "uuid-1234".into(),
            slug: "gws".into(),
            name: "Google Workspace CLI".into(),
            version: "1.2.0".into(),
            description: "CLI for Google Workspace".into(),
            author: "neboai".into(),
            platforms: {
                let mut m = HashMap::new();
                m.insert(
                    "darwin-arm64".into(),
                    PlatformBinary {
                        binary_name: "gws".into(),
                        sha256: "abc123".into(),
                        signature: "sig==".into(),
                        size: 1024,
                        download_url: "https://cdn.neboai.com/plugins/gws/1.2.0/darwin-arm64/gws"
                            .into(),
                    },
                );
                m
            },
            signing_key_id: "key-1".into(),
            env_var: String::new(),
            auth: None,
            events: None,
            dependencies: vec![],
            capabilities: None,
            permissions: None,
            category: String::new(),
            triggers: vec![],
        };

        let json = serde_json::to_string(&manifest).unwrap();
        let parsed: PluginManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.slug, "gws");
        assert_eq!(parsed.version, "1.2.0");
        assert!(parsed.platforms.contains_key("darwin-arm64"));
        assert!(parsed.auth.is_none());
        assert!(parsed.events.is_none());
    }

    #[test]
    fn test_manifest_serde_with_auth() {
        let json = r#"{
            "id": "uuid-1234",
            "slug": "gws",
            "name": "gws",
            "version": "0.22.3",
            "platforms": {},
            "auth": {
                "type": "oauth_cli",
                "env": {
                    "GOOGLE_WORKSPACE_CLI_CLIENT_ID": "123.apps.googleusercontent.com",
                    "GOOGLE_WORKSPACE_CLI_CLIENT_SECRET": "GOCSPX-test"
                },
                "commands": {
                    "login": "auth login",
                    "status": "auth status",
                    "logout": "auth logout"
                },
                "label": "Google Account",
                "description": "Sign in to access Gmail, Drive, and more."
            }
        }"#;

        let parsed: PluginManifest = serde_json::from_str(json).unwrap();
        let auth = parsed.auth.unwrap();
        assert_eq!(auth.auth_type, "oauth_cli");
        assert_eq!(auth.commands.login, "auth login");
        assert_eq!(auth.commands.status.as_deref(), Some("auth status"));
        assert_eq!(auth.env.len(), 2);
        assert_eq!(auth.label, "Google Account");
    }

    #[test]
    fn test_manifest_without_auth_field() {
        // Existing manifests without auth field should deserialize fine
        let json = r#"{
            "id": "uuid-1234",
            "slug": "ffmpeg",
            "name": "ffmpeg",
            "version": "5.0.0",
            "platforms": {}
        }"#;

        let parsed: PluginManifest = serde_json::from_str(json).unwrap();
        assert!(parsed.auth.is_none());
        assert!(parsed.events.is_none());
    }

    #[test]
    fn test_manifest_with_events() {
        let json = r#"{
            "id": "uuid-1234",
            "slug": "gws",
            "name": "Google Workspace",
            "version": "1.3.0",
            "platforms": {},
            "events": [
                {
                    "name": "email.new",
                    "description": "Fires when a new email arrives",
                    "command": "gmail +watch --format ndjson"
                },
                {
                    "name": "calendar.event",
                    "description": "Calendar event changes",
                    "command": "calendar +watch --format ndjson",
                    "multiplexed": true
                }
            ]
        }"#;

        let parsed: PluginManifest = serde_json::from_str(json).unwrap();
        let events = parsed.events.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].name, "email.new");
        assert_eq!(events[0].command, "gmail +watch --format ndjson");
        assert!(!events[0].multiplexed);
        assert_eq!(events[1].name, "calendar.event");
        assert!(events[1].multiplexed);
    }

    #[test]
    fn test_manifest_without_events_backward_compat() {
        // Existing manifests without events field should deserialize fine
        let json = r#"{
            "id": "uuid-1234",
            "slug": "gws",
            "name": "gws",
            "version": "0.22.3",
            "platforms": {},
            "auth": {
                "type": "oauth_cli",
                "commands": { "login": "auth login" },
                "label": "Google"
            }
        }"#;

        let parsed: PluginManifest = serde_json::from_str(json).unwrap();
        assert!(parsed.auth.is_some());
        assert!(parsed.events.is_none());
    }

    #[test]
    fn test_event_def_serde_defaults() {
        let json = r#"{
            "name": "email.new",
            "command": "gmail +watch"
        }"#;
        let event: PluginEventDef = serde_json::from_str(json).unwrap();
        assert_eq!(event.name, "email.new");
        assert_eq!(event.command, "gmail +watch");
        assert!(event.description.is_empty());
        assert!(!event.multiplexed);
    }

    // ── Validation Tests ────────────────────────────────────────────

    fn make_valid_manifest() -> PluginManifest {
        PluginManifest {
            id: "uuid-1234".into(),
            slug: "gws".into(),
            name: "Google Workspace".into(),
            version: "1.2.0".into(),
            description: "CLI for Google Workspace".into(),
            author: "neboai".into(),
            platforms: {
                let mut m = HashMap::new();
                m.insert(
                    "darwin-arm64".into(),
                    PlatformBinary {
                        binary_name: "gws".into(),
                        sha256: "abc123".into(),
                        signature: "sig==".into(),
                        size: 1024,
                        download_url: "https://cdn.neboai.com/gws".into(),
                    },
                );
                m
            },
            signing_key_id: String::new(),
            env_var: String::new(),
            auth: None,
            events: None,
            dependencies: vec![],
            capabilities: None,
            permissions: None,
            category: String::new(),
            triggers: vec![],
        }
    }

    #[test]
    fn test_validate_good_manifest() {
        let m = make_valid_manifest();
        assert!(m.validate().is_ok());
    }

    #[test]
    fn test_validate_empty_slug() {
        let mut m = make_valid_manifest();
        m.slug = String::new();
        assert!(m.validate().is_err());
    }

    #[test]
    fn test_validate_uppercase_slug() {
        let mut m = make_valid_manifest();
        m.slug = "GWS".into();
        assert!(m.validate().is_err());
    }

    #[test]
    fn test_validate_leading_hyphen_slug() {
        let mut m = make_valid_manifest();
        m.slug = "-gws".into();
        assert!(m.validate().is_err());
    }

    #[test]
    fn test_validate_trailing_hyphen_slug() {
        let mut m = make_valid_manifest();
        m.slug = "gws-".into();
        assert!(m.validate().is_err());
    }

    #[test]
    fn test_validate_consecutive_hyphens_slug() {
        let mut m = make_valid_manifest();
        m.slug = "gws--cli".into();
        assert!(m.validate().is_err());
    }

    #[test]
    fn test_validate_invalid_semver() {
        let mut m = make_valid_manifest();
        m.version = "latest".into();
        assert!(m.validate().is_err());
    }

    #[test]
    fn test_validate_empty_platforms() {
        let mut m = make_valid_manifest();
        m.platforms.clear();
        assert!(m.validate().is_err());
    }

    #[test]
    fn test_validate_binary_name_path_traversal() {
        let mut m = make_valid_manifest();
        m.platforms.get_mut("darwin-arm64").unwrap().binary_name = "../evil".into();
        assert!(m.validate().is_err());
    }

    #[test]
    fn test_validate_binary_name_path_separator() {
        let mut m = make_valid_manifest();
        m.platforms.get_mut("darwin-arm64").unwrap().binary_name = "bin/gws".into();
        assert!(m.validate().is_err());
    }

    #[test]
    fn test_validate_binary_name_empty() {
        let mut m = make_valid_manifest();
        m.platforms.get_mut("darwin-arm64").unwrap().binary_name = String::new();
        assert!(m.validate().is_err());
    }

    #[test]
    fn test_validate_auth_empty_login() {
        let mut m = make_valid_manifest();
        m.auth = Some(PluginAuth {
            auth_type: "oauth_cli".into(),
            env: HashMap::new(),
            commands: PluginAuthCommands {
                login: String::new(),
                status: None,
                logout: None,
            },
            label: String::new(),
            description: String::new(),
        });
        assert!(m.validate().is_err());
    }

    #[test]
    fn test_validate_event_empty_name() {
        let mut m = make_valid_manifest();
        m.events = Some(vec![PluginEventDef {
            name: String::new(),
            description: String::new(),
            command: "watch".into(),
            multiplexed: false,
        }]);
        assert!(m.validate().is_err());
    }

    #[test]
    fn test_validate_event_name_with_path_separator() {
        let mut m = make_valid_manifest();
        m.events = Some(vec![PluginEventDef {
            name: "../hack".into(),
            description: String::new(),
            command: "watch".into(),
            multiplexed: false,
        }]);
        assert!(m.validate().is_err());
    }

    #[test]
    fn test_validate_event_empty_command() {
        let mut m = make_valid_manifest();
        m.events = Some(vec![PluginEventDef {
            name: "email.new".into(),
            description: String::new(),
            command: String::new(),
            multiplexed: false,
        }]);
        assert!(m.validate().is_err());
    }

    // ── Dependency Tests ────────────────────────────────────────────

    #[test]
    fn test_manifest_with_dependencies() {
        let json = r#"{
            "id": "uuid-1234",
            "slug": "digest",
            "name": "Digest",
            "version": "1.2.0",
            "platforms": {
                "darwin-arm64": {
                    "binaryName": "digest",
                    "sha256": "abc",
                    "signature": "sig",
                    "size": 1024,
                    "downloadUrl": "https://cdn.neboai.com/digest"
                }
            },
            "dependencies": [
                { "name": "ffmpeg", "version": ">=5.0.0" },
                { "name": "nebo-pdf", "optional": true }
            ]
        }"#;

        let parsed: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.dependencies.len(), 2);
        assert_eq!(parsed.dependencies[0].name, "ffmpeg");
        assert_eq!(parsed.dependencies[0].version, ">=5.0.0");
        assert!(!parsed.dependencies[0].optional);
        assert_eq!(parsed.dependencies[1].name, "nebo-pdf");
        assert_eq!(parsed.dependencies[1].version, "*");
        assert!(parsed.dependencies[1].optional);
    }

    #[test]
    fn test_manifest_without_dependencies_backward_compat() {
        let json = r#"{
            "id": "uuid-1234",
            "slug": "ffmpeg",
            "name": "ffmpeg",
            "version": "5.0.0",
            "platforms": {}
        }"#;

        let parsed: PluginManifest = serde_json::from_str(json).unwrap();
        assert!(parsed.dependencies.is_empty());
    }

    #[test]
    fn test_validate_manifest_with_dependencies() {
        let mut m = make_valid_manifest();
        m.dependencies = vec![PluginDependency {
            name: "ffmpeg".into(),
            version: ">=5.0.0".into(),
            optional: false,
        }];
        assert!(m.validate().is_ok());
    }

    // ── Capabilities Tests ──────────────────────────────────────────

    #[test]
    fn test_manifest_without_capabilities_backward_compat() {
        let json = r#"{
            "id": "uuid-1234",
            "slug": "gws",
            "name": "gws",
            "version": "1.0.0",
            "platforms": {}
        }"#;
        let parsed: PluginManifest = serde_json::from_str(json).unwrap();
        assert!(parsed.capabilities.is_none());
    }

    #[test]
    fn test_manifest_with_capabilities_tools() {
        let json = r#"{
            "id": "uuid-1234",
            "slug": "gws",
            "name": "Google Workspace",
            "version": "1.3.0",
            "platforms": {},
            "capabilities": {
                "tools": [
                    {
                        "name": "gws.gmail.triage",
                        "description": "Triage recent Gmail messages.",
                        "command": "gmail +triage",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "max": { "type": "integer", "default": 5 }
                            }
                        },
                        "timeoutSeconds": 180
                    }
                ]
            }
        }"#;
        let parsed: PluginManifest = serde_json::from_str(json).unwrap();
        let caps = parsed.capabilities.unwrap();
        assert_eq!(caps.tools.len(), 1);
        assert_eq!(caps.tools[0].name, "gws.gmail.triage");
        assert_eq!(caps.tools[0].command, "gmail +triage");
        assert!(caps.tools[0].approval); // default true
        assert_eq!(caps.tools[0].timeout_seconds, 180);
        assert!(caps.tools[0].input_schema.is_some());
    }

    #[test]
    fn test_manifest_with_capabilities_hooks() {
        let json = r#"{
            "id": "uuid-1234",
            "slug": "audit",
            "name": "Audit Plugin",
            "version": "1.0.0",
            "platforms": {},
            "capabilities": {
                "hooks": [
                    {
                        "hook": "tool.pre_execute",
                        "hookType": "filter",
                        "priority": 50,
                        "command": "hooks tool-pre-execute",
                        "timeoutMs": 1000
                    }
                ]
            }
        }"#;
        let parsed: PluginManifest = serde_json::from_str(json).unwrap();
        let caps = parsed.capabilities.unwrap();
        assert_eq!(caps.hooks.len(), 1);
        assert_eq!(caps.hooks[0].hook, "tool.pre_execute");
        assert_eq!(caps.hooks[0].hook_type, "filter");
        assert_eq!(caps.hooks[0].priority, 50);
        assert_eq!(caps.hooks[0].timeout_ms, 1000);
    }

    #[test]
    fn test_manifest_with_capabilities_providers() {
        let json = r#"{
            "id": "uuid-1234",
            "slug": "openrouter",
            "name": "OpenRouter",
            "version": "1.0.0",
            "platforms": {},
            "capabilities": {
                "providers": [
                    {
                        "id": "openrouter",
                        "displayName": "OpenRouter",
                        "providerType": "model",
                        "modelsCommand": "provider models",
                        "chatCommand": "provider chat"
                    }
                ]
            }
        }"#;
        let parsed: PluginManifest = serde_json::from_str(json).unwrap();
        let caps = parsed.capabilities.unwrap();
        assert_eq!(caps.providers.len(), 1);
        assert_eq!(caps.providers[0].id, "openrouter");
        assert_eq!(caps.providers[0].provider_type, "model");
    }

    #[test]
    fn test_capabilities_serde_round_trip() {
        let caps = PluginCapabilities {
            tools: vec![PluginToolDef {
                name: "test.tool".into(),
                description: "A test tool".into(),
                command: "run test".into(),
                input_schema: None,
                approval: true,
                timeout_seconds: 60,
            }],
            hooks: vec![],
            commands: vec![PluginCommandDef {
                name: "/test".into(),
                description: "A test command".into(),
                command: "test cmd".into(),
                slash: true,
            }],
            routes: vec![],
            providers: vec![],
            config_schema: vec![],
        };

        let json = serde_json::to_string(&caps).unwrap();
        let parsed: PluginCapabilities = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.tools.len(), 1);
        assert_eq!(parsed.commands.len(), 1);
        assert!(parsed.hooks.is_empty());
        assert!(parsed.routes.is_empty());
        assert!(parsed.providers.is_empty());
    }
}
