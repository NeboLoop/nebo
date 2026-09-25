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
use crate::registry::ToolResult;

/// The exec budget when the call names no `timeout`.
const EXEC_TIMEOUT_DEFAULT_SECS: u64 = 120;

/// A recovery step (auth probe, refresh, login, retry) is not started with
/// less than this left of the exec budget: it could not finish, and the wait
/// would be read as the plugin timing out.
const RECOVERY_MIN_REMAINING: Duration = Duration::from_secs(10);

/// The install card's answer once the plugin is on disk.
/// What an install/hire card submits once POST /codes has succeeded. Shared
/// with the employee hire card so both resume the same way.
pub const INSTALL_CARD_INSTALLED: &str = "installed";

/// What a card submits when its action failed: `failed:<the error the owner
/// saw>`. Live (2026-09-24): the install card showed the error but answered
/// nothing, the run stayed parked until the owner stopped it, and the tool
/// then called that "declined" — so the model offered the broken plugin
/// eight more times.
pub const CARD_FAILED_PREFIX: &str = "failed:";

/// How a card (install, hire, connect) ended, read from the parked ask's
/// answer. `done` is the value the card sends on success.
#[derive(Debug, PartialEq, Eq)]
pub enum CardAnswer<'a> {
    Done,
    /// The action ran and failed; the reason is the error the owner saw.
    Failed(&'a str),
    /// The owner dismissed the card.
    Skipped,
    /// No answer: the run was stopped, or there was no channel to ask on.
    NoAnswer,
}

impl<'a> CardAnswer<'a> {
    pub fn read(answer: Option<&'a str>, done: &str) -> Self {
        match answer {
            None => Self::NoAnswer,
            Some(v) if v == done => Self::Done,
            Some(v) => match v.strip_prefix(CARD_FAILED_PREFIX) {
                Some(reason) => Self::Failed(reason.trim()),
                None => Self::Skipped,
            },
        }
    }
}

/// The install card a discover call parks on: the ONE producer of the
/// `install_plugin` widget, so [`install_card_plugin`] reads the shape it
/// writes.
pub fn install_card_widget(code: &str, name: &str, slug: &str, description: &str) -> serde_json::Value {
    serde_json::json!([{
        "type": "install_plugin",
        "code": code,
        "name": name,
        "plugin": slug,
        "description": description,
    }])
}

/// The plugin slug a parked question's widgets offer to install, when the
/// question is an install card. An install that lands by any other door (a
/// pasted code, the marketplace, a hub push) answers that card for it.
pub fn install_card_plugin(widgets: &serde_json::Value) -> Option<&str> {
    widgets
        .as_array()?
        .iter()
        .find(|w| w.get("type").and_then(|t| t.as_str()) == Some("install_plugin"))?
        .get("plugin")?
        .as_str()
        .filter(|slug| !slug.is_empty())
}

/// The listing a query most plausibly names. The marketplace ranks by
/// relevance, but a query that IS a listing's name must beat one that merely
/// mentions it — "gmail" kept carding the deprecated Google Workspace bundle
/// and the user could never install the thing they named. Exact name or slug
/// first, then a prefix, then the top result.
pub(crate) fn best_match<'a>(items: &'a [serde_json::Value], query: &str) -> &'a serde_json::Value {
    let q = query.trim().to_lowercase();
    let field = |it: &serde_json::Value, k: &str| {
        it.get(k).and_then(|x| x.as_str()).unwrap_or("").to_lowercase()
    };
    items
        .iter()
        .find(|it| !q.is_empty() && (field(it, "name") == q || field(it, "slug") == q))
        .or_else(|| {
            items.iter().find(|it| {
                !q.is_empty() && (field(it, "name").starts_with(&q) || field(it, "slug").starts_with(&q))
            })
        })
        .unwrap_or(&items[0])
}

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

/// Runs installed plugin binaries: the handlers behind the plugin tools
/// (`plugin_tools`) and the operations plugins provide (`operation_tools`).
///
/// Plugins ship with their own skills (`skills/` directory inside the plugin).
/// These skills are the plugin's documentation — they describe the CLI syntax,
/// flags, and examples. The skill loader indexes them like any other skill;
/// the plugin's tool only names them.
///
/// When a plugin command fails due to stale OAuth credentials, the runner
/// automatically detects the auth failure and self-heals: first a SILENT
/// token renewal via the manifest's `auth.commands.refresh` (when declared),
/// and only then — in interactive chat — browser re-authentication via the
/// plugin's `auth login` command, retrying the original command on success.
/// Unattended runs (workflow/channel/schedule) never block on interactive
/// login: the account is flagged `needs_reauth` and the turn ends.
pub struct PluginRunner {
    plugin_store: Arc<napp::plugin::PluginStore>,
    db_store: Arc<db::Store>,
    broadcaster: Option<crate::web_tool::Broadcaster>,
}

/// One command run against an installed plugin.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct PluginCall {
    /// The installed plugin's slug.
    #[serde(default)]
    pub(crate) slug: String,
    /// Subcommand and flags passed to the plugin binary.
    #[serde(default)]
    pub(crate) command: String,
    /// Named flags passed directly to the binary without shell parsing.
    /// Each key becomes --key and the value is passed as a separate OS arg.
    #[serde(default)]
    pub(crate) args: std::collections::HashMap<String, String>,
    /// Timeout in seconds (default: 120).
    #[serde(default)]
    pub(crate) timeout: i64,
}

/// The id a typed operation's input or result names a record by.
pub fn record_id(v: &serde_json::Value) -> Option<String> {
    match v.get("id")? {
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
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

/// Every plugin the owner has installed and not disabled — what EXISTS.
/// This is the model-facing set: the tool description, the `resource` enum,
/// and the "not installed" error all read from it, so a plugin that is
/// installed but not yet connected is still nameable and still readable
/// through its skills. (2026-09-15: readiness gated this, the auth cache is
/// only warmed by a successful exec, and a freshly started Nebo therefore
/// told the model "No plugins are installed yet" with seven installed.)
pub fn installed_plugin_slugs(
    plugin_store: &napp::plugin::PluginStore,
    db_store: &db::Store,
) -> Vec<String> {
    let mut slugs = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (slug, _, _, _) in &plugin_store.list_installed() {
        if !seen.insert(slug.clone()) {
            continue;
        }
        if let Ok(Some(row)) = db_store.get_plugin_by_slug(slug) {
            if row.is_enabled == 0 {
                continue;
            }
        }
        slugs.push(slug.clone());
    }
    slugs
}

/// The plugins that can RUN right now: installed, not disabled, and ready
/// (credentials and required config present). Typed ports read this — an
/// unconnected provider must not claim a port a local fallback can serve.
pub fn active_plugin_slugs(plugin_store: &napp::plugin::PluginStore, db_store: &db::Store) -> Vec<String> {
    let installed = plugin_store.list_installed();
    let mut seen = std::collections::HashSet::new();
    let mut slugs = Vec::new();
    for (slug, _, _, _) in &installed {
        if !seen.insert(slug.clone()) {
            continue;
        }
        if let Ok(Some(row)) = db_store.get_plugin_by_slug(slug) {
            if row.is_enabled == 0 {
                continue;
            }
        }
        if !plugin_store.is_ready(slug) {
            continue;
        }
        slugs.push(slug.clone());
    }
    slugs
}

/// The active plugins that bind a typed operation (`mail.message.send`),
/// by its `capability.resource.action` suffix. Empty: the port has no
/// provider, and a local fallback (the desktop mail app) is the way to
/// send. Non-empty: the port is the way, and the fallback steps aside.
pub fn bound_providers(plugin_store: &napp::plugin::PluginStore, db_store: &db::Store, operation: &str) -> Vec<String> {
    let suffix = port_suffix(operation);
    active_plugin_slugs(plugin_store, db_store)
        .into_iter()
        .filter(|slug| plugin_store.get_manifest(slug).is_some_and(|m| m.interface_bindings.contains_key(&suffix)))
        .collect()
}

/// The one sentence that sends a reader to the tool catalog's door: an
/// employee hires on a card, a tool installs through plugin discover. Said
/// once here and used wherever it is still needed — the same redirect used to
/// be written out in a dozen places, and had already drifted (2026-09-19).
pub const TOOL_INSTALL_DOOR: &str =
    "A tool, connection or service installs through find_plugins.";

impl PluginRunner {
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

    pub(crate) fn plugin_store(&self) -> &napp::plugin::PluginStore {
        &self.plugin_store
    }

    /// Build a deduplicated list of active plugin slugs (installed + not disabled + ready).
    pub(crate) fn active_slugs(&self) -> Vec<String> {
        active_plugin_slugs(&self.plugin_store, &self.db_store)
    }

    /// What exists — see `installed_plugin_slugs`.
    pub(crate) fn installed_slugs(&self) -> Vec<String> {
        installed_plugin_slugs(&self.plugin_store, &self.db_store)
    }

    /// The gated interface operation a raw exec command corresponds to, if any.
    ///
    /// Matches the command's leading tokens against the plugin's declared
    /// `interfaceBindings` values (a binding command may be multi-word, e.g.
    /// `documents list`), and returns the operation only when the catalog marks
    /// it gated — ungated reads stay runnable through exec.
    /// The gated operation a raw command IS, with the binding template that
    /// says how the typed call is shaped (`{field}` placeholders name its
    /// input fields).
    fn gated_operation_for_command(&self, slug: &str, command: &str) -> Option<(String, String)> {
        let manifest = self.plugin_store.get_manifest(slug)?;
        let command = command.trim();
        for (op, bound_cmd) in &manifest.interface_bindings {
            if command_matches_binding(command, bound_cmd) && crate::interface_catalog::is_gated(op)
            {
                return Some((op.clone(), bound_cmd.clone()));
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

    pub(crate) async fn handle_discover(&self, query: &str, ctx: &crate::ToolContext) -> ToolResult {
        let api = match crate::build_neboai_api(&self.db_store) {
            Ok(a) => a,
            Err(e) => return ToolResult::error(format!("marketplace unavailable: {}", e)),
        };
        let q = if query.trim().is_empty() {
            None
        } else {
            Some(query.trim())
        };
        // A query that names an installed plugin has nothing to discover. Live
        // (2026-09-05): "quickbooks connect" searched the marketplace, found
        // nothing, and the model concluded the plugin needed installing.
        let words: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
            .map(|w| w.to_ascii_lowercase())
            .collect();
        if let Some((slug, version, _, _)) = self
            .plugin_store
            .list_installed()
            .into_iter()
            .find(|(slug, ..)| words.iter().any(|w| w == &slug.to_ascii_lowercase()))
        {
            return ToolResult::ok(format!(
                "{slug} v{version} was already installed; nothing to discover or install. \
                 Its tool is {tool}: its description names the skills that document its \
                 commands (read one with use_skill). If a result says no account is \
                 connected, the owner connects one in Settings, Plugins; there is no \
                 command for that.",
                tool = crate::plugin_tools::plugin_tool_name(&slug)
            ));
        }
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
                        // An empty query is browsing, not asking for a particular tool:
                        // there is nothing for best_match to match, so a card here offers
                        // whatever the hub returned first. Live (2026-09-15): `discover ""`
                        // carded the owner's own retired Google Workspace plugin — whose
                        // description opens "[DEPRECATED — do not install]" — because a
                        // publisher sees their own private listings. Browsing lists; naming
                        // a tool cards it.
                        let interactive = crate::origin::ExecutionMode::from(ctx.origin)
                            == crate::origin::ExecutionMode::Interactive
                            && ctx.ask_channels.is_some()
                            && !query.trim().is_empty();
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
                        let top = best_match(arr, query);
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
                                    install_card_widget(top_code, top_name, top_slug, top_desc),
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
                                        return ToolResult::ok(match CardAnswer::read(
                                            connected.as_deref(),
                                            "connected",
                                        ) {
                                            CardAnswer::Done => format!(
                                                "{top_name} {state} and its account is \
                                                 connected. Continue the task NOW with \
                                                 {tool} (load it with find_tools) — no setup \
                                                 narration.",
                                                tool = crate::plugin_tools::plugin_tool_name(top_slug)
                                            ),
                                            CardAnswer::Failed(reason) => format!(
                                                "{top_name} {state}, but connecting the \
                                                 {label} FAILED: {reason}. Tell the owner that \
                                                 error in plain words and stop. Do NOT offer \
                                                 the card again and do NOT suggest commands."
                                            ),
                                            CardAnswer::Skipped => format!(
                                                "{top_name} {state}; the owner skipped \
                                                 connecting the {label}. The connect card \
                                                 re-appears on first use — continue, or ask \
                                                 what they'd like to do."
                                            ),
                                            CardAnswer::NoAnswer => format!(
                                                "{top_name} {state}; the {label} was not \
                                                 connected — no answer (the owner stopped the \
                                                 run). Do NOT offer the card again."
                                            ),
                                        });
                                    }
                                }
                                return ToolResult::ok(format!(
                                    "{top_name} {state}. Use it through {tool} (load it with \
                                     find_tools). If it needs an account, the connect card will \
                                     appear on first use — no setup narration needed.",
                                    tool = crate::plugin_tools::plugin_tool_name(top_slug)
                                ));
                            }
                            // Not installed: say exactly why. Only a skip keeps
                            // the listing, so the conversation can move on to
                            // other options.
                            return ToolResult::ok(
                                match CardAnswer::read(answer.as_deref(), INSTALL_CARD_INSTALLED) {
                                    CardAnswer::Failed(reason) => format!(
                                        "Installing {top_name} FAILED: {reason}. Tell the owner \
                                         that error in plain words and stop. Do NOT offer the \
                                         card again and do NOT suggest commands or other ways \
                                         to install it."
                                    ),
                                    CardAnswer::NoAnswer => format!(
                                        "The install card for {top_name} got no answer (the \
                                         owner stopped the run). Do NOT offer it again."
                                    ),
                                    CardAnswer::Skipped | CardAnswer::Done => format!(
                                        "{listing}\n\nThe owner skipped the install card for \
                                         {top_name}. Do NOT offer it again unless they ask. \
                                         Never paste install codes into chat."
                                    ),
                                },
                            );
                        }
                        if interactive {
                            // Interactive but no usable card (top match had no
                            // code): never fall back to narrating codes.
                            ToolResult::ok(format!(
                                "{listing}\n\nAsk the user which one they want, then call \
                                 find_plugins again with its exact name to offer the install card. \
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

    /// The plugin's skills (name, description), from its `skills/` directory.
    pub(crate) fn list_services(&self, slug: &str) -> Vec<(String, String)> {
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

impl PluginRunner {
    /// Perform a catalog operation the plugin `slug` binds. The binding
    /// shapes the command: a template's placeholders take their fields, and
    /// fields it does not name go on as `--key value` flags. The seat's
    /// `clientKey` is the runtime's, never a flag: the same write under one
    /// key runs once (`effects::guarded_write`).
    pub(crate) async fn perform_operation(
        &self,
        ctx: &ToolContext,
        slug: &str,
        operation: &str,
        mut input: serde_json::Value,
    ) -> ToolResult {
        let Some(binding) = self
            .plugin_store
            .get_manifest(slug)
            .and_then(|m| m.interface_bindings.get(operation).cloned())
        else {
            return ToolResult::error(format!("{slug} no longer performs {operation}."));
        };
        let client_key = take_client_key(&mut input);
        let (command, consumed) = match render_binding(operation, &binding, &input) {
            Ok(x) => x,
            Err(e) => return ToolResult::error(e),
        };
        let mut args = std::collections::HashMap::new();
        if let serde_json::Value::Object(map) = &input {
            for (k, v) in map {
                if consumed.contains(k) || v.is_null() {
                    continue;
                }
                let value = match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                args.insert(k.clone(), value);
            }
        }
        let call = PluginCall { slug: slug.to_string(), command, args, timeout: 0 };
        match client_key {
            Some(key) => {
                crate::effects::guarded_write(&self.db_store, ctx, slug, operation, &key, || {
                    self.run_bound(ctx, &call, operation, &input)
                })
                .await
            }
            None => self.run_bound(ctx, &call, operation, &input).await,
        }
    }

    /// Run a bound operation's command.
    ///
    /// A customer-facing send goes through the effect ledger: recorded
    /// before it goes, never sent twice for the same input in one run, held
    /// when the outcome is unknown. The plugin vouches for the outcome with
    /// a typed report on stdout (see `SendOutcome::from_plugin_output`); a
    /// plugin that reports nothing typed leaves the send unknown, which
    /// holds it — the words in an error are never the verdict.
    async fn run_bound(
        &self,
        ctx: &ToolContext,
        call: &PluginCall,
        operation: &str,
        input: &serde_json::Value,
    ) -> ToolResult {
        if crate::effects::is_customer_send(operation) {
            return crate::effects::guarded_send(&self.db_store, ctx, "messaging", &call.slug, operation, input, || async {
                let r = self.handle_exec(call, ctx).await;
                crate::effects::SendOutcome::from_plugin_output(&r.content)
            })
            .await;
        }
        self.handle_exec(call, ctx).await
    }

    /// Run a command on an installed plugin (the `plugin__<slug>` tool). A
    /// command that IS a gated bound operation is refused and pointed at its
    /// operation tool: raw exec must not be a side door around the owner's
    /// rules for that operation.
    pub(crate) async fn run_command(&self, ctx: &ToolContext, call: &PluginCall) -> ToolResult {
        // The refusal carries the binding: a gate run (2026-09-23) showed the
        // model retrying the identical exec three times because it had no
        // field names to fill.
        if let Some((op, template)) = self.gated_operation_for_command(&call.slug, &call.command) {
            return ToolResult::error(format!(
                "'{}' on {} is the operation {op}, which the owner's approval rules cover. \
                 Call {} instead (load it with find_tools) — do not retry it here. The \
                 binding is `{template}`: each {{field}} is a parameter of that tool, and \
                 fields the binding does not name are passed on as --key value flags.",
                call.command,
                call.slug,
                crate::operation_tools::operation_tool_name(&op)
            ));
        }
        self.handle_exec(call, ctx).await
    }

    pub(crate) fn handle_events(&self, slug: &str) -> ToolResult {
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
                    "\nAn employee can run on these through a watch trigger in its agent.json \
                     (create_employee with agent_json):\n  \
                       {{\"workflows\": {{\"<name>\": {{\"trigger\": {{\"type\": \"watch\", \"plugin\": \"{}\", \"event\": \"<event-name>\"}}, \"activities\": [...]}}}}}}",
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

    async fn handle_exec(&self, pi: &PluginCall, ctx: &ToolContext) -> ToolResult {
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
            if let Some((binary, auth)) = self.plugin_store.get_auth_info(&pi.slug) {
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
                                    &pi.slug,
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

                    // Does an account exist for this employee? Only a
                    // profile-dir plugin keeps a row per employee; for a
                    // single-account plugin we cannot tell "expired" from
                    // "never connected" from here, and `None` is the honest
                    // answer — the wording below says so rather than guessing.
                    let had_account: Option<bool> =
                        auth.profile_dir_env.as_ref().map(|_| profile.is_some());

                    // Confirm with a fresh auth-status check (the one canonical
                    // decision, via PluginStore) if the command is available.
                    if auth.commands.status.is_some() {
                        match bounded(&budget, &command_label, "the auth status check", self.probe_auth(&pi.slug, probe_dir)).await {
                            // Status says authenticated — false positive, return original error
                            Ok(Some(true)) => return result,
                            Ok(_) => {}
                            Err(text) => return out_of_time(text, &result),
                        }
                    }

                    info!(plugin = %pi.slug, "auth failure detected");

                    // FIRST: silent, non-interactive token renewal when the
                    // manifest declares a refresh command. No user interruption,
                    // no browser — renew, re-probe, retry.
                    if auth.commands.refresh.is_some() {
                        if let Err(text) = bounded(&budget, &command_label, "the silent token refresh", self.plugin_store.run_auth_refresh(&pi.slug, probe_dir)).await {
                            return out_of_time(text, &result);
                        }
                        match bounded(&budget, &command_label, "the auth status check after the refresh", self.probe_auth(&pi.slug, probe_dir)).await {
                            Ok(Some(true)) => {
                                info!(plugin = %pi.slug, "silent token refresh healed auth, retrying command");
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
                        warn!(plugin = %pi.slug, "auth expired in unattended run; silent refresh failed");
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
                                    "plugin": &pi.slug,
                                    "error": "Authentication expired and silent refresh failed",
                                }),
                            );
                        }
                        // Say what actually happened. "Expired and renewal
                        // failed" sent the owner hunting a broken refresh for a
                        // plugin that was never connected and declares no
                        // refresh command at all (meta-marketing, 2026-09-15).
                        // No account at all is the owner's to connect: say
                        // so as data. An expired one is the reconnect notice's.
                        let need = (had_account == Some(false))
                            .then(|| types::OwnerNeed::Account { plugin: pi.slug.clone() });
                        let refused = ToolResult::terminal(match (had_account, auth.commands.refresh.is_some()) {
                            (Some(false), _) => format!(
                                "I couldn't reach **{}** — no account is connected for this \
                                 employee. Connect one in the employee's Settings, Plugins, \
                                 then ask me again.",
                                pi.slug
                            ),
                            (Some(true), true) => format!(
                                "I couldn't reach **{}** — its authentication expired and \
                                 automatic renewal didn't work. Please reconnect this account in \
                                 the employee's Settings, Plugins, then ask me again.",
                                pi.slug
                            ),
                            (Some(true), false) => format!(
                                "I couldn't reach **{}** — its authentication expired, and this \
                                 plugin cannot renew itself. Please reconnect this account in the \
                                 employee's Settings, Plugins, then ask me again.",
                                pi.slug
                            ),
                            (None, _) => format!(
                                "I couldn't reach **{}** — it has no working sign-in: either it \
                                 was never connected, or its credentials stopped working. Connect \
                                 it in Settings, Plugins, then ask me again.",
                                pi.slug
                            ),
                        });
                        return ToolResult { need, ..refused };
                    }

                    // A plugin whose account is entered in Nebo's own dialog
                    // (auth type env) has no browser sign-in to fall through
                    // to: its `auth login` takes the fields from the
                    // environment, and run without them it serves a local
                    // form and waits for minutes. Say where the account is
                    // connected and end the turn.
                    if auth.auth_type == "env" {
                        return ToolResult::terminal(format!(
                            "I couldn't reach **{}** — no working account is connected for this \
                             employee. Connect one in the employee's Settings, Plugins, then ask \
                             me again.",
                            pi.slug
                        ))
                        .with_need(types::OwnerNeed::Account { plugin: pi.slug.clone() });
                    }

                    // Interactive chat: fall through to today's browser OAuth path.
                    // Broadcast re-auth request so frontend can show a notification
                    if let Some(ref bc) = self.broadcaster {
                        bc(
                            "plugin_reauth_request",
                            serde_json::json!({
                                "plugin": &pi.slug,
                                "label": &auth.label,
                            }),
                        );
                    }

                    // Attempt re-auth via plugin's auth login command
                    let login_time = match budget.step(&command_label, "the browser login") {
                        Ok(given) => given,
                        Err(text) => return out_of_time(text, &result),
                    };
                    if self.run_auth_login(&pi.slug, &binary, &auth, login_time).await {
                        info!(plugin = %pi.slug, "re-authentication succeeded, retrying command");

                        // Broadcast success
                        if let Some(ref bc) = self.broadcaster {
                            bc(
                                "plugin_auth_complete",
                                serde_json::json!({ "plugin": &pi.slug }),
                            );
                        }

                        return match budget.step(&command_label, "the retry after the login") {
                            Ok(given) => self.run_plugin_command(pi, ctx, given).await,
                            Err(text) => out_of_time(text, &result),
                        };
                    }

                    // Re-auth failed
                    warn!(plugin = %pi.slug, "re-authentication failed");
                    if let Some(ref bc) = self.broadcaster {
                        bc(
                            "plugin_auth_error",
                            serde_json::json!({
                                "plugin": &pi.slug,
                                "error": "Re-authentication failed or timed out",
                            }),
                        );
                    }

                    // Terminal: auth genuinely expired and reauth failed. End the
                    // turn and surface to the user — do not let the agent keep
                    // retrying/improvising (FRAMES.md Phase 1).
                    let need = (had_account == Some(false))
                        .then(|| types::OwnerNeed::Account { plugin: pi.slug.clone() });
                    let refused = ToolResult::terminal(match had_account {
                        Some(true) => format!(
                            "I couldn't reach **{}** — its account is no longer authenticated and \
                             signing in again didn't work. Please reconnect it in the employee's \
                             Settings, Plugins, then ask me again.",
                            pi.slug
                        ),
                        Some(false) => format!(
                            "I couldn't reach **{}** — no account is connected for this employee, \
                             and signing in didn't complete. Connect one in the employee's \
                             Settings, Plugins, then ask me again.",
                            pi.slug
                        ),
                        None => format!(
                            "I couldn't reach **{}** — it has no working sign-in, and signing in \
                             didn't complete. Connect it in Settings, Plugins, then ask me again.",
                            pi.slug
                        ),
                    });
                    return ToolResult { need, ..refused };
                }
            }
        }

        result
    }

    /// The exec budget a call asked for, or the default.
    fn exec_timeout(pi: &PluginCall) -> Duration {
        if pi.timeout > 0 {
            Duration::from_secs(pi.timeout as u64)
        } else {
            Duration::from_secs(EXEC_TIMEOUT_DEFAULT_SECS)
        }
    }

    /// How a budget message names the command that ran.
    fn command_label(pi: &PluginCall) -> String {
        if pi.command.is_empty() {
            "the command".to_string()
        } else {
            pi.command.clone()
        }
    }

    /// Execute a plugin command and return the result. Shared by initial call
    /// and retry; `timeout` is what is left of the exec budget.
    async fn run_plugin_command(&self, pi: &PluginCall, ctx: &ToolContext, timeout: Duration) -> ToolResult {
        if pi.command.is_empty() && pi.args.is_empty() {
            return ToolResult::error(
                "command is required: the subcommand and flags, as the plugin's skills document them (read one with use_skill).",
            );
        }

        // Resolve binary path
        let binary_path = match self.plugin_store.resolve(&pi.slug, "*") {
            Some(p) => p,
            None => {
                let slugs = self.installed_slugs();
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
                    pi.slug, available, disabled_desc
                ));
            }
        };

        debug!(
            plugin = %pi.slug,
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

        // A plugin is executed directly, never through a shell, so a shell
        // operator arrives at the binary as a positional argument and its
        // parser refuses it ("error: unexpected argument '|' found", shopify
        // 2026-09-16). The model reads that as a bad query and starts guessing
        // at the command instead of dropping the pipe. Say what happened.
        if let Some(op) = shell_operator(&args) {
            return ToolResult::error(format!(
                "`{op}` is a shell operator and `{}` runs directly, with no shell — so `{op}` \
                 was handed to it as an argument and it refused. Run the command without it: the \
                 whole output comes back here for you to read. To get less back, narrow the \
                 command itself (a filter, a smaller query); there is no pipe to filter through.",
                pi.slug
            ));
        }

        // Forgive a leading plugin-name token. Models often prefix the plugin
        // slug (e.g. `gws calendar events list`); the binary expects a service
        // first (`calendar events list`), so a leading `gws` makes it see
        // service "gws" → "Unknown service 'gws'". Drop it so both forms work.
        if args.first().map(|a| a.eq_ignore_ascii_case(&pi.slug)) == Some(true) {
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
                        pi.slug
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
            .get_manifest(&pi.slug)
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
                            &pi.slug,
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
                                    .list_plugin_account_profiles(a, &pi.slug)
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
                                res = pi.slug,
                                labels = connected.join(", ")
                            ));
                        }
                        let none_msg = format!(
                            "No {res} account is connected for this agent. Connect one in \
                             this agent's Settings, Plugins before using {res}.",
                            res = pi.slug
                        );
                        // doctor and help are how a model checks the state; the
                        // state is the answer, not an error to recover from.
                        let first = pi.command.split_whitespace().next().unwrap_or("");
                        if first == "doctor" || first == "help" || pi.command.contains("--help") {
                            return ToolResult::ok(format!(
                                "{res} {first}: not connected. {none_msg} Nothing else to \
                                 diagnose until then.",
                                res = pi.slug
                            ));
                        }
                        // Nothing connected. Interactive chat renders an inline
                        // connect card via ask_user, which parks THIS tool call
                        // until the account is connected — the run then resumes
                        // at the same call.
                        let interactive = crate::origin::ExecutionMode::from(ctx.origin)
                            == crate::origin::ExecutionMode::Interactive
                            && ctx.ask_channels.is_some();
                        let Some(agent_id) = agent_id.as_deref() else {
                            return ToolResult::error(none_msg);
                        };
                        if !interactive {
                            // Unattended: the error steers to what IS connected
                            // for this employee, and the turn goes on. Seen live:
                            // an employee with gmail connected called
                            // google-workspace first, the terminal error ended
                            // the turn, and the lead was never answered. When
                            // nothing at all is connected there is nothing to
                            // steer to, and the run stops cleanly rather than
                            // improvising around the failure.
                            let mut connected: Vec<String> = self
                                .db_store
                                .list_all_plugin_account_profiles_for_agent(agent_id)
                                .unwrap_or_default()
                                .into_iter()
                                .map(|p| p.plugin_slug)
                                .filter(|s| s != &pi.slug)
                                .collect();
                            connected.sort();
                            connected.dedup();
                            let ports: Vec<String> = self
                                .bound_operations()
                                .into_iter()
                                .filter(|(_, slug)| connected.contains(slug))
                                .map(|(op, slug)| {
                                    format!("{} (via {slug})", crate::operation_tools::operation_tool_name(&op))
                                })
                                .collect();
                            if connected.is_empty() {
                                return ToolResult::terminal(none_msg)
                                    .with_need(types::OwnerNeed::Account { plugin: pi.slug.clone() });
                            }
                            let mut msg = format!(
                                "{none_msg} Connected for this employee: {}.",
                                connected.join(", ")
                            );
                            if !ports.is_empty() {
                                msg.push_str(&format!(
                                    " Operation tools those serve: {}.",
                                    ports.join(", ")
                                ));
                            }
                            msg.push_str(
                                " Do the work with what is connected; if it cannot be done without \
                                 this account, exit the turn saying so instead of retrying it.",
                            );
                            return ToolResult::error(msg);
                        }
                        let display_label = self
                            .plugin_store
                            .get_manifest(&pi.slug)
                            .and_then(|m| m.auth)
                            .map(|a| a.label)
                            .filter(|l| !l.is_empty())
                            .unwrap_or_else(|| pi.slug.clone());
                        let answer = ctx
                            .ask_user(
                                &format!(
                                    "I need your {display_label} connected to continue. \
                                     Connect it on the card and I'll pick up right where I \
                                     left off."
                                ),
                                Self::connect_account_widget(
                                    &pi.slug,
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
                                &pi.slug,
                                selected_account.as_deref(),
                            )
                            .ok()
                            .flatten()
                        {
                            Some(p) => Some((env_name, p.config_dir)),
                            None => {
                                return ToolResult::error(format!(
                                    "The {res} account didn't finish connecting. {none_msg}",
                                    res = pi.slug
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
            &pi.slug,
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
                pi.slug,
                timeout.as_secs()
            )),
            Err(e) => ToolResult::error(format!("Plugin '{}' command failed: {}", pi.slug, e)),
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
                        pi.slug, how, text
                    ));
                }

                if text.is_empty() {
                    text = "(command exited 0 with no stdout or stderr)".to_string();
                }

                // Truncate very long output (char-boundary safe). Say what the
                // cut means and what to do, because "truncated" alone reads as
                // a transient failure: a store manager re-ran the same 98 KB
                // schema dump eight times (2026-09-16), getting the same half a
                // JSON document each time, until the spiral guard stopped it.
                if text.len() > crate::MAX_SUBPROCESS_OUTPUT {
                    let total = text.len();
                    types::strutil::safe_truncate(&mut text, crate::MAX_SUBPROCESS_OUTPUT);
                    text.push_str(&format!(
                        "\n\n[Cut off: this is the first {} bytes of {}. The rest is gone, so any \
                         JSON here ends mid-structure and cannot be parsed. Running it again returns \
                         the same first {} bytes — ask a narrower question instead: one record \
                         rather than a list, one type rather than a whole schema, a smaller page \
                         size, or a filter.]",
                        crate::MAX_SUBPROCESS_OUTPUT, total, crate::MAX_SUBPROCESS_OUTPUT
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
        pi: &PluginCall,
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
                pi.slug
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

        let key = channel_bridge::channel_bridge_key(&agent_id, &pi.slug);
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
                pi.slug, agent_id
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
                    pi.slug
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
                pi.slug, agent_id
            ));
        }

        info!(
            plugin = %pi.slug,
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
                pi.slug, agent_id, req_id
            )),
            Ok(Ok(res)) => ToolResult::error(format!(
                "Op `{op}` on plugin `{}` failed: {}",
                pi.slug,
                res.error.unwrap_or_else(|| "unknown error".into())
            )),
            Ok(Err(_)) => ToolResult::error(format!(
                "Bridge for plugin `{}` (agent `{}`) closed before reporting \
                 the result of `{op}`. The op may or may not have run on the \
                 platform — check the channel for evidence and retry if needed.",
                pi.slug, agent_id
            )),
            Err(_) => {
                handle.pending_ops.lock().await.remove(&req_id);
                ToolResult::error(format!(
                    "Op `{op}` on plugin `{}` timed out after 30s without a \
                     result from the bridge. The op may still complete \
                     asynchronously, but its outcome is unknown.",
                    pi.slug
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

/// The first shell operator sitting among parsed args, if any. A plugin runs
/// without a shell, so one of these is never something the binary can use — it
/// is a pipeline the model wrote by hand. A token straight after a flag is that
/// flag's value (`--filter '>'`), not an operator.
fn shell_operator(args: &[String]) -> Option<&str> {
    const OPS: [&str; 8] = ["|", "||", "&&", ";", ">", ">>", "<", "&"];
    args.iter()
        .enumerate()
        .find(|(i, a)| {
            OPS.contains(&a.as_str()) && !(*i > 0 && args[i - 1].starts_with('-'))
        })
        .map(|(_, a)| a.as_str())
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

/// The seat's `clientKey`, taken out of a port call's input. The ledger
/// contract puts one on every write; it is the runtime's idempotency key
/// and never a plugin flag. A key that is empty or not a string (a number
/// is read as one) is no key, and the call runs as a call without one.
fn take_client_key(input: &mut serde_json::Value) -> Option<String> {
    match input.as_object_mut()?.remove("clientKey")? {
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Whether a raw exec command invokes a bound operation's command — the bound
/// command exactly, or with additional arguments/flags after it. A binding may
/// be multi-word ("documents list"), so plain prefix matching would false-match
/// "documents listing"; the boundary must be end-of-string or whitespace.
/// A template binding ("bill create --vendor-ref {vendorId}") is matched on its
/// leading plain words ("bill create"): the flags are the call's shape, not
/// part of what names the operation.
fn command_matches_binding(command: &str, bound_cmd: &str) -> bool {
    let bound_cmd = if bound_cmd.contains('{') {
        let words: Vec<&str> = bound_cmd
            .split_whitespace()
            .take_while(|w| !w.starts_with("--") && !w.contains('{'))
            .collect();
        words.join(" ")
    } else {
        bound_cmd.to_string()
    };
    match command.strip_prefix(bound_cmd.as_str()) {
        Some(rest) => rest.is_empty() || rest.starts_with(char::is_whitespace),
        None => false,
    }
}

/// Shape a port call from its binding: the plugin command to run and the input
/// fields the binding consumed (so they are not also appended as flags).
///
/// A plain binding (no placeholders) comes back byte-for-byte as written and
/// consumes nothing — every input field is appended as `--key value` exactly
/// as before. A template binding is split into shell words first, and each
/// placeholder is filled in its own word, so a value with a space or a quote
/// stays one argument; the argv is then re-encoded with shell quoting for the
/// exec path, which splits it back losslessly. See `napp::plugin::BindingPart`
/// for the grammar. A placeholder whose field the call did not supply (an
/// optional one excepted: absent emits nothing), a list where one value is
/// expected (or the reverse), or a cents field that is not an integer is an
/// error naming the field and the operation.
fn render_binding(
    operation: &str,
    bound_cmd: &str,
    input: &serde_json::Value,
) -> Result<(String, std::collections::HashSet<String>), String> {
    use napp::plugin::BindingPart;
    let words = napp::plugin::parse_binding_template(bound_cmd)
        .map_err(|e| format!("operation '{operation}': {e}"))?;
    let mut consumed = std::collections::HashSet::new();
    if words.iter().all(|w| matches!(w.as_slice(), [BindingPart::Literal(_)])) {
        return Ok((bound_cmd.to_string(), consumed));
    }
    let field = |name: &str| -> Result<&serde_json::Value, String> {
        input
            .get(name)
            .filter(|v| !v.is_null())
            .ok_or_else(|| format!("operation '{operation}' needs input field '{name}', which the call did not supply"))
    };
    let scalar = |name: &str, v: &serde_json::Value| -> Result<String, String> {
        match v {
            serde_json::Value::String(s) => Ok(s.clone()),
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => Err(format!(
                "operation '{operation}': input field '{name}' is a list, but the binding expects one value"
            )),
            other => Ok(other.to_string()),
        }
    };
    let mut argv: Vec<String> = Vec::new();
    for parts in &words {
        if let [BindingPart::List { field: name, flag }] = parts.as_slice() {
            let items = field(name)?.as_array().ok_or_else(|| {
                format!("operation '{operation}': input field '{name}' is one value, but the binding expects a list")
            })?;
            consumed.insert(name.clone());
            for item in items {
                argv.push(flag.clone());
                argv.push(match item {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                });
            }
            continue;
        }
        if let [BindingPart::Optional { field: name, flag }] = parts.as_slice() {
            if let Some(v) = input.get(name).filter(|v| !v.is_null()) {
                argv.push(flag.clone());
                argv.push(scalar(name, v)?);
                consumed.insert(name.clone());
            }
            continue;
        }
        let mut word = String::new();
        for part in parts {
            match part {
                BindingPart::Literal(s) => word.push_str(s),
                BindingPart::Field(name) => {
                    word.push_str(&scalar(name, field(name)?)?);
                    consumed.insert(name.clone());
                }
                BindingPart::Cents(name) => {
                    let v = field(name)?;
                    let cents = match v {
                        serde_json::Value::Number(n) => n.as_i64(),
                        serde_json::Value::String(s) => s.trim().parse::<i64>().ok(),
                        _ => None,
                    }
                    .ok_or_else(|| {
                        format!("operation '{operation}': input field '{name}' must be an integer number of cents, got {v}")
                    })?;
                    let sign = if cents < 0 { "-" } else { "" };
                    word.push_str(&format!("{sign}{}.{:02}", cents.abs() / 100, cents.abs() % 100));
                    consumed.insert(name.clone());
                }
                BindingPart::List { .. } | BindingPart::Optional { .. } => {
                    unreachable!("parse_binding_template keeps list and optional placeholders alone in their word")
                }
            }
        }
        argv.push(word);
    }
    let command = shlex::try_join(argv.iter().map(String::as_str))
        .map_err(|e| format!("operation '{operation}': an input value cannot be passed as an argument ({e})"))?;
    Ok((command, consumed))
}

#[cfg(test)]
mod tests {

    // `discover ""` is browsing. best_match has nothing to match on, so an
    // install card there offers whatever the hub returned first — live on
    // 2026-09-15 that was the owner's own retired Google Workspace plugin,
    // visible because a publisher sees their own private listings, and
    // described "[DEPRECATED — do not install]". Browsing lists; naming a
    // tool cards it.
    #[test]
    fn an_empty_query_has_no_best_match_to_offer() {
        let items = vec![
            serde_json::json!({"name": "Gws", "slug": "gws", "description": "[DEPRECATED — do not install"}),
            serde_json::json!({"name": "Gmail", "slug": "gmail"}),
        ];
        // With no query every listing is equally unmatched, so best_match can
        // only fall back to first-returned — which is why offer() must not card.
        assert_eq!(best_match(&items, "")["slug"], "gws");
        assert_eq!(best_match(&items, "gmail")["slug"], "gmail");
    }

    // "receptionist" must card the Receptionist, not a bundle whose blurb
    // mentions receptionists and happens to rank first.
    #[test]
    fn best_match_prefers_the_listing_the_query_names() {
        let items = vec![
            serde_json::json!({"name": "Front Desk Bundle", "slug": "front-desk", "description": "receptionist and more"}),
            serde_json::json!({"name": "Receptionist", "slug": "receptionist"}),
            serde_json::json!({"name": "Receptionist Pro", "slug": "receptionist-pro"}),
        ];
        assert_eq!(best_match(&items, "receptionist")["slug"], "receptionist");
        assert_eq!(best_match(&items, "Receptionist P")["slug"], "receptionist-pro", "prefix wins over rank");
        assert_eq!(best_match(&items, "office manager")["slug"], "front-desk", "no match falls back to the top result");
    }
    use super::*;

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

    /// A gated template binding is still recognised through raw exec by its
    /// leading plain words; the shape flags never widen the side door.
    #[test]
    fn exec_binding_match_uses_a_templates_plain_prefix() {
        let t = "bill create --vendor-ref {vendorId} {lines[]:--line}";
        assert!(command_matches_binding("bill create", t));
        assert!(command_matches_binding("bill create --vendor-ref 7", t));
        assert!(!command_matches_binding("bill created", t));
        assert!(!command_matches_binding("bill", t));
        assert!(command_matches_binding("invoice send 9", "invoice send {invoiceId}"));
    }

    fn argv(cmd: &str) -> Vec<String> {
        shlex::split(cmd).unwrap()
    }

    fn consumed(names: &[&str]) -> std::collections::HashSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    /// The three QuickBooks bindings written against the ledger contract:
    /// renamed flags, a repeated flag from a list, cents as dollars, and a
    /// positional — each producing exactly the argv the plugin accepts.
    #[test]
    fn template_binding_shapes_the_quickbooks_calls() {
        let (cmd, used) = render_binding(
            "ledger.bill.create",
            "bill create --vendor-ref {vendorId} {lines[]:--line} --txn-date {txnDate} --due-date {dueDate}",
            &serde_json::json!({
                "vendorId": "56", "lines": ["Consulting:150.00", "Travel:42.10"],
                "txnDate": "2026-09-13", "dueDate": "2026-10-13"
            }),
        )
        .unwrap();
        assert_eq!(
            argv(&cmd),
            ["bill", "create", "--vendor-ref", "56", "--line", "Consulting:150.00", "--line", "Travel:42.10",
             "--txn-date", "2026-09-13", "--due-date", "2026-10-13"]
        );
        assert_eq!(used, consumed(&["vendorId", "lines", "txnDate", "dueDate"]));

        let (cmd, used) = render_binding(
            "ledger.payment.apply",
            "payment apply --customer-ref {customerId} {invoiceIds[]:--line} --total-amt {amountCents:cents->dollars}",
            &serde_json::json!({"customerId": "21", "invoiceIds": ["1041", "1042"], "amountCents": 125005}),
        )
        .unwrap();
        assert_eq!(
            argv(&cmd),
            ["payment", "apply", "--customer-ref", "21", "--line", "1041", "--line", "1042", "--total-amt", "1250.05"]
        );
        assert_eq!(used, consumed(&["customerId", "invoiceIds", "amountCents"]));

        let (cmd, used) = render_binding(
            "ledger.invoice.send",
            "invoice send {invoiceId}",
            &serde_json::json!({"invoiceId": "1041", "sendTo": "ap@example.com"}),
        )
        .unwrap();
        assert_eq!(argv(&cmd), ["invoice", "send", "1041"]);
        // The field the template does not mention is left for the flag path.
        assert_eq!(used, consumed(&["invoiceId"]));
    }

    /// `{name?:--flag}`: present emits the pair, one argument per value;
    /// absent emits nothing and consumes nothing, so nothing is appended either.
    #[test]
    fn optional_placeholder_emits_only_when_present() {
        let t = "invoice send {invoiceId} {sendTo?:--send-to}";
        let (cmd, used) = render_binding("ledger.invoice.send", t, &serde_json::json!({"invoiceId": "1041", "sendTo": "ap@example.com"})).unwrap();
        assert_eq!(argv(&cmd), ["invoice", "send", "1041", "--send-to", "ap@example.com"]);
        assert_eq!(used, consumed(&["invoiceId", "sendTo"]));

        let (cmd, used) = render_binding("ledger.invoice.send", t, &serde_json::json!({"invoiceId": "1041"})).unwrap();
        assert_eq!(argv(&cmd), ["invoice", "send", "1041"]);
        assert_eq!(used, consumed(&["invoiceId"]));
        let (cmd, _) = render_binding("ledger.invoice.send", t, &serde_json::json!({"invoiceId": "1041", "sendTo": null})).unwrap();
        assert_eq!(argv(&cmd), ["invoice", "send", "1041"]);

        let (cmd, _) = render_binding("ledger.invoice.send", t, &serde_json::json!({"invoiceId": "1041", "sendTo": "Accounts Payable <ap@example.com>"})).unwrap();
        assert_eq!(argv(&cmd), ["invoice", "send", "1041", "--send-to", "Accounts Payable <ap@example.com>"]);

        let err = render_binding("ledger.invoice.send", t, &serde_json::json!({"invoiceId": "1041", "sendTo": ["a", "b"]})).unwrap_err();
        assert!(err.contains("'sendTo'") && err.contains("is a list"), "{err}");
    }

    /// No placeholders: the bound command is passed through untouched and
    /// nothing is consumed, so every input field becomes `--key value` as before.
    #[test]
    fn plain_binding_is_unchanged_and_consumes_nothing() {
        let input = serde_json::json!({"to": "ap@example.com", "subject": "Hi there"});
        let (cmd, used) = render_binding("mail.message.send", "send", &input).unwrap();
        assert_eq!(cmd, "send");
        assert!(used.is_empty());
        let (cmd, used) = render_binding("kb.article.list", "documents list  --limit 5", &input).unwrap();
        assert_eq!(cmd, "documents list  --limit 5");
        assert!(used.is_empty());
    }

    #[test]
    fn template_values_stay_one_argument_each() {
        let (cmd, _) = render_binding(
            "ledger.bill.create",
            "bill create --memo {memo} {lines[]:--line}",
            &serde_json::json!({"memo": "Q3 \"catch up\" invoice", "lines": ["Consulting hours:150.00"]}),
        )
        .unwrap();
        assert_eq!(
            argv(&cmd),
            ["bill", "create", "--memo", "Q3 \"catch up\" invoice", "--line", "Consulting hours:150.00"]
        );
        // Numbers and booleans fill a word too; text may sit beside a placeholder.
        let (cmd, _) = render_binding("x.y.z", "run --page={page} --dry={dry}", &serde_json::json!({"page": 3, "dry": true})).unwrap();
        assert_eq!(argv(&cmd), ["run", "--page=3", "--dry=true"]);
        // A negative cents amount keeps its sign.
        let (cmd, _) = render_binding("x.y.z", "adjust {amountCents:cents->dollars}", &serde_json::json!({"amountCents": -7})).unwrap();
        assert_eq!(argv(&cmd), ["adjust", "-0.07"]);
    }

    #[test]
    fn template_errors_name_the_field_and_operation() {
        let missing = render_binding("ledger.invoice.send", "invoice send {invoiceId}", &serde_json::json!({"sendTo": "x"}))
            .unwrap_err();
        assert!(missing.contains("'invoiceId'") && missing.contains("ledger.invoice.send"), "{missing}");
        let null = render_binding("ledger.invoice.send", "invoice send {invoiceId}", &serde_json::json!({"invoiceId": null}))
            .unwrap_err();
        assert!(null.contains("'invoiceId'"), "{null}");

        let list_in_scalar = render_binding("ledger.bill.create", "bill create --vendor-ref {vendorId}", &serde_json::json!({"vendorId": ["1", "2"]}))
            .unwrap_err();
        assert!(list_in_scalar.contains("'vendorId'") && list_in_scalar.contains("is a list"), "{list_in_scalar}");

        let scalar_in_list = render_binding("ledger.bill.create", "bill create {lines[]:--line}", &serde_json::json!({"lines": "one"}))
            .unwrap_err();
        assert!(scalar_in_list.contains("'lines'") && scalar_in_list.contains("expects a list"), "{scalar_in_list}");

        let bad_cents = render_binding("ledger.payment.apply", "payment apply --total-amt {amountCents:cents->dollars}", &serde_json::json!({"amountCents": "12.50"}))
            .unwrap_err();
        assert!(bad_cents.contains("'amountCents'") && bad_cents.contains("cents"), "{bad_cents}");

        let malformed = render_binding("ledger.bill.create", "bill create {vendorId", &serde_json::json!({"vendorId": "1"})).unwrap_err();
        assert!(malformed.contains("ledger.bill.create") && malformed.contains("no matching"), "{malformed}");
    }

    /// The key leaves the input and nothing else does; no key, an empty
    /// key, or a null is the same call without one.
    #[test]
    fn client_key_comes_out_of_the_input_and_nothing_else_does() {
        let mut input = serde_json::json!({"clientKey": " bill-77 ", "vendorId": "V7"});
        assert_eq!(take_client_key(&mut input).as_deref(), Some("bill-77"));
        assert_eq!(input, serde_json::json!({"vendorId": "V7"}));
        assert_eq!(take_client_key(&mut input), None);
        assert_eq!(take_client_key(&mut serde_json::json!({"clientKey": 4102})).as_deref(), Some("4102"));
        assert_eq!(take_client_key(&mut serde_json::json!({"clientKey": ""})), None);
        assert_eq!(take_client_key(&mut serde_json::json!({"clientKey": null})), None);
        assert_eq!(take_client_key(&mut serde_json::Value::Null), None);
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

    /// An exec that writes a picture hands the picture back: the raster a
    /// `screencapture` or `sips` run produced is detected like any document,
    /// and a path `image_url` is read from disk by every provider.
    #[test]
    fn test_produced_work_document_detects_rasters() {
        let dir = std::env::temp_dir().join(format!("nebo-pwd-raster-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shot = dir.join("a.png");
        let crop = dir.join("b.jpg");
        let input = dir.join("in.png");
        std::fs::write(&input, "old").unwrap();
        let started = std::time::SystemTime::now();
        std::fs::write(&shot, "png").unwrap();
        std::fs::write(&crop, "jpg").unwrap();
        let s = |p: &std::path::Path| p.to_string_lossy().into_owned();

        let cap: Vec<String> = ["screencapture", "-x", "-R", "0,0,100,100", &s(&shot)].map(String::from).to_vec();
        assert_eq!(produced_work_document(&cap, None, started), Some(s(&shot)));
        let sips: Vec<String> = ["sips", "-s", "format", "jpeg", &s(&input), "--out", &s(&crop)].map(String::from).to_vec();
        assert_eq!(produced_work_document(&sips, None, started), Some(s(&crop)), "the fresh output, not the stale input");
        let stale = started + std::time::Duration::from_secs(5);
        assert_eq!(produced_work_document(&cap, None, stale), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod budget_and_install_tests {
    use super::*;
    use crate::registry::DynTool;

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

    /// A plugin whose accounts are per employee, with a manifest that says so.
    fn install_account_plugin(root: &std::path::Path, slug: &str, bindings: serde_json::Value) {
        let version_dir = root.join("plugins").join(slug).join("0.1.0");
        std::fs::create_dir_all(&version_dir).unwrap();
        std::fs::write(
            version_dir.join("plugin.json"),
            serde_json::json!({
                "id": slug, "slug": slug, "name": slug, "version": "0.1.0", "platforms": {},
                "auth": {"type": "oauth", "profileDirEnv": format!("{}_CONFIG_DIR", slug.to_uppercase())},
                "interfaceBindings": bindings,
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(version_dir.join(slug), b"#!/bin/sh\necho ok\n").unwrap();
    }

    /// Through the port path itself: a template binding shapes the call, and
    /// the one input field the template does not mention still reaches the
    /// plugin as `--key value`, appended after the shaped words.
    /// The refusal names the binding's fields, so the second call can be the
    /// typed one instead of the same exec again.
    #[tokio::test]
    async fn a_gated_exec_is_refused_with_the_binding_it_should_have_used() {
        let tmp = tempfile::tempdir().unwrap();
        let (plugin_store, db_store) = stores(tmp.path());
        let version_dir = tmp.path().join("plugins").join("quickbooks").join("0.1.0");
        std::fs::create_dir_all(&version_dir).unwrap();
        std::fs::write(
            version_dir.join("plugin.json"),
            serde_json::json!({
                "id": "quickbooks", "slug": "quickbooks", "name": "quickbooks", "version": "0.1.0", "platforms": {},
                "interfaceBindings": {"ledger.payment.apply": "payment create --customer-ref {customerRef} --total-amt {totalAmt}"},
            })
            .to_string(),
        )
        .unwrap();
        let runner = Arc::new(PluginRunner::new(plugin_store, db_store));
        let tool = crate::plugin_tools::PluginCliTool::new(runner, "quickbooks");
        let ctx = ToolContext { session_key: "agent:ic:main".into(), ..Default::default() };
        let r = tool
            .execute_dyn(&ctx, serde_json::json!({"command": "payment create --dry-run --json"}))
            .await;
        assert!(r.is_error, "{}", r.content);
        assert!(r.content.contains("ledger.payment.apply") && r.content.contains("Call ledger_payment_apply"), "{}", r.content);
        assert!(r.content.contains("--customer-ref {customerRef}"), "the binding template is in the refusal: {}", r.content);
    }

    #[tokio::test]
    async fn port_call_takes_its_shape_from_the_binding() {
        let tmp = tempfile::tempdir().unwrap();
        let (plugin_store, db_store) = stores(tmp.path());
        let version_dir = tmp.path().join("plugins").join("quickbooks").join("0.1.0");
        std::fs::create_dir_all(&version_dir).unwrap();
        std::fs::write(
            version_dir.join("plugin.json"),
            serde_json::json!({
                "id": "quickbooks", "slug": "quickbooks", "name": "quickbooks", "version": "0.1.0", "platforms": {},
                "interfaceBindings": {"ledger.invoice.send": "invoice send {invoiceId}"},
            })
            .to_string(),
        )
        .unwrap();
        let bin = version_dir.join("quickbooks");
        std::fs::write(&bin, b"#!/bin/sh\nprintf '%s\\n' \"$@\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let tool = operation_tool(plugin_store, db_store, "quickbooks", "ledger_invoice_send");
        let ctx = ToolContext { session_key: "agent:ic:main".into(), ..Default::default() };
        let r = tool
            .execute_dyn(&ctx, serde_json::json!({"invoiceId": "Invoice 1041", "sendTo": "ap@example.com"}))
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("invoice\nsend\nInvoice 1041\n--sendTo\nap@example.com"), "{}", r.content);
    }

    /// The operation tool `name` as the registry builds it from `slug`'s
    /// bindings.
    fn operation_tool(
        plugin_store: Arc<napp::plugin::PluginStore>,
        db_store: Arc<db::Store>,
        slug: &str,
        name: &str,
    ) -> crate::operation_tools::OperationTool {
        let runner = Arc::new(PluginRunner::new(plugin_store, db_store));
        let provider: Arc<dyn crate::operation_tools::OperationProvider> =
            Arc::new(crate::operation_tools::PluginProvider::new(runner, slug));
        crate::operation_tools::operation_tools(&[provider])
            .into_iter()
            .find(|t| t.name() == name)
            .unwrap_or_else(|| panic!("{slug} binds no {name}"))
    }

    /// A plugin that binds the given operations and, as its binary, runs the
    /// given shell script.
    fn install_port_plugin(root: &std::path::Path, slug: &str, bindings: serde_json::Value, script: &str) {
        let version_dir = root.join("plugins").join(slug).join("0.1.0");
        std::fs::create_dir_all(&version_dir).unwrap();
        std::fs::write(
            version_dir.join("plugin.json"),
            serde_json::json!({
                "id": slug, "slug": slug, "name": slug, "version": "0.1.0", "platforms": {},
                "interfaceBindings": bindings,
            })
            .to_string(),
        )
        .unwrap();
        let bin = version_dir.join(slug);
        std::fs::write(&bin, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// The ledger contract's `clientKey` is the runtime's: it never reaches
    /// the plugin as a flag; the same write under one key runs the plugin
    /// once and is answered from the ledger after that; another key runs
    /// again; a call with no key behaves as it always did.
    #[tokio::test]
    async fn client_key_stays_with_the_runtime_and_a_write_under_it_runs_once() {
        let tmp = tempfile::tempdir().unwrap();
        let (plugin_store, db_store) = stores(tmp.path());
        let calls = tmp.path().join("calls.log");
        install_port_plugin(
            tmp.path(),
            "quickbooks",
            serde_json::json!({"ledger.bill.create": "bill create --vendor-ref {vendorId}"}),
            &format!("#!/bin/sh\necho \"$@\" >> '{}'\nprintf '%s\\n' \"$@\"\n", calls.display()),
        );
        let tool = operation_tool(plugin_store, db_store, "quickbooks", "ledger_bill_create");
        let ctx = ToolContext { session_key: "agent:ap:main".into(), ..Default::default() };
        let call = |key: &str| serde_json::json!({"clientKey": key, "vendorId": "V7", "txnDate": "2026-09-13"});
        let invocations = || std::fs::read_to_string(&calls).unwrap_or_default().lines().count();

        let first = tool.execute_dyn(&ctx, call("bill-77")).await;
        assert!(!first.is_error, "{}", first.content);
        assert!(first.content.contains("bill\ncreate\n--vendor-ref\nV7\n--txnDate\n2026-09-13"), "{}", first.content);
        assert!(!first.content.contains("clientKey"), "the key is not a flag: {}", first.content);
        assert_eq!(invocations(), 1);

        let again = tool.execute_dyn(&ctx, call("bill-77")).await;
        assert!(!again.is_error, "{}", again.content);
        assert!(again.content.contains("Already performed under clientKey bill-77"), "{}", again.content);
        assert!(again.content.contains("--vendor-ref\nV7"), "the recorded result comes back: {}", again.content);
        assert_eq!(invocations(), 1, "the plugin ran once");

        let other = tool.execute_dyn(&ctx, call("bill-78")).await;
        assert!(!other.is_error && !other.content.contains("Already performed"), "{}", other.content);
        assert_eq!(invocations(), 2, "another key is another write");

        let bare = serde_json::json!({"vendorId": "V7"});
        let r = tool.execute_dyn(&ctx, bare.clone()).await;
        assert!(!r.is_error && r.content.contains("bill\ncreate\n--vendor-ref\nV7"), "{}", r.content);
        let r = tool.execute_dyn(&ctx, bare).await;
        assert!(!r.is_error && !r.content.contains("Already performed"), "{}", r.content);
        assert_eq!(invocations(), 4, "no key: every call runs, as before");
    }

    /// Seen live: an employee with gmail connected called google-workspace
    /// first (nothing connected there), the terminal error ended the turn,
    /// and the lead was never answered. Unattended, the error now steers to
    /// what IS connected and the turn goes on; with nothing connected at all
    /// there is nothing to steer to, and the run stops cleanly.
    #[tokio::test]
    async fn unattended_no_account_steers_to_what_is_connected() {
        let tmp = tempfile::tempdir().unwrap();
        let (plugin_store, db_store) = stores(tmp.path());
        install_account_plugin(tmp.path(), "gws", serde_json::json!({}));
        install_account_plugin(tmp.path(), "gmail", serde_json::json!({"mail.message.send": "send"}));
        let tool = PluginRunner::new(plugin_store, db_store.clone());
        let ctx = ToolContext { session_key: "agent:ic:workflow:run-1".into(), ..Default::default() };
        let pi = PluginCall { slug: "gws".into(), command: "calendar events list".into(), ..Default::default() };

        let r = tool.run_plugin_command(&pi, &ctx, Duration::from_secs(5)).await;
        assert!(r.is_error && r.terminal, "nothing connected at all: {}", r.content);

        db_store
            .upsert_plugin_account_profile("p1", "ic", "gmail", "sales@example.com", tmp.path().join("gmail-acct").to_str().unwrap())
            .unwrap();
        let r = tool.run_plugin_command(&pi, &ctx, Duration::from_secs(5)).await;
        assert!(r.is_error && !r.terminal, "{}", r.content);
        assert!(r.content.contains("No gws account is connected"), "{}", r.content);
        assert!(r.content.contains("Connected for this employee: gmail"), "{}", r.content);
        assert!(r.content.contains("Operation tools those serve: mail_message_send (via gmail)"), "{}", r.content);
    }

    /// A best match that is already installed gets no install card: the
    /// result says so and points at the plugin, even in a run that could not
    /// show a card.
    #[tokio::test]
    async fn discover_does_not_offer_to_install_what_is_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let (plugin_store, db_store) = stores(tmp.path());
        let tool = PluginRunner::new(plugin_store, db_store);
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
        assert!(known.content.contains("plugin__quickbooks"), "{}", known.content);
        assert!(!known.content.contains("Install it on the card"), "{}", known.content);
        assert!(!known.content.contains("owner's approval"), "{}", known.content);
    }

    /// Offer an install card in an interactive chat and end it the given
    /// way: `Some(value)` answers the card with it, `None` stops the run.
    async fn install_card_ending(answer: Option<&str>) -> ToolResult {
        let tmp = tempfile::tempdir().unwrap();
        let (plugin_store, db_store) = stores(tmp.path());
        let tool = PluginRunner::new(plugin_store, db_store);
        let (stream_tx, mut stream_rx) = tokio::sync::mpsc::channel(4);
        let channels: crate::origin::AskChannels = Default::default();
        let mut ctx = ToolContext::new(crate::origin::Origin::User);
        ctx.session_key = "agent:ic:main".into();
        ctx.stream_tx = Some(stream_tx);
        ctx.ask_channels = Some(channels.clone());
        let cancel = ctx.cancel_token.clone();
        let products = vec![serde_json::json!({
            "name": "Email", "slug": "email", "code": "PLUG-ABCD-1234",
            "description": "Mail", "type": "plugin"
        })];
        let offering = tokio::spawn(async move { tool.offer("email", &ctx, &products, 1).await });
        let request = stream_rx.recv().await.expect("the install card is shown");
        let request_id = request.error.clone().unwrap();
        match answer {
            Some(v) => {
                let tx = channels.lock().await.remove(&request_id).unwrap();
                tx.send(v.to_string()).unwrap();
            }
            None => cancel.cancel(),
        }
        offering.await.unwrap()
    }

    /// Live (2026-09-24): the install failed on the card, the tool called it
    /// "declined", and the model offered the broken plugin eight more times.
    /// A failure now comes back as a failure, with the error the owner saw.
    #[tokio::test]
    async fn a_failed_install_is_reported_as_failed_not_declined() {
        let r = install_card_ending(Some("failed:Email: NeboAI returned 404: not found")).await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(
            r.content,
            "Installing Email FAILED: Email: NeboAI returned 404: not found. Tell the owner that error in plain \
             words and stop. Do NOT offer the card again and do NOT suggest commands or other ways to install it."
        );
    }

    #[tokio::test]
    async fn a_skipped_install_card_is_not_offered_again_unasked() {
        let r = install_card_ending(Some(crate::origin::SKIP_SENTINEL)).await;
        assert!(r.content.starts_with("Found 1 plugin(s):"), "the listing stays: {}", r.content);
        assert!(r.content.contains("The owner skipped the install card for Email. Do NOT offer it again unless they ask."), "{}", r.content);
        assert!(!r.content.contains("declined"), "{}", r.content);
    }

    #[tokio::test]
    async fn a_stopped_run_is_no_answer_not_a_decline() {
        let r = install_card_ending(None).await;
        assert_eq!(
            r.content,
            "The install card for Email got no answer (the owner stopped the run). Do NOT offer it again."
        );
    }

    #[test]
    fn a_card_answer_reads_as_done_failed_skipped_or_none() {
        assert_eq!(CardAnswer::read(Some("installed"), INSTALL_CARD_INSTALLED), CardAnswer::Done);
        assert_eq!(CardAnswer::read(Some("failed: boom"), INSTALL_CARD_INSTALLED), CardAnswer::Failed("boom"));
        assert_eq!(CardAnswer::read(Some(crate::origin::SKIP_SENTINEL), "connected"), CardAnswer::Skipped);
        assert_eq!(CardAnswer::read(Some("installed"), "connected"), CardAnswer::Skipped);
        assert_eq!(CardAnswer::read(None, "connected"), CardAnswer::NoAnswer);
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

        let named = PluginCall { command: "doctor".into(), timeout: 45, ..Default::default() };
        assert_eq!(PluginRunner::exec_timeout(&named), Duration::from_secs(45));
        assert_eq!(PluginRunner::command_label(&named), "doctor");
        let bare = PluginCall::default();
        assert_eq!(PluginRunner::exec_timeout(&bare), Duration::from_secs(EXEC_TIMEOUT_DEFAULT_SECS));
        assert_eq!(PluginRunner::command_label(&bare), "the command");
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

    #[test]
    fn a_hand_written_pipeline_is_named_as_the_problem() {
        let split = |c: &str| shlex::split(c).unwrap();
        // The shopify case: the model piped graphql output into grep.
        let args = split("graphql --query '{ __type(name: \"Mutation\") { fields { name } } }' | grep -A 20 inventorySetQuantities");
        assert_eq!(shell_operator(&args), Some("|"));
        // Every operator a shell would honour and a plugin cannot.
        for c in ["a && b", "a || b", "a ; b", "a > f", "a >> f", "a < f", "a &"] {
            assert!(shell_operator(&split(c)).is_some(), "{c}");
        }
        // A flag's own value is not an operator, however it looks.
        assert_eq!(shell_operator(&split("orders list --filter '>'")), None);
        assert_eq!(shell_operator(&split("products list --limit 20")), None);
    }
}
