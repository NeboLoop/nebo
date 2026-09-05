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

/// The exec budget when the call names no `timeout`.
const EXEC_TIMEOUT_DEFAULT_SECS: u64 = 120;

/// A recovery step (auth probe, refresh, login, retry) is not started with
/// less than this left of the exec budget: it could not finish, and the wait
/// would be read as the plugin timing out.
const RECOVERY_MIN_REMAINING: Duration = Duration::from_secs(10);

/// The install card's answer once the plugin is on disk.
const INSTALL_CARD_INSTALLED: &str = "installed";

/// The Google Workspace plugin's slug, named in the description only while
/// it is installed.
const GOOGLE_WORKSPACE_SLUG: &str = "gws";

/// One deadline for a whole exec, auth recovery included. The runner caps a
/// tool call at its own limit; a 120 s command followed by an auth probe,
/// a refresh and a second probe, each with its own budget, passed that cap
/// and the model read "timed out" for a plugin that had answered (QuickBooks
/// `doctor`, 2026-09-05). Every step gets the time that is left, and a step
/// that could not finish is skipped and named.
struct ExecBudget {
    started: std::time::Instant,
    total: Duration,
}

impl ExecBudget {
    fn start(total: Duration) -> Self {
        Self { started: std::time::Instant::now(), total }
    }

    fn remaining_at(&self, now: std::time::Instant) -> Duration {
        self.total.saturating_sub(now.saturating_duration_since(self.started))
    }

    fn remaining(&self) -> Duration {
        self.remaining_at(std::time::Instant::now())
    }

    /// The time `step` may take, or the text that says it was skipped.
    fn step_at(&self, now: std::time::Instant, command: &str, step: &str) -> Result<Duration, String> {
        let remaining = self.remaining_at(now);
        if remaining < RECOVERY_MIN_REMAINING {
            return Err(format!(
                "{command} finished; {step} was skipped because only {} s of the {} s exec budget remained.",
                remaining.as_secs(),
                self.total.as_secs()
            ));
        }
        Ok(remaining)
    }

    fn step(&self, command: &str, step: &str) -> Result<Duration, String> {
        self.step_at(std::time::Instant::now(), command, step)
    }

    /// The text for a step that started and did not answer in time.
    fn ran_out(&self, command: &str, step: &str, given: Duration) -> String {
        format!(
            "{command} finished; {step} did not answer within the remaining {} s of the {} s exec budget.",
            given.as_secs(),
            self.total.as_secs()
        )
    }
}

/// Run one recovery step inside what is left of the budget: skipped and
/// named when too little is left, cut off and named when it runs out.
async fn bounded<T>(
    budget: &ExecBudget,
    command: &str,
    step: &str,
    fut: impl std::future::Future<Output = T>,
) -> Result<T, String> {
    let given = budget.step(command, step)?;
    cut_off(budget, command, step, given, fut).await
}

/// The cut itself: `fut` gets `given`, and past it the step is named.
async fn cut_off<T>(
    budget: &ExecBudget,
    command: &str,
    step: &str,
    given: Duration,
    fut: impl std::future::Future<Output = T>,
) -> Result<T, String> {
    tokio::time::timeout(given, fut)
        .await
        .map_err(|_| budget.ran_out(command, step, given))
}

/// The budget ended the recovery: say which step, and keep the plugin's own
/// answer in view so the model does not read a bare timeout.
fn out_of_time(text: String, original: &ToolResult) -> ToolResult {
    ToolResult::error(format!("{text}\n\nThe command's own result:\n{}", original.content))
}

/// STRAP domain tool for installed plugin binaries.
///
/// Plugins ship with their own skills (`skills/` directory inside the plugin).
/// These skills are the plugin's documentation — they describe the CLI syntax,
/// flags, and examples. The plugin tool routes to them via `action: "help"`.
///
/// When a plugin command fails due to stale OAuth credentials, the tool
/// automatically detects the auth failure and self-heals: first a SILENT
/// token renewal via the manifest's `auth.commands.refresh` (when declared),
/// and only then — in interactive chat — browser re-authentication via the
/// plugin's `auth login` command, retrying the original command on success.
/// Unattended runs (workflow/channel/schedule) never block on interactive
/// login: the account is flagged `needs_reauth` and the turn ends.
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
// NOTE: gated operations also carry a `display` arg (declared in the tool
// schema below) — the approval gate reads it from the RAW tool-call args
// before dispatch, so it is deliberately absent from this struct and never
// forwarded to the plugin binary.

fn default_action() -> String {
    "exec".to_string()
}

/// The `capability.resource.action` suffix a plugin binding matches on. A fully-
/// qualified port (`department.role.capability.resource.action`) reduces to its
/// last three segments; a bare operation is returned unchanged. This is what keeps
/// one plugin binding (`ledger.bill.create`) satisfying every seat that calls it.
pub fn port_suffix(operation: &str) -> String {
    let parts: Vec<&str> = operation.split('.').collect();
    if parts.len() > 3 {
        parts[parts.len() - 3..].join(".")
    } else {
        operation.to_string()
    }
}

/// The calling department in a fully-qualified port (the first segment of
/// `department.role.capability.resource.action`). `None` for a bare operation.
/// Load-bearing: when a shared operation (e.g. `mail.message.send`) has multiple
/// installed providers, the department is what selects the right one — without it
/// two departments would collide on whichever provider happened to be first.
fn port_department(operation: &str) -> Option<String> {
    let parts: Vec<&str> = operation.split('.').collect();
    if parts.len() > 3 {
        Some(parts[0].to_string())
    } else {
        None
    }
}

/// The capability a port targets (the first segment of the operation suffix,
/// e.g. "ledger" for `…ledger.bill.create`).
fn port_capability(operation: &str) -> String {
    port_suffix(operation)
        .split('.')
        .next()
        .unwrap_or_default()
        .to_string()
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
    fn resolve_port(&self, operation: &str) -> Result<(String, String), String> {
        let suffix = port_suffix(operation);
        // Every installed provider that implements this operation.
        let mut providers: Vec<(String, String)> = Vec::new();
        for slug in self.active_slugs() {
            if let Some(m) = self.plugin_store.get_manifest(&slug) {
                if let Some(cmd) = m.interface_bindings.get(&suffix) {
                    providers.push((slug, cmd.clone()));
                }
            }
        }
        match providers.len() {
            0 => {
                // Every operation an installed plugin does bind, so the model can
                // see what IS available before going to the marketplace.
                let mut bound: Vec<String> = Vec::new();
                for slug in self.active_slugs() {
                    if let Some(m) = self.plugin_store.get_manifest(&slug) {
                        bound.extend(m.interface_bindings.keys().cloned());
                    }
                }
                bound.sort();
                bound.dedup();
                let bound_desc = if bound.is_empty() {
                    "none".to_string()
                } else {
                    bound.join(", ")
                };
                Err(format!(
                    "no installed provider implements operation '{suffix}'. Bound operations: {bound_desc}. To add a provider: plugin(action: \"discover\", query: \"{}\").",
                    port_capability(operation)
                ))
            }
            1 => Ok(providers.into_iter().next().unwrap()),
            _ => {
                // Ambiguous: a shared operation (e.g. mail.message.send) with several
                // providers. The calling DEPARTMENT's binding disambiguates — this is why
                // the port carries department.role. Never guess; a wrong provider here
                // could send from the wrong account or move money the wrong way.
                let dept = port_department(operation);
                let cap = port_capability(operation);
                if let Some(bound) = self.department_provider(dept.as_deref(), &cap) {
                    if let Some(p) = providers.iter().find(|(s, _)| *s == bound) {
                        return Ok(p.clone());
                    }
                }
                let names: Vec<&str> = providers.iter().map(|(s, _)| s.as_str()).collect();
                Err(format!(
                    "operation '{suffix}' is implemented by more than one installed plugin ({}). Call one of them directly: plugin(resource: \"<slug>\", command: \"{}\", args: {{...}}).",
                    names.join(", "),
                    cap
                ))
            }
        }
    }

    /// The gated interface operation a raw exec command corresponds to, if any.
    ///
    /// Matches the command's leading tokens against the plugin's declared
    /// `interfaceBindings` values (a binding command may be multi-word, e.g.
    /// `documents list`), and returns the operation only when the catalog marks
    /// it gated — ungated reads stay runnable through exec.
    fn gated_operation_for_command(&self, slug: &str, command: &str) -> Option<String> {
        let manifest = self.plugin_store.get_manifest(slug)?;
        let command = command.trim();
        for (op, bound_cmd) in &manifest.interface_bindings {
            if command_matches_binding(command, bound_cmd) && crate::interface_catalog::is_gated(op)
            {
                return Some(op.clone());
            }
        }
        None
    }

    /// The provider a department has bound for a capability (e.g. accounting's
    /// `mail` → "postmark", support's `mail` → a different provider). This is what
    /// makes resolution department-scoped and collision-free. Populated by the
    /// install wizard's per-department capability binding; `None` until bound, so an
    /// ambiguous port fails loudly rather than resolving to the wrong provider.
    fn department_provider(&self, department: Option<&str>, capability: &str) -> Option<String> {
        let _dept = department?;
        let _ = capability;
        // TICKET-02: the install wizard writes the per-department capability→provider
        // binding (keyed "<department>.<capability>", e.g. "accounting.mail" → "postmark",
        // "customer-support.mail" → a different provider); this reads it. Until that store
        // exists, return None — so an ambiguous port fails loudly demanding a binding,
        // never resolving to the wrong provider.
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
                 find plugins in the marketplace; installing offers the user a card to approve.",
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

    /// Search the NeboAI marketplace for plugins. In interactive chat the top
    /// match renders as an inline INSTALL CARD (ask_user widget) that parks
    /// this call — the button redeems the install code through the canonical
    /// `POST /codes` pathway, so there is still exactly one install path and
    /// the user approves by tapping, not by reading a code out of prose.
    /// Unattended runs (and a skipped card) fall back to the text listing.
    /// The ONE connect-account card payload — used by the install→connect
    /// chain in discover and by first-use auth in exec. Two producers of this
    /// widget shape would drift (CODE_AUDITOR 8.1).
    fn connect_account_widget(plugin: &str, agent_id: &str, label: &str) -> serde_json::Value {
        serde_json::json!([{
            "type": "connect_account",
            "plugin": plugin,
            "agentId": agent_id,
            "label": label,
        }])
    }

    async fn handle_discover(&self, query: &str, ctx: &crate::ToolContext) -> ToolResult {
        let api = match crate::build_neboai_api(&self.db_store) {
            Ok(a) => a,
            Err(e) => return ToolResult::error(format!("marketplace unavailable: {}", e)),
        };
        let q = if query.trim().is_empty() {
            None
        } else {
            Some(query.trim())
        };
        // No type straitjacket: the standalone services (Gmail, Drive, …) are
        // `connector`-typed in the catalog, so a plugin-only search made them
        // INVISIBLE to discover — the user asked for Gmail and could never get
        // it. Search untyped, then keep the installable capability types.
        match api.list_products(None, q, None, None, Some(20)).await {
            Ok(v) => {
                // Canonical envelope is ListProductsResponse: { "products": [...] }.
                // A missing array is a contract break, NOT zero results — say so.
                let items = v.get("products").and_then(|x| x.as_array());
                if items.is_none() {
                    return ToolResult::error(format!(
                        "marketplace search returned an unexpected shape (no `products` array): {}",
                        crate::truncate_str(&v.to_string(), 200)
                    ));
                }
                let installable: Vec<serde_json::Value> = items
                    .into_iter()
                    .flatten()
                    .filter(|it| {
                        matches!(
                            it.get("type").and_then(|x| x.as_str()),
                            Some("plugin") | Some("connector") | None
                        )
                    })
                    .cloned()
                    .collect();
                let matched = items.map(|a| a.len()).unwrap_or(0);
                self.offer(query, ctx, &installable, matched).await
            }
            Err(e) => ToolResult::error(format!("marketplace search failed: {}", e)),
        }
    }

    /// The second half of discover: the listing, and in interactive chat the
    /// install card for the best match. A plugin that is already installed
    /// gets no card (live, 2026-09-05: QuickBooks was offered for install
    /// while installed) and goes straight to the step after "installed".
    async fn offer(
        &self,
        query: &str,
        ctx: &crate::ToolContext,
        arr: &[serde_json::Value],
        matched: usize,
    ) -> ToolResult {
                if !arr.is_empty() {
                    {
                        // Listings NEVER carry install codes — codes are machine
                        // currency (the card button redeems them; the marketplace
                        // shows them to humans). A code in model-visible text is
                        // one hop from a code pasted into chat.
                        let interactive = crate::origin::ExecutionMode::from(ctx.origin)
                            == crate::origin::ExecutionMode::Interactive
                            && ctx.ask_channels.is_some();
                        let mut lines = Vec::new();
                        for it in arr {
                            let name = it.get("name").and_then(|x| x.as_str()).unwrap_or("?");
                            let slug = it.get("slug").and_then(|x| x.as_str()).unwrap_or("");
                            let desc =
                                it.get("description").and_then(|x| x.as_str()).unwrap_or("");
                            lines.push(format!("- {} ({}) — {}", name, slug, desc));
                        }
                        let listing = format!(
                            "Found {} plugin(s):\n{}",
                            lines.len(),
                            lines.join("\n")
                        );

                        // Interactive chat: park on an install card for the best
                        // match instead of narrating a code. The card's button
                        // redeems the code via POST /codes (the one install
                        // pathway); "installed" resumes this call.
                        //
                        // Best match, not arr[0]: the marketplace ranks by
                        // relevance, but a query that IS a product's name must
                        // beat a bundle that merely mentions it — "gmail" kept
                        // carding the deprecated Google Workspace bundle and the
                        // user could never install the thing they named.
                        let q = query.trim().to_lowercase();
                        let field = |it: &serde_json::Value, k: &str| {
                            it.get(k).and_then(|x| x.as_str()).unwrap_or("").to_lowercase()
                        };
                        let top = arr
                            .iter()
                            .find(|it| !q.is_empty() && (field(it, "name") == q || field(it, "slug") == q))
                            .or_else(|| {
                                arr.iter().find(|it| {
                                    !q.is_empty()
                                        && (field(it, "name").starts_with(&q)
                                            || field(it, "slug").starts_with(&q))
                                })
                            })
                            .unwrap_or(&arr[0]);
                        let top_code = top.get("code").and_then(|x| x.as_str()).unwrap_or("");
                        let top_slug = top.get("slug").and_then(|x| x.as_str()).unwrap_or("");
                        let already_installed =
                            !top_slug.is_empty() && self.plugin_store.resolve(top_slug, "*").is_some();
                        if already_installed || (interactive && !top_code.is_empty()) {
                            let top_name =
                                top.get("name").and_then(|x| x.as_str()).unwrap_or("plugin");
                            let top_desc = top
                                .get("description")
                                .and_then(|x| x.as_str())
                                .unwrap_or("");
                            // Installed already: no card, the answer is known.
                            let answer = if already_installed {
                                Some(INSTALL_CARD_INSTALLED.to_string())
                            } else {
                                ctx.ask_user(
                                    &format!(
                                        "**{top_name}** can do this. Install it on the card and \
                                         I'll pick up right where I left off."
                                    ),
                                    serde_json::json!([{
                                        "type": "install_plugin",
                                        "code": top_code,
                                        "name": top_name,
                                        "plugin": top_slug,
                                        "description": top_desc,
                                    }]),
                                )
                                .await
                            };
                            let state = if already_installed { "was already installed" } else { "is installed" };
                            if answer.as_deref() == Some(INSTALL_CARD_INSTALLED) {
                                // ONE-CARD CHAIN: a plugin that declares an
                                // account gets its connect step NOW, in the
                                // same flow — install → connect → straight to
                                // work, no model turn or "set it up later"
                                // between. Auth-less plugins skip straight on.
                                let auth_label = self
                                    .plugin_store
                                    .get_manifest(top_slug)
                                    .and_then(|m| m.auth)
                                    .map(|a| a.label)
                                    .filter(|l| !l.is_empty());
                                if let Some(label) = auth_label
                                    && interactive
                                {
                                    let agent_id =
                                        types::keyparser::extract_agent_id(&ctx.session_key);
                                    if !agent_id.is_empty() {
                                        let connected = ctx
                                            .ask_user(
                                                &format!(
                                                    "**{top_name}** {state}. Connect your \
                                                     {label} on the card and I'll get straight \
                                                     to work."
                                                ),
                                                Self::connect_account_widget(
                                                    top_slug, &agent_id, &label,
                                                ),
                                            )
                                            .await;
                                        if connected.as_deref() == Some("connected") {
                                            return ToolResult::ok(format!(
                                                "{top_name} {state} and its account is \
                                                 connected. Continue the task NOW via \
                                                 plugin(resource: \"{top_slug}\", ...) — no \
                                                 setup narration."
                                            ));
                                        }
                                        return ToolResult::ok(format!(
                                            "{top_name} {state}; the account was not \
                                             connected (card skipped). The connect card \
                                             re-appears on first use — continue, or ask what \
                                             they'd like to do."
                                        ));
                                    }
                                }
                                return ToolResult::ok(format!(
                                    "{top_name} {state}. Use it via plugin(resource: \
                                     \"{top_slug}\", ...). If it needs an account, the connect \
                                     card will appear on first use — no setup narration needed."
                                ));
                            }
                            // Skipped/declined: fall through to the listing so the
                            // conversation can continue (other options, questions).
                            return ToolResult::ok(format!(
                                "{listing}\n\nThe user declined the install card for {top_name}. \
                                 Discuss alternatives or answer questions — do NOT paste install \
                                 codes into chat; if they change their mind, call discover again \
                                 to re-offer the card."
                            ));
                        }
                        if interactive {
                            // Interactive but no usable card (top match had no
                            // code): never fall back to narrating codes.
                            ToolResult::ok(format!(
                                "{listing}\n\nAsk the user which one they want, then call \
                                 discover again with its exact name to offer the install card. \
                                 Do NOT paste install codes into chat."
                            ))
                        } else {
                            // Unattended runs can't approve an install card —
                            // recommend, never transact.
                            ToolResult::ok(format!(
                                "{listing}\n\nInstalling needs the owner's approval in the \
                                 app — report which one fits and why. Never include install \
                                 codes in any message."
                            ))
                        }
                    }
                } else if matched > 0 {
                    ToolResult::ok(format!(
                        "{} results matched but none are installable plugins/connectors.",
                        matched
                    ))
                } else {
                    ToolResult::ok("No plugins found in the marketplace for that query.")
                }
    }

    /// Find the skills directory for a plugin slug.
    ///
    /// Walks up from the binary path looking for a `skills/` directory.
    /// Handles both layouts:
    ///   - Installed plugins: `<data>/plugins/<slug>/<version>/{binary,skills/}`
    ///     (skills/ is sibling of binary, 1 level up)
    ///   - Symlinked dev plugins: `<data_dir>/user/plugins/<slug>/{target/release/binary,skills/}`
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

/// Render a skill name as a command label, but ONLY when the plugin actually
/// follows the convention that makes the label true.
///
/// GWS names its skill dirs `<slug>-<service>-<verb>` (`gws-gmail-triage`), and
/// each one really is a subcommand, so `gmail +triage` is a fact the model can
/// act on. A plugin that does NOT prefix its skills with its slug is not making
/// that promise: nebo-office ships `pptx-design`, which documents a JSON spec and
/// is not a subcommand at all.
///
/// This used to split on the first dash regardless, so `pptx-design` was
/// advertised as `pptx +design`. An agent believed it, ran `pptx design --help`,
/// got "unrecognized subcommand", and concluded from the invented label that the
/// plugin's 21 skills documented features that did not exist — filing a report
/// recommending they be deleted. An invented command name is worse than a raw
/// directory name, so a skill that isn't slug-prefixed is shown as-is.
fn display_command_for_skill(
    slug: &str,
    skill_name: &str,
    siblings: &std::collections::HashSet<String>,
) -> String {
    match skill_name.strip_prefix(&format!("{}-", slug)) {
        // Slug-prefixed: the remainder may be service + verb, per the GWS
        // convention (`gws-gmail-send` → `gmail +send`). It is only a helper
        // when the service it claims to extend is itself a skill here — GWS
        // ships `gws-gmail` alongside `gws-gmail-send`. Without that check a
        // multi-word service name reads as a helper on a service that does not
        // exist: `google-calendar-free-busy` printed `free +busy`, sending
        // agents after a `free` subcommand when the command is `free-busy`.
        Some(trimmed) => match trimmed.split_once('-') {
            Some((service, verb)) if siblings.contains(&format!("{}-{}", slug, service)) => {
                format!("{} +{}", service, verb)
            }
            _ => trimmed.to_string(),
        },
        // Not slug-prefixed: we have no idea whether this names a subcommand.
        None => skill_name.to_string(),
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
                    query: \"<keyword>\") to find plugins in the marketplace (installing \
                    offers the user a card to approve). Once one is installed, every command \
                    call names it by the slug plugin(action: \"list\") shows: \
                    plugin(resource: \"<slug>\", action: \"exec\", command: \"<subcommand and flags>\")."
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
        // Said only when that plugin is installed: a made-up example slug was
        // copied verbatim by a live run and reported as "not installed".
        if slugs.iter().any(|s| s == GOOGLE_WORKSPACE_SLUG) {
            out.push_str(&format!("For Google Calendar/Gmail/Drive use plugin(resource: \"{GOOGLE_WORKSPACE_SLUG}\", ...); for the local Mac calendar use os(resource: \"calendar\").\n\n"));
        } else {
            out.push('\n');
        }
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
            let sibling_names: std::collections::HashSet<String> =
                services.iter().map(|(n, _)| n.clone()).collect();
            for (name, desc) in services {
                let label = display_command_for_skill(slug, name, &sibling_names);
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
        let mut props = serde_json::Map::new();
        props.insert("resource".into(), Self::resource_schema(&self.active_slugs()));
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
        props.insert(
            "display".into(),
            serde_json::json!({
                "type": "string",
                "description": "REQUIRED with any gated `operation` (money movement, outbound send, irreversible write): ONE plain-language sentence describing the action for the business owner's approval prompt. Use real names a non-technical person recognizes — company/person names, formatted amounts ('$2,500.00'), dates — never raw ids or cents. Example: 'Pay Acme Supplies $2,500.00 for bill #1042, due Jul 28'."
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
        // `discover` is read-only but can PARK on the inline install card
        // (ask_user). A concurrently-executed tool races the model turn's
        // stream teardown: the ask_request lands in a dropped channel and the
        // oneshot waits forever (observed live on the first card test,
        // 2026-08-22). Anything that may ask must run sequentially.
        matches!(action, "list" | "events" | "help")
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
                    Ok(x) => x,
                    Err(e) => return ToolResult::error(e),
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
                "discover" => self.handle_discover(&pi.query, ctx).await,
                "exec" | "" => {
                    if pi.resource.is_empty() {
                        return ToolResult::error(self.resource_required("exec", "exec\", command: \"doctor"));
                    }
                    // Raw exec must not be a side door around the per-employee
                    // operation gate: a command that IS a gated bound operation
                    // (e.g. ballast's `ingest` = kb.article.create) only runs
                    // through the typed port, where the runner's OperationPolicy
                    // gate (Blocked / Approval) applies. Observed live: an agent
                    // whose kb.article.create was Blocked offered to run the
                    // same write via exec instead.
                    if let Some(op) = self.gated_operation_for_command(&pi.resource, &pi.command) {
                        return ToolResult::error(format!(
                            "'{}' on {} is the gated operation '{op}'. Call it as \
                             plugin(operation: \"{op}\", input: {{...}}, display: \"<plain-language \
                             summary for the owner>\") so the owner's approval controls apply — \
                             do not retry it through exec.",
                            pi.command, pi.resource
                        ));
                    }
                    self.handle_exec(&pi, ctx).await
                }
                "events" => {
                    if pi.resource.is_empty() {
                        return ToolResult::error(self.resource_required("events", "events"));
                    }
                    self.handle_events(&pi.resource)
                }
                "help" => {
                    if pi.resource.is_empty() {
                        return ToolResult::error(self.resource_required("help", "help"));
                    }
                    self.handle_help(&pi.resource, &pi.command)
                }
                "search" | "skills" | "services" => ToolResult::error(format!(
                    "action '{}' was removed in v0.10.0. Use action: \"list\" to see installed plugins, \"discover\" to search the marketplace, or call commands directly with action: \"exec\".",
                    pi.action
                )),
                other => ToolResult::error(format!(
                    "Unknown action: '{}'. Valid actions: list, discover, help, exec, events.",
                    other
                )),
            }
        })
    }
}

impl PluginTool {
    /// The `resource` property: the installed slugs as an enum when there are
    /// any. With none installed the enum is left out, because `enum: []`
    /// makes every value schema-invalid and a validating provider then
    /// rejects the right slug too (audit 2026-09-05).
    fn resource_schema(slugs: &[String]) -> serde_json::Value {
        let mut schema = serde_json::json!({
            "type": "string",
            "description": "Plugin slug: which installed plugin this call is about, as plugin(action: \"list\") shows it"
        });
        if !slugs.is_empty() {
            schema["enum"] = serde_json::Value::Array(
                slugs.iter().map(|s| serde_json::Value::String(s.clone())).collect(),
            );
        }
        schema
    }

    /// `resource` names which installed plugin a call is about. Said with
    /// the plugins that are actually installed, because a made-up example
    /// slug ("gws") was copied verbatim by a live run and then reported as
    /// "not installed" (2026-09-05).
    fn resource_required(&self, action: &str, example_tail: &str) -> String {
        let installed: Vec<String> = self
            .plugin_store
            .list_installed()
            .into_iter()
            .map(|(slug, _, _, _)| slug)
            .collect();
        let choices = match installed.len() {
            0 => "No plugin is installed; plugin(action: \"discover\", query: ...) finds one.".to_string(),
            1 => format!("The only installed plugin is \"{}\".", installed[0]),
            _ => format!("Installed plugins: {}.", installed.join(", ")),
        };
        format!(
            "resource is required for action \"{action}\": the slug of the installed plugin. {choices} Example: plugin(resource: \"{}\", action: \"{example_tail}\")",
            installed.first().map(String::as_str).unwrap_or("<slug>")
        )
    }

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
                // "Not installed" and "installed without docs" are different
                // facts with different next calls.
                if self.plugin_store.resolve(slug, "*").is_none() {
                    let slugs = self.active_slugs();
                    let installed = if slugs.is_empty() {
                        "none".to_string()
                    } else {
                        slugs.join(", ")
                    };
                    return ToolResult::error(format!(
                        "Plugin '{}' is not installed (installed: {})",
                        slug, installed
                    ));
                }
                return ToolResult::error(format!(
                    "'{}' is installed but ships no skills/ documentation; run plugin(resource: \"{}\", command: \"--help\").",
                    slug, slug
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
            out.push_str("## Bundled skills\n\n");
            let sibling_names: std::collections::HashSet<String> =
                services.iter().map(|(n, _)| n.clone()).collect();
            for (name, desc) in &services {
                let label = display_command_for_skill(slug, name, &sibling_names);
                if desc.is_empty() {
                    out.push_str(&format!("- {}\n", label));
                } else {
                    out.push_str(&format!("- {} — {}\n", label, desc));
                }
            }
            // These are skill directories, and only some plugins name them after
            // subcommands. Reading one is always right; assuming it is a
            // subcommand is how an agent ends up running `pptx design --help`.
            out.push_str(&format!(
                "\nRead a skill with skill(action: \"load\", name: \"<name above>\") — a skill is documentation, \
                 and only names a subcommand when it is written as `<service> +<verb>`. \
                 For a subcommand's real flags, ask the binary: \
                 plugin(resource: \"{}\", action: \"exec\", command: \"<subcommand> --help\").",
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
                     agent(resource: \"registry\", action: \"create\", name: \"...\", automations: [\n  \
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

        let budget = ExecBudget::start(Self::exec_timeout(pi));
        let command_label = Self::command_label(pi);
        let result = self.run_plugin_command(pi, ctx, budget.remaining()).await;

        // On error, check if it's an auth failure and attempt self-heal:
        // silent refresh first (manifest `auth.commands.refresh`), interactive
        // browser login only as the last resort — and never when unattended.
        if result.is_error {
            if let Some((binary, auth)) = self.plugin_store.get_auth_info(&pi.resource) {
                if is_auth_error(&result.content) {
                    // Resolve this agent's account profile for profile-dir
                    // plugins (e.g. gws) so the confirm probe and the silent
                    // refresh hit the SAME config dir the failing command ran
                    // against — the plugin's global default dir may be a
                    // different (healthy) account.
                    let profile = auth.profile_dir_env.as_deref().and_then(|_| {
                        let selected = pi.args.get("account").cloned().or_else(|| {
                            shlex::split(&pi.command)
                                .and_then(|mut a| extract_and_strip_flag(&mut a, "account"))
                        });
                        Some(types::keyparser::extract_agent_id(&ctx.session_key))
                            .filter(|id| !id.is_empty())
                            .and_then(|agent_id| {
                            self.db_store
                                .resolve_plugin_account_profile(
                                    &agent_id,
                                    &pi.resource,
                                    selected.as_deref(),
                                )
                                .ok()
                                .flatten()
                        })
                    });
                    let probe_dir: Option<(&str, &str)> =
                        match (auth.profile_dir_env.as_deref(), profile.as_ref()) {
                            (Some(env), Some(p)) => Some((env, p.config_dir.as_str())),
                            _ => None,
                        };

                    // Confirm with a fresh auth-status check (the one canonical
                    // decision, via PluginStore) if the command is available.
                    if auth.commands.status.is_some() {
                        match bounded(&budget, &command_label, "the auth status check", self.probe_auth(&pi.resource, probe_dir)).await {
                            // Status says authenticated — false positive, return original error
                            Ok(Some(true)) => return result,
                            Ok(_) => {}
                            Err(text) => return out_of_time(text, &result),
                        }
                    }

                    info!(plugin = %pi.resource, "auth failure detected");

                    // FIRST: silent, non-interactive token renewal when the
                    // manifest declares a refresh command. No user interruption,
                    // no browser — renew, re-probe, retry.
                    if auth.commands.refresh.is_some() {
                        if let Err(text) = bounded(&budget, &command_label, "the silent token refresh", self.plugin_store.run_auth_refresh(&pi.resource, probe_dir)).await {
                            return out_of_time(text, &result);
                        }
                        match bounded(&budget, &command_label, "the auth status check after the refresh", self.probe_auth(&pi.resource, probe_dir)).await {
                            Ok(Some(true)) => {
                                info!(plugin = %pi.resource, "silent token refresh healed auth, retrying command");
                                return match budget.step(&command_label, "the retry after the refresh") {
                                    Ok(given) => self.run_plugin_command(pi, ctx, given).await,
                                    Err(text) => out_of_time(text, &result),
                                };
                            }
                            Ok(_) => {}
                            Err(text) => return out_of_time(text, &result),
                        }
                    }

                    // Unattended run (workflow / channel / schedule — nobody at
                    // the keyboard): NEVER block on interactive browser login.
                    // Flag the account for reconnect + fire the one canonical
                    // reauth notification, then end the turn.
                    let interactive = crate::origin::ExecutionMode::from(ctx.origin)
                        == crate::origin::ExecutionMode::Interactive
                        && ctx.ask_channels.is_some();
                    if !interactive {
                        warn!(plugin = %pi.resource, "auth expired in unattended run; silent refresh failed");
                        if let Some(p) = profile.as_ref() {
                            if let Err(e) = self.db_store.set_plugin_account_reauth(&p.id, true) {
                                warn!(error = %e, "failed to set plugin reauth flag");
                            }
                            if !p.reauth_notified {
                                notify_plugin_needs_reauth(
                                    &self.db_store,
                                    |ev, data| {
                                        if let Some(ref bc) = self.broadcaster {
                                            bc(ev, data);
                                        }
                                    },
                                    p,
                                );
                                let _ =
                                    self.db_store.mark_plugin_account_reauth_notified(&p.id);
                            }
                        }
                        if let Some(ref bc) = self.broadcaster {
                            bc(
                                "plugin_auth_error",
                                serde_json::json!({
                                    "plugin": &pi.resource,
                                    "error": "Authentication expired and silent refresh failed",
                                }),
                            );
                        }
                        return ToolResult::terminal(format!(
                            "I couldn't reach **{}** — its authentication expired and automatic \
                             renewal didn't work. Please reconnect this account in the agent's \
                             Settings, Plugins, then ask me again.",
                            pi.resource
                        ));
                    }

                    // Interactive chat: fall through to today's browser OAuth path.
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
                    let login_time = match budget.step(&command_label, "the browser login") {
                        Ok(given) => given,
                        Err(text) => return out_of_time(text, &result),
                    };
                    if self.run_auth_login(&pi.resource, &binary, &auth, login_time).await {
                        info!(plugin = %pi.resource, "re-authentication succeeded, retrying command");

                        // Broadcast success
                        if let Some(ref bc) = self.broadcaster {
                            bc(
                                "plugin_auth_complete",
                                serde_json::json!({ "plugin": &pi.resource }),
                            );
                        }

                        return match budget.step(&command_label, "the retry after the login") {
                            Ok(given) => self.run_plugin_command(pi, ctx, given).await,
                            Err(text) => out_of_time(text, &result),
                        };
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
                         agent's Settings, Plugins, then ask me again.",
                        pi.resource
                    ));
                }
            }
        }

        result
    }

    /// The exec budget a call asked for, or the default.
    fn exec_timeout(pi: &PluginInput) -> Duration {
        if pi.timeout > 0 {
            Duration::from_secs(pi.timeout as u64)
        } else {
            Duration::from_secs(EXEC_TIMEOUT_DEFAULT_SECS)
        }
    }

    /// How a budget message names the command that ran.
    fn command_label(pi: &PluginInput) -> String {
        if pi.command.is_empty() {
            "the command".to_string()
        } else {
            pi.command.clone()
        }
    }

    /// Execute a plugin command and return the result. Shared by initial call
    /// and retry; `timeout` is what is left of the exec budget.
    async fn run_plugin_command(&self, pi: &PluginInput, ctx: &ToolContext, timeout: Duration) -> ToolResult {
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
                let available = if slugs.is_empty() {
                    "none installed".to_string()
                } else {
                    slugs.join(", ")
                };
                // Installed-but-disabled plugins are a different fact from
                // absent ones: the fix is a toggle, not an install.
                let mut disabled: Vec<String> = Vec::new();
                for (slug, _, _, _) in self.plugin_store.list_installed() {
                    if disabled.contains(&slug) {
                        continue;
                    }
                    if let Ok(Some(row)) = self.db_store.get_plugin_by_slug(&slug)
                        && row.is_enabled == 0
                    {
                        disabled.push(slug);
                    }
                }
                let disabled_desc = if disabled.is_empty() {
                    String::new()
                } else {
                    format!(" (disabled: {})", disabled.join(", "))
                };
                return ToolResult::error(format!(
                    "Plugin '{}' not found. Available: {}{}",
                    pi.resource, available, disabled_desc
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

        // Split command string into args (subcommand + simple flags).
        let mut args = if !pi.command.is_empty() {
            match shlex::split(&pi.command) {
                Some(a) => a,
                None => {
                    return ToolResult::error(format!(
                        "Could not parse command '{}' (unbalanced quotes). Put values with quotes/special characters in args: {{\"key\": \"value\"}} instead.",
                        pi.command
                    ));
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
                         in this agent's Settings, Plugins.",
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
                let agent_id =
                    Some(types::keyparser::extract_agent_id(&ctx.session_key))
                        .filter(|id| !id.is_empty());
                let profile = agent_id.as_deref().and_then(|agent_id| {
                    self.db_store
                        .resolve_plugin_account_profile(
                            agent_id,
                            &pi.resource,
                            selected_account.as_deref(),
                        )
                        .ok()
                        .flatten()
                });
                match profile {
                    Some(p) => Some((env_name, p.config_dir)),
                    None => {
                        // Tell the truth about WHICH failure this is. Claiming
                        // "no account is connected" when accounts exist but the
                        // `--account` label didn't match sent workflows into
                        // early exit over one apostrophe glyph — the model
                        // trusts this message verbatim. Naming the connected
                        // labels lets it retry with the right one.
                        let connected: Vec<String> = agent_id
                            .as_deref()
                            .and_then(|a| {
                                self.db_store
                                    .list_plugin_account_profiles(a, &pi.resource)
                                    .ok()
                            })
                            .unwrap_or_default()
                            .into_iter()
                            .map(|p| p.account_label)
                            .collect();
                        if let (Some(label), false) = (selected_account.as_deref(), connected.is_empty()) {
                            // Wrong label with accounts present: the model can
                            // fix this itself, so it stays a plain error.
                            return ToolResult::error(format!(
                                "No {res} account named \"{label}\" for this agent. Connected \
                                 {res} accounts: {labels}. Retry with one of those exact labels \
                                 (or omit --account to use the primary).",
                                res = pi.resource,
                                labels = connected.join(", ")
                            ));
                        }
                        let none_msg = format!(
                            "No {res} account is connected for this agent. Connect one in \
                             this agent's Settings, Plugins before using {res}.",
                            res = pi.resource
                        );
                        // Nothing connected. Interactive chat renders an inline
                        // connect card via ask_user, which parks THIS tool call
                        // until the account is connected — the run then resumes
                        // at the same call. Unattended runs stop cleanly rather
                        // than letting the model improvise around the failure.
                        let interactive = crate::origin::ExecutionMode::from(ctx.origin)
                            == crate::origin::ExecutionMode::Interactive
                            && ctx.ask_channels.is_some();
                        let Some(agent_id) = agent_id.as_deref() else {
                            return ToolResult::error(none_msg);
                        };
                        if !interactive {
                            return ToolResult::terminal(none_msg);
                        }
                        let display_label = self
                            .plugin_store
                            .get_manifest(&pi.resource)
                            .and_then(|m| m.auth)
                            .map(|a| a.label)
                            .filter(|l| !l.is_empty())
                            .unwrap_or_else(|| pi.resource.clone());
                        let answer = ctx
                            .ask_user(
                                &format!(
                                    "I need your {display_label} connected to continue. \
                                     Connect it on the card and I'll pick up right where I \
                                     left off."
                                ),
                                Self::connect_account_widget(
                                    &pi.resource,
                                    agent_id,
                                    &display_label,
                                ),
                            )
                            .await;
                        if answer.as_deref() != Some("connected") {
                            return ToolResult::error(none_msg);
                        }
                        match self
                            .db_store
                            .resolve_plugin_account_profile(
                                agent_id,
                                &pi.resource,
                                selected_account.as_deref(),
                            )
                            .ok()
                            .flatten()
                        {
                            Some(p) => Some((env_name, p.config_dir)),
                            None => {
                                return ToolResult::error(format!(
                                    "The {res} account didn't finish connecting. {none_msg}",
                                    res = pi.resource
                                ));
                            }
                        }
                    }
                }
            }
            None => None,
        };

        // ONE canonical launch path (CODE_AUDITOR 8.1). This used to construct
        // its own Command right after building the runtime, which meant it also
        // owned — and drifted on — kill_on_drop, env assembly and pid tracking.
        // Per-invocation context goes through `with_env` so the runtime stays the
        // single place that knows how to assemble a plugin's environment.
        let mut runtime = napp::PluginRuntime::new(
            &pi.resource,
            binary_path.clone(),
            self.plugin_store.clone(),
        )
        .with_deps()
        .with_permissions();

        // The local API's address and the acting agent — the same pair
        // channel bridges and auth-login spawns already get, so a plugin
        // command run BY an agent (e.g. `phonecall dial`) can reach this
        // Nebo's own endpoints as that agent. Loopback address, not a
        // credential.
        for (key, value) in napp::plugin::plugin_base_env() {
            runtime = runtime.with_env(key, value);
        }
        let key_agent_id = types::keyparser::extract_agent_id(&ctx.session_key);
        if !key_agent_id.is_empty() {
            runtime = runtime.with_env("NEBO_AGENT_ID", key_agent_id);
        }

        // Channel context so channel-plugin subcommands (e.g. `slack upload`)
        // can target the current channel/thread without the agent looking up ids.
        // See `docs/publishers-guide/channel-plugins.md`.
        if let Some(ch) = &ctx.channel {
            runtime = runtime
                .with_env("NEBO_CHANNEL_KIND", &ch.kind)
                .with_env("NEBO_CHANNEL_ID", &ch.channel_id);
            if let Some(ts) = &ch.thread_ts {
                runtime = runtime.with_env("NEBO_THREAD_TS", ts);
            }
        }

        // Per-account credential isolation: this agent's chosen account dir.
        if let Some((env_name, config_dir)) = &profile_dir_injection {
            runtime = runtime.with_env(env_name.clone(), config_dir.clone());
        }

        let started = std::time::SystemTime::now();
        let result = runtime
            .run_capture_args(&args, timeout)
            .await;

        match result {
            Err(napp::plugin_runtime::LaunchError::TimedOut { .. }) => ToolResult::error(format!(
                "Plugin '{}' command timed out after {}s",
                pi.resource,
                timeout.as_secs()
            )),
            Err(e) => ToolResult::error(format!("Plugin '{}' command failed: {}", pi.resource, e)),
            Ok(output) => {
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
                    // No exit code means the process was killed by a signal.
                    let how = match output.status.code() {
                        Some(code) => format!("exited with code {}", code),
                        None => "was terminated by a signal".to_string(),
                    };
                    return ToolResult::error(format!(
                        "Plugin '{}' {}\n{}",
                        pi.resource, how, text
                    ));
                }

                if text.is_empty() {
                    text = "(command exited 0 with no stdout or stderr)".to_string();
                }

                // Truncate very long output (char-boundary safe)
                if text.len() > crate::MAX_SUBPROCESS_OUTPUT {
                    let total = text.len();
                    types::strutil::safe_truncate(&mut text, crate::MAX_SUBPROCESS_OUTPUT);
                    text.push_str(&format!(
                        "\n[output truncated: showing first {} of {} bytes]",
                        crate::MAX_SUBPROCESS_OUTPUT, total
                    ));
                }

                // A plugin that produced a user-facing document (e.g. a deck via
                // `nebo-office pptx create spec.json -o out.pptx`) must surface it
                // exactly like an `os` write does, or it never reaches the Work
                // panel / chat cards. Same is_work_document gate; the mtime check
                // keeps inputs the plugin only read (a spec, a template) out.
                let result = ToolResult::ok(text);
                match produced_work_document(&args, None, started) {
                    Some(path) => result.with_image_url(path),
                    None => result,
                }
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
            Some(types::keyparser::extract_agent_id(&ctx.session_key))
                .filter(|s| !s.is_empty())
                .as_deref()
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
                "Bridge for plugin `{}` (agent `{}`) has closed its stdin ({e}). \
                 Restart the channel in Settings > Channels.",
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
                "Op `{op}` completed on plugin `{}` (agent `{}`, req_id {}).",
                pi.resource, agent_id, req_id
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
    /// Fresh auth probe against the exact credential dir the failing command
    /// used: profile-aware when the plugin declares per-account config dirs,
    /// otherwise the global slug-level check. `Some(true)` = authenticated,
    /// `Some(false)` = definitively not, `None` = inconclusive.
    async fn probe_auth(&self, slug: &str, profile_dir: Option<(&str, &str)>) -> Option<bool> {
        match profile_dir {
            Some((env, dir)) => {
                self.plugin_store
                    .check_auth_for_profile(slug, env, dir)
                    .await
            }
            None => Some(self.plugin_store.check_auth_now(slug).await),
        }
    }

    async fn run_auth_login(
        &self,
        slug: &str,
        binary: &Path,
        auth: &napp::plugin::PluginAuth,
        budget: Duration,
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

        // Wait for the auth login process for what is left of the exec budget.
        let login_result = tokio::time::timeout(budget, async {
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
                warn!(plugin = %slug, secs = budget.as_secs(), "plugin auth login timed out");
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
/// The last arg naming a work document (same gate as `os` writes) that was
/// modified during this execution — i.e. a file the command just produced, not
/// an input it read. Output flags conventionally come last (`-o out.pptx`),
/// hence the reverse scan. Relative tokens resolve against `base` (a shell
/// `cwd`) when given. The 1s slack absorbs coarse filesystem mtimes.
/// Shared by plugin exec and shell exec so every way a run creates a document
/// surfaces it identically. Ceiling: files a command creates WITHOUT naming
/// them in its args (e.g. `unzip`) aren't detected.
pub(crate) fn produced_work_document(
    args: &[String],
    base: Option<&std::path::Path>,
    started: std::time::SystemTime,
) -> Option<String> {
    let cutoff = started - std::time::Duration::from_secs(1);
    args.iter().rev().find_map(|a| {
        if !crate::file_tool::is_work_document(a) {
            return None;
        }
        let path = match base {
            Some(b) if std::path::Path::new(a).is_relative() => b.join(a.as_str()),
            _ => std::path::PathBuf::from(a.as_str()),
        };
        let fresh = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .map(|m| m >= cutoff)
            .unwrap_or(false);
        fresh.then(|| path.to_string_lossy().to_string())
    })
}

/// Fire the one-time "reconnect this account" notification (bell + toast) and
/// broadcast it, mirroring the canonical proactive-notification pathway. This
/// is the ONE pathway for plugin-account reauth notifications — used by the
/// server's proactive token refresher (`spawn_plugin_token_refresher`) and by
/// the mid-run unattended auth-failure path above. `broadcast` is the caller's
/// event fan-out (hub broadcast / tool broadcaster).
pub fn notify_plugin_needs_reauth(
    store: &db::Store,
    broadcast: impl Fn(&str, serde_json::Value),
    p: &db::PluginAccountProfile,
) {
    // Fresh id per occurrence: the `reauth_notified` flag (reset on recovery) is
    // the once-per-spell guard, so a unique id lets a *future* expiry notify again
    // rather than being suppressed by a stale, already-read notification.
    let notif_id = uuid::Uuid::new_v4().to_string();
    let title = format!("Reconnect {}", p.account_label);
    let body = format!(
        "{}'s connection to {} expired. Reconnect it in the agent's Settings, Plugins.",
        p.account_label, p.plugin_slug
    );
    let action_url = format!("/{}/settings/accounts", p.agent_id);
    crate::owner_notify::emit(
        store,
        Some(&|ev, payload| broadcast(ev, payload)),
        &crate::owner_notify::OwnerNotification {
            id: &notif_id,
            kind: "warning",
            title: &title,
            body: Some(&body),
            action_url: Some(&action_url),
            agent_id: Some(p.agent_id.as_ref()),
            loud: false,
        },
    );
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

/// Whether a raw exec command invokes a bound operation's command — the bound
/// command exactly, or with additional arguments/flags after it. A binding may
/// be multi-word ("documents list"), so plain prefix matching would false-match
/// "documents listing"; the boundary must be end-of-string or whitespace.
fn command_matches_binding(command: &str, bound_cmd: &str) -> bool {
    match command.strip_prefix(bound_cmd) {
        Some(rest) => rest.is_empty() || rest.starts_with(char::is_whitespace),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill_set(names: &[&str]) -> std::collections::HashSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn exec_binding_match_requires_word_boundary() {
        assert!(command_matches_binding("ingest", "ingest"));
        assert!(command_matches_binding("ingest --limit 5", "ingest"));
        assert!(command_matches_binding("documents list --limit 2", "documents list"));
        // No boundary → not the bound command.
        assert!(!command_matches_binding("ingestion-report", "ingest"));
        assert!(!command_matches_binding("documents listing", "documents list"));
        assert!(!command_matches_binding("search foo", "ingest"));
    }

    #[test]
    fn skill_labels_never_invent_a_subcommand() {
        // GWS prefixes its skill dirs with its slug, and each really is a
        // subcommand — the `+` label is a fact, vouched for by the service
        // skill sitting next to the helper.
        let gws = skill_set(&["gws-gmail", "gws-gmail-triage", "gws-calendar", "gws-calendar-insert", "gws-auth"]);
        assert_eq!(display_command_for_skill("gws", "gws-gmail-triage", &gws), "gmail +triage");
        assert_eq!(display_command_for_skill("gws", "gws-calendar-insert", &gws), "calendar +insert");
        assert_eq!(display_command_for_skill("gws", "gws-auth", &gws), "auth");

        // A multi-word service is not a helper. google-calendar ships
        // `free-busy` with no `free` service, so the label must stay whole —
        // `free +busy` advertised a subcommand the binary does not have.
        let cal = skill_set(&["google-calendar-calendars", "google-calendar-free-busy", "google-calendar-shared"]);
        assert_eq!(
            display_command_for_skill("google-calendar", "google-calendar-free-busy", &cal),
            "free-busy"
        );

        // nebo-office does not prefix with its slug, and `pptx design` is not a
        // subcommand. Showing the raw name is the only honest option — the
        // `pptx +design` label this used to print sent an agent chasing a
        // command that never existed.
        let office = skill_set(&["pptx", "pptx-design", "pptx-shapes", "docx-tables", "xlsx-formulas"]);
        for name in ["pptx-design", "pptx-shapes", "docx-tables", "xlsx-formulas"] {
            assert_eq!(
                display_command_for_skill("nebo-office", name, &office),
                name,
                "a skill not prefixed with the plugin slug must be shown verbatim"
            );
        }
        assert_eq!(display_command_for_skill("nebo-office", "pptx", &office), "pptx");
    }

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
    fn test_port_department_and_capability_scope_resolution() {
        // The department is what disambiguates a shared operation across departments:
        // accounting.collections-specialist.mail.message.send and
        // customer-support.escalation-specialist.mail.message.send are the SAME operation
        // but must be able to resolve to different providers.
        assert_eq!(
            port_department("accounting.collections-specialist.mail.message.send").as_deref(),
            Some("accounting")
        );
        assert_eq!(
            port_department("customer-support.escalation-specialist.mail.message.send").as_deref(),
            Some("customer-support")
        );
        // Both target the same capability — hence the collision the department resolves.
        assert_eq!(port_capability("accounting.collections-specialist.mail.message.send"), "mail");
        assert_eq!(port_capability("customer-support.escalation-specialist.mail.message.send"), "mail");
        assert_eq!(port_capability("accounting.ap-specialist.ledger.bill.create"), "ledger");
        // A bare operation has no department (nothing to scope by).
        assert_eq!(port_department("mail.message.send"), None);
    }


    #[test]
    fn test_workflow_session_key_round_trips_to_agent_id() {
        // The workflow engine builds its session key with this constructor;
        // per-agent plugin account resolution must recover the id from it.
        // (The old dash format `agent-<id>-<run>` parsed to None — every
        // workflow run lost its account.)
        let key = crate::origin::workflow_session_key("abc-123", "run-9");
        assert_eq!(types::keyparser::extract_agent_id(&key), "abc-123");
        // Standalone (non-agent) runs carry no identity by design.
        assert_eq!(crate::origin::workflow_session_key("", "run-9"), "");
        assert_eq!(types::keyparser::extract_agent_id(""), "");
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

    #[test]
    fn test_produced_work_document_detects_fresh_output() {
        let dir = std::env::temp_dir().join(format!("nebo-pwd-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let deck = dir.join("out.pptx");
        let spec = dir.join("spec.json");
        std::fs::write(&spec, "{}").unwrap();
        let started = std::time::SystemTime::now();
        std::fs::write(&deck, "fake-deck").unwrap();

        let args: Vec<String> = vec![
            "pptx".into(),
            "create".into(),
            spec.to_string_lossy().into_owned(),
            "-o".into(),
            deck.to_string_lossy().into_owned(),
        ];
        // The fresh .pptx output is detected; the .json spec is not a work doc.
        assert_eq!(
            produced_work_document(&args, None, started),
            Some(deck.to_string_lossy().into_owned())
        );
        // Relative token resolves against base.
        let rel: Vec<String> = vec!["out.pptx".into()];
        assert_eq!(
            produced_work_document(&rel, Some(&dir), started),
            Some(deck.to_string_lossy().into_owned())
        );
        // A work doc that predates the run (an input) is excluded.
        let stale = started + std::time::Duration::from_secs(5);
        assert_eq!(produced_work_document(&args, None, stale), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod budget_and_install_tests {
    use super::*;

    fn stores(tmp: &std::path::Path) -> (Arc<napp::plugin::PluginStore>, Arc<db::Store>) {
        let installed = tmp.join("plugins");
        let user = tmp.join("user_plugins");
        std::fs::create_dir_all(&installed).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        let plugin_store = Arc::new(napp::plugin::PluginStore::new(installed, user, None));
        let db_store = Arc::new(db::Store::new(tmp.join("t.db").to_str().unwrap()).unwrap());
        (plugin_store, db_store)
    }

    /// A versioned install the store resolves: `<root>/<slug>/<version>/` with
    /// a manifest and one plain file that stands in for the binary.
    fn install_fake(root: &std::path::Path, slug: &str) {
        let version_dir = root.join("plugins").join(slug).join("0.1.0");
        std::fs::create_dir_all(&version_dir).unwrap();
        std::fs::write(
            version_dir.join("plugin.json"),
            serde_json::json!({"id": slug, "slug": slug, "name": slug, "version": "0.1.0", "platforms": {}}).to_string(),
        )
        .unwrap();
        std::fs::write(version_dir.join(slug), b"#!/bin/sh\necho ok\n").unwrap();
    }

    /// With nothing installed the resource property carries no enum at all
    /// (an empty enum makes every slug invalid), and the description says
    /// where a slug comes from.
    #[test]
    fn no_plugins_means_no_enum_and_a_pointer_to_list() {
        let tmp = tempfile::tempdir().unwrap();
        let (plugin_store, db_store) = stores(tmp.path());
        let tool = PluginTool::new(plugin_store, db_store);
        let resource = &tool.schema()["properties"]["resource"];
        assert!(resource.get("enum").is_none(), "{resource}");
        assert!(resource["description"].as_str().unwrap().contains("plugin(action: \"list\")"));
        let description = tool.description();
        assert!(description.contains("plugin(resource: \"<slug>\""), "{description}");
        assert!(description.contains("plugin(action: \"list\")"), "{description}");
        assert!(!description.contains("gws"), "{description}");

        install_fake(tmp.path(), "quickbooks");
        let resource = &tool.schema()["properties"]["resource"];
        assert_eq!(resource["enum"], serde_json::json!(["quickbooks"]));
        assert!(!tool.description().contains("resource: \"gws\""));
    }

    /// A best match that is already installed gets no install card: the
    /// result says so and points at the plugin, even in a run that could not
    /// show a card.
    #[tokio::test]
    async fn discover_does_not_offer_to_install_what_is_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let (plugin_store, db_store) = stores(tmp.path());
        let tool = PluginTool::new(plugin_store, db_store);
        let ctx = ToolContext::default();
        let products = vec![serde_json::json!({
            "name": "QuickBooks Online", "slug": "quickbooks", "code": "PLUG-ABCD-1234",
            "description": "Books", "type": "plugin"
        })];

        let offered = tool.offer("quickbooks", &ctx, &products, 1).await;
        assert!(offered.content.contains("Installing needs the owner's approval"), "{}", offered.content);

        install_fake(tmp.path(), "quickbooks");
        let known = tool.offer("quickbooks", &ctx, &products, 1).await;
        assert!(!known.is_error, "{}", known.content);
        assert!(known.content.contains("QuickBooks Online was already installed"), "{}", known.content);
        assert!(known.content.contains("plugin(resource: \"quickbooks\""), "{}", known.content);
        assert!(!known.content.contains("Install it on the card"), "{}", known.content);
        assert!(!known.content.contains("owner's approval"), "{}", known.content);
    }

    /// Every recovery step gets what is left of the one exec budget, a step
    /// with less than the minimum left is skipped and named, and a step that
    /// ran out is named with the time it was given.
    #[test]
    fn the_exec_budget_hands_each_step_the_time_that_remains() {
        let start = std::time::Instant::now();
        let budget = ExecBudget { started: start, total: Duration::from_secs(120) };
        let at = |secs: u64| start + Duration::from_secs(secs);

        assert_eq!(budget.remaining_at(at(0)), Duration::from_secs(120));
        assert_eq!(budget.remaining_at(at(100)), Duration::from_secs(20));
        assert_eq!(budget.remaining_at(at(500)), Duration::ZERO);

        assert_eq!(budget.step_at(at(100), "doctor", "the auth status check"), Ok(Duration::from_secs(20)));
        assert_eq!(budget.step_at(at(110), "doctor", "the auth status check"), Ok(RECOVERY_MIN_REMAINING));
        let skipped = budget.step_at(at(115), "doctor", "the auth status check").unwrap_err();
        assert_eq!(
            skipped,
            "doctor finished; the auth status check was skipped because only 5 s of the 120 s exec budget remained."
        );
        assert_eq!(
            budget.ran_out("doctor", "the auth status check", Duration::from_secs(12)),
            "doctor finished; the auth status check did not answer within the remaining 12 s of the 120 s exec budget."
        );

        let named: PluginInput =
            serde_json::from_value(serde_json::json!({"command": "doctor", "timeout": 45})).unwrap();
        assert_eq!(PluginTool::exec_timeout(&named), Duration::from_secs(45));
        assert_eq!(PluginTool::command_label(&named), "doctor");
        let bare: PluginInput = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(PluginTool::exec_timeout(&bare), Duration::from_secs(EXEC_TIMEOUT_DEFAULT_SECS));
        assert_eq!(PluginTool::command_label(&bare), "the command");
    }

    /// The bound is real: a step that outlives what is left is cut off with
    /// the text that names it, and the plugin's own answer stays attached.
    #[tokio::test]
    async fn a_step_that_outlives_the_budget_is_cut_off_and_named() {
        let budget = ExecBudget { started: std::time::Instant::now(), total: Duration::from_secs(120) };
        // The cut with a short allowance, so the test takes milliseconds.
        let err = cut_off(&budget, "doctor", "the silent token refresh", Duration::from_millis(20), async {
            tokio::time::sleep(Duration::from_secs(60)).await;
        })
        .await
        .unwrap_err();
        assert_eq!(err, "doctor finished; the silent token refresh did not answer within the remaining 0 s of the 120 s exec budget.");
        // Too little left: skipped before the future is even polled.
        let spent = ExecBudget { started: std::time::Instant::now() - Duration::from_secs(115), total: Duration::from_secs(120) };
        let skipped = bounded(&spent, "doctor", "the browser login", async { unreachable!("not started") }).await.unwrap_err();
        assert!(skipped.contains("the browser login was skipped"), "{skipped}");
        let original = ToolResult::error("Not authenticated");
        let shown = out_of_time(err, &original);
        assert!(shown.is_error);
        assert!(shown.content.ends_with("The command's own result:\nNot authenticated"), "{}", shown.content);
        let quick = bounded(&budget, "doctor", "x", async { 7 }).await;
        assert_eq!(quick, Ok(7));
    }
}
