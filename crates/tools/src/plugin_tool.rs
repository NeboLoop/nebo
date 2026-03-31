use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use serde::Deserialize;
use tracing::debug;

use crate::origin::ToolContext;
use crate::process;
use crate::registry::{DynTool, ToolResult};

/// STRAP domain tool for installed plugin binaries.
///
/// Plugins ship with their own skills (`skills/` directory inside the plugin).
/// These skills are the plugin's documentation — they describe the CLI syntax,
/// flags, and examples. The plugin tool routes to them via `action: "help"`.
pub struct PluginTool {
    plugin_store: Arc<napp::plugin::PluginStore>,
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
        Self { plugin_store }
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
             - help: Read documentation for a specific service (use topic param)\n\n\
             For exec: use `command` for subcommand + simple flags, and `args` for values \
             that may contain special characters (quotes, backticks, $, etc.).\n\n\
             Available plugins:\n",
        );
        for slug in &slugs {
            if let Some(manifest) = self.plugin_store.get_manifest(slug) {
                desc.push_str(&format!("- {} — {}\n", slug, manifest.description));
            } else {
                desc.push_str(&format!("- {}\n", slug));
            }
        }
        desc.push_str("\nWorkflow: services → help → exec. Always check docs before executing unfamiliar commands.");
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
                "description": "Action: exec (run command), services (list available), help (read docs for a topic)",
                "enum": ["exec", "services", "help"],
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
        // help and services are read-only, exec needs approval
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
            std::time::Duration::from_secs(timeout_secs),
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
}
