use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tracing::{debug, info, warn};

use crate::channel_bridge;
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
    db_store: Arc<db::Store>,
    broadcaster: Option<crate::web_tool::Broadcaster>,
}

#[derive(Debug, Deserialize)]
struct PluginInput {
    /// Plugin slug (e.g., "gws", "slack").
    #[serde(default)]
    resource: String,
    /// Action: "exec" (default — run a plugin command) or "events"
    /// (list the plugin's declared NDJSON watch events).
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
    /// Optional timeout in seconds (default: 120).
    #[serde(default)]
    timeout: i64,
    /// Search query for action: "discover" (marketplace plugin search).
    #[serde(default)]
    query: String,
    /// Typed capability operation to invoke (e.g. "ledger.bill.create", or the
    /// fully-qualified "accounting.ap-specialist.ledger.bill.create"). When set,
    /// the port is resolved on its operation suffix to whichever installed plugin
    /// declares that binding, and `input` is passed as flags — no `resource`/
    /// `command` needed. This is the provider-agnostic port pathway.
    #[serde(default)]
    operation: String,
    /// Typed input object for a port `operation`; each field becomes a `--key value` flag.
    #[serde(default)]
    input: serde_json::Value,
}

fn default_action() -> String {
    "exec".to_string()
}

/// The `capability.resource.action` suffix a plugin binding matches on. A fully-
/// qualified port (`department.role.capability.resource.action`) reduces to its
/// last three segments; a bare operation is returned unchanged. This is what keeps
/// one plugin binding (`ledger.bill.create`) satisfying every seat that calls it.
fn port_suffix(operation: &str) -> String {
    let parts: Vec<&str> = operation.split('.').collect();
    if parts.len() > 3 {
        parts[parts.len() - 3..].join(".")
    } else {
        operation.to_string()
    }
}

impl PluginTool {
    pub fn new(
        plugin_store: Arc<napp::plugin::PluginStore>,
        db_store: Arc<db::Store>,
    ) -> Self {
        Self {
            plugin_store,
            db_store,
            broadcaster: None,
        }
    }

    pub fn with_broadcaster(mut self, broadcaster: crate::web_tool::Broadcaster) -> Self {
        self.broadcaster = Some(broadcaster);
        self
    }

    /// Build a deduplicated list of active plugin slugs (installed + not disabled + ready).
    fn active_slugs(&self) -> Vec<String> {
        let installed = self.plugin_store.list_installed();
        let mut seen = std::collections::HashSet::new();
        let mut slugs = Vec::new();
        for (slug, _, _, _) in &installed {
            if !seen.insert(slug.clone()) {
                continue;
            }
            if let Ok(Some(row)) = self.db_store.get_plugin_by_slug(&slug) {
                if row.is_enabled == 0 {
                    continue;
                }
            }
            if !self.plugin_store.is_ready(&slug) {
                continue;
            }
            slugs.push(slug.clone());
        }
        slugs
    }

    /// Resolve a typed capability operation to (plugin slug, command) by scanning
    /// active plugins' declared `interface_bindings`. Matches on the
    /// `capability.resource.action` suffix, so a fully-qualified port
    /// (`department.role.capability.resource.action`) binds the same as a bare op.
    fn resolve_port(&self, operation: &str) -> Option<(String, String)> {
        let target = port_suffix(operation);
        for slug in self.active_slugs() {
            if let Some(m) = self.plugin_store.get_manifest(&slug) {
                if let Some(cmd) = m.interface_bindings.get(&target) {
                    return Some((slug, cmd.clone()));
                }
            }
        }
        None
    }

    /// (operation, provider-slug) for every port the installed plugins implement.
    fn bound_operations(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for slug in self.active_slugs() {
            if let Some(m) = self.plugin_store.get_manifest(&slug) {
                for op in m.interface_bindings.keys() {
                    out.push((op.clone(), slug.clone()));
                }
            }
        }
        out.sort();
        out
    }

    /// List installed plugins (slug, version, enabled/disabled, signature status).
    /// The direct answer to "what plugins are installed?" — parity with skill catalog.
    fn handle_list(&self) -> ToolResult {
        let installed = self.plugin_store.list_installed();
        if installed.is_empty() {
            return ToolResult::ok(
                "No plugins installed. Use plugin(action: \"discover\", query: \"<keyword>\") to \
                 find plugins in the marketplace; install one with its PLUG-XXXX-XXXX code (this \
                 requires your approval).",
            );
        }
        let mut seen = std::collections::HashSet::new();
        let mut lines = Vec::new();
        for (slug, version, _path, sig) in &installed {
            if !seen.insert(slug.clone()) {
                continue;
            }
            let enabled = self
                .db_store
                .get_plugin_by_slug(slug)
                .ok()
                .flatten()
                .map(|r| r.is_enabled != 0)
                .unwrap_or(true);
            lines.push(format!(
                "- {} v{} ({}, signature: {})",
                slug,
                version,
                if enabled { "enabled" } else { "disabled" },
                sig
            ));
        }
        ToolResult::ok(format!(
            "{} installed plugin(s):\n{}",
            lines.len(),
            lines.join("\n")
        ))
    }

    /// Search the NeboAI marketplace for plugins. Returns names + install codes so the
    /// agent can offer to install one (install is HIL — the user pastes/approves the code,
    /// which installs via the canonical code path).
    async fn handle_discover(&self, query: &str) -> ToolResult {
        let api = match crate::build_neboai_api(&self.db_store) {
            Ok(a) => a,
            Err(e) => return ToolResult::error(format!("marketplace unavailable: {}", e)),
        };
        let q = if query.trim().is_empty() {
            None
        } else {
            Some(query.trim())
        };
        match api.list_products(Some("plugin"), q, None, None, Some(20)).await {
            Ok(v) => {
                let items = v
                    .get("results")
                    .and_then(|x| x.as_array())
                    .or_else(|| v.get("plugins").and_then(|x| x.as_array()));
                match items {
                    Some(arr) if !arr.is_empty() => {
                        let mut lines = Vec::new();
                        for it in arr {
                            let name = it.get("name").and_then(|x| x.as_str()).unwrap_or("?");
                            let slug = it.get("slug").and_then(|x| x.as_str()).unwrap_or("");
                            let code = it.get("code").and_then(|x| x.as_str()).unwrap_or("");
                            let desc =
                                it.get("description").and_then(|x| x.as_str()).unwrap_or("");
                            lines.push(format!("- {} ({}) — {} [{}]", name, slug, desc, code));
                        }
                        ToolResult::ok(format!(
                            "Found {} plugin(s):\n{}\n\nTo install one, share its PLUG-XXXX-XXXX \
                             code with the user to approve (installs via the marketplace code path).",
                            lines.len(),
                            lines.join("\n")
                        ))
                    }
                    _ => ToolResult::ok("No plugins found in the marketplace for that query."),
                }
            }
            Err(e) => ToolResult::error(format!("marketplace search failed: {}", e)),
        }
    }

    /// Find the skills directory for a plugin slug.
    ///
    /// Walks up from the binary path looking for a `skills/` directory.
    /// Handles both layouts:
    ///   - Installed plugins: `<data>/plugins/<slug>/<version>/{binary,skills/}`
    ///     (skills/ is sibling of binary, 1 level up)
    ///   - Symlinked dev plugins: `~/.nebo/user/plugins/<slug>/{target/release/binary,skills/}`
    ///     (skills/ is 3 levels up — past `target/release/`)
    fn skills_dir(&self, slug: &str) -> Option<PathBuf> {
        let binary_path = self.plugin_store.resolve(slug, "*")?;
        let mut cur = binary_path.parent()?;
        for _ in 0..5 {
            let candidate = cur.join("skills");
            if candidate.is_dir() {
                return Some(candidate);
            }
            cur = cur.parent()?;
        }
        None
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

}

/// Render a skill name as a likely command label for the description.
///
/// Plugins follow the convention `<slug>-<service>-<verb>` for skill dirs
/// (e.g. `gws-gmail-triage` → command `gmail +triage`). Strip the slug
/// prefix and convert dashes; surface the rest as-is.
///
/// If the skill name doesn't follow the convention, show the raw name —
/// better to over-disclose than mislead.
fn display_command_for_skill(slug: &str, skill_name: &str) -> String {
    let prefix = format!("{}-", slug);
    let trimmed = skill_name.strip_prefix(&prefix).unwrap_or(skill_name);
    // First segment becomes the service, rest becomes the command (with `+` per GWS convention).
    // For non-GWS-style plugins this collapses to a single token, which is fine.
    if let Some((service, verb)) = trimmed.split_once('-') {
        format!("{} +{}", service, verb)
    } else {
        trimmed.to_string()
    }
}

impl DynTool for PluginTool {
    fn name(&self) -> &str {
        "plugin"
    }

    fn description(&self) -> String {
        let slugs = self.active_slugs();
        if slugs.is_empty() {
            return "Run installed plugin binaries. No plugins are installed yet — use \
                    plugin(action: \"list\") to confirm, and plugin(action: \"discover\", \
                    query: \"<keyword>\") to find plugins in the marketplace (install requires \
                    the user's approval via the plugin's PLUG-XXXX-XXXX code)."
                .to_string();
        }

        let mut out = String::from(
            "Run installed plugin binaries. plugin(action: \"list\") shows what's installed; \
             plugin(action: \"discover\", query: \"…\") searches the marketplace.\n\n",
        );
        out.push_str("ALWAYS use this tool for channel messaging — Slack, Discord, Teams, and any other channel-backed plugin. \
                      `plugin(resource: \"<channel-slug>\", command: \"upload|post|dm|reply ...\")` is the canonical pathway for \
                      sending files, messages, and DMs out through a channel. \
                      NEVER use `skill discover` or `skill help` to look up channel operations — channels are plugins, \
                      not skills, and the skill catalog does not contain them.\n\n");
        out.push_str("Usage: plugin(resource: \"<plugin-slug>\", action: \"exec\", command: \"<subcommand and flags>\")\n");
        out.push_str("       plugin(resource: \"<plugin-slug>\", action: \"events\") — list declared NDJSON watch events\n");
        out.push_str("       plugin(resource: \"<plugin-slug>\", action: \"help\" [, command: \"<service>\"]) — read the plugin's command grammar / a service's usage\n");
        out.push_str("`command` is passed straight to the plugin binary — the FIRST token is a service (e.g. calendar, gmail, drive), NOT the plugin name. \
                      Grammar: `<service> <resource> <method> [flags]` (e.g. `calendar events list`).\n");
        out.push_str("For Google Calendar/Gmail/Drive use plugin(resource: \"gws\", ...); for the local Mac calendar use os(resource: \"calendar\").\n\n");
        out.push_str("Installed plugins:\n\n");

        const PER_PLUGIN_BUDGET: usize = 4096;
        const TOTAL_BUDGET: usize = 12_288;

        let mut with_services: Vec<(String, Vec<(String, String)>)> = slugs
            .iter()
            .map(|s| (s.clone(), self.list_services(s)))
            .collect();
        with_services.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

        let mut overflow_slugs: Vec<String> = Vec::new();
        for (slug, services) in &with_services {
            let is_channel = self.plugin_store.get_channel_def(slug).is_some();
            if services.is_empty() && !is_channel {
                overflow_slugs.push(slug.clone());
                continue;
            }
            let mut section = format!("### {}\n", slug);
            // Channel plugins expose real-time messaging ops via the running
            // bridge. Lead with the USE CASE (what the user asked for), not
            // the syntax — agents that picked the wrong tool ("send me this
            // file in slack" → markdown image link instead of upload) did so
            // because the description listed commands without naming the
            // intent each one serves. Replies to inbound messages are NOT
            // listed: the bridge sends `op: reply` automatically when the
            // agent's response comes back through channel dispatch; the
            // agent never invokes a reply command directly.
            if is_channel {
                section.push_str("  Channel actions (use these instead of generating markdown links / image syntax):\n");
                section.push_str(&format!("  - Share a file with someone in this channel: plugin(resource: \"{slug}\", command: \"upload --channel <id> --path <abs-path> [--caption <text>] [--thread_ts <ts>]\")\n"));
                section.push_str(&format!("    Use this when the user says \"send/share/attach/grab/let me see/upload a file\" — pass the absolute local path; the bridge handles the upload to the platform.\n"));
                section.push_str(&format!("  - Post an unsolicited message: plugin(resource: \"{slug}\", command: \"post --channel <id> --text <body> [--thread_ts <ts>]\")\n"));
                section.push_str(&format!("    Use for proactive posts (briefings, alerts, workflow output) when not directly replying to an inbound message.\n"));
                section.push_str(&format!("  - Direct message a specific user: plugin(resource: \"{slug}\", command: \"dm --user <id> --text <body>\")\n"));
                section.push_str("  Note: replies to inbound channel messages are automatic — your normal text response goes through the bridge with no command needed. Do NOT include markdown image links (`![alt](url)`) for files — call `upload` instead.\n");
                if !services.is_empty() {
                    section.push_str("  Stateless commands (auth/init/doctor/sync etc.):\n");
                }
            }
            let total = services.len();
            let mut included = 0usize;
            let mut truncated = false;
            for (name, desc) in services {
                let label = display_command_for_skill(slug, name);
                let line = if desc.is_empty() {
                    format!("  - {}\n", label)
                } else {
                    format!("  - {} — {}\n", label, desc)
                };
                if section.len() + line.len() > PER_PLUGIN_BUDGET {
                    truncated = true;
                    break;
                }
                section.push_str(&line);
                included += 1;
            }
            if truncated {
                section.push_str(&format!(
                    "  - … and {} more — use skill(action: \"discover\", query: \"{}\") for full list\n",
                    total - included,
                    slug
                ));
            }
            section.push('\n');
            if out.len() + section.len() > TOTAL_BUDGET {
                overflow_slugs.push(slug.clone());
                continue;
            }
            out.push_str(&section);
        }

        if !overflow_slugs.is_empty() {
            out.push_str("Also installed: ");
            out.push_str(&overflow_slugs.join(", "));
            out.push_str("\nUse skill(action: \"discover\", query: \"<plugin-name>\") to see available commands.\n");
        }

        out.push_str("\nFor commands listed above, use the exact syntax shown. For other plugins, discover commands first via the skill tool.");

        // Typed capability ports currently bound (provider-agnostic).
        let ops = self.bound_operations();
        if !ops.is_empty() {
            out.push_str("\n\nTyped ports (provider-agnostic): call plugin(operation: \"<op>\", input: {...}). \
                          The operation resolves to the bound provider below:\n");
            for (op, slug) in &ops {
                out.push_str(&format!("  - {op}  (via {slug})\n"));
            }
        }
        out
    }

    fn schema(&self) -> serde_json::Value {
        let slugs = self.active_slugs();
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
                "description": "Action: 'list' (installed plugins), 'discover' (search the marketplace by query), 'exec' (default — run a plugin command), 'help' (read a plugin's command grammar / a service's usage), or 'events' (the plugin's declared NDJSON watch events)",
                "enum": ["list", "discover", "exec", "help", "events"],
                "default": "exec"
            }),
        );
        props.insert(
            "query".into(),
            serde_json::json!({
                "type": "string",
                "description": "Search query for action: 'discover'."
            }),
        );
        props.insert(
            "command".into(),
            serde_json::json!({
                "type": "string",
                "description": "Subcommand and flags ONLY — the binary path is auto-resolved. Do NOT include the plugin name (e.g. for a plugin 'acme' with subcommand 'reports generate', pass 'reports generate --period month', NOT 'acme reports generate'). Use only commands listed in this tool's description or confirmed via a skill/help; do not guess syntax."
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
            "timeout".into(),
            serde_json::json!({
                "type": "integer",
                "description": "Command timeout in seconds (default: 120)"
            }),
        );
        props.insert(
            "operation".into(),
            serde_json::json!({
                "type": "string",
                "description": "Typed capability operation to invoke (provider-agnostic), e.g. 'ledger.bill.create' or the fully-qualified 'accounting.ap-specialist.ledger.bill.create'. Resolves on the operation suffix to whichever installed plugin declares that binding. Use this instead of resource/command to call a port; pass fields via `input`. See this tool's description for the operations currently bound."
            }),
        );
        props.insert(
            "input".into(),
            serde_json::json!({
                "type": "object",
                "description": "Typed input for a port `operation`. Each field is passed to the bound plugin as a --key value flag."
            }),
        );

        serde_json::json!({
            "type": "object",
            "properties": serde_json::Value::Object(props),
            "required": []
        })
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn requires_approval_for(&self, input: &serde_json::Value) -> bool {
        // help, services, and events are read-only; exec needs approval
        let action = input
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("exec");
        action == "exec"
    }

    fn is_concurrent_safe(&self, input: &serde_json::Value) -> bool {
        let action = input
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("exec");
        matches!(action, "list" | "discover" | "events" | "help")
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let pi: PluginInput = match serde_json::from_value(input) {
                Ok(v) => v,
                Err(e) => return ToolResult::error(format!("invalid input: {}", e)),
            };

            // Typed port pathway: an `operation` resolves to whichever installed plugin
            // declares that binding (provider-agnostic), and `input` becomes flags. This
            // is how a seat's capability port (`department.role.ledger.bill.create`) runs
            // without naming a vendor tool.
            if !pi.operation.is_empty() {
                let (slug, command) = match self.resolve_port(&pi.operation) {
                    Some(x) => x,
                    None => {
                        return ToolResult::error(format!(
                            "no installed plugin implements operation '{}'. Enable a provider whose \
                             interface_bindings declare this operation.",
                            pi.operation
                        ))
                    }
                };
                let mut args = pi.args.clone();
                if let serde_json::Value::Object(map) = &pi.input {
                    for (k, v) in map {
                        let sval = match v {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        args.entry(k.clone()).or_insert(sval);
                    }
                }
                let port_pi = PluginInput {
                    resource: slug,
                    action: "exec".to_string(),
                    command,
                    args,
                    timeout: pi.timeout,
                    query: String::new(),
                    operation: String::new(),
                    input: serde_json::Value::Null,
                };
                return self.handle_exec(&port_pi, ctx).await;
            }

            // `list` and `discover` don't need a plugin slug; `exec`/`events` do.
            match pi.action.as_str() {
                "list" => self.handle_list(),
                "discover" => self.handle_discover(&pi.query).await,
                "exec" | "" => {
                    if pi.resource.is_empty() {
                        return ToolResult::error(
                            "resource is required — set it to the plugin slug. \
                             Example: plugin(resource: \"gws\", action: \"exec\", command: \"gmail +triage\")"
                                .to_string(),
                        );
                    }
                    self.handle_exec(&pi, ctx).await
                }
                "events" => {
                    if pi.resource.is_empty() {
                        return ToolResult::error(
                            "resource is required (the plugin slug) for action: \"events\".".to_string(),
                        );
                    }
                    self.handle_events(&pi.resource)
                }
                "help" => {
                    if pi.resource.is_empty() {
                        return ToolResult::error(
                            "resource is required (the plugin slug) for action: \"help\". \
                             Example: plugin(resource: \"gws\", action: \"help\")"
                                .to_string(),
                        );
                    }
                    self.handle_help(&pi.resource, &pi.command)
                }
                "search" | "skills" | "services" => ToolResult::error(format!(
                    "action '{}' was removed in v0.10.0. Use action: \"list\" to see installed plugins, \"discover\" to search the marketplace, or call commands directly with action: \"exec\".",
                    pi.action
                )),
                other => ToolResult::error(format!(
                    "Unknown action: '{}'. Valid actions: list, discover, exec, events.",
                    other
                )),
            }
        })
    }
}

impl PluginTool {
    /// Read-only usage lookup for a plugin's command grammar.
    ///
    /// `plugin(action: "help", resource: "gws")` returns the service list plus
    /// the shared grammar (gws-shared/SKILL.md). An optional `command` narrows
    /// to a single service: `plugin(action: "help", resource: "gws", command:
    /// "calendar")` returns gws-calendar/SKILL.md. Lenient: if no skill matches,
    /// fall back to the service list + shared grammar.
    fn handle_help(&self, slug: &str, command: &str) -> ToolResult {
        let skills_dir = match self.skills_dir(slug) {
            Some(d) => d,
            None => {
                return ToolResult::error(format!(
                    "Plugin '{}' has no bundled skills/ documentation. Try plugin(action: \"list\") \
                     or call commands directly with action: \"exec\".",
                    slug
                ));
            }
        };

        // A specific service was requested — return that service's SKILL.md.
        let service = command.split_whitespace().next().unwrap_or("").trim();
        if !service.is_empty() {
            let candidate = skills_dir.join(format!("{}-{}", slug, service)).join("SKILL.md");
            if let Ok(body) = std::fs::read_to_string(&candidate) {
                return ToolResult::ok(format!("# {} {} usage\n\n{}", slug, service, body));
            }
            // No exact match — fall through to the overview below.
        }

        let mut out = format!("# {} usage\n\n", slug);

        // Lead with the shared grammar reference if the plugin ships one.
        let shared = skills_dir.join(format!("{}-shared", slug)).join("SKILL.md");
        if let Ok(body) = std::fs::read_to_string(&shared) {
            out.push_str(&body);
            out.push_str("\n\n");
        } else {
            out.push_str(
                "Grammar: `<service> <resource> <method> [flags]` (the first token is a service, \
                 NOT the plugin name).\n\n",
            );
        }

        let services = self.list_services(slug);
        if !services.is_empty() {
            out.push_str("## Available services / commands\n\n");
            for (name, desc) in &services {
                let label = display_command_for_skill(slug, name);
                if desc.is_empty() {
                    out.push_str(&format!("- {}\n", label));
                } else {
                    out.push_str(&format!("- {} — {}\n", label, desc));
                }
            }
            out.push_str(&format!(
                "\nFor a specific service's full usage: plugin(resource: \"{}\", action: \"help\", command: \"<service>\").",
                slug
            ));
        }

        ToolResult::ok(out)
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
                        if ev.description.is_empty() {
                            "(no description)"
                        } else {
                            &ev.description
                        },
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

    async fn handle_exec(&self, pi: &PluginInput, ctx: &ToolContext) -> ToolResult {
        // Channel-plugin messaging ops route through the running bridge sidecar's
        // stdin — never through a fresh CLI invocation. Two processes hitting the
        // same upstream socket race each other (we observed this with orphan
        // Slack bridges all posting "_Thinking..._" for one inbound message).
        // See `docs/publishers-guide/channel-plugins.md` for the contract.
        let verb = pi
            .command
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string();
        if matches!(verb.as_str(), "reply" | "post" | "upload" | "dm") {
            return self.route_through_bridge(&verb, pi, ctx).await;
        }

        let result = self.run_plugin_command(pi, ctx).await;

        // On error, check if it's an auth failure and attempt re-auth.
        if result.is_error {
            if let Some((binary, auth)) = self.plugin_store.get_auth_info(&pi.resource) {
                if is_auth_error(&result.content) {
                    // Confirm with a fresh auth-status check (the one canonical
                    // decision, via PluginStore) if the command is available.
                    if auth.commands.status.is_some()
                        && self.plugin_store.check_auth_now(&pi.resource).await
                    {
                        // Status says authenticated — false positive, return original error
                        return result;
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

                        return self.run_plugin_command(pi, ctx).await;
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

                    // Terminal: auth genuinely expired and reauth failed. End the
                    // turn and surface to the user — do not let the agent keep
                    // retrying/improvising (FRAMES.md Phase 1).
                    return ToolResult::terminal(format!(
                        "I couldn't reach **{}** — it isn't authenticated and automatic \
                         re-authentication didn't work. Please reconnect this account in the \
                         agent's Connected Accounts (Settings), then ask me again.",
                        pi.resource
                    ));
                }
            }
        }

        result
    }

    /// Execute a plugin command and return the result. Shared by initial call and retry.
    async fn run_plugin_command(&self, pi: &PluginInput, ctx: &ToolContext) -> ToolResult {
        if pi.command.is_empty() && pi.args.is_empty() {
            return ToolResult::error(
                "command is required for exec. Run plugin(action: \"list\") to see installed plugins; each plugin's commands are shown in this tool's description (or load the plugin's skill for full syntax).",
            );
        }

        // Resolve binary path
        let binary_path = match self.plugin_store.resolve(&pi.resource, "*") {
            Some(p) => p,
            None => {
                let slugs = self.active_slugs();
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

        // Forgive a leading plugin-name token. Models often prefix the plugin
        // slug (e.g. `gws calendar events list`); the binary expects a service
        // first (`calendar events list`), so a leading `gws` makes it see
        // service "gws" → "Unknown service 'gws'". Drop it so both forms work.
        if args.first().map(|a| a.eq_ignore_ascii_case(&pi.resource)) == Some(true) {
            args.remove(0);
        }

        // Agents must NEVER self-initiate an auth flow. `auth login`/`logout`/`setup`
        // are privileged, interactive, account-mutating actions that belong to the
        // user — when an agent ran `gws auth login` on a (syntax) error it spiraled
        // into endless browser/curl/re-auth attempts (see FRAMES.md). Refuse, and
        // make it terminal so the turn ends instead of the agent improvising. Read-only
        // `auth status`/`export` stay allowed (the host uses them to verify auth).
        if args.first().map(|a| a.eq_ignore_ascii_case("auth")) == Some(true) {
            if let Some(sub) = args.get(1).map(|s| s.to_ascii_lowercase()) {
                if sub == "login" || sub == "logout" || sub == "setup" {
                    return ToolResult::terminal(format!(
                        "I can't sign in to or re-authenticate **{}** on my own — that's \
                         handled for you. If this account needs reconnecting, you can do it \
                         in this agent's Connected Accounts (Settings).",
                        pi.resource
                    ));
                }
            }
        }

        // Append named args directly — no shell parsing, special characters preserved.
        for (key, value) in &pi.args {
            args.push(format!("--{}", key));
            args.push(value.clone());
        }

        // `--account <label>` is a Nebo-level selector for multi-account
        // plugins (the "resource" credential model). It picks which of the
        // agent's accounts to use; it is NOT forwarded to the plugin (the
        // plugin only sees its profile_dir_env). Extract + strip it here.
        let selected_account = extract_and_strip_flag(&mut args, "account");

        // Resolve the per-account credential directory to inject. A plugin that
        // declares a profile_dir_env (the "resource" credential model, e.g. gws)
        // must use THIS agent's own connected account — never a global default.
        // If the agent has no account for the plugin, refuse rather than fall
        // through to the plugin's on-disk default (which would leak whichever
        // account authed first to every account-less agent).
        let profile_dir_injection: Option<(String, String)> = match self
            .plugin_store
            .get_manifest(&pi.resource)
            .and_then(|m| m.auth)
            .and_then(|a| a.profile_dir_env)
        {
            Some(env_name) => {
                let profile = agent_id_from_session_key(&ctx.session_key).and_then(|agent_id| {
                    self.db_store
                        .resolve_plugin_account_profile(
                            &agent_id,
                            &pi.resource,
                            selected_account.as_deref(),
                        )
                        .ok()
                        .flatten()
                });
                match profile {
                    Some(p) => Some((env_name, p.config_dir)),
                    None => {
                        return ToolResult::error(format!(
                            "No {res} account is connected for this agent. Connect one in \
                             this agent's Settings → Connected Accounts before using {res}.",
                            res = pi.resource
                        ));
                    }
                }
            }
            None => None,
        };

        let runtime = napp::PluginRuntime::new(
            &pi.resource,
            binary_path.clone(),
            self.plugin_store.clone(),
        )
        .with_deps()
        .with_permissions();

        let mut cmd = tokio::process::Command::new(&binary_path);
        cmd.args(&args);
        cmd.env_clear();
        for (k, v) in runtime.build_env() {
            cmd.env(k, v);
        }

        // Inject channel context as env vars so channel-plugin CLI subcommands
        // (e.g. `slack upload`) can target the current channel/thread without
        // the agent having to look up IDs. See
        // `docs/publishers-guide/channel-plugins.md` for the convention.
        if let Some(ch) = &ctx.channel {
            cmd.env("NEBO_CHANNEL_KIND", &ch.kind);
            cmd.env("NEBO_CHANNEL_ID", &ch.channel_id);
            if let Some(ts) = &ch.thread_ts {
                cmd.env("NEBO_THREAD_TS", ts);
            }
        }

        // Per-account credential isolation: point the plugin at this agent's
        // chosen account directory (set last so it wins over any global value).
        if let Some((env_name, config_dir)) = &profile_dir_injection {
            cmd.env(env_name, config_dir);
        }

        process::hide_window(&mut cmd);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let effective_timeout = runtime
            .effective_timeout(Duration::from_secs(timeout_secs));
        let result = tokio::time::timeout(effective_timeout, cmd.output()).await;

        match result {
            Err(_) => ToolResult::error(format!(
                "Plugin '{}' command timed out after {}s",
                pi.resource, timeout_secs
            )),
            Ok(Err(e)) => {
                ToolResult::error(format!("Plugin '{}' command failed: {}", pi.resource, e))
            }
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

                // Truncate very long output (char-boundary safe)
                const MAX_OUTPUT: usize = 50000;
                if text.len() > MAX_OUTPUT {
                    types::strutil::safe_truncate(&mut text, MAX_OUTPUT);
                    text.push_str("\n... (output truncated)");
                }

                ToolResult::ok(text)
            }
        }
    }

    /// Route a messaging op (reply/post/upload/dm) through the channel plugin's
    /// running bridge sidecar instead of spawning a fresh process. This is the
    /// canonical pathway — see `docs/publishers-guide/channel-plugins.md`.
    ///
    /// Resolves the bridge handle from the global registry by
    /// `{agent_id}:{plugin_slug}`. If no bridge is registered for the current
    /// agent, returns a structured error pointing the user at the channel
    /// settings — there is NO fallback to one-shot CLI execution.
    async fn route_through_bridge(
        &self,
        op: &str,
        pi: &PluginInput,
        ctx: &ToolContext,
    ) -> ToolResult {
        // Caller agent_id is encoded in session_key as "agent:<id>:..." for
        // channel and chat runs. For non-agent runs (cron without channel
        // context, system tasks) there's no agent to look up a bridge for.
        let agent_id = if ctx.session_key.starts_with("agent:") {
            ctx.session_key
                .split(':')
                .nth(1)
                .unwrap_or("")
                .to_string()
        } else {
            String::new()
        };

        if agent_id.is_empty() {
            return ToolResult::error(format!(
                "Cannot route `{op}` to channel plugin `{}` — this run has no agent context. \
                 Channel ops only work inside agent-bound conversations or scheduled tasks \
                 that preserve their originating channel.",
                pi.resource
            ));
        }

        let registry = match channel_bridge::channel_bridges() {
            Some(r) => r,
            None => {
                return ToolResult::error(
                    "Channel bridge registry not initialized — Nebo is still starting up.".to_string(),
                );
            }
        };

        let key = channel_bridge::channel_bridge_key(&agent_id, &pi.resource);
        let handle = {
            let guard = registry.read().await;
            guard.get(&key).cloned()
        };
        let Some(handle) = handle else {
            return ToolResult::error(format!(
                "Channel plugin `{}` is not running for agent `{}`. \
                 Enable it for this agent in Settings → Channels. \
                 (Real-time messaging ops {{reply, post, upload, dm}} only work \
                 when the bridge sidecar is live — there is no fallback CLI path.)",
                pi.resource, agent_id
            ));
        };

        // Build the op JSON. Args come from pi.args (named flags) plus any
        // `--key value` flags inside pi.command after the verb.
        let mut args = parse_command_flags(&pi.command);
        for (k, v) in &pi.args {
            args.insert(k.clone(), v.clone());
        }

        // Default channel/thread_ts from the run's ChannelContext when the
        // caller didn't supply them explicitly.
        if let Some(ch) = &ctx.channel {
            if !args.contains_key("channel") && !ch.channel_id.is_empty() {
                args.insert("channel".into(), ch.channel_id.clone());
            }
            if !args.contains_key("thread_ts") {
                if let Some(ts) = &ch.thread_ts {
                    args.insert("thread_ts".into(), ts.clone());
                }
            }
        }

        let mut op_json = match build_op_json(op, &args) {
            Ok(v) => v,
            Err(e) => {
                return ToolResult::error(format!(
                    "Channel op `{op}` for plugin `{}`: {e}",
                    pi.resource
                ));
            }
        };

        // Generate a req_id, register a oneshot to await the bridge's
        // `op_result` event, and stamp the id on the outgoing JSON. The
        // bridge echoes req_id back in its op_result so we can correlate.
        // Without this, the tool result would acknowledge the queueing
        // (which always succeeds the moment the mpsc accepts the value)
        // and the agent would tell the user "uploaded" even if the bridge
        // then failed asynchronously — see Rule 10.2 in CODE_AUDITOR.md.
        let req_id = uuid::Uuid::new_v4().to_string();
        op_json
            .as_object_mut()
            .expect("build_op_json always returns an Object")
            .insert("req_id".into(), serde_json::Value::String(req_id.clone()));

        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        handle
            .pending_ops
            .lock()
            .await
            .insert(req_id.clone(), result_tx);

        if let Err(e) = handle.stdin_tx.send(op_json).await {
            handle.pending_ops.lock().await.remove(&req_id);
            return ToolResult::error(format!(
                "Bridge for plugin `{}` (agent `{}`) appears to have closed its \
                 stdin: {e}. Restart the channel in Settings → Channels.",
                pi.resource, agent_id
            ));
        }

        info!(
            plugin = %pi.resource,
            agent = %agent_id,
            op = %op,
            req_id = %req_id,
            "channel op routed through bridge; awaiting result"
        );

        // Bridge ops do real HTTP work; 30s is generous for the slowest
        // case (large file uploads through `files.uploadV2`). Past that
        // it's almost certainly a stuck bridge — drop the pending entry
        // and surface a real timeout error instead of waiting forever.
        match tokio::time::timeout(Duration::from_secs(30), result_rx).await {
            Ok(Ok(res)) if res.ok => ToolResult::ok(format!(
                "Op `{op}` completed on plugin `{}`.",
                pi.resource
            )),
            Ok(Ok(res)) => ToolResult::error(format!(
                "Op `{op}` on plugin `{}` failed: {}",
                pi.resource,
                res.error.unwrap_or_else(|| "unknown error".into())
            )),
            Ok(Err(_)) => ToolResult::error(format!(
                "Bridge for plugin `{}` (agent `{}`) closed before reporting \
                 the result of `{op}`. The op may or may not have run on the \
                 platform — check the channel for evidence and retry if needed.",
                pi.resource, agent_id
            )),
            Err(_) => {
                handle.pending_ops.lock().await.remove(&req_id);
                ToolResult::error(format!(
                    "Op `{op}` on plugin `{}` timed out after 30s without a \
                     result from the bridge. The op may still complete \
                     asynchronously, but its outcome is unknown.",
                    pi.resource
                ))
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
        let runtime = napp::PluginRuntime::new(slug, binary.to_path_buf(), self.plugin_store.clone());
        let mut cmd = runtime.command(&auth.commands.login);
        process::hide_window(&mut cmd);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

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
                                    open_auth_url(&slug_for_stdout, &url, &broadcaster_for_stdout);
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
pub fn is_auth_error(output: &str) -> bool {
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

/// Extract the agent id from a session key. Handles both
/// `agent:<id>:...` and `subagent:<parentId>:...` (a subagent runs under its
/// parent agent's credentials). Returns `None` for non-agent sessions.
fn agent_id_from_session_key(key: &str) -> Option<String> {
    // Delegated (subagent) runs nest the parent's FULL session key:
    //   subagent:<parent_session_key>:<task_id>
    // and <parent_session_key> is itself `agent:<id>:…`. So a naive split on a
    // subagent key returns the literal token "agent" instead of the id, which
    // means a delegated agent's per-account plugin calls resolve to no profile
    // and fall back to the plugin's global credentials (e.g. a duplicated agent
    // reading the original account's inbox). Strip any number of leading
    // `subagent:` wrappers, then read the id out of the inner `agent:<id>:…` key.
    let mut inner = key;
    while let Some(rest) = inner.strip_prefix("subagent:") {
        inner = rest;
    }
    let mut parts = inner.splitn(3, ':');
    match parts.next()? {
        "agent" => parts.next().map(|s| s.to_string()),
        _ => None,
    }
}

/// Find `--<name> <value>` in an arg vector, remove both tokens, and return
/// the value. Used to consume Nebo-level selectors (e.g. `--account`) that
/// must not be forwarded to the plugin binary.
fn extract_and_strip_flag(args: &mut Vec<String>, name: &str) -> Option<String> {
    let flag = format!("--{}", name);
    let idx = args.iter().position(|a| a == &flag)?;
    // Need a value token following the flag.
    if idx + 1 >= args.len() {
        args.remove(idx);
        return None;
    }
    let value = args.remove(idx + 1);
    args.remove(idx);
    Some(value)
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

/// Pull `--key value` flags from a shlex-parsed command. The leading verb is
/// dropped; only flag pairs are kept. Bare flags without a value are treated
/// as boolean `true` so `--dryrun` works.
fn parse_command_flags(command: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let Some(tokens) = shlex::split(command) else {
        return out;
    };
    let mut it = tokens.into_iter();
    let _verb = it.next();
    let toks: Vec<String> = it.collect();
    let mut i = 0;
    while i < toks.len() {
        let tok = &toks[i];
        if let Some(key) = tok.strip_prefix("--") {
            if i + 1 < toks.len() && !toks[i + 1].starts_with("--") {
                out.insert(key.to_string(), toks[i + 1].clone());
                i += 2;
            } else {
                out.insert(key.to_string(), "true".to_string());
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Translate parsed flag args into the NDJSON op JSON line that the channel
/// plugin bridge expects on stdin. See
/// `docs/publishers-guide/channel-plugins.md` for the op contract.
///
/// Required fields per op:
///   - reply:  channel, text (placeholder_ts / thread_ts / files / username optional)
///   - post:   channel, text (thread_ts / files / username optional)
///   - upload: channel, path (thread_ts / caption optional)
///   - dm:     user,    text (files / username optional)
fn build_op_json(
    op: &str,
    args: &std::collections::HashMap<String, String>,
) -> Result<serde_json::Value, String> {
    let mut obj = serde_json::Map::new();
    obj.insert("op".into(), serde_json::Value::String(op.to_string()));

    let want = |key: &str| -> Result<String, String> {
        args.get(key)
            .cloned()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("missing required `--{key}`"))
    };
    let opt = |key: &str| -> Option<String> {
        args.get(key).cloned().filter(|s| !s.is_empty())
    };

    match op {
        "reply" | "post" => {
            obj.insert("channel".into(), serde_json::Value::String(want("channel")?));
            obj.insert("text".into(), serde_json::Value::String(want("text")?));
            if let Some(v) = opt("thread_ts") {
                obj.insert("thread_ts".into(), serde_json::Value::String(v));
            }
            if op == "reply" {
                if let Some(v) = opt("placeholder_ts") {
                    obj.insert("placeholder_ts".into(), serde_json::Value::String(v));
                }
            }
            if let Some(v) = opt("username") {
                obj.insert("username".into(), serde_json::Value::String(v));
            }
        }
        "upload" => {
            obj.insert("channel".into(), serde_json::Value::String(want("channel")?));
            obj.insert("path".into(), serde_json::Value::String(want("path")?));
            if let Some(v) = opt("thread_ts") {
                obj.insert("thread_ts".into(), serde_json::Value::String(v));
            }
            if let Some(v) = opt("caption") {
                obj.insert("caption".into(), serde_json::Value::String(v));
            }
        }
        "dm" => {
            obj.insert("user".into(), serde_json::Value::String(want("user")?));
            obj.insert("text".into(), serde_json::Value::String(want("text")?));
            if let Some(v) = opt("username") {
                obj.insert("username".into(), serde_json::Value::String(v));
            }
        }
        other => return Err(format!("unknown op `{other}`")),
    }

    Ok(serde_json::Value::Object(obj))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_port_suffix_matches_operation() {
        // Fully-qualified port reduces to the capability.resource.action a plugin declares.
        assert_eq!(
            port_suffix("accounting.ap-specialist.ledger.bill.create"),
            "ledger.bill.create"
        );
        assert_eq!(
            port_suffix("sales.account-executive.crm.opportunity.status"),
            "crm.opportunity.status"
        );
        // A bare operation (already the suffix) is returned unchanged.
        assert_eq!(port_suffix("ledger.bill.create"), "ledger.bill.create");
        assert_eq!(port_suffix("mail.message.send"), "mail.message.send");
    }

    #[test]
    fn test_agent_id_from_session_key_resolves_nested_subagent() {
        // Direct agent chat.
        assert_eq!(
            agent_id_from_session_key("agent:abc-123:thread:t1").as_deref(),
            Some("abc-123")
        );
        assert_eq!(agent_id_from_session_key("agent:abc-123").as_deref(), Some("abc-123"));
        // Delegated run: the orchestrator nests the parent's full session key as
        // `subagent:<parent_session_key>:<task_id>`. Must recover the agent id,
        // NOT the literal "agent" token (the bug that made a duplicated agent
        // read the original account's inbox).
        assert_eq!(
            agent_id_from_session_key("subagent:agent:abc-123:thread:t1:task-9").as_deref(),
            Some("abc-123")
        );
        // Doubly-nested delegation.
        assert_eq!(
            agent_id_from_session_key("subagent:subagent:agent:abc-123:thread:t1:task-9:task-10")
                .as_deref(),
            Some("abc-123")
        );
        assert_eq!(agent_id_from_session_key("acp:xyz"), None);
        // Agent-bound workflow runs — plugin tool must resolve credentials here.
        assert_eq!(
            agent_id_from_session_key("agent:cos-uuid:workflow:run-42").as_deref(),
            Some("cos-uuid")
        );
    }

    #[test]
    fn test_workflow_session_key_round_trips_to_agent_id() {
        // The workflow engine builds its session key with this constructor;
        // per-agent plugin account resolution must recover the id from it.
        // (The old dash format `agent-<id>-<run>` parsed to None — every
        // workflow run lost its account.)
        let key = crate::origin::workflow_session_key("abc-123", "run-9");
        assert_eq!(agent_id_from_session_key(&key).as_deref(), Some("abc-123"));
        // Standalone (non-agent) runs carry no identity by design.
        assert_eq!(crate::origin::workflow_session_key("", "run-9"), "");
        assert_eq!(agent_id_from_session_key(""), None);
    }

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
            extract_url(
                "Visit https://accounts.google.com/o/oauth2 to continue",
                false
            ),
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
