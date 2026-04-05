use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tracing::{debug, info, warn};

use crate::origin::ToolContext;
use crate::process;
use crate::registry::{DynTool, ToolResult};

/// STRAP domain tool for installed plugin binaries.
///
/// Plugins ship with their own skills (`skills/` directory inside the plugin).
/// These skills are the plugin's documentation — they describe the CLI syntax,
/// flags, and examples. The plugin tool routes to them via `action: "help"`.
///
/// When a plugin command fails due to stale OAuth credentials, the tool
/// automatically detects the auth failure, triggers re-authentication via
/// the plugin's declared `auth login` command, and retries the original command.
pub struct PluginTool {
    plugin_store: Arc<napp::plugin::PluginStore>,
    broadcaster: Option<crate::web_tool::Broadcaster>,
}

#[derive(Debug, Deserialize)]
struct PluginInput {
    /// Plugin slug (e.g., "gws").
    resource: String,
    /// Action: "exec" (default) to run a command, "help" to read plugin skill docs,
    /// "services" to list available services.
    #[serde(default = "default_action")]
    action: String,
    /// CLI arguments passed to the plugin binary (required for exec).
    #[serde(default)]
    command: String,
    /// Named flags passed directly to the binary without shell parsing.
    /// Each key becomes --key and the value is passed as a separate OS arg.
    /// Use this for content that may contain special characters.
    #[serde(default)]
    args: std::collections::HashMap<String, String>,
    /// Service/topic name for help lookup (e.g., "gmail", "docs", "calendar").
    #[serde(default)]
    topic: String,
    /// Optional timeout in seconds (default: 120).
    #[serde(default)]
    timeout: i64,
}

fn default_action() -> String {
    "exec".to_string()
}

impl PluginTool {
    pub fn new(plugin_store: Arc<napp::plugin::PluginStore>) -> Self {
        Self {
            plugin_store,
            broadcaster: None,
        }
    }

    pub fn with_broadcaster(mut self, broadcaster: crate::web_tool::Broadcaster) -> Self {
        self.broadcaster = Some(broadcaster);
        self
    }

    /// Build a deduplicated list of installed plugin slugs.
    fn installed_slugs(&self) -> Vec<String> {
        let installed = self.plugin_store.list_installed();
        let mut seen = std::collections::HashSet::new();
        let mut slugs = Vec::new();
        for (slug, _, _, _) in &installed {
            if seen.insert(slug.clone()) {
                slugs.push(slug.clone());
            }
        }
        slugs
    }

    /// Find the skills directory for a plugin slug.
    fn skills_dir(&self, slug: &str) -> Option<PathBuf> {
        // Resolve binary path, then look for skills/ sibling directory
        let binary_path = self.plugin_store.resolve(slug, "*")?;
        let version_dir = binary_path.parent()?;
        let skills_dir = version_dir.join("skills");
        if skills_dir.is_dir() {
            Some(skills_dir)
        } else {
            None
        }
    }

    /// List available services (top-level skill names) for a plugin.
    fn list_services(&self, slug: &str) -> Vec<(String, String)> {
        let skills_dir = match self.skills_dir(slug) {
            Some(d) => d,
            None => return Vec::new(),
        };

        let mut services = Vec::new();
        let entries = match std::fs::read_dir(&skills_dir) {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let skill_md = path.join("SKILL.md");
            if !skill_md.exists() {
                continue;
            }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            // Read first few lines to get the description from frontmatter
            let description = Self::read_skill_description(&skill_md);
            services.push((name, description));
        }
        services.sort_by(|a, b| a.0.cmp(&b.0));
        services
    }

    /// Read skill SKILL.md and extract the description from YAML frontmatter.
    fn read_skill_description(path: &std::path::Path) -> String {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return String::new(),
        };
        // Parse YAML frontmatter between --- markers
        if let Some(rest) = content.strip_prefix("---") {
            if let Some(end) = rest.find("---") {
                let yaml = &rest[..end];
                for line in yaml.lines() {
                    let line = line.trim();
                    if let Some(desc) = line.strip_prefix("description:") {
                        return desc.trim().trim_matches('"').to_string();
                    }
                }
            }
        }
        String::new()
    }

    /// Read a specific skill's full SKILL.md content for help.
    fn read_skill_help(&self, slug: &str, topic: &str) -> Option<String> {
        let skills_dir = self.skills_dir(slug)?;

        // Try exact match first (e.g., "gws-gmail"), then prefixed (e.g., "gmail" → "gws-gmail")
        let candidates = vec![
            skills_dir.join(topic),
            skills_dir.join(format!("{}-{}", slug, topic)),
        ];

        for dir in candidates {
            let skill_md = dir.join("SKILL.md");
            if skill_md.exists() {
                return std::fs::read_to_string(&skill_md).ok();
            }
        }
        None
    }
}

impl DynTool for PluginTool {
    fn name(&self) -> &str {
        "plugin"
    }

    fn description(&self) -> String {
        let slugs = self.installed_slugs();
        let mut desc = String::from(
            "Execute installed plugin binaries or browse their documentation.\n\n\
             Actions:\n\
             - exec: Run a plugin command (default)\n\
             - services: List available services/commands for a plugin\n\
             - help: Read documentation for a specific service (use topic param)\n\
             - events: List declared events for a plugin (NDJSON watch capabilities)\n\n\
             For exec: use `command` for subcommand + simple flags, and `args` for values \
             that may contain special characters (quotes, backticks, $, etc.).\n\n\
             Plugin capabilities:\n\
             - Events: Plugins can declare watch events in plugin.json (e.g. email.new, calendar.event).\n  \
               These are long-running NDJSON processes that auto-emit into the EventBus.\n  \
               Use `events` action to discover what events a plugin provides.\n  \
               Agents reference plugin events via watch triggers: plugin=\"gws\", event=\"email.new\".\n\
             - Auth: Plugins can declare OAuth flows. Auth is handled by Nebo — plugins receive\n  \
               tokens via environment variables at runtime.\n\n\
             Available plugins:\n",
        );
        for slug in &slugs {
            if let Some(manifest) = self.plugin_store.get_manifest(slug) {
                desc.push_str(&format!("- {} — {}\n", slug, manifest.description));
            } else {
                desc.push_str(&format!("- {}\n", slug));
            }
        }
        desc.push_str("\nWorkflow: services → help → exec. Use events to discover watch capabilities. Always check docs before executing unfamiliar commands.");
        desc
    }

    fn schema(&self) -> serde_json::Value {
        let slugs = self.installed_slugs();
        let enum_values: Vec<serde_json::Value> = slugs
            .iter()
            .map(|s| serde_json::Value::String(s.clone()))
            .collect();

        let mut props = serde_json::Map::new();
        props.insert(
            "resource".into(),
            serde_json::json!({
                "type": "string",
                "description": "Plugin slug",
                "enum": enum_values
            }),
        );
        props.insert(
            "action".into(),
            serde_json::json!({
                "type": "string",
                "description": "Action: exec (run command), services (list available), help (read docs for a topic), events (list declared NDJSON events)",
                "enum": ["exec", "services", "help", "events"],
                "default": "exec"
            }),
        );
        props.insert(
            "command".into(),
            serde_json::json!({
                "type": "string",
                "description": "CLI subcommand and flags (e.g., 'gmail +triage --max 5')"
            }),
        );
        props.insert(
            "args".into(),
            serde_json::json!({
                "type": "object",
                "description": "Named flags passed directly to the binary. Each key becomes --key with the value as a separate argument. Use this for content that may contain special characters (quotes, backticks, dollar signs, etc.). Example: {\"text\": \"Hello world!\", \"max\": \"5\"}",
                "additionalProperties": { "type": "string" }
            }),
        );
        props.insert(
            "topic".into(),
            serde_json::json!({
                "type": "string",
                "description": "Service name for help action (e.g., 'gmail', 'docs', 'calendar')"
            }),
        );
        props.insert(
            "timeout".into(),
            serde_json::json!({
                "type": "integer",
                "description": "Command timeout in seconds (default: 120)"
            }),
        );

        serde_json::json!({
            "type": "object",
            "properties": serde_json::Value::Object(props),
            "required": ["resource"]
        })
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn requires_approval_for(&self, input: &serde_json::Value) -> bool {
        // help, services, and events are read-only; exec needs approval
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("exec");
        action == "exec"
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let pi: PluginInput = match serde_json::from_value(input) {
                Ok(v) => v,
                Err(e) => return ToolResult::error(format!("invalid input: {}", e)),
            };

            if pi.resource.is_empty() {
                let slugs = self.installed_slugs();
                return ToolResult::error(format!(
                    "resource is required. Available plugins: {}",
                    slugs.join(", ")
                ));
            }

            match pi.action.as_str() {
                "services" => self.handle_services(&pi.resource),
                "help" => self.handle_help(&pi.resource, &pi.topic),
                "events" => self.handle_events(&pi.resource),
                _ => self.handle_exec(&pi).await,
            }
        })
    }
}

impl PluginTool {
    fn handle_services(&self, slug: &str) -> ToolResult {
        let services = self.list_services(slug);
        if services.is_empty() {
            return ToolResult::error(format!(
                "No services found for plugin '{}'. The plugin may not include documentation.",
                slug
            ));
        }

        let mut result = format!("Available services for **{}**:\n\n", slug);
        for (name, desc) in &services {
            // Strip slug prefix for display (gws-gmail → gmail)
            let short = name.strip_prefix(&format!("{}-", slug)).unwrap_or(name);
            if desc.is_empty() {
                result.push_str(&format!("- {}\n", short));
            } else {
                result.push_str(&format!("- **{}** — {}\n", short, desc));
            }
        }
        result.push_str(&format!(
            "\nUse plugin(resource: \"{}\", action: \"help\", topic: \"<service>\") to read docs for a specific service.",
            slug
        ));
        ToolResult::ok(result)
    }

    fn handle_events(&self, slug: &str) -> ToolResult {
        let events = self.plugin_store.get_events(slug);
        match events {
            Some(evts) if !evts.is_empty() => {
                let mut result = format!("Declared events for **{}**:\n\n", slug);
                for ev in &evts {
                    result.push_str(&format!(
                        "- **{}.{}** — {}{}\n",
                        slug,
                        ev.name,
                        if ev.description.is_empty() { "(no description)" } else { &ev.description },
                        if ev.multiplexed { " [multiplexed]" } else { "" }
                    ));
                }
                result.push_str(&format!(
                    "\nAgents can reference these via watch triggers:\n\
                     persona(action: \"create\", name: \"...\", automations: [\n  \
                       {{\"name\": \"...\", \"plugin\": \"{}\", \"event\": \"<event-name>\", \"steps\": [...]}}])",
                    slug
                ));
                ToolResult::ok(result)
            }
            _ => ToolResult::ok(format!(
                "Plugin '{}' has no declared events. Not all plugins produce events — \
                 events are for plugins that run long-lived watch processes outputting NDJSON.",
                slug
            )),
        }
    }

    fn handle_help(&self, slug: &str, topic: &str) -> ToolResult {
        if topic.is_empty() {
            // No topic — show the shared/root docs if available, otherwise list services
            if let Some(content) = self.read_skill_help(slug, "shared") {
                return ToolResult::ok(content);
            }
            return self.handle_services(slug);
        }

        match self.read_skill_help(slug, topic) {
            Some(content) => ToolResult::ok(content),
            None => {
                // Try to suggest similar services
                let services = self.list_services(slug);
                let names: Vec<&str> = services.iter()
                    .map(|(n, _)| {
                        n.strip_prefix(&format!("{}-", slug)).unwrap_or(n.as_str())
                    })
                    .collect();
                ToolResult::error(format!(
                    "No docs found for '{}'. Available services: {}",
                    topic,
                    names.join(", ")
                ))
            }
        }
    }

    async fn handle_exec(&self, pi: &PluginInput) -> ToolResult {
        let result = self.run_plugin_command(pi).await;

        // On error, check if it's an auth failure and attempt re-auth.
        if result.is_error {
            if let Some((binary, auth)) = self.plugin_store.get_auth_info(&pi.resource) {
                if is_auth_error(&result.content) {
                    // Confirm with auth status if the command is available
                    if let Some(ref _status_cmd) = auth.commands.status {
                        if self.run_auth_status(&pi.resource, &binary, &auth).await {
                            // Status says authenticated — false positive, return original error
                            return result;
                        }
                    }

                    info!(plugin = %pi.resource, "auth failure detected, triggering re-authentication");

                    // Broadcast re-auth request so frontend can show a notification
                    if let Some(ref bc) = self.broadcaster {
                        bc(
                            "plugin_reauth_request",
                            serde_json::json!({
                                "plugin": &pi.resource,
                                "label": &auth.label,
                            }),
                        );
                    }

                    // Attempt re-auth via plugin's auth login command
                    if self.run_auth_login(&pi.resource, &binary, &auth).await {
                        info!(plugin = %pi.resource, "re-authentication succeeded, retrying command");

                        // Broadcast success
                        if let Some(ref bc) = self.broadcaster {
                            bc(
                                "plugin_auth_complete",
                                serde_json::json!({ "plugin": &pi.resource }),
                            );
                        }

                        return self.run_plugin_command(pi).await;
                    }

                    // Re-auth failed
                    warn!(plugin = %pi.resource, "re-authentication failed");
                    if let Some(ref bc) = self.broadcaster {
                        bc(
                            "plugin_auth_error",
                            serde_json::json!({
                                "plugin": &pi.resource,
                                "error": "Re-authentication failed or timed out",
                            }),
                        );
                    }

                    return ToolResult::error(format!(
                        "Plugin '{}' authentication expired. Re-authentication was attempted but failed. \
                         The user must re-authenticate in Settings > Plugins. \
                         Do NOT call this plugin again until re-authenticated.",
                        pi.resource
                    ));
                }
            }
        }

        result
    }

    /// Execute a plugin command and return the result. Shared by initial call and retry.
    async fn run_plugin_command(&self, pi: &PluginInput) -> ToolResult {
        if pi.command.is_empty() && pi.args.is_empty() {
            return ToolResult::error(
                "command is required for exec. Use action: \"services\" to discover available commands."
            );
        }

        // Resolve binary path
        let binary_path = match self.plugin_store.resolve(&pi.resource, "*") {
            Some(p) => p,
            None => {
                let slugs = self.installed_slugs();
                return ToolResult::error(format!(
                    "Plugin '{}' not found. Available: {}",
                    pi.resource,
                    slugs.join(", ")
                ));
            }
        };

        debug!(
            plugin = %pi.resource,
            command = %pi.command,
            args = ?pi.args,
            binary = %binary_path.display(),
            "executing plugin"
        );

        let timeout_secs = if pi.timeout > 0 {
            pi.timeout as u64
        } else {
            120
        };

        // Split command string into args (subcommand + simple flags).
        let mut args = if !pi.command.is_empty() {
            match shlex::split(&pi.command) {
                Some(a) => a,
                None => {
                    return ToolResult::error("Failed to parse command arguments. Check quoting.");
                }
            }
        } else {
            Vec::new()
        };

        // Append named args directly — no shell parsing, special characters preserved.
        for (key, value) in &pi.args {
            args.push(format!("--{}", key));
            args.push(value.clone());
        }

        let mut cmd = tokio::process::Command::new(&binary_path);
        cmd.args(&args);

        process::hide_window(&mut cmd);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Clean env + inject sanitized env
        cmd.env_clear();
        for (k, v) in process::sanitized_env() {
            cmd.env(k, v);
        }

        // Plugin binary env var (e.g., GWS_BIN=/path/to/gws)
        cmd.env(
            napp::plugin::plugin_env_var(&pi.resource),
            binary_path.to_string_lossy().as_ref(),
        );

        // Augmented PATH with all plugin directories
        cmd.env("PATH", self.plugin_store.path_with_plugins());

        // Auth env vars (client_id, client_secret, etc.)
        if let Some((_bin, auth)) = self.plugin_store.get_auth_info(&pi.resource) {
            for (k, v) in &auth.env {
                cmd.env(k, v);
            }
        }

        let result = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            cmd.output(),
        )
        .await;

        match result {
            Err(_) => ToolResult::error(format!(
                "Plugin '{}' command timed out after {}s",
                pi.resource, timeout_secs
            )),
            Ok(Err(e)) => ToolResult::error(format!(
                "Plugin '{}' command failed: {}",
                pi.resource, e
            )),
            Ok(Ok(output)) => {
                let mut text = String::new();

                let stdout = String::from_utf8_lossy(&output.stdout);
                if !stdout.is_empty() {
                    text.push_str(&stdout);
                }

                let stderr = String::from_utf8_lossy(&output.stderr);
                if !stderr.is_empty() {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str("STDERR:\n");
                    text.push_str(&stderr);
                }

                if !output.status.success() {
                    let code = output.status.code().unwrap_or(-1);
                    return ToolResult::error(format!(
                        "Plugin '{}' exited with code {}\n{}",
                        pi.resource, code, text
                    ));
                }

                if text.is_empty() {
                    text = "(no output)".to_string();
                }

                // Truncate very long output
                const MAX_OUTPUT: usize = 50000;
                if text.len() > MAX_OUTPUT {
                    text.truncate(MAX_OUTPUT);
                    text.push_str("\n... (output truncated)");
                }

                ToolResult::ok(text)
            }
        }
    }

    /// Run the plugin's `auth status` command. Returns `true` if authenticated.
    async fn run_auth_status(
        &self,
        slug: &str,
        binary: &Path,
        auth: &napp::plugin::PluginAuth,
    ) -> bool {
        let status_cmd = match auth.commands.status.as_deref() {
            Some(c) => c,
            None => return false,
        };

        let args: Vec<&str> = status_cmd.split_whitespace().collect();
        let mut cmd = tokio::process::Command::new(binary);
        cmd.args(&args);

        process::hide_window(&mut cmd);
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());

        cmd.env_clear();
        for (k, v) in process::sanitized_env() {
            cmd.env(k, v);
        }
        cmd.env("PATH", self.plugin_store.path_with_plugins());
        for (k, v) in &auth.env {
            cmd.env(k, v);
        }

        match tokio::time::timeout(Duration::from_secs(10), cmd.output()).await {
            Ok(Ok(output)) => {
                let authenticated = output.status.success();
                debug!(plugin = %slug, authenticated, "plugin auth status check");
                authenticated
            }
            _ => {
                warn!(plugin = %slug, "plugin auth status check failed or timed out");
                false
            }
        }
    }

    /// Run the plugin's `auth login` command to trigger OAuth re-authentication.
    /// Opens the browser for the user to complete the OAuth flow.
    /// Returns `true` if login succeeded (exit code 0).
    async fn run_auth_login(
        &self,
        slug: &str,
        binary: &Path,
        auth: &napp::plugin::PluginAuth,
    ) -> bool {
        let args: Vec<&str> = auth.commands.login.split_whitespace().collect();
        let mut cmd = tokio::process::Command::new(binary);
        cmd.args(&args);

        process::hide_window(&mut cmd);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        cmd.env_clear();
        for (k, v) in process::sanitized_env() {
            cmd.env(k, v);
        }
        cmd.env("PATH", self.plugin_store.path_with_plugins());
        for (k, v) in &auth.env {
            cmd.env(k, v);
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                warn!(plugin = %slug, error = %e, "failed to spawn auth login");
                return false;
            }
        };

        // Read stderr for OAuth URLs (plugins write the URL to stderr).
        let stderr_handle = child.stderr.take();
        let slug_owned = slug.to_string();
        let broadcaster = self.broadcaster.clone();

        let stderr_task = tokio::spawn(async move {
            let mut all = String::new();
            let mut opened = false;
            if let Some(mut stream) = stderr_handle {
                let mut buf = [0u8; 4096];
                loop {
                    let has_candidate = !opened && has_url_candidate(&all);
                    let read_result = if has_candidate {
                        match tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buf))
                            .await
                        {
                            Ok(r) => r,
                            Err(_) => {
                                // Timeout — treat URL as complete
                                if let Some(url) = extract_url(&all, true) {
                                    open_auth_url(&slug_owned, &url, &broadcaster);
                                    opened = true;
                                }
                                continue;
                            }
                        }
                    } else {
                        stream.read(&mut buf).await
                    };
                    match read_result {
                        Ok(0) => break,
                        Ok(n) => {
                            let chunk = String::from_utf8_lossy(&buf[..n]);
                            debug!(plugin = %slug_owned, chunk = %chunk, "auth login stderr");
                            all.push_str(&chunk);
                            if !opened {
                                if let Some(url) = extract_url(&all, false) {
                                    open_auth_url(&slug_owned, &url, &broadcaster);
                                    opened = true;
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
            all
        });

        // Also read stdout (some plugins may write URL there)
        let stdout_handle = child.stdout.take();
        let slug_for_stdout = slug.to_string();
        let broadcaster_for_stdout = self.broadcaster.clone();

        let stdout_task = tokio::spawn(async move {
            let mut all = String::new();
            let mut opened = false;
            if let Some(mut stream) = stdout_handle {
                let mut buf = [0u8; 4096];
                loop {
                    let has_candidate = !opened && has_url_candidate(&all);
                    let read_result = if has_candidate {
                        match tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buf))
                            .await
                        {
                            Ok(r) => r,
                            Err(_) => {
                                if let Some(url) = extract_url(&all, true) {
                                    open_auth_url(&slug_for_stdout, &url, &broadcaster_for_stdout);
                                    opened = true;
                                }
                                continue;
                            }
                        }
                    } else {
                        stream.read(&mut buf).await
                    };
                    match read_result {
                        Ok(0) => break,
                        Ok(n) => {
                            let chunk = String::from_utf8_lossy(&buf[..n]);
                            debug!(plugin = %slug_for_stdout, chunk = %chunk, "auth login stdout");
                            all.push_str(&chunk);
                            if !opened {
                                if let Some(url) = extract_url(&all, false) {
                                    open_auth_url(
                                        &slug_for_stdout,
                                        &url,
                                        &broadcaster_for_stdout,
                                    );
                                    opened = true;
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
            all
        });

        // Wait for the auth login process with a 120s timeout.
        let login_result = tokio::time::timeout(Duration::from_secs(120), async {
            let (stderr_out, stdout_out) = tokio::join!(stderr_task, stdout_task);
            let _stderr = stderr_out.unwrap_or_default();
            let _stdout = stdout_out.unwrap_or_default();
            child.wait().await
        })
        .await;

        match login_result {
            Ok(Ok(status)) if status.success() => {
                info!(plugin = %slug, "plugin re-authentication succeeded");
                true
            }
            Ok(Ok(status)) => {
                warn!(plugin = %slug, code = ?status.code(), "plugin re-authentication failed");
                false
            }
            Ok(Err(e)) => {
                warn!(plugin = %slug, error = %e, "plugin auth login process error");
                false
            }
            Err(_) => {
                warn!(plugin = %slug, "plugin auth login timed out after 120s");
                // Kill the child process on timeout
                let _ = child.kill().await;
                false
            }
        }
    }
}

// ── Auth error detection ────────────────────────────────────────────

/// Check if a plugin command failure is due to stale/expired authentication.
/// Matches common OAuth/auth error patterns in the combined output text.
fn is_auth_error(output: &str) -> bool {
    let lower = output.to_lowercase();
    const PATTERNS: &[&str] = &[
        "unauthorized",
        "token expired",
        "login required",
        "invalid_grant",
        "not authenticated",
        "credentials expired",
        "re-authenticate",
        "please login",
        "sign in again",
        "token has been revoked",
        "refresh token",
        "oauth2: cannot fetch token",
        "401",
    ];
    PATTERNS.iter().any(|p| lower.contains(p))
}

// ── URL extraction (duplicated from handlers/plugins.rs) ────────────

/// Returns true if the text ends with an incomplete URL-like token.
fn has_url_candidate(text: &str) -> bool {
    let words: Vec<&str> = text.split_whitespace().collect();
    if let Some(last) = words.last() {
        let trimmed = last.trim_matches(|c: char| c == '"' || c == '\'' || c == '<' || c == '>');
        (trimmed.starts_with("https://") || trimmed.starts_with("http://"))
            && !text.ends_with(char::is_whitespace)
    } else {
        false
    }
}

/// Extract the first HTTP(S) URL from accumulated output text.
///
/// When `complete` is false (streaming), only returns a URL that is followed by
/// more text — avoids matching a partial URL still being written.
/// When `complete` is true (after timeout), the last token is accepted.
fn extract_url(text: &str, complete: bool) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    for (i, word) in words.iter().enumerate() {
        let trimmed = word.trim_matches(|c: char| c == '"' || c == '\'' || c == '<' || c == '>');
        if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
            let is_last = i == words.len() - 1;
            if complete || !is_last || text.ends_with(char::is_whitespace) {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Open an OAuth URL: broadcast via WebSocket so the frontend can call `window.open()`.
fn open_auth_url(slug: &str, url: &str, broadcaster: &Option<crate::web_tool::Broadcaster>) {
    info!(plugin = %slug, url = %url, "opening plugin OAuth URL for re-authentication");
    if let Some(bc) = broadcaster {
        bc(
            "plugin_auth_url",
            serde_json::json!({
                "plugin": slug,
                "url": url,
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_auth_error_detects_common_patterns() {
        assert!(is_auth_error("Error: unauthorized"));
        assert!(is_auth_error("token expired, please re-authenticate"));
        assert!(is_auth_error("HTTP 401 Unauthorized"));
        assert!(is_auth_error("Error: login required"));
        assert!(is_auth_error("invalid_grant: Token has been revoked"));
        assert!(is_auth_error("Not authenticated. Run: gws auth login"));
        assert!(is_auth_error("credentials expired"));
        assert!(is_auth_error("Please sign in again"));
        assert!(is_auth_error("oauth2: cannot fetch token: 400 Bad Request"));
    }

    #[test]
    fn test_is_auth_error_ignores_non_auth() {
        assert!(!is_auth_error("file not found"));
        assert!(!is_auth_error("invalid argument: --foo"));
        assert!(!is_auth_error("network timeout"));
        assert!(!is_auth_error("rate limited, try again later"));
        assert!(!is_auth_error("permission denied: /etc/shadow"));
    }

    #[test]
    fn test_extract_url_streaming() {
        // URL followed by more text → extracted
        assert_eq!(
            extract_url("Visit https://accounts.google.com/o/oauth2 to continue", false),
            Some("https://accounts.google.com/o/oauth2".to_string())
        );
        // URL as last token without trailing whitespace → NOT extracted (still streaming)
        assert_eq!(
            extract_url("Visit https://accounts.google.com/o/oauth2", false),
            None
        );
        // URL as last token with trailing whitespace → extracted
        assert_eq!(
            extract_url("Visit https://accounts.google.com/o/oauth2 ", false),
            Some("https://accounts.google.com/o/oauth2".to_string())
        );
    }

    #[test]
    fn test_extract_url_complete() {
        // In complete mode, last token is accepted
        assert_eq!(
            extract_url("Visit https://accounts.google.com/o/oauth2", true),
            Some("https://accounts.google.com/o/oauth2".to_string())
        );
    }

    #[test]
    fn test_extract_url_strips_quotes() {
        assert_eq!(
            extract_url("URL: \"https://example.com/auth\" done", false),
            Some("https://example.com/auth".to_string())
        );
    }

    #[test]
    fn test_has_url_candidate() {
        assert!(has_url_candidate("Visit https://example.com"));
        assert!(!has_url_candidate("Visit https://example.com "));
        assert!(!has_url_candidate("no url here"));
    }
}
