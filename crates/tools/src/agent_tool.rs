use std::collections::{HashMap, HashSet};

use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::origin::ToolContext;
use crate::plugin_tool::{CardAnswer, INSTALL_CARD_INSTALLED, TOOL_INSTALL_DOOR};
use crate::registry::ToolResult;
use db::Store;

/// The three places an employee can come from, as `info` reports them. A
/// marketplace employee has a code and files under the installed tree or a
/// sealed .napp; the other two are local and have nothing to install.
const SOURCE_MARKETPLACE: &str = "marketplace";
const SOURCE_USER_CREATED: &str = "user-created";
const SOURCE_LOCAL_DATABASE: &str = "local employee (database only)";

/// How much of a long persona `info` shows inline; the rest is in AGENT.md.
const INFO_PERSONA_PREVIEW_BYTES: usize = 500;

/// A single active agent — its own bot with isolated persona and scoped capabilities.
#[derive(Debug, Clone)]
pub struct ActiveAgent {
    /// Unique agent identifier (DB id or filesystem name).
    pub agent_id: String,
    /// Human-readable display name.
    pub name: String,
    /// Full AGENT.md body — told in the turn's identity row.
    pub agent_md: String,
    /// Parsed agent.json config (workflows, skills, triggers).
    pub config: Option<napp::agent::AgentConfig>,
    /// Optional bound NeboAI channel.
    pub channel_id: Option<String>,
    /// If set, this agent has unmet skill dependencies and is degraded.
    /// The string describes which skill dependencies are missing.
    pub degraded: Option<String>,
    /// Per-agent soul: voice, tone, personality, boundaries (SOUL.md content).
    pub soul: Option<String>,
    /// Per-agent rules: behavior constraints and guardrails.
    pub rules: Option<String>,
}

/// Registry of all currently active agents. Multiple agents run concurrently.
/// Key = agent_id (lowercase name or DB id).
pub type AgentRegistry = Arc<RwLock<HashMap<String, ActiveAgent>>>;

/// Legacy alias — callers that only need the old behavior can still compile.
pub type ActiveAgentState = AgentRegistry;

/// Validate agent→skill dependencies for all active agents.
///
/// For each agent that declares required skills (via `config.skills`), checks
/// whether those skills are actually loaded. Agents with missing skill
/// dependencies are marked as `degraded` with a descriptive reason string.
///
/// This does NOT prevent loading — degraded agents remain activatable but
/// their missing capabilities will be logged at warn level.
///
/// Call this after both the agent registry and skill loader have been populated.
pub async fn validate_agent_dependencies(
    agent_registry: &AgentRegistry,
    skill_loader: &crate::skills::Loader,
) {
    let mut registry = agent_registry.write().await;
    for (agent_id, active_agent) in registry.iter_mut() {
        let skill_refs = match active_agent.config {
            Some(ref cfg) if !cfg.skills.is_empty() => &cfg.skills,
            _ => continue,
        };

        // Scoped to this employee: the procedures a package ships with belong
        // to the seat that shipped them, so the shared roster alone would
        // report every packaged employee as missing its own skills.
        let skill_names: HashSet<String> = skill_loader
            .list_summaries(Some(agent_id))
            .await
            .into_iter()
            .map(|s| s.name)
            .collect();

        let mut missing = Vec::new();
        for skill_ref in skill_refs {
            let skill_name = extract_skill_name_from_ref(skill_ref);
            if !skill_names.contains(&skill_name) {
                missing.push(skill_ref.clone());
            }
        }

        if !missing.is_empty() {
            let reason = format!("missing skills: {}", missing.join(", "));
            warn!(
                agent = %active_agent.name,
                agent_id = %agent_id,
                reason = %reason,
                "agent degraded: unmet skill dependencies"
            );
            active_agent.degraded = Some(reason);
        } else if active_agent.degraded.is_some() {
            info!(
                agent = %active_agent.name,
                agent_id = %agent_id,
                "agent restored: all skill dependencies now met"
            );
            active_agent.degraded = None;
        }
    }
}

/// Extract the short skill name from a qualified reference.
///
/// Qualified refs look like `@nebo/skills/briefing-writer@^1.0.0` — this
/// extracts `briefing-writer`. Install codes like `SKIL-XXXX-XXXX` are
/// returned as-is since they can't be resolved to a skill name at load time.
fn extract_skill_name_from_ref(skill_ref: &str) -> String {
    // Install codes — return as-is (can't resolve without API call)
    if skill_ref.starts_with("SKIL-") {
        return skill_ref.to_string();
    }

    // Qualified: @org/skills/name@version → name
    if skill_ref.starts_with('@') {
        let without_at = &skill_ref[1..];
        // Strip version suffix: @org/skills/name@^1.0.0 → org/skills/name
        let name_part = if let Some(idx) = without_at.find('@') {
            &without_at[..idx]
        } else {
            without_at
        };
        // Split on '/' and take the last segment
        if let Some(last) = name_part.rsplit('/').next() {
            if !last.is_empty() {
                return last.to_string();
            }
        }
    }

    // Fallback: return as-is (bare name)
    skill_ref.to_string()
}

/// PersonaTool manages the agent's personas — the top of the hierarchy.
/// A persona defines who the agent is: persona, workflows, skills, triggers.
pub struct PersonaTool {
    store: Arc<Store>,
    agent_registry: AgentRegistry,
    agent_loader: Arc<napp::AgentLoader>,
    /// Shared cell holding the canonical code installer (filled late by the server).
    code_installer: Arc<std::sync::RwLock<Option<Arc<dyn crate::bot_tool::CodeInstaller>>>>,
    /// Shared cell holding the permission system's side of making and
    /// changing jobs (filled late by the server, like `code_installer`).
    job_consent: crate::needs::JobConsentCell,
}

/// A listing NeboAI published: its qualified name lives under the @neboai
/// namespace. Everything else is a third-party publisher.
fn is_neboai(it: &serde_json::Value) -> bool {
    it.get("qualifiedName")
        .and_then(|x| x.as_str())
        .map(|q| q.starts_with("@neboai/"))
        .unwrap_or(false)
}

/// Stable partition: NeboAI's listings first, then the rest, each group in
/// the order the hub ranked them.
fn prefer_neboai(items: &mut [serde_json::Value]) {
    items.sort_by_key(|it| !is_neboai(it));
}

impl PersonaTool {
    pub fn new(
        store: Arc<Store>,
        agent_registry: AgentRegistry,
        agent_loader: Arc<napp::AgentLoader>,
    ) -> Self {
        Self {
            store,
            agent_registry,
            agent_loader,
            code_installer: Arc::new(std::sync::RwLock::new(None)),
            job_consent: Arc::new(std::sync::RwLock::new(None)),
        }
    }

    /// Inject the shared job-consent cell (from the `Registry`): create and
    /// update draft a job, and grant it on the owner's yes, through it.
    pub fn with_job_consent(mut self, cell: crate::needs::JobConsentCell) -> Self {
        self.job_consent = cell;
        self
    }

    fn job_consent(&self) -> Option<Arc<dyn crate::needs::JobConsent>> {
        self.job_consent.read().ok().and_then(|c| c.clone())
    }

    /// Inject the shared canonical-installer cell (from the `Registry`). When set, the
    /// `install` action routes ANY code through the server's `codes::handle_code`
    /// pathway (skills/plugins/agents/apps/collections, with cascade + binary download).
    pub fn with_code_installer(
        mut self,
        installer: Arc<std::sync::RwLock<Option<Arc<dyn crate::bot_tool::CodeInstaller>>>>,
    ) -> Self {
        self.code_installer = installer;
        self
    }

    pub(crate) async fn handle_list(&self) -> ToolResult {
        // Get agents from loader cache
        let fs_agents = self.agent_loader.list().await;
        let installed: Vec<_> = fs_agents
            .iter()
            .filter(|a| a.source == napp::AgentSource::Installed)
            .cloned()
            .collect();
        let user: Vec<_> = fs_agents
            .iter()
            .filter(|a| a.source == napp::AgentSource::User)
            .cloned()
            .collect();

        // Also check DB for agents
        let db_agents = self.store.list_agents(100, 0).unwrap_or_default();

        if installed.is_empty() && user.is_empty() && db_agents.is_empty() {
            return ToolResult::ok("No employees yet.");
        }

        let mut lines = Vec::new();

        for agent in &installed {
            lines.push(format!(
                "- [installed] {} — {}",
                agent.agent_def.name,
                if agent.agent_def.description.is_empty() {
                    "-"
                } else {
                    &agent.agent_def.description
                }
            ));
        }
        for agent in &user {
            lines.push(format!(
                "- [user] {} — {}",
                agent.agent_def.name,
                if agent.agent_def.description.is_empty() {
                    "-"
                } else {
                    &agent.agent_def.description
                }
            ));
        }
        // Add DB-only agents not already in filesystem list
        let fs_names: Vec<&str> = installed
            .iter()
            .chain(user.iter())
            .map(|r| r.agent_def.name.as_str())
            .collect();
        for agent in &db_agents {
            if !fs_names.contains(&agent.name.as_str()) {
                let enabled = if agent.is_enabled != 0 {
                    "enabled"
                } else {
                    "disabled"
                };
                // Apps are agents with a UI — label them so they're not overlooked.
                let kind = if agent.is_app.unwrap_or(0) != 0 {
                    "app"
                } else {
                    "agent"
                };
                lines.push(format!(
                    "- [{}/{}] {} — {}",
                    kind,
                    enabled,
                    agent.name,
                    if agent.description.is_empty() {
                        "-"
                    } else {
                        &agent.description
                    }
                ));
            }
        }

        let registry = self.agent_registry.read().await;
        let active_count = registry.len();
        // "Active" = loaded into memory this session — a SUBSET of installed. Phrase it
        // so it's not misread as "only these are installed" (every line below IS installed).
        let status = if active_count > 0 {
            let names: Vec<&str> = registry.values().map(|r| r.name.as_str()).collect();
            format!(" ({} currently active in memory: {})", active_count, names.join(", "))
        } else {
            String::new()
        };

        // Every line is installed; the breakdown says where each came from so the
        // total reconciles with what is listed.
        let db_only = lines.len() - installed.len() - user.len();
        let breakdown = if db_only > 0 {
            format!(
                "{} marketplace, {} user-created, {} database only",
                installed.len(),
                user.len(),
                db_only
            )
        } else {
            format!("{} marketplace, {} user-created", installed.len(), user.len())
        };
        ToolResult::ok(format!(
            "{} employee(s)/app(s) ({}){}:\n{}",
            lines.len(),
            breakdown,
            status,
            lines.join("\n")
        ))
    }

    pub(crate) async fn handle_activate(&self, input: &serde_json::Value) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");
        if name.is_empty() {
            return ToolResult::error(crate::errors::missing_param(
                "set_employee_active",
                "name",
                "set_employee_active(name: \"chief-of-staff\", active: true)",
            ));
        }

        // Try loading from filesystem first
        let agent = self.find_agent(name).await;

        match agent {
            Some(loaded) => {
                let body = loaded.agent_def.body.clone();
                let agent_name = loaded.agent_def.name.clone();

                // Use the DB ID — every agent should have a DB entry
                let agent_id = if let Ok(agents) = self.store.list_agents(100, 0) {
                    if let Some(db_agent) = agents.iter().find(|r| r.name == agent_name) {
                        if db_agent.is_enabled == 0 {
                            let _ = self.store.toggle_agent(&db_agent.id);
                        }
                        db_agent.id.clone()
                    } else {
                        // No DB entry yet — create one
                        let id = uuid::Uuid::new_v4().to_string();
                        let frontmatter = loaded
                            .config
                            .as_ref()
                            .and_then(|c| serde_json::to_string(c).ok())
                            .unwrap_or_else(|| "{}".to_string());
                        match self.store.create_agent(
                            &id,
                            None,
                            &agent_name,
                            &loaded.agent_def.description,
                            &body,
                            &frontmatter,
                            None,
                            None,
                        ) {
                            Ok(_) => {
                                let agent_dir = self.agent_loader.user_dir().join(&agent_name);
                                if agent_dir.exists() {
                                    let _ = self
                                        .store
                                        .set_agent_napp_path(&id, &agent_dir.to_string_lossy());
                                }
                            }
                            Err(e) => {
                                warn!(name = %agent_name, error = %e, "failed to create DB entry for agent")
                            }
                        }
                        id
                    }
                } else if !loaded.agent_def.id.is_empty() {
                    loaded.agent_def.id.clone()
                } else {
                    uuid::Uuid::new_v4().to_string()
                };

                // Insert into agent registry (multiple agents can be active)
                let active = ActiveAgent {
                    agent_id: agent_id.clone(),
                    name: agent_name.clone(),
                    agent_md: body,
                    config: loaded.config.clone(),
                    channel_id: None,
                    degraded: None,
                    soul: None,
                    rules: None,
                };
                self.agent_registry
                    .write()
                    .await
                    .insert(agent_id.clone(), active);

                let mut result = format!("Turned on {} (id: {})", agent_name, agent_id);
                if let Some(ref config) = loaded.config {
                    let wf_count = config.workflows.len();
                    let skill_count = config.skills.len();
                    if wf_count > 0 || skill_count > 0 {
                        result.push_str(&format!(
                            "\nDependencies: {} workflows, {} skills",
                            wf_count, skill_count
                        ));
                    }

                    // Register triggers (cron jobs, agent_workflows DB records)
                    self.register_config_triggers(&agent_id, config);
                }

                ToolResult::ok(result)
            }
            None => ToolResult::error(format!(
                "No employee named '{}'. list_employees shows every employee here.",
                name
            )),
        }
    }

    pub(crate) async fn handle_deactivate(&self, input: &serde_json::Value) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");

        let mut registry = self.agent_registry.write().await;

        if name.is_empty() {
            // Deactivate all agents
            if registry.is_empty() {
                return ToolResult::ok("No employees are on.");
            }
            let names: Vec<String> = registry.values().map(|r| r.name.clone()).collect();
            registry.clear();
            ToolResult::ok(format!("Turned off every employee: {}", names.join(", ")))
        } else {
            // Deactivate a specific agent by name or id
            let lower = name.to_lowercase();
            let key = registry
                .iter()
                .find(|(k, v)| k.to_lowercase() == lower || v.name.to_lowercase() == lower)
                .map(|(k, _)| k.clone());
            match key {
                Some(k) => {
                    let agent = registry.remove(&k).unwrap();
                    ToolResult::ok(format!("Turned off {}", agent.name))
                }
                None => ToolResult::error(format!(
                    "{} is not active. Employees that are on: {}",
                    name,
                    if registry.is_empty() {
                        "none".to_string()
                    } else {
                        registry
                            .values()
                            .map(|r| r.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                )),
            }
        }
    }

    pub(crate) async fn handle_info(&self, input: &serde_json::Value) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");
        if name.is_empty() {
            // Show all active agents
            let registry = self.agent_registry.read().await;
            if registry.is_empty() {
                return ToolResult::ok("No employees are on.");
            }
            let mut lines = Vec::new();
            for (id, agent) in registry.iter() {
                let preview = if agent.agent_md.len() > 200 {
                    format!(
                        "(first 200 of {} bytes; full text via get_employee(name: \"{}\"))\n{}",
                        agent.agent_md.len(),
                        agent.name,
                        crate::truncate_str(&agent.agent_md, 200)
                    )
                } else {
                    agent.agent_md.clone()
                };
                lines.push(format!("**{}** (id: {})\n{}", agent.name, id, preview));
            }
            return ToolResult::ok(format!(
                "Active agents ({}):\n\n{}",
                registry.len(),
                lines.join("\n\n---\n\n")
            ));
        }

        match self.find_agent(name).await {
            Some(loaded) => ToolResult::ok(Self::info_text(
                &loaded,
                self.agent_loader.user_dir(),
                self.agent_loader.installed_dir(),
            )),
            None => ToolResult::error(format!("No employee named '{}'.", name)),
        }
    }

    /// Where an employee comes from, said so that an edit starts in the right
    /// place. "marketplace" was printed for every database row without a
    /// directory, and a live run spent forty calls hunting the installed tree
    /// for a blank hire that lived only in the database (2026-09-05).
    fn source_label(napp_path: Option<&std::path::Path>, user_dir: &std::path::Path, installed_dir: &std::path::Path) -> &'static str {
        match napp_path {
            None => SOURCE_LOCAL_DATABASE,
            Some(p) if p.starts_with(user_dir) => SOURCE_USER_CREATED,
            Some(p) if p.starts_with(installed_dir) || p.extension().is_some_and(|e| e == "napp") => SOURCE_MARKETPLACE,
            Some(_) => SOURCE_USER_CREATED,
        }
    }

    /// The persona an AGENT.md carries: the body after the frontmatter, never
    /// the frontmatter itself. A blank hire's file is frontmatter only.
    fn agent_body(agent_md: &str) -> String {
        napp::agent::split_frontmatter(agent_md)
            .map(|(_, body)| body)
            .unwrap_or_else(|_| agent_md.to_string())
            .trim()
            .to_string()
    }

    /// The ONE rendering of an employee for `info`. Lines that would carry
    /// nothing are left out rather than printed with a dash.
    fn info_text(loaded: &napp::agent_loader::LoadedAgent, user_dir: &std::path::Path, installed_dir: &std::path::Path) -> String {
                let source = Self::source_label(loaded.napp_path.as_deref(), user_dir, installed_dir);
                let mut info = format!("Name: {}\n", loaded.agent_def.name);
                if let Some(version) = loaded.version.as_deref().filter(|v| !v.is_empty()) {
                    info.push_str(&format!("Version: {}\n", version));
                }
                if !loaded.agent_def.description.is_empty() {
                    info.push_str(&format!("Description: {}\n", loaded.agent_def.description));
                }
                info.push_str(&format!("Source: {}\n", source));
                if source != SOURCE_MARKETPLACE {
                    info.push_str("Created here, not hired from the marketplace: it has no install code. Its persona and automations are below.\n");
                }

                if let Some(ref config) = loaded.config {
                    if !config.workflows.is_empty() {
                        info.push_str("\nWorkflows:\n");
                        for (binding, wf) in &config.workflows {
                            let trigger_desc = match &wf.trigger {
                                napp::agent::AgentTrigger::Schedule { cron, .. } => {
                                    format!("schedule({})", cron)
                                }
                                napp::agent::AgentTrigger::Heartbeat { interval, window } => {
                                    match window {
                                        Some(w) => format!("heartbeat({}, {})", interval, w),
                                        None => format!("heartbeat({})", interval),
                                    }
                                }
                                napp::agent::AgentTrigger::Event { sources } => {
                                    format!("event({})", sources.join(", "))
                                }
                                napp::agent::AgentTrigger::Watch {
                                    plugin,
                                    event,
                                    command,
                                    ..
                                } => match event {
                                    Some(ev) => format!("watch({}, event:{})", plugin, ev),
                                    None => format!("watch({}, {})", plugin, command),
                                },
                                napp::agent::AgentTrigger::Folder { path, .. } => {
                                    format!("folder({})", path)
                                }
                                napp::agent::AgentTrigger::Manual => "manual".to_string(),
                                napp::agent::AgentTrigger::Call { line } => {
                                    format!("call(line: {})", if line.is_empty() { "any" } else { line })
                                }
                            };
                            let desc = if wf.description.is_empty() {
                                ""
                            } else {
                                &wf.description
                            };
                            let activities_note = if wf.has_activities() {
                                format!(" ({} activities)", wf.activities.len())
                            } else {
                                String::new()
                            };
                            info.push_str(&format!(
                                "  - {} [{}]{} {}\n",
                                binding, trigger_desc, activities_note, desc
                            ));
                        }
                    }
                    if !config.skills.is_empty() {
                        info.push_str(&format!("\nSkills: {}\n", config.skills.join(", ")));
                    }
                    if let Some(ref pricing) = config.pricing {
                        info.push_str(&format!(
                            "\nPricing: {} (${:.2})\n",
                            pricing.model, pricing.cost
                        ));
                    }
                }

                // The files: named on every info, so an edit never has to
                // search for them. A directory is the agent's own; a .napp
                // is sealed and edited through the tool, not on disk.
                if loaded.source_path.is_dir() {
                    info.push_str(&format!(
                        "\nFiles: {} (AGENT.md is the persona; agent.json holds inputs and workflows). Edit with update_employee(name, instructions | agent_md | automations), or edit the files and reload_employee.\n",
                        loaded.source_path.display()
                    ));
                } else if let Some(napp) = loaded.napp_path.as_ref().filter(|p| p.is_file()) {
                    info.push_str(&format!(
                        "\nFiles: sealed package {} (no editable files on disk; change it with update_employee).\n",
                        napp.display()
                    ));
                } else {
                    info.push_str(
                        "\nFiles: none on disk; this employee lives in the database. Do not search for its files. Change it with update_employee(name, instructions | agent_md | automations).\n",
                    );
                }
                let body = Self::agent_body(&loaded.agent_md);
                if body.is_empty() {
                    info.push_str(&format!(
                        "\nPersona: none yet. This employee has no instructions; set them with update_employee(name: \"{}\", instructions: \"...\").",
                        loaded.agent_def.name
                    ));
                } else if body.len() > INFO_PERSONA_PREVIEW_BYTES {
                    info.push_str(&format!(
                        "\nPersona (first {} of {} bytes; the full text is in AGENT.md above):\n{}",
                        INFO_PERSONA_PREVIEW_BYTES,
                        body.len(),
                        crate::truncate_str(&body, INFO_PERSONA_PREVIEW_BYTES)
                    ));
                } else {
                    info.push_str(&format!("\nPersona:\n{}", body));
                }

                info
    }

    /// Search the marketplace the way the owner asks: one call for every role
    /// they named, employees AND tools per query from the one search door,
    /// installed state on every row, and — in an interactive chat — ONE hire
    /// card for the best match of each query, one confirm. The card's button
    /// redeems each listing's code through POST /codes, the one install
    /// pathway, in sequence; "installed" resumes this call. Nothing here
    /// installs.
    ///
    /// Before this the contract was serial: one query per call, one hire per
    /// card, tools behind a different tool. "Set me up a bookkeeper and
    /// someone for social media" was five round trips through the model and
    /// five card-and-wait cycles for five roles (2026-09-19).
    pub(crate) async fn handle_discover(&self, input: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
        // An unreachable marketplace is an error the model can say out loud,
        // never an empty list that reads as "there are none".
        let api = match crate::build_neboai_api(&self.store) {
            Ok(a) => a,
            Err(e) => return ToolResult::error(format!("marketplace unavailable: {}", e)),
        };
        // `query` is one string or a list of them. A list that arrives as a
        // JSON string (`"[\"a\", \"b\"]"` — models do this) is still a list.
        let queries: Vec<String> = match &input["query"] {
            serde_json::Value::String(q) => {
                let q = q.trim();
                if q.is_empty() {
                    Vec::new()
                } else if let Ok(list) = serde_json::from_str::<Vec<String>>(q) {
                    list.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
                } else {
                    vec![q.to_string()]
                }
            }
            serde_json::Value::Array(a) => a
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::trim)
                .filter(|q| !q.is_empty())
                .map(str::to_string)
                .collect(),
            _ => Vec::new(),
        };
        const MAX_QUERIES: usize = 10;
        if queries.len() > MAX_QUERIES {
            return ToolResult::error(format!(
                "discover takes at most {MAX_QUERIES} queries per call; you passed {}. Split the list.",
                queries.len()
            ));
        }
        let department = input["department"].as_str().map(str::trim).filter(|d| !d.is_empty());
        let limit = input["limit"].as_i64().unwrap_or(20).clamp(1, 100);
        let offset = input["offset"].as_i64().unwrap_or(0).max(0);
        let str_of = |it: &serde_json::Value, k: &str| it.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let installed = |it: &serde_json::Value| it.get("installed").and_then(|x| x.as_bool()).unwrap_or(false);
        let employee_line = |it: &serde_json::Value| {
            let depts = it
                .get("departments")
                .and_then(|d| d.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            format!(
                "- {}{}{} ({}){} — {}",
                str_of(it, "name"),
                if is_neboai(it) { " [NeboAI]" } else { "" },
                if installed(it) { " [already hired]" } else { "" },
                str_of(it, "slug"),
                if depts.is_empty() { String::new() } else { format!(" [{depts}]") },
                str_of(it, "description"),
            )
        };
        let tool_line = |it: &serde_json::Value| {
            format!(
                "- {}{} ({}) — {}",
                str_of(it, "name"),
                if installed(it) { " [already installed]" } else { "" },
                str_of(it, "slug"),
                str_of(it, "description"),
            )
        };

        // Browsing: no query, one page of BOTH catalogs — the same pair of
        // searches a query makes. A browse that listed employees only had to
        // end in a sentence sending the reader to the other door, and that
        // sentence then had to be repeated everywhere the model might land
        // (2026-09-19). The page is the answer.
        if queries.is_empty() {
            let (employees, tools) = tokio::join!(
                api.browse_marketplace(Some("employees"), department, None, None, Some(limit), Some(offset)),
                api.browse_marketplace(Some("tools"), None, None, None, Some(limit), Some(offset)),
            );
            let mut resp = match employees {
                Ok(v) => v,
                Err(e) => return ToolResult::error(format!("marketplace search failed: {}", e)),
            };
            crate::installed::enrich_installed_state(&mut resp, &self.store);
            let total = resp.get("total").and_then(|t| t.as_i64()).unwrap_or(0);
            let mut items: Vec<serde_json::Value> =
                resp.get("products").and_then(|p| p.as_array()).cloned().unwrap_or_default();
            prefer_neboai(&mut items);
            let tool_items: Vec<serde_json::Value> = match tools {
                Ok(mut resp) => {
                    crate::installed::enrich_installed_state(&mut resp, &self.store);
                    resp.get("products").and_then(|p| p.as_array()).cloned().unwrap_or_default()
                }
                Err(_) => Vec::new(),
            };
            if items.is_empty() && tool_items.is_empty() {
                let scope = department.map(|d| format!(" in {d}")).unwrap_or_default();
                return ToolResult::ok(format!(
                    "Nothing in the marketplace{scope}. The catalog is organised by department; try \
                     another department or a query. Do not invent a listing."
                ));
            }
            let shown = items.len();
            let mut page = format!(
                "{} marketplace employee(s){}:\n{}",
                total,
                if total > shown as i64 { format!(" (showing {shown}; page with offset)") } else { String::new() },
                items.iter().map(employee_line).collect::<Vec<_>>().join("\n")
            );
            if !tool_items.is_empty() {
                page.push_str(&format!(
                    "\n\nTools, connections and services ({}):\n{}",
                    tool_items.len(),
                    tool_items.iter().map(tool_line).collect::<Vec<_>>().join("\n")
                ));
            }
            page.push_str(&format!(
                "\n\nTo hire an employee, call discover with the roles you want (one or a list) and the \
                 hire card will appear. {TOOL_INSTALL_DOOR} Never paste install codes into chat."
            ));
            return ToolResult::ok(page);
        }

        // Every query at once, employees and tools together: N roles is one round trip.
        let searches = futures::future::join_all(queries.iter().map(|q| async {
            let (emp, tools) = tokio::join!(
                api.browse_marketplace(Some("employees"), department, None, Some(q), Some(limit), Some(offset)),
                api.browse_marketplace(Some("tools"), None, None, Some(q), Some(limit), Some(0)),
            );
            (q.clone(), emp, tools)
        }))
        .await;

        let mut sections: Vec<String> = Vec::new();
        // The best not-yet-hired employee per query, for the one card.
        let mut hires: Vec<serde_json::Value> = Vec::new();
        let mut already: Vec<String> = Vec::new();
        let mut failures = 0usize;
        for (q, emp, tools) in searches {
            let mut section = format!("## {q}");
            match emp {
                Ok(mut resp) => {
                    crate::installed::enrich_installed_state(&mut resp, &self.store);
                    let total = resp.get("total").and_then(|t| t.as_i64()).unwrap_or(0);
                    let mut items: Vec<serde_json::Value> =
                        resp.get("products").and_then(|p| p.as_array()).cloned().unwrap_or_default();
                    prefer_neboai(&mut items);
                    if items.is_empty() {
                        section.push_str("\nEmployees: none found.");
                    } else {
                        let shown = items.len();
                        section.push_str(&format!(
                            "\nEmployees ({}{}):\n{}",
                            total,
                            if total > shown as i64 { format!(", showing {shown}") } else { String::new() },
                            items.iter().map(employee_line).collect::<Vec<_>>().join("\n")
                        ));
                        let top = crate::plugin_tool::best_match(&items, &q);
                        let top_code = str_of(top, "code");
                        // Two queries for one role ("receptionist", "phone answerer")
                        // meet the same listing: one card entry, one mention.
                        if installed(top) {
                            if !already.contains(&str_of(top, "name")) {
                                already.push(str_of(top, "name"));
                            }
                        } else if !top_code.is_empty() && !hires.iter().any(|h| str_of(h, "code") == top_code) {
                            hires.push(serde_json::json!({
                                "query": q,
                                "code": str_of(top, "code"),
                                "name": str_of(top, "name"),
                                "plugin": str_of(top, "slug"),
                                "description": str_of(top, "description"),
                            }));
                        }
                    }
                }
                Err(e) => {
                    failures += 1;
                    section.push_str(&format!("\nEmployees: search failed: {e}"));
                }
            }
            match tools {
                Ok(mut resp) => {
                    crate::installed::enrich_installed_state(&mut resp, &self.store);
                    let items: Vec<serde_json::Value> =
                        resp.get("products").and_then(|p| p.as_array()).cloned().unwrap_or_default();
                    if !items.is_empty() {
                        section.push_str(&format!(
                            "\nTools ({}):\n{}",
                            items.len(),
                            items.iter().map(tool_line).collect::<Vec<_>>().join("\n")
                        ));
                    }
                }
                Err(e) => {
                    failures += 1;
                    section.push_str(&format!("\nTools: search failed: {e}"));
                }
            }
            sections.push(section);
        }
        if failures == sections.len() * 2 {
            return ToolResult::error("marketplace search failed for every query".to_string());
        }
        let mut listing = sections.join("\n\n");
        if !already.is_empty() {
            listing.push_str(&format!(
                "\n\nAlready on the roster: {}. Reach them as employees; do not offer to hire them again.",
                already.join(", ")
            ));
        }

        let interactive = crate::origin::ExecutionMode::from(ctx.origin)
            == crate::origin::ExecutionMode::Interactive
            && ctx.ask_channels.is_some();
        if !interactive || hires.is_empty() {
            // No chat to park in, or nothing left to hire: the list is the answer.
            // Codes are machine currency and never appear in model-visible text.
            return ToolResult::ok(format!(
                "{listing}\n\nTo hire, call discover with the roles you want (one or a list) and one hire \
                 card will appear for all of them. {TOOL_INSTALL_DOOR} Never paste install codes into chat."
            ));
        }

        let names: Vec<String> = hires.iter().map(|h| str_of(h, "name")).collect();
        let named = match names.len() {
            1 => format!("**{}**", names[0]),
            n => format!(
                "{} and **{}**",
                names[..n - 1].iter().map(|s| format!("**{s}**")).collect::<Vec<_>>().join(", "),
                names[n - 1]
            ),
        };
        let first = &hires[0];
        let answer = ctx
            .ask_user(
                &format!(
                    "{named} can do this. Hire {} on the card and I'll pick up right where I left off.",
                    if names.len() == 1 { "them" } else { "them all" }
                ),
                serde_json::json!([{
                    "type": "hire_employee",
                    "code": str_of(first, "code"),
                    "name": names.join(", "),
                    "plugin": str_of(first, "plugin"),
                    "description": str_of(first, "description"),
                    "hires": hires,
                }]),
            )
            .await;
        if answer.as_deref() == Some(INSTALL_CARD_INSTALLED) {
            // Everyone the owner asked for is accounted for here: the newly hired
            // and the ones already on the roster. Nothing is left to search.
            let was_already = if already.is_empty() {
                String::new()
            } else {
                format!(" Already on the roster before this: {}.", already.join(", "))
            };
            return ToolResult::ok(format!(
                "Hired and on the roster: {}.{was_already} Reach them as employees (they appear in \
                 list_employees); no setup narration and no further search needed.",
                names.join(", ")
            ));
        }
        let who = names.join(", ");
        ToolResult::ok(match CardAnswer::read(answer.as_deref(), INSTALL_CARD_INSTALLED) {
            CardAnswer::Failed(reason) => format!(
                "Hiring {who} FAILED: {reason}. Tell the owner that error in plain words and stop. \
                 Do NOT offer the card again and do NOT suggest commands or other ways to hire."
            ),
            CardAnswer::NoAnswer => {
                format!("The hire card for {who} got no answer (the owner stopped the run). Do NOT offer it again.")
            }
            CardAnswer::Skipped | CardAnswer::Done => format!(
                "{listing}\n\nThe owner skipped the hire card for {who}. Do NOT offer it again unless \
                 they ask. Never paste install codes into chat."
            ),
        })
    }

    /// The name a person reads in the Employees list.
    ///
    /// A model asked to hire someone passes an identifier — "receptionist",
    /// "office-manager" — because that is also the folder name. The roster
    /// shows manifest.json's name (the loader prefers it over the AGENT.md
    /// frontmatter slug), so writing the identifier into both is what put a
    /// lowercase "receptionist" next to "Ecommerce Store Manager". Title-case
    /// it for display and leave the slug alone.
    ///
    /// A name that already carries capitals or spaces is the caller's own
    /// wording and comes back untouched.
    fn display_name(name: &str) -> String {
        // Business titles are mostly initialisms, and "Cfo" is no better than
        // "cfo". Anything not listed gets ordinary title case.
        const INITIALISMS: &[&str] = &[
            "ceo", "cfo", "coo", "cto", "cmo", "cio", "cro", "hr", "it", "qa", "ea", "pr",
            "sdr", "ar", "ap", "seo", "ppc", "crm", "kpi", "ai", "bd", "csm",
        ];
        if name.contains(char::is_whitespace) || name.chars().any(char::is_uppercase) {
            return name.to_string();
        }
        name.split(['-', '_'])
            .filter(|w| !w.is_empty())
            .map(|w| {
                if INITIALISMS.contains(&w) {
                    return w.to_uppercase();
                }
                let mut chars = w.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The employee an update names: by name (any case) or id.
    fn find_agent_row(&self, name: &str) -> Result<Option<db::models::Agent>, String> {
        let agents = self.store.list_agents(500, 0).map_err(|e| format!("Failed to query agents: {}", e))?;
        let lower = name.to_lowercase();
        Ok(agents.into_iter().find(|r| r.name.to_lowercase() == lower || r.id == name))
    }

    /// Fields of an update that change what the job is.
    const JOB_FIELDS: &[&str] =
        &["description", "instructions", "agent_md", "automations", "add_automations", "update_automation"];

    /// What a job is made from, read off a create or update call: its
    /// words, the capability terms and plugins it declares, its workflows.
    fn job_parts(input: &serde_json::Value) -> (String, Vec<String>, Vec<String>, Vec<napp::agent::WorkflowBinding>) {
        let text: Vec<&str> = ["description", "instructions", "agent_md"]
            .iter()
            .filter_map(|k| input[*k].as_str())
            .filter(|s| !s.trim().is_empty())
            .collect();
        let strings = |v: &serde_json::Value| -> Vec<String> {
            v.as_array()
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                .unwrap_or_default()
        };
        let terms = strings(&input["requires"]["interfaces"]);
        let plugins = strings(&input["requires"]["plugins"]);
        let mut configs = Vec::new();
        for key in ["automations", "add_automations"] {
            if let Some(autos) = input[key].as_array().filter(|a| !a.is_empty())
                && let Ok(json) = Self::build_agent_json_from_automations(autos)
            {
                configs.push(json.to_string());
            }
        }
        match &input["agent_json"] {
            serde_json::Value::String(raw) => configs.push(raw.clone()),
            raw if raw.is_object() => configs.push(raw.to_string()),
            _ => {}
        }
        let workflows = configs
            .iter()
            .filter_map(|c| napp::agent::parse_agent_config(c).ok())
            .flat_map(|c| c.workflows.into_values())
            .collect();
        (text.join("\n\n"), terms, plugins, workflows)
    }

    /// The one needs step, for a create or update call. `agent_id`: the
    /// employee the job is for, or for a new one the employee creating it.
    async fn work_out(
        consent: &dyn crate::needs::JobConsent,
        agent_id: &str,
        name: &str,
        input: &serde_json::Value,
    ) -> crate::needs::Needs {
        let (description, terms, plugins, workflows) = Self::job_parts(input);
        let src = crate::needs::JobSource {
            name,
            agent_id,
            description: &description,
            skills: &terms,
            plugins: &plugins,
            workflows: &workflows,
            declared: None,
            installed: &[],
        };
        crate::needs::work_out_needs(&src, consent).await
    }

    /// The employee whose run this call is: the one creating or editing.
    fn creator_of(ctx: &ToolContext) -> String {
        match &ctx.grant {
            Some(g) => g.agent_id.clone(),
            None => types::keyparser::extract_agent_id(&ctx.session_key),
        }
    }

    /// Keep a draft of `input`, shown in this call's chat; its id and line.
    fn draft(
        &self,
        ctx: &ToolContext,
        kind: &str,
        agent_id: &str,
        name: &str,
        input: &serde_json::Value,
        needs: &crate::needs::Needs,
    ) -> Result<(String, String), String> {
        let creator_id = Self::creator_of(ctx);
        let chat_id =
            if ctx.session_id.is_empty() { String::new() } else { self.store.resolve_session_chat_id(&ctx.session_id) };
        crate::needs::save_draft(
            &self.store,
            &crate::needs::Draft { kind, agent_id, creator_id: &creator_id, chat_id: &chat_id, name, input, needs },
        )
    }

    /// The consent line as the chat shows it: an inline chip.
    fn consent_payload(draft_id: &str, employee: &str, line: &str, needs: &crate::needs::Needs, adds: bool) -> serde_json::Value {
        serde_json::json!({
            "kind": "employee_consent",
            "draftId": draft_id,
            "employee": employee,
            "line": line,
            "adds": adds,
            "items": needs.items().into_iter().map(|t| t.words).collect::<Vec<_>>(),
        })
    }

    /// What the grant came to, for the model to tell the owner plainly.
    fn granted_text(name: &str, g: &crate::needs::Granted, consented: bool) -> String {
        use crate::needs::consent_line;
        if consented {
            return format!("\nThe owner agreed to this job: {}", consent_line(name, &g.granted));
        }
        let mut text = String::from("\nThe owner has not said yes in this chat, so this grants no more than you hold yourself.");
        if !g.granted.is_empty() {
            text.push_str(&format!(" Granted now: {}", consent_line(name, &g.granted)));
        }
        if !g.extras.is_empty() {
            text.push_str(&format!(
                " Waiting on one card to the owner: \"{}\" Until they answer, {name} can't do that; tell the owner \
                 plainly what is waiting and never say it can yet.",
                consent_line(name, &g.extras)
            ));
        }
        text
    }

    /// Create, in two steps: without `draft_id` the call drafts the
    /// employee, works out its needs and answers with the one line to put to
    /// the owner; with it, the drafted employee is created and its job
    /// granted: in full when the owner said yes in this chat after the line
    /// was shown, else no more than the run's own employee holds, with the
    /// rest on one card to the owner.
    pub(crate) async fn create_with_consent(&self, input: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let Some(consent) = self.job_consent() else {
            return ToolResult::error("Employees can't be created right now: permissions aren't ready yet. Try again shortly.");
        };
        if let Some(draft_id) = input["draft_id"].as_str().filter(|d| !d.is_empty()) {
            let (draft, drafted, needs) = match crate::needs::open_draft(&self.store, draft_id, "create") {
                Ok(d) => d,
                Err(e) => return ToolResult::error(e),
            };
            let consented = consent.owner_consented(draft_id);
            let id = uuid::Uuid::new_v4().to_string();
            let created = self.handle_create(&drafted, &id).await;
            if created.is_error {
                return created;
            }
            let _ = self.store.use_employee_draft(draft_id);
            let job = crate::needs::JobGrant {
                agent_id: &id,
                name: &draft.name,
                needs: &needs,
                draft_id,
                consented,
                created: true,
            };
            let said = match consent.grant(ctx, &job) {
                Ok(g) => Self::granted_text(&draft.name, &g, consented),
                Err(e) => format!("\nIts job was not granted ({e}); it asks before using anything beyond its own notes and tasks."),
            };
            return ToolResult { content: format!("{}{said}", created.content), ..created };
        }
        let name = input["name"].as_str().unwrap_or("");
        if name.is_empty() {
            return ToolResult::error(crate::errors::missing_param(
                "create_employee",
                "name",
                "create_employee(name: \"Office Manager\", description: \"Keeps the office running...\")",
            ));
        }
        if input["description"].as_str().unwrap_or("").is_empty() && input["agent_md"].as_str().unwrap_or("").is_empty() {
            return ToolResult::error("either 'agent_md' or 'description' is required to create an employee");
        }
        if self.agent_loader.user_dir().join(name).exists() {
            return ToolResult::error(format!(
                "An employee named '{}' already exists. Change it with update_employee, or choose another name.",
                name
            ));
        }
        let display = Self::display_name(name);
        let needs = Self::work_out(consent.as_ref(), &Self::creator_of(ctx), &display, input).await;
        let (draft_id, line) = match self.draft(ctx, "create", "", &display, input, &needs) {
            Ok(d) => d,
            Err(e) => return ToolResult::error(e),
        };
        ToolResult::ok(format!(
            "Drafted {display}; nothing is created yet. What it will be able to do, in one line:\n\"{line}\"\n\
             Tell the owner that line in plain words and ask them to confirm. When they say yes, call \
             create_employee(draft_id: \"{draft_id}\") and nothing else: it creates \
             exactly this job. If they want it different, draft again."
        ))
        .with_payload(Self::consent_payload(&draft_id, &display, &line, &needs, false))
    }

    /// Update. An edit that adds to the job works out its needs again and
    /// drafts first, putting only what is new to the owner; the update with
    /// `draft_id` applies the drafted edit and grants the addition on the
    /// owner's yes (an employee never widens another: without it, the
    /// addition is one card to the owner). An edit that adds nothing runs
    /// at once.
    pub(crate) async fn update_with_consent(&self, input: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let consent = self.job_consent();
        if let Some(draft_id) = input["draft_id"].as_str().filter(|d| !d.is_empty()) {
            let Some(consent) = consent else {
                return ToolResult::error("Jobs can't be changed right now: permissions aren't ready yet. Try again shortly.");
            };
            let (draft, drafted, needs) = match crate::needs::open_draft(&self.store, draft_id, "edit") {
                Ok(d) => d,
                Err(e) => return ToolResult::error(e),
            };
            let consented = consent.owner_consented(draft_id);
            let updated = self.handle_update(&drafted).await;
            if updated.is_error {
                return updated;
            }
            let _ = self.store.use_employee_draft(draft_id);
            let job = crate::needs::JobGrant {
                agent_id: &draft.agent_id,
                name: &draft.name,
                needs: &needs,
                draft_id,
                consented,
                created: false,
            };
            let said = match consent.grant(ctx, &job) {
                Ok(g) => Self::granted_text(&draft.name, &g, consented),
                Err(e) => format!("\nThe addition to its job was not granted ({e})."),
            };
            return ToolResult { content: format!("{}{said}", updated.content), ..updated };
        }
        let shapes_the_job = Self::JOB_FIELDS.iter().any(|k| !input[*k].is_null());
        let name = input["name"].as_str().unwrap_or("");
        let (Some(consent), true, false) = (consent, shapes_the_job, name.is_empty()) else {
            return self.handle_update(input).await;
        };
        let Ok(Some(agent)) = self.find_agent_row(name) else {
            return self.handle_update(input).await;
        };
        let before = consent.job_of(&agent.id);
        let after = Self::work_out(consent.as_ref(), &agent.id, &agent.name, input).await;
        let new = crate::needs::added(&before, &after);
        if new.is_empty() {
            return self.handle_update(input).await;
        }
        let (draft_id, line) = match self.draft(ctx, "edit", &agent.id, &agent.name, input, &new) {
            Ok(d) => d,
            Err(e) => return ToolResult::error(e),
        };
        ToolResult::ok(format!(
            "Nothing is changed yet: this edit adds to {}'s job. Only what is new, in one line:\n\"{line}\"\n\
             Tell the owner just that (not what the job already has) and ask them to confirm. When they say yes, \
             call update_employee(draft_id: \"{draft_id}\") and nothing else.",
            agent.name
        ))
        .with_payload(Self::consent_payload(&draft_id, &agent.name, &line, &new, true))
    }

    /// Create the employee `input` describes under `id`. Reached only
    /// through [`Self::create_with_consent`], with the drafted input.
    async fn handle_create(&self, input: &serde_json::Value, id: &str) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");
        if name.is_empty() {
            return ToolResult::error(crate::errors::missing_param(
                "create_employee",
                "name",
                "create_employee(name: \"Office Manager\", description: \"Keeps the office running...\")",
            ));
        }

        let description = input["description"].as_str().unwrap_or("");
        // `name` stays the identifier: it is the folder and the AGENT.md slug.
        // `display` is what the Employees list shows.
        let display = Self::display_name(name);

        // Build agent_json from structured automations, or use raw agent_json
        let mut agent_json: Option<serde_json::Value> = if let Some(autos) = input["automations"].as_array() {
            if autos.is_empty() {
                None
            } else {
                match Self::build_agent_json_from_automations(autos) {
                    Ok(v) => Some(v),
                    Err(e) => return ToolResult::error(e),
                }
            }
        } else {
            let raw = &input["agent_json"];
            match raw.as_str() {
                Some(s) => match serde_json::from_str::<serde_json::Value>(s) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        return ToolResult::error(format!(
                            "agent.json is invalid and was not saved: {}. Fix the config and retry.",
                            e
                        ));
                    }
                },
                None if raw.is_object() => Some(raw.clone()),
                None => None,
            }
        };
        // What the employee needs (plugins, tools, interfaces) lives in
        // agent.json next to its workflows.
        if let Some(requires) = input.get("requires").filter(|r| r.is_object()) {
            agent_json.get_or_insert_with(|| serde_json::json!({}))["requires"] = requires.clone();
        }
        let agent_json_str = agent_json.map(|v| v.to_string());

        // Auto-generate AGENT.md if not provided but name/description exist
        let agent_md_raw = input["agent_md"].as_str().unwrap_or("");
        let agent_md = if agent_md_raw.is_empty() {
            if description.is_empty() {
                return ToolResult::error(
                    "either 'agent_md' or 'description' is required to create an agent",
                );
            }
            // Serialize frontmatter through serde_yaml so values with colons,
            // quotes, etc. are properly escaped — a raw format! wrote YAML the
            // loader could never parse back, creating invisible "ghost" agents
            // whose duties kept running.
            let fm = serde_yaml::to_string(&serde_json::json!({
                "name": name,
                "description": description,
            }))
            .unwrap_or_default();
            format!("---\n{}---\nYou are {}. {}", fm, display, description)
        } else {
            // LLMs often send literal \n instead of real newlines in tool call strings.
            // Unescape so AGENT.md frontmatter parses correctly.
            agent_md_raw.replace("\\n", "\n")
        };

        // Validate BEFORE persisting, with the same parsers the roster loader
        // uses (load_from_dir): never write an agent to disk/DB that the
        // Employees list cannot load — its duties would run invisibly.
        if let Err(e) = napp::agent::parse_agent(&agent_md) {
            return ToolResult::error(format!(
                "AGENT.md is invalid and was not saved: {}. Fix the frontmatter and retry.",
                e
            ));
        }
        if let Some(ref rj) = agent_json_str {
            if let Err(e) = Self::validated_frontmatter(rj) {
                return ToolResult::error(e);
            }
        }
        // An app: `app` marks the manifest; `ui`/`ui_jsx` are its page. A
        // page without `app` still means an app — nobody writes ui/ for an
        // employee that is never served.
        let ui_files = match Self::app_ui_files(input, &display) {
            Ok(files) => files,
            Err(e) => return ToolResult::error(e),
        };
        let app_fields = match input.get("app") {
            Some(app) => match napp::AppFields::from_json(app) {
                Ok(fields) => Some(fields),
                Err(e) => return ToolResult::error(e.to_string()),
            },
            None if !ui_files.is_empty() => Some(napp::AppFields::default()),
            None => None,
        };

        let agent_dir = self.agent_loader.user_dir().join(name);
        if agent_dir.exists() {
            return ToolResult::error(format!(
                "Agent '{}' already exists at {}. Use action: \"update\" to change it, or choose another name.",
                name,
                agent_dir.display()
            ));
        }

        if let Err(e) = std::fs::create_dir_all(&agent_dir) {
            return ToolResult::error(format!("Failed to create directory: {}", e));
        }

        // The DB row's UUID, written into the manifest too: the loader keys
        // the directory by manifest id and would otherwise mint a second one.
        let id = id.to_string();

        // The package through the ONE writer (`napp::write_user_agent`): the
        // manifest (with the row's id, the app fields when it is an app),
        // agent.json, AGENT.md, and the page under ui/ — the same call the
        // REST create and duplicate make.
        let package = napp::AgentPackage {
            id: &id,
            name: &display,
            description,
            agent_md: &agent_md,
            agent_json: agent_json_str.as_deref(),
            app: app_fields.clone(),
            ui: ui_files.clone(),
        };
        if let Err(e) = napp::write_user_agent(&agent_dir, &package) {
            return ToolResult::error(e.to_string());
        }

        // Create DB entry so the agent has a proper UUID
        let frontmatter = agent_json_str.as_deref().unwrap_or("{}");
        match self.store.create_agent(
            &id,
            None,
            &display,
            description,
            &agent_md,
            frontmatter,
            None,
            None,
        ) {
            Ok(_) => {
                let _ = self
                    .store
                    .set_agent_napp_path(&id, &agent_dir.to_string_lossy());
            }
            Err(e) => {
                // Files exist but the roster row does not: the employee would
                // not appear anywhere and every later call would say "not found".
                warn!(name, error = %e, "failed to create DB entry for agent");
                return ToolResult::error(format!(
                    "Created the files for '{}' at {} but the database row failed ({}), so the employee is not registered. Delete that directory and retry, or report the error.",
                    name,
                    agent_dir.display(),
                    e
                ));
            }
        }

        let mut result = format!("Created employee '{}' (id: {})", name, id);
        if app_fields.is_some() {
            result.push_str(&Self::app_created_note(&id, &ui_files));
        }
        let mut has_heartbeat_or_event = false;

        // Parse config and register triggers
        let parsed_config = if let Some(ref rj) = agent_json_str {
            match napp::agent::parse_agent_config(rj) {
                Ok(config) => {
                    self.register_config_triggers(&id, &config);

                    // Describe what was registered
                    let trigger_descs: Vec<String> = config
                        .workflows
                        .iter()
                        .map(|(name, wf)| {
                            let t = match &wf.trigger {
                                napp::agent::AgentTrigger::Schedule { cron, .. } => {
                                    format!("schedule({})", cron)
                                }
                                napp::agent::AgentTrigger::Heartbeat { interval, window } => {
                                    has_heartbeat_or_event = true;
                                    match window {
                                        Some(w) => format!("heartbeat({}, {})", interval, w),
                                        None => format!("heartbeat({})", interval),
                                    }
                                }
                                napp::agent::AgentTrigger::Event { sources } => {
                                    has_heartbeat_or_event = true;
                                    format!("event({})", sources.join(", "))
                                }
                                napp::agent::AgentTrigger::Watch { plugin, .. } => {
                                    has_heartbeat_or_event = true;
                                    format!("watch({})", plugin)
                                }
                                napp::agent::AgentTrigger::Folder { path, .. } => {
                                    has_heartbeat_or_event = true;
                                    format!("folder({})", path)
                                }
                                napp::agent::AgentTrigger::Manual => "manual".to_string(),
                                napp::agent::AgentTrigger::Call { line } => {
                                    format!("call(line: {})", if line.is_empty() { "any" } else { line })
                                }
                            };
                            format!("{} [{}]", name, t)
                        })
                        .collect();
                    if !trigger_descs.is_empty() {
                        result.push_str(&format!("\nAutomations: {}", trigger_descs.join(", ")));
                    }

                    Some(config)
                }
                Err(e) => {
                    result.push_str(&format!("\nWarning: agent.json parse error: {}", e));
                    None
                }
            }
        } else {
            None
        };

        // Auto-activate: insert into agent registry so it appears in sidebar immediately
        let active = ActiveAgent {
            agent_id: id.clone(),
            name: name.to_string(),
            agent_md: agent_md.clone(),
            config: parsed_config,
            channel_id: None,
            degraded: None,
            soul: None,
            rules: None,
        };
        self.agent_registry.write().await.insert(id.clone(), active);
        result.push_str("\nAgent activated and visible in sidebar.");

        if has_heartbeat_or_event {
            result.push_str("\nNote: schedule automations are running now; heartbeat/event/watch automations are registered but do not run until the app restarts.");
        }

        ToolResult::ok(result)
    }

    /// Every field `update` acts on. Anything else in the call is refused
    /// before a byte is written: a field this handler does not read would
    /// otherwise vanish and the result would still say "Updated".
    const UPDATE_FIELDS: &[&str] = &[
        "name",
        "new_name",
        "description",
        "agent_md",
        "instructions",
        "input_values",
        "inputs",
        "toggle_automation",
        "update_automation",
        "automations",
        "add_automations",
        "remove_automations",
        "app",
        "ui",
        "ui_jsx",
        "draft_id",
    ];

    fn unknown_update_fields(input: &serde_json::Value) -> Vec<String> {
        input
            .as_object()
            .map(|o| {
                o.keys()
                    .filter(|k| !Self::UPDATE_FIELDS.contains(&k.as_str()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// New instructions for an employee: the body of AGENT.md replaced, the
    /// frontmatter (identity, inputs, workflows) kept as it is.
    fn replace_agent_body(current_md: &str, body: &str) -> String {
        let trimmed = current_md.trim_start();
        if let Some(rest) = trimmed.strip_prefix("---")
            && let Some(end) = rest.find("\n---")
        {
            let close = end + "\n---".len();
            let frontmatter = &trimmed[..3 + close];
            return format!("{}\n{}\n", frontmatter, body.trim());
        }
        format!("{}\n", body.trim())
    }

    /// The frontmatter an update starts from. An employee with no agent.json
    /// (a blank hire, or one created here without automations) has an empty
    /// column; that is an EMPTY config, not an invalid one. Read as `""` it
    /// fails validation with "EOF while parsing a value" and every update of
    /// such an employee is refused (employee-update-description, 2026-09-17).
    fn stored_frontmatter(stored: &str) -> String {
        if stored.trim().is_empty() {
            "{}".to_string()
        } else {
            stored.to_string()
        }
    }

    /// agent.json is parsed before it is written, never after: a file the
    /// loader rejects must not reach the disk or the DB, or the employee is
    /// refused on every scan from then on.
    fn validated_frontmatter(frontmatter: &str) -> Result<(), String> {
        napp::agent::parse_agent_config(frontmatter)
            .map(|_| ())
            .map_err(|e| format!("agent.json would be invalid and was not saved: {}", e))
    }

    async fn handle_update(&self, input: &serde_json::Value) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");
        if name.is_empty() {
            return ToolResult::error(crate::errors::missing_param(
                "update_employee",
                "name",
                "update_employee(name: \"Office Manager\", description: \"Updated description\")",
            ));
        }

        let unknown = Self::unknown_update_fields(input);
        if !unknown.is_empty() {
            return ToolResult::error(format!(
                "update_employee does not handle {}; nothing was changed. Fields it acts on: {}.",
                unknown.iter().map(|k| format!("`{k}`")).collect::<Vec<_>>().join(", "),
                Self::UPDATE_FIELDS
                    .iter()
                    .map(|k| format!("`{k}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        // Find the agent in DB
        let db_agent = match self.find_agent_row(name) {
            Ok(found) => found,
            Err(e) => return ToolResult::error(e),
        };
        let db_agent = match db_agent {
            Some(r) => r,
            None => {
                return ToolResult::error(format!(
                    "No employee named '{}'. list_employees shows every employee here.",
                    name
                ));
            }
        };

        let agent_id = &db_agent.id;
        let mut current_name = db_agent.name.clone();
        let mut current_desc = db_agent.description.clone();
        let mut current_md = db_agent.agent_md.clone();
        let mut current_frontmatter = Self::stored_frontmatter(&db_agent.frontmatter);
        let mut changes = Vec::new();

        // Update name (rename)
        if let Some(new_name) = input["new_name"].as_str() {
            if !new_name.is_empty() && new_name != current_name {
                // Rename filesystem directory if it exists
                let old_dir = self.agent_loader.user_dir().join(&current_name);
                let new_dir = self.agent_loader.user_dir().join(new_name);
                if old_dir.exists() {
                    if new_dir.exists() {
                        return ToolResult::error(format!(
                            "Cannot rename: '{}' already exists",
                            new_name
                        ));
                    }
                    if let Err(e) = std::fs::rename(&old_dir, &new_dir) {
                        return ToolResult::error(format!("Failed to rename directory: {}", e));
                    }
                    let _ = self
                        .store
                        .set_agent_napp_path(agent_id, &new_dir.to_string_lossy());
                }
                changes.push(format!("renamed to '{}'", new_name));
                current_name = new_name.to_string();
            }
        }

        // Update description
        if let Some(desc) = input["description"].as_str() {
            if !desc.is_empty() {
                current_desc = desc.to_string();
                changes.push("description updated".to_string());
            }
        }

        // Update agent_md (persona)
        if let Some(md) = input["agent_md"].as_str() {
            if !md.is_empty() {
                let candidate = md.replace("\\n", "\n");
                // Same guard as create: never persist an AGENT.md the roster
                // loader cannot parse back.
                if let Err(e) = napp::agent::parse_agent(&candidate) {
                    return ToolResult::error(format!(
                        "AGENT.md is invalid and was not saved: {}. Fix the frontmatter and retry.",
                        e
                    ));
                }
                current_md = candidate;
                // Write to filesystem
                let agent_dir = self.agent_loader.user_dir().join(&current_name);
                if agent_dir.exists() {
                    let _ = std::fs::write(agent_dir.join("AGENT.md"), &current_md);
                }
                changes.push("persona (AGENT.md) updated".to_string());
            }
        }
        // Update the instructions only (`instructions`): the body
        // of AGENT.md, frontmatter kept. Said in the result by that name so
        // nobody reads it as a full AGENT.md replacement.
        let instructions = ["instructions"]
            .iter()
            .find_map(|k| input[*k].as_str().filter(|v| !v.trim().is_empty()).map(|v| (*k, v)));
        if let Some((field, body)) = instructions {
            let candidate = Self::replace_agent_body(&current_md, &body.replace("\\n", "\n"));
            if let Err(e) = napp::agent::parse_agent(&candidate) {
                return ToolResult::error(format!(
                    "AGENT.md would be invalid and was not saved: {}. Fix the `{}` text and retry.",
                    e, field
                ));
            }
            current_md = candidate;
            let agent_dir = self.agent_loader.user_dir().join(&current_name);
            if agent_dir.exists() {
                let _ = std::fs::write(agent_dir.join("AGENT.md"), &current_md);
            }
            // A model asked to change the description often sends `prompt`
            // (audit 2026-09-06) and then reports the description changed.
            let description_note = if input.get("description").and_then(|d| d.as_str()).is_none() {
                "; the description is UNCHANGED (pass description: \"...\" to change it)"
            } else {
                ""
            };
            changes.push(format!(
                "instructions (AGENT.md body) replaced from `{}`; frontmatter kept{}",
                field, description_note
            ));
        }

        // Update input_values (user-supplied configuration values)
        if let Some(vals) = input.get("input_values") {
            if vals.is_object() {
                let vals_str = vals.to_string();
                match self.store.update_agent_input_values(agent_id, &vals_str) {
                    Ok(_) => changes.push("input values updated".to_string()),
                    Err(e) => changes.push(format!("failed to update input values: {}", e)),
                }
            }
        }

        // Update input schema (field definitions in agent.json)
        if let Some(schema) = input.get("inputs") {
            if schema.is_array() {
                let mut fm: serde_json::Value =
                    serde_json::from_str(&current_frontmatter).unwrap_or(serde_json::json!({}));
                fm["inputs"] = schema.clone();
                current_frontmatter = fm.to_string();
                let agent_dir = self.agent_loader.user_dir().join(&current_name);
                if let Err(e) = Self::validated_frontmatter(&current_frontmatter) {
                    return ToolResult::error(e);
                }
                if agent_dir.exists() {
                    let _ = std::fs::write(agent_dir.join("agent.json"), &current_frontmatter);
                }
                changes.push("input field schema updated".to_string());
            }
        }

        // App manifest (`app`) and page (`ui`, `ui_jsx`), through the ONE
        // writer create and the REST doors use. Same rules as create; an
        // `app` block changes only the fields it names, and a manifest
        // already on disk keeps everything the writer is not given.
        if input.get("app").is_some() || input.get("ui").is_some() || input.get("ui_jsx").is_some() {
            let agent_dir = self.agent_loader.user_dir().join(&current_name);
            if !agent_dir.is_dir() {
                return ToolResult::error(format!(
                    "'{}' has no directory on disk (it lives in the database), so app and ui cannot be written. Nothing was changed.",
                    current_name
                ));
            }
            let ui_files = match Self::app_ui_files(input, &current_name) {
                Ok(files) => files,
                Err(e) => return ToolResult::error(e),
            };
            let app_fields = match input.get("app") {
                Some(app) => match napp::AppFields::from_json(app) {
                    Ok(fields) => Some(fields),
                    Err(e) => return ToolResult::error(e.to_string()),
                },
                None if !ui_files.is_empty() => Some(napp::AppFields::default()),
                None => None,
            };
            // An `app` block is a manifest change; a page on its own is not.
            let app_named = input.get("app").is_some();
            let package = napp::AgentPackage {
                id: agent_id,
                name: &current_name,
                description: &current_desc,
                agent_md: &current_md,
                // agent.json is the business of the automation edits above;
                // an app edit never rewrites it.
                agent_json: None,
                app: app_fields,
                ui: ui_files.clone(),
            };
            if let Err(e) = napp::write_user_agent(&agent_dir, &package) {
                return ToolResult::error(e.to_string());
            }
            if app_named {
                changes.push(format!(
                    "app manifest updated. {}",
                    Self::app_created_note(agent_id, &ui_files).trim_start()
                ));
            } else if !ui_files.is_empty() {
                changes.push(
                    Self::app_created_note(agent_id, &ui_files).trim_start().to_string(),
                );
            }
        }

        // toggle_automation: toggle a single binding on/off
        if let Some(binding_name) = input["toggle_automation"].as_str() {
            match self.store.toggle_agent_workflow(agent_id, binding_name) {
                Ok(new_state) => {
                    let state_str = if new_state { "enabled" } else { "disabled" };
                    changes.push(format!("automation '{}' {}", binding_name, state_str));
                }
                Err(e) => changes.push(format!("failed to toggle '{}': {}", binding_name, e)),
            }
        }

        // update_automation: update a single binding by name (non-destructive)
        if let Some(update_obj) = input.get("update_automation") {
            if let Some(binding_name) = update_obj["name"].as_str() {
                let mut fm: serde_json::Value =
                    serde_json::from_str(&current_frontmatter).unwrap_or(serde_json::json!({}));

                if let Some(existing_binding) = fm
                    .get_mut("workflows")
                    .and_then(|w| w.get_mut(binding_name))
                {
                    // Merge individual fields into the existing binding
                    if let Some(desc) = update_obj["description"].as_str() {
                        existing_binding["description"] =
                            serde_json::Value::String(desc.to_string());
                    }
                    if let Some(emit) = update_obj.get("emit") {
                        existing_binding["emit"] = emit.clone();
                    }
                    if let Some(steps) = update_obj["steps"].as_array() {
                        let activities: Vec<serde_json::Value> = steps.iter().enumerate().map(|(i, step)| {
                            let intent = step.as_str().unwrap_or("Execute step");
                            serde_json::json!({ "id": format!("step-{}", i + 1), "intent": intent })
                        }).collect();
                        existing_binding["activities"] = serde_json::Value::Array(activities);
                    }

                    // Update trigger if any trigger field is provided
                    let has_trigger_change = update_obj["schedule"].is_string()
                        || update_obj["interval"].is_string()
                        || !update_obj["sources"].is_null()
                        || update_obj["trigger"].is_string();
                    if has_trigger_change {
                        let trigger_type = if update_obj["schedule"].is_string() {
                            "schedule"
                        } else if update_obj["interval"].is_string() {
                            "heartbeat"
                        } else if !update_obj["sources"].is_null() {
                            "event"
                        } else {
                            update_obj["trigger"].as_str().unwrap_or("manual")
                        };
                        let trigger = match trigger_type {
                            "schedule" => {
                                let raw = update_obj["schedule"].as_str().unwrap_or("0 9 * * *");
                                let cron = Self::normalize_cron(raw);
                                serde_json::json!({ "type": "schedule", "cron": cron })
                            }
                            "heartbeat" => {
                                let interval = update_obj["interval"].as_str().unwrap_or("30m");
                                let mut t = serde_json::json!({ "type": "heartbeat", "interval": interval });
                                if let Some(window) = update_obj["window"].as_str() {
                                    t["window"] = serde_json::Value::String(window.to_string());
                                }
                                t
                            }
                            "event" => {
                                let sources: Vec<serde_json::Value> = if let Some(arr) =
                                    update_obj["sources"].as_array()
                                {
                                    arr.clone()
                                } else if let Some(s) = update_obj["sources"].as_str() {
                                    s.split(',')
                                        .map(|s| serde_json::Value::String(s.trim().to_string()))
                                        .collect()
                                } else {
                                    vec![]
                                };
                                serde_json::json!({ "type": "event", "sources": sources })
                            }
                            _ => serde_json::json!({ "type": "manual" }),
                        };
                        existing_binding["trigger"] = trigger;

                        // Re-register trigger for this binding
                        let cron_name = format!("agent-{}-{}", agent_id, binding_name);
                        let _ = self.store.delete_cron_job_by_name(&cron_name);
                    }

                    current_frontmatter = fm.to_string();
                    let agent_dir = self.agent_loader.user_dir().join(&current_name);
                    if let Err(e) = Self::validated_frontmatter(&current_frontmatter) {
                        return ToolResult::error(e);
                    }
                    if agent_dir.exists() {
                        let _ = std::fs::write(agent_dir.join("agent.json"), &current_frontmatter);
                    }

                    // Re-register triggers for the updated config
                    if let Ok(config) = napp::agent::parse_agent_config(&current_frontmatter) {
                        self.register_config_triggers(agent_id, &config);
                    }

                    // Upsert the workflow binding row in DB
                    if let Ok(config) = napp::agent::parse_agent_config(&current_frontmatter) {
                        if let Some(binding) = config.workflows.get(binding_name) {
                            let (trigger_type, trigger_config) =
                                Self::flatten_trigger(&binding.trigger);
                            let activities_json = serde_json::to_string(&binding.activities).ok();
                            let inputs_json = if binding.inputs.is_empty() {
                                None
                            } else {
                                serde_json::to_string(&binding.inputs).ok()
                            };
                            let connections_json = if binding.connections.is_empty() {
                                None
                            } else {
                                serde_json::to_string(&binding.connections).ok()
                            };
                            let _ = self.store.upsert_agent_workflow(
                                agent_id,
                                binding_name,
                                &trigger_type,
                                &trigger_config,
                                Some(&binding.description),
                                inputs_json.as_deref(),
                                binding.emit_names().as_deref(),
                                activities_json.as_deref(),
                                connections_json.as_deref(),
                                true,
                            );
                        }
                    }

                    changes.push(format!("updated automation '{}'", binding_name));
                } else {
                    changes.push(format!(
                        "automation '{}' not found — use add_automations to create it",
                        binding_name
                    ));
                }
            }
        }

        // Handle automations changes
        let mut automations_changed = false;

        // remove_automations: remove specific automations by name
        if let Some(removals) = input["remove_automations"].as_array() {
            for removal in removals {
                if let Some(binding_name) = removal.as_str() {
                    match self
                        .store
                        .delete_single_agent_workflow(agent_id, binding_name)
                    {
                        Ok(_) => {
                            // Also remove cron job if it was a schedule trigger
                            let cron_name = format!("agent-{}-{}", agent_id, binding_name);
                            let _ = self.store.delete_cron_job_by_name(&cron_name);
                            changes.push(format!("removed automation '{}'", binding_name));
                            automations_changed = true;
                        }
                        Err(e) => {
                            changes.push(format!("failed to remove '{}': {}", binding_name, e));
                        }
                    }
                }
            }
        }

        // automations: replace ALL automations
        if let Some(autos) = input["automations"].as_array() {
            // Clear existing workflows and cron jobs
            let _ = self.store.delete_agent_workflows(agent_id);
            let cron_prefix = format!("agent-{}-", agent_id);
            let _ = self.store.delete_cron_jobs_by_prefix(&cron_prefix);

            // Replace only the workflows key — preserve other frontmatter (inputs, skills, etc.)
            let mut existing: serde_json::Value =
                serde_json::from_str(&current_frontmatter).unwrap_or(serde_json::json!({}));

            if !autos.is_empty() {
                let agent_json = match Self::build_agent_json_from_automations(autos) {
                    Ok(v) => v,
                    Err(e) => return ToolResult::error(e),
                };
                existing["workflows"] = agent_json["workflows"].clone();
                current_frontmatter = existing.to_string();

                // Write to filesystem
                let agent_dir = self.agent_loader.user_dir().join(&current_name);
                if let Err(e) = Self::validated_frontmatter(&current_frontmatter) {
                    return ToolResult::error(e);
                }
                if agent_dir.exists() {
                    let _ = std::fs::write(agent_dir.join("agent.json"), &current_frontmatter);
                }

                if let Ok(config) = napp::agent::parse_agent_config(&current_frontmatter) {
                    self.register_config_triggers(agent_id, &config);
                    changes.push(format!(
                        "replaced all automations ({} total)",
                        config.workflows.len()
                    ));
                }
            } else {
                if let Some(obj) = existing.as_object_mut() {
                    obj.remove("workflows");
                }
                current_frontmatter = existing.to_string();

                // Write to filesystem so agent.json matches the DB
                let agent_dir = self.agent_loader.user_dir().join(&current_name);
                if let Err(e) = Self::validated_frontmatter(&current_frontmatter) {
                    return ToolResult::error(e);
                }
                if agent_dir.exists() {
                    let _ = std::fs::write(agent_dir.join("agent.json"), &current_frontmatter);
                }
                changes.push("removed all automations".to_string());
            }
            automations_changed = true;
        }

        // add_automations: add new automations without removing existing ones
        if let Some(additions) = input["add_automations"].as_array() {
            if !additions.is_empty() {
                let new_json = match Self::build_agent_json_from_automations(additions) {
                    Ok(v) => v,
                    Err(e) => return ToolResult::error(e),
                };
                // Parse BEFORE merging: a config that does not parse must not be
                // written to agent.json or the DB, or the next load fails on it.
                let config = match napp::agent::parse_agent_config(&new_json.to_string()) {
                    Ok(c) => c,
                    Err(e) => {
                        return ToolResult::error(format!(
                            "add_automations rejected: {}. Nothing was written.",
                            e
                        ));
                    }
                };
                self.register_config_triggers(agent_id, &config);
                let names: Vec<&str> = config.workflows.keys().map(|s| s.as_str()).collect();
                changes.push(format!("added automations: {}", names.join(", ")));
                automations_changed = true;

                // Merge into frontmatter for DB storage
                let mut existing: serde_json::Value =
                    serde_json::from_str(&current_frontmatter).unwrap_or(serde_json::json!({}));
                if let Some(new_wfs) = new_json["workflows"].as_object() {
                    let existing_wfs = existing["workflows"]
                        .as_object()
                        .cloned()
                        .unwrap_or_default();
                    let mut merged = existing_wfs;
                    for (k, v) in new_wfs {
                        merged.insert(k.clone(), v.clone());
                    }
                    existing["workflows"] = serde_json::Value::Object(merged);
                }
                current_frontmatter = existing.to_string();

                // Write merged agent.json to filesystem
                let agent_dir = self.agent_loader.user_dir().join(&current_name);
                if let Err(e) = Self::validated_frontmatter(&current_frontmatter) {
                    return ToolResult::error(e);
                }
                if agent_dir.exists() {
                    let _ = std::fs::write(agent_dir.join("agent.json"), &current_frontmatter);
                }
            }
        }

        // Persist DB update
        if let Err(e) = Self::validated_frontmatter(&current_frontmatter) {
            return ToolResult::error(e);
        }
        if let Err(e) = self.store.update_agent(
            agent_id,
            &current_name,
            &current_desc,
            &current_md,
            &current_frontmatter,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        ) {
            return ToolResult::error(format!("Failed to update agent in DB: {}", e));
        }

        // Update live registry if agent is active
        let mut registry = self.agent_registry.write().await;
        if let Some(active) = registry.get_mut(agent_id) {
            active.name = current_name.clone();
            active.agent_md = current_md.clone();
            if automations_changed {
                active.config = napp::agent::parse_agent_config(&current_frontmatter).ok();
            }
            // Only a real change reaches the live agent; an update that
            // changed nothing must not say it did.
            if !changes.is_empty() {
                changes.push("live employee updated".to_string());
            }
        }

        if changes.is_empty() {
            return ToolResult::ok(format!("No changes made to employee '{}'.", current_name));
        }

        // A header that says "Updated" over a list of failures was read as
        // success. Count them, and make any failure the result's verdict.
        let failed = changes.iter().filter(|c| c.starts_with("failed to") || c.contains("not found")).count();
        if failed > 0 {
            return ToolResult::error(format!(
                "Agent '{}' (id: {}): {} change(s) applied, {} failed:\n- {}",
                current_name,
                agent_id,
                changes.len() - failed,
                failed,
                changes.join("\n- ")
            ));
        }
        ToolResult::ok(format!(
            "Updated employee '{}' (id: {}):\n- {}",
            current_name,
            agent_id,
            changes.join("\n- ")
        ))
    }

    pub(crate) async fn handle_delete(&self, input: &serde_json::Value) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");
        if name.is_empty() {
            return ToolResult::error(crate::errors::missing_param(
                "delete_employee",
                "name",
                "delete_employee(name: \"Office Manager\")",
            ));
        }

        // Find in DB
        let db_agent = match self.store.list_agents(500, 0) {
            Ok(agents) => {
                let lower = name.to_lowercase();
                agents
                    .into_iter()
                    .find(|r| r.name.to_lowercase() == lower || r.id == name)
            }
            Err(e) => return ToolResult::error(format!("Failed to query agents: {}", e)),
        };
        let db_agent = match db_agent {
            Some(r) => r,
            None => return ToolResult::error(format!("No employee named '{}'.", name)),
        };

        let agent_id = &db_agent.id;
        let agent_name = &db_agent.name;

        // Remove from live registry
        self.agent_registry.write().await.remove(agent_id);

        // Delete cron jobs for this agent
        let cron_prefix = format!("agent-{}-", agent_id);
        let _ = self.store.delete_cron_jobs_by_prefix(&cron_prefix);

        // Delete agent workflows from DB
        let _ = self.store.delete_agent_workflows(agent_id);

        // Delete agent from DB
        if let Err(e) = self.store.delete_agent(agent_id) {
            return ToolResult::error(format!("Failed to delete agent from DB: {}", e));
        }

        // Remove filesystem directory (user-created only)
        let user_dir = self.agent_loader.user_dir().join(agent_name);
        if user_dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&user_dir) {
                return ToolResult::ok(format!(
                    "Deleted {} from the roster, but failed to remove its directory {}: {}",
                    agent_name,
                    user_dir.display(),
                    e
                ));
            }
        }

        ToolResult::ok(format!(
            "Deleted {} (id: {}): its record, workflows, schedules and files are gone.",
            agent_name, agent_id
        ))
    }

    pub(crate) async fn handle_install(
        &self,
        ctx: &ToolContext,
        input: &serde_json::Value,
    ) -> ToolResult {
        let code = input["code"].as_str().unwrap_or("").trim();
        if code.is_empty() {
            return ToolResult::error(
                "'code' is required (e.g. AGNT-XXXX-XXXX, SKIL-…, PLUG-…, COLL-…)",
            );
        }
        if let Some(reason) = install_code_shape_error(code) {
            return ToolResult::error(reason);
        }
        // ONE canonical install pathway (`codes::handle_code`): redeem + persist + reload,
        // cascade dependencies, download plugin binaries, re-register tools/hooks, payment
        // + auth handling. Routes ANY code type. No direct-API bypass.
        let installer = self.code_installer.read().unwrap().clone();
        match installer {
            Some(installer) => {
                let text = installer.install(code, crate::InstalledBy::of(ctx)).await;
                // The installer trait returns one String for both outcomes; a
                // failure must reach the model as an error, never as success text.
                if install_text_is_failure(&text) {
                    ToolResult::error(text)
                } else {
                    ToolResult::ok(text)
                }
            }
            None => ToolResult::error(
                "install requires the running app (no installer configured).",
            ),
        }
    }

    pub(crate) async fn handle_reload(&self, input: &serde_json::Value) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");
        if name.is_empty() {
            return ToolResult::error(crate::errors::missing_param(
                "reload_employee",
                "name",
                "reload_employee(name: \"Office Manager\")",
            ));
        }
        let check_update = input["check_update"].as_bool().unwrap_or(false);
        let apply_update = input["apply_update"].as_bool().unwrap_or(false);

        // Find the agent in DB
        let db_agent = match self.store.list_agents(500, 0) {
            Ok(agents) => {
                let lower = name.to_lowercase();
                agents
                    .into_iter()
                    .find(|r| r.name.to_lowercase() == lower || r.id == name)
            }
            Err(e) => return ToolResult::error(format!("Failed to query agents: {}", e)),
        };
        let db_agent = match db_agent {
            Some(r) => r,
            None => return ToolResult::error(format!("No employee named '{}'.", name)),
        };

        let agent_id = &db_agent.id;
        let mut changes = Vec::new();
        let mut current_md = db_agent.agent_md.clone();
        let mut current_frontmatter = db_agent.frontmatter.clone();
        let mut current_name = db_agent.name.clone();
        let mut current_desc = db_agent.description.clone();

        // --- Marketplace update check ---
        if (check_update || apply_update) && db_agent.kind.is_some() {
            match crate::build_neboai_api(&self.store) {
                Ok(api) => {
                    match api.get_skill(agent_id).await {
                        Ok(detail) => {
                            let remote_version = &detail.item.version;
                            // Get local version from manifest.json if it exists
                            let manifest_path = db_agent
                                .napp_path
                                .as_ref()
                                .map(|p| std::path::PathBuf::from(p).join("manifest.json"));
                            let local_version_read = manifest_path
                                .as_ref()
                                .and_then(|p| std::fs::read_to_string(p).ok())
                                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                                .and_then(|v| v["version"].as_str().map(|s| s.to_string()));
                            let local_version = local_version_read
                                .clone()
                                .unwrap_or_else(|| "unknown".to_string());

                            if local_version_read.is_none() && !apply_update {
                                // No local version to compare against: report both
                                // facts, never call it an update.
                                let where_looked = manifest_path
                                    .as_ref()
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_else(|| "no install path recorded".to_string());
                                changes.push(format!(
                                    "local version unknown (no manifest.json at {}); marketplace version is {}. Not necessarily an update.",
                                    where_looked, remote_version
                                ));
                            } else if remote_version != &local_version && !remote_version.is_empty() {
                                if apply_update {
                                    // Re-fetch and apply the update
                                    match crate::persist_agent_from_api(
                                        &api,
                                        agent_id,
                                        &db_agent.name,
                                        db_agent.kind.as_deref().unwrap_or(""),
                                        &self.store,
                                    )
                                    .await
                                    {
                                        Ok(_) => {
                                            // Re-read from DB after persist
                                            if let Ok(Some(updated)) =
                                                self.store.get_agent(agent_id)
                                            {
                                                current_md = updated.agent_md;
                                                current_frontmatter = updated.frontmatter;
                                                current_name = updated.name;
                                                current_desc = updated.description;
                                            }
                                            changes.push(format!(
                                                "upgraded from {} → {}",
                                                local_version, remote_version
                                            ));
                                        }
                                        Err(e) => changes.push(format!("upgrade failed: {}", e)),
                                    }
                                } else {
                                    changes.push(format!("update available: {} → {} (use apply_update: true to upgrade)", local_version, remote_version));
                                }
                            } else {
                                changes.push(format!("up to date (version {})", local_version));
                            }
                        }
                        Err(e) => changes.push(format!("failed to check for updates: {}", e)),
                    }
                }
                Err(_) => {
                    changes.push("NeboAI is not connected; connect it in Settings > Account, then retry with check_update.".to_string())
                }
            }

            if check_update && !apply_update {
                // Just checking, don't reload from filesystem
                return ToolResult::ok(format!(
                    "Agent '{}':\n- {}",
                    db_agent.name,
                    changes.join("\n- ")
                ));
            }
        }

        // --- Filesystem reload ---
        let agent_dir = if let Some(ref napp_path) = db_agent.napp_path {
            std::path::PathBuf::from(napp_path)
        } else {
            self.agent_loader.user_dir().join(&db_agent.name)
        };

        if !agent_dir.exists() {
            if changes.is_empty() {
                return ToolResult::error(format!(
                    "Filesystem directory not found: {}. Cannot reload. The DB copy is still the source of truth; use action: \"update\" to change it.",
                    agent_dir.display()
                ));
            }
            // Had marketplace changes but no filesystem — still report
        } else {
            // Reload AGENT.md
            let agent_md_path = agent_dir.join("AGENT.md");
            if agent_md_path.exists() {
                match std::fs::read_to_string(&agent_md_path) {
                    Ok(content) => {
                        if content != current_md {
                            current_md = content;
                            changes.push("AGENT.md reloaded".to_string());
                        }
                    }
                    Err(e) => changes.push(format!("failed to read AGENT.md: {}", e)),
                }
            }

            // Reload agent.json
            let agent_json_path = agent_dir.join("agent.json");
            if agent_json_path.exists() {
                match std::fs::read_to_string(&agent_json_path) {
                    Ok(content) => {
                        if content.trim() != current_frontmatter.trim() {
                            match napp::agent::parse_agent_config(&content) {
                                Ok(config) => {
                                    current_frontmatter = content;

                                    let cron_prefix = format!("agent-{}-", agent_id);
                                    let _ = self.store.delete_cron_jobs_by_prefix(&cron_prefix);
                                    let _ = self.store.delete_agent_workflows(agent_id);
                                    self.register_config_triggers(agent_id, &config);

                                    changes.push(format!(
                                        "agent.json reloaded ({} workflows, {} inputs)",
                                        config.workflows.len(),
                                        config.inputs.len()
                                    ));
                                }
                                Err(e) => {
                                    changes.push(format!("agent.json invalid, skipped: {}", e))
                                }
                            }
                        }
                    }
                    Err(e) => changes.push(format!("failed to read agent.json: {}", e)),
                }
            }
        }

        if changes.is_empty() {
            return ToolResult::ok(format!("{} is already in sync.", db_agent.name));
        }

        // Persist to DB
        if let Err(e) = self.store.update_agent(
            agent_id,
            &current_name,
            &current_desc,
            &current_md,
            &current_frontmatter,
            db_agent.pricing_model.as_deref(),
            db_agent.pricing_cost,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        ) {
            return ToolResult::error(format!("Failed to update DB: {}", e));
        }

        // Update live registry
        let mut registry = self.agent_registry.write().await;
        if let Some(active) = registry.get_mut(agent_id) {
            active.name = current_name.clone();
            active.agent_md = current_md;
            active.config = napp::agent::parse_agent_config(&current_frontmatter).ok();
            changes.push("live employee updated".to_string());
        }

        ToolResult::ok(format!(
            "Agent '{}':\n- {}",
            current_name,
            changes.join("\n- ")
        ))
    }

    pub(crate) async fn handle_repair(&self, input: &serde_json::Value) -> ToolResult {
        let name_filter = input["name"].as_str().unwrap_or("");
        let mut fixes = Vec::new();

        // 1. Fix cron expressions in agent_workflows table
        let agents = self.store.list_agents(500, 0).unwrap_or_default();
        let target_agents: Vec<&db::models::Agent> = if name_filter.is_empty() {
            agents.iter().collect()
        } else {
            let lower = name_filter.to_lowercase();
            agents
                .iter()
                .filter(|r| r.name.to_lowercase() == lower || r.id == name_filter)
                .collect()
        };

        if target_agents.is_empty() && !name_filter.is_empty() {
            return ToolResult::error(format!("No employee named '{}'.", name_filter));
        }

        for agent in &target_agents {
            let bindings = self
                .store
                .list_agent_workflows(&agent.id)
                .unwrap_or_default();
            for binding in &bindings {
                if binding.trigger_type != "schedule" {
                    continue;
                }
                let normalized = Self::normalize_cron(&binding.trigger_config);
                if normalized != binding.trigger_config {
                    // Update agent_workflows
                    // Pass the row's own values back — upsert overwrites every
                    // column, and None here would erase a workflow's activity
                    // graph just to repair its cron.
                    let activities_str = binding.activities.as_ref().map(|v| v.to_string());
                    let connections_str = binding.connections.as_ref().map(|v| v.to_string());
                    if let Err(e) = self.store.upsert_agent_workflow(
                        &agent.id,
                        &binding.binding_name,
                        "schedule",
                        &normalized,
                        binding.description.as_deref(),
                        binding.inputs.as_deref(),
                        binding.emit.as_deref(),
                        activities_str.as_deref(),
                        connections_str.as_deref(),
                        true,
                    ) {
                        fixes.push(format!(
                            "FAILED {}/{}: {} ({})",
                            agent.name, binding.binding_name, normalized, e
                        ));
                        continue;
                    }

                    // Update cron_jobs
                    let cron_name = format!("agent-{}-{}", agent.id, binding.binding_name);
                    let command = format!("agent:{}:{}", agent.id, binding.binding_name);
                    let _ = self.store.delete_cron_job_by_name(&cron_name);
                    let _ = self.store.upsert_cron_job(
                        &cron_name,
                        &normalized,
                        &command,
                        "agent_workflow",
                        None,
                        None,
                        None,
                        true,
                        Some(&agent.id),
                        None,
                        None,
                    );

                    fixes.push(format!(
                        "fixed {}/{}: '{}' → '{}'",
                        agent.name, binding.binding_name, binding.trigger_config, normalized
                    ));
                }
            }

            // 2. Fix cron in frontmatter (agent.json stored in DB)
            if !agent.frontmatter.is_empty() && agent.frontmatter != "{}" {
                if let Ok(mut config) = napp::agent::parse_agent_config(&agent.frontmatter) {
                    let mut frontmatter_changed = false;
                    let mut updated_workflows = config.workflows.clone();

                    for (wf_name, binding) in &config.workflows {
                        if let napp::agent::AgentTrigger::Schedule { cron, .. } = &binding.trigger {
                            let normalized = Self::normalize_cron(cron);
                            if normalized != *cron {
                                let mut updated = binding.clone();
                                updated.trigger = napp::agent::AgentTrigger::Schedule {
                                    cron: normalized.clone(),
                                    schedule: None,
                                };
                                updated_workflows.insert(wf_name.clone(), updated);
                                frontmatter_changed = true;
                                fixes.push(format!(
                                    "fixed {}/{} frontmatter: '{}' → '{}'",
                                    agent.name, wf_name, cron, normalized
                                ));
                            }
                        }
                    }

                    if frontmatter_changed {
                        config.workflows = updated_workflows;
                        if let Ok(new_fm) = serde_json::to_string(&config) {
                            let _ = self.store.update_agent(
                                &agent.id,
                                &agent.name,
                                &agent.description,
                                &agent.agent_md,
                                &new_fm,
                                agent.pricing_model.as_deref(),
                                agent.pricing_cost,
                                None,
                                None,
                                None,
                                None,
                                None,
                                None,
                                None,
                                None,
                            );

                            // Also update agent.json on disk
                            let agent_dir = self.agent_loader.user_dir().join(&agent.name);
                            if agent_dir.join("agent.json").exists() {
                                let _ = std::fs::write(agent_dir.join("agent.json"), &new_fm);
                            }
                        }
                    }
                }
            }

            // 3. Update live registry if active
            let mut registry = self.agent_registry.write().await;
            if let Some(active) = registry.get_mut(&agent.id) {
                if !agent.frontmatter.is_empty() {
                    active.config = napp::agent::parse_agent_config(&agent.frontmatter).ok();
                }
            }
        }

        // 4. Clean up orphan cron_jobs that reference deleted agents
        let cron_jobs = self.store.list_cron_jobs(1000, 0).unwrap_or_default();
        let all_agent_ids: Vec<&str> = agents.iter().map(|r| r.id.as_str()).collect();
        for job in &cron_jobs {
            if job.name.starts_with("agent-") && job.task_type == "agent_workflow" {
                // Extract agent ID from cron name: agent-{uuid}-{binding}
                // UUID format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx (36 chars)
                if let Some(rest) = job.name.strip_prefix("agent-") {
                    if rest.len() > 36 {
                        let aid = &rest[..36];
                        if !all_agent_ids.contains(&aid) {
                            let _ = self.store.delete_cron_job_by_name(&job.name);
                            fixes.push(format!(
                                "removed orphan cron job: {} (agent deleted)",
                                job.name
                            ));
                        }
                    }
                }
            }
        }

        if fixes.is_empty() {
            let scope = if name_filter.is_empty() {
                "all agents"
            } else {
                name_filter
            };
            ToolResult::ok(format!("No repairs needed for {}.", scope))
        } else {
            let failed = fixes.iter().filter(|f| f.starts_with("FAILED ")).count();
            ToolResult::ok(format!(
                "Repaired {} issue(s), {} failed:\n- {}",
                fixes.len() - failed,
                failed,
                fixes.join("\n- ")
            ))
        }
    }

    /// Register triggers from an agent's config into the DB (cron_jobs + agent_workflows).
    fn register_config_triggers(&self, agent_id: &str, config: &napp::agent::AgentConfig) {
        for (binding_name, binding) in &config.workflows {
            let (trigger_type, trigger_config) = match &binding.trigger {
                napp::agent::AgentTrigger::Schedule { cron, .. } => {
                    ("schedule", Self::normalize_cron(cron))
                }
                napp::agent::AgentTrigger::Heartbeat { interval, window } => {
                    let cfg = match window {
                        Some(w) => format!("{}|{}", interval, w),
                        None => interval.clone(),
                    };
                    ("heartbeat", cfg)
                }
                napp::agent::AgentTrigger::Event { sources } => ("event", sources.join(",")),
                napp::agent::AgentTrigger::Watch {
                    plugin,
                    command,
                    event,
                    restart_delay_secs,
                } => {
                    let mut cfg = serde_json::json!({
                        "plugin": plugin,
                        "command": command,
                        "restart_delay_secs": restart_delay_secs
                    });
                    if let Some(ev) = event {
                        cfg["event"] = serde_json::json!(ev);
                    }
                    ("watch", cfg.to_string())
                }
                napp::agent::AgentTrigger::Folder {
                    path,
                    extensions,
                    recursive,
                    debounce_secs,
                } => {
                    let cfg = serde_json::json!({
                        "path": path,
                        "extensions": extensions,
                        "recursive": recursive,
                        "debounce_secs": debounce_secs
                    });
                    ("folder", cfg.to_string())
                }
                napp::agent::AgentTrigger::Manual => ("manual", String::new()),
                napp::agent::AgentTrigger::Call { line } => ("call", line.clone()),
            };

            let inputs_json = if binding.inputs.is_empty() {
                None
            } else {
                serde_json::to_string(&binding.inputs).ok()
            };
            let desc = if binding.description.is_empty() {
                None
            } else {
                Some(binding.description.as_str())
            };

            let activities_json = if binding.activities.is_empty() {
                None
            } else {
                serde_json::to_string(&binding.activities).ok()
            };

            let connections_json = if binding.connections.is_empty() {
                None
            } else {
                serde_json::to_string(&binding.connections).ok()
            };

            if let Err(e) = self.store.upsert_agent_workflow(
                agent_id,
                binding_name,
                trigger_type,
                &trigger_config,
                desc,
                inputs_json.as_deref(),
                binding.emit_names().as_deref(),
                activities_json.as_deref(),
                connections_json.as_deref(),
                true,
            ) {
                warn!(agent = agent_id, binding = %binding_name, error = %e, "failed to upsert agent workflow");
            }
        }

        // Register schedule triggers as cron jobs
        if let Ok(bindings) = self.store.list_agent_workflows(agent_id) {
            for binding in &bindings {
                if binding.trigger_type == "schedule" {
                    let cron_name = format!("agent-{}-{}", agent_id, binding.binding_name);
                    let command = format!("agent:{}:{}", agent_id, binding.binding_name);
                    if let Err(e) = self.store.upsert_cron_job(
                        &cron_name,
                        &binding.trigger_config,
                        &command,
                        "agent_workflow",
                        None,
                        None,
                        None,
                        true,
                        Some(agent_id),
                        None,
                        None,
                    ) {
                        warn!(agent = agent_id, binding = %binding.binding_name, error = %e, "failed to register schedule trigger");
                    }
                }
            }
        }
    }

    /// Convert structured `automations` array into an AgentConfig-compatible agent.json value.
    ///
    /// Each automation entry maps to a WorkflowBinding:
    /// - `name` → binding key
    /// - `trigger` ("schedule"|"heartbeat"|"event"|"manual") + trigger-specific fields
    /// - `steps` string array → AgentActivity objects with auto-generated IDs
    /// - `emit` → emit field on the binding
    /// - `description` → binding description
    /// Normalize a cron expression to the 7-field format required by the `cron` crate.
    ///
    /// The `cron` crate v0.12 expects: `sec min hour dom month dow year`
    /// LLMs commonly produce:
    ///   - Standard 5-field: `min hour dom month dow` (e.g. "0 7 * * *")
    ///   - Time notation: `H:MM` in the hour field (e.g. "0 9:30 * * 1-5")
    ///   - Human-readable: "every 30 seconds", "every 2 minutes", "daily at 7am"
    ///
    /// This function handles all these cases.
    pub fn normalize_cron(expr: &str) -> String {
        let trimmed = expr.trim();

        // Handle human-readable expressions like "every 30 seconds", "weekdays at 9am", etc.
        // " at " catches phrases like "weekdays at 9am" / "mornings at 8" — no real cron
        // expression contains it. Route through fix_dow_field: human_to_cron emits Unix
        // dow numbers (1-5, 0) that the Quartz-convention cron crate would misread.
        let lower = trimmed.to_lowercase();
        if lower.starts_with("every ")
            || lower.starts_with("at ")
            || lower.contains(" at ")
            || lower.contains("daily")
            || lower.contains("weekly")
            || lower.contains("hourly")
            || lower.contains("weekday")
            || lower.contains("weekend")
        {
            return Self::fix_dow_field(&Self::human_to_cron(&lower));
        }

        // Pre-process: fix H:MM or HH:MM notation in fields (e.g. "0 9:30 * * 1-5")
        let processed = Self::fix_time_notation(trimmed);
        let fields: Vec<&str> = processed.split_whitespace().collect();

        let seven_field = match fields.len() {
            5 => format!("0 {} *", processed), // standard 5-field → 7-field
            6 => {
                // Ambiguous: `sec min hour dom mon dow` (the cron crate's own
                // 6-field form — what the schedule editors emit) vs
                // `min hour dom mon dow year` (5-field + year). Only a last
                // field that IS a year means seconds are missing; assuming
                // "missing seconds" for both silently shifted every field of a
                // sec-first cron (hour became dom…) and broke the timer.
                let last = fields[5];
                let is_year = last.len() == 4 && last.chars().all(|c| c.is_ascii_digit());
                if is_year {
                    format!("0 {}", processed) // min-first + year → prepend seconds
                } else {
                    format!("{} *", processed) // sec-first → append year
                }
            }
            7 => processed, // already 7-field
            _ => format!("0 {} * * * *", processed), // best effort
        };
        Self::fix_dow_field(&seven_field)
    }

    /// Translate numeric day-of-week values from the Unix convention
    /// (0=Sunday..6=Saturday, 7=Sunday) into named days (SUN..SAT).
    ///
    /// The `cron` crate uses Quartz ordinals — 1=Sunday..7=Saturday, and 0 is
    /// a parse error. Everything that writes crons here (LLMs, the workflow
    /// builder, `cron_to_human_readable`) assumes Unix numbering, so without
    /// this translation "1-5" (Mon–Fri) fires Sun–Thu and "0,6" (weekends)
    /// never fires at all. Named days are unambiguous in both conventions.
    pub fn fix_dow_field(expr: &str) -> String {
        let mut fields: Vec<String> = expr.split_whitespace().map(String::from).collect();
        if fields.len() != 7 {
            return expr.to_string();
        }
        const NAMES: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];
        let map_tok = |tok: &str| -> String {
            match tok.parse::<u8>() {
                Ok(n) if n <= 7 => NAMES[(n % 7) as usize].to_string(),
                _ => tok.to_string(),
            }
        };
        let translated = fields[5]
            .split(',')
            .map(|part| {
                let (range, step) = match part.split_once('/') {
                    Some((r, s)) => (r, Some(s)),
                    None => (part, None),
                };
                let mapped = match range.split_once('-') {
                    Some((a, b)) => format!("{}-{}", map_tok(a), map_tok(b)),
                    None => map_tok(range),
                };
                match step {
                    Some(s) => format!("{}/{}", mapped, s),
                    None => mapped,
                }
            })
            .collect::<Vec<_>>()
            .join(",");
        fields[5] = translated;
        fields.join(" ")
    }

    /// Fix H:MM or HH:MM time notation in cron fields.
    ///
    /// LLMs write "0 9:30 * * 1-5" meaning "at 9:30, weekdays".
    /// This converts the H:MM to proper minute and hour fields.
    pub fn fix_time_notation(expr: &str) -> String {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        let mut result: Vec<String> = Vec::new();
        let mut i = 0;

        while i < fields.len() {
            let field = fields[i];
            if field.contains(':') {
                // Split H:MM into separate hour and minute fields
                let parts: Vec<&str> = field.split(':').collect();
                if parts.len() == 2 {
                    let hour = parts[0];
                    let minute = parts[1];
                    // If this is the second field (index 1 in 5-field cron), the preceding
                    // field is likely "0" (minute placeholder). Replace it with the actual minute.
                    if i > 0 && result.last().map_or(false, |f| f == "0") {
                        result.pop();
                        result.push(minute.to_string());
                    } else {
                        result.push(minute.to_string());
                    }
                    result.push(hour.to_string());
                } else {
                    result.push(field.to_string());
                }
            } else {
                result.push(field.to_string());
            }
            i += 1;
        }

        result.join(" ")
    }

    /// Convert human-readable schedule expressions to 7-field cron.
    ///
    /// Handles: "every N seconds/minutes/hours", "daily at Ham/Hpm",
    ///          "hourly", "weekly", "every weekday at H:MM"
    pub fn human_to_cron(expr: &str) -> String {
        let lower = expr.trim().to_lowercase();

        // "every N seconds" → */N * * * * * *
        if lower.contains("second") {
            if let Some(n) = Self::extract_number(&lower) {
                return format!("*/{} * * * * * *", n);
            }
            return "*/30 * * * * * *".to_string(); // default: every 30s
        }

        // "every N minutes" → 0 */N * * * * *
        if lower.contains("minute") {
            if let Some(n) = Self::extract_number(&lower) {
                return format!("0 */{} * * * * *", n);
            }
            return "0 */5 * * * * *".to_string(); // default: every 5min
        }

        // "every N hours" or "hourly" → 0 0 */N * * * *
        if lower.contains("hour") {
            if let Some(n) = Self::extract_number(&lower) {
                return format!("0 0 */{} * * * *", n);
            }
            return "0 0 * * * * *".to_string(); // every hour
        }

        // "daily at H" / "daily at H:MM" / "daily at Ham/Hpm"
        if lower.contains("daily") || lower.starts_with("at ") {
            let (hour, minute) = Self::extract_time(&lower);
            return format!("0 {} {} * * * *", minute, hour);
        }

        // "weekly" → Sunday at midnight
        if lower.contains("weekly") {
            let (hour, minute) = Self::extract_time(&lower);
            return format!("0 {} {} * * 0 *", minute, hour);
        }

        // "weekday" / "weekdays" → Mon-Fri
        if lower.contains("weekday") {
            let (hour, minute) = Self::extract_time(&lower);
            return format!("0 {} {} * * 1-5 *", minute, hour);
        }

        // "weekend" / "weekends" → Sat-Sun
        if lower.contains("weekend") {
            let (hour, minute) = Self::extract_time(&lower);
            return format!("0 {} {} * * 0,6 *", minute, hour);
        }

        // Fallback: daily at 9am
        "0 0 9 * * * *".to_string()
    }

    /// Extract the first number from a string.
    pub fn extract_number(s: &str) -> Option<u32> {
        s.split_whitespace()
            .find_map(|word| word.parse::<u32>().ok())
    }

    /// Extract hour and minute from a human-readable time expression.
    /// Returns (hour, minute) as strings for cron fields.
    pub fn extract_time(s: &str) -> (String, String) {
        // Look for H:MM pattern
        for word in s.split_whitespace() {
            let clean = word.trim_end_matches(|c: char| !c.is_ascii_digit());
            if clean.contains(':') {
                let parts: Vec<&str> = clean.split(':').collect();
                if parts.len() == 2 {
                    if let (Ok(mut h), Ok(m)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                        // Handle am/pm suffix
                        if word.to_lowercase().contains("pm") && h < 12 {
                            h += 12;
                        }
                        return (h.to_string(), m.to_string());
                    }
                }
            }
            // Look for Hpm / Ham pattern (e.g. "7am", "6pm")
            let is_pm = word.to_lowercase().ends_with("pm");
            let is_am = word.to_lowercase().ends_with("am");
            if is_pm || is_am {
                let num_part = word.trim_end_matches(|c: char| !c.is_ascii_digit());
                if let Ok(mut h) = num_part.parse::<u32>() {
                    if is_pm && h < 12 {
                        h += 12;
                    }
                    if is_am && h == 12 {
                        h = 0;
                    }
                    return (h.to_string(), "0".to_string());
                }
            }
        }

        // Look for bare number after "at"
        if let Some(at_pos) = s.find("at ") {
            let after_at = &s[at_pos + 3..];
            for word in after_at.split_whitespace() {
                if let Ok(h) = word.parse::<u32>() {
                    if h <= 23 {
                        return (h.to_string(), "0".to_string());
                    }
                }
            }
        }

        // Default: midnight
        ("0".to_string(), "0".to_string())
    }

    /// Convert a parsed AgentTrigger into flat (type, config) strings for DB storage.
    fn flatten_trigger(trigger: &napp::agent::AgentTrigger) -> (String, String) {
        match trigger {
            napp::agent::AgentTrigger::Schedule { cron, .. } => {
                ("schedule".to_string(), cron.clone())
            }
            napp::agent::AgentTrigger::Heartbeat { interval, window } => {
                let config = match window {
                    Some(w) => format!("{}|{}", interval, w),
                    None => interval.clone(),
                };
                ("heartbeat".to_string(), config)
            }
            napp::agent::AgentTrigger::Event { sources } => {
                ("event".to_string(), sources.join(","))
            }
            napp::agent::AgentTrigger::Watch {
                plugin,
                command,
                event,
                restart_delay_secs,
            } => {
                let mut cfg = serde_json::json!({
                    "plugin": plugin,
                    "command": command,
                    "restart_delay_secs": restart_delay_secs
                });
                if let Some(ev) = event {
                    cfg["event"] = serde_json::json!(ev);
                }
                ("watch".to_string(), cfg.to_string())
            }
            napp::agent::AgentTrigger::Folder {
                path,
                extensions,
                recursive,
                debounce_secs,
            } => {
                let cfg = serde_json::json!({
                    "path": path,
                    "extensions": extensions,
                    "recursive": recursive,
                    "debounce_secs": debounce_secs
                });
                ("folder".to_string(), cfg.to_string())
            }
            napp::agent::AgentTrigger::Manual => ("manual".to_string(), String::new()),
            napp::agent::AgentTrigger::Call { line } => ("call".to_string(), line.clone()),
        }
    }

    /// The script an app page loads for `NeboAppSDK`.
    const APP_SDK_SCRIPT: &str = "<script src=\"/sdk/nebo.global.js\"></script>";

    /// The files of an app's `ui/` directory from the call: `ui` (relative
    /// path → content) and `ui_jsx` (one component, compiled to
    /// `index.html`). Every path is checked before a byte is written.
    fn app_ui_files(
        input: &serde_json::Value,
        title: &str,
    ) -> Result<Vec<(std::path::PathBuf, Vec<u8>)>, String> {
        let mut files = Vec::new();
        if let Some(ui) = input.get("ui") {
            let Some(map) = ui.as_object() else {
                return Err(
                    "`ui` must be a map of relative path → file content, e.g. {\"index.html\": \"<!doctype html>...\", \"app.js\": \"...\"}. Nothing was written."
                        .into(),
                );
            };
            for (path, content) in map {
                let Some(text) = content.as_str() else {
                    return Err(format!(
                        "`ui[\"{path}\"]` must be a string (the file's content). Nothing was written."
                    ));
                };
                files.push((
                    napp::user_agent::ui_relative_path(path).map_err(|e| e.to_string())?,
                    text.as_bytes().to_vec(),
                ));
            }
        }
        if let Some(src) = input.get("ui_jsx") {
            let Some(src) = src.as_str() else {
                return Err(
                    "`ui_jsx` must be a string: the source of ONE JSX/TSX file that `export default`s a React component. Nothing was written."
                        .into(),
                );
            };
            files.push((
                std::path::PathBuf::from("index.html"),
                Self::app_ui_from_jsx(src, title)?.into_bytes(),
            ));
        }
        Ok(files)
    }

    /// `ui_jsx` through the one JSX converter — the same call
    /// `os(file, convert, to: "html")` makes — then given the app SDK: the
    /// converter's shell is an artifact page, and an app page also needs
    /// `NeboAppSDK`. One string carries no extension to pick the language
    /// by, so plain JSX is tried first and TSX when only types make it parse.
    fn app_ui_from_jsx(src: &str, title: &str) -> Result<String, String> {
        let html = match render::jsx_to_html(src, title, render::JsxLang::Jsx) {
            Ok(html) => html,
            Err(jsx_err) => render::jsx_to_html(src, title, render::JsxLang::Tsx)
                .map_err(|_| format!("`ui_jsx` did not compile and nothing was written: {jsx_err}"))?,
        };
        Ok(html.replacen("</head>", &format!("{}\n</head>", Self::APP_SDK_SCRIPT), 1))
    }

    /// What a create or update says once an app's manifest and page are on
    /// disk: where it is served and what its page can call.
    fn app_created_note(id: &str, ui_files: &[(std::path::PathBuf, Vec<u8>)]) -> String {
        let page = if ui_files.is_empty() {
            "No ui/ files yet — add them with update (ui: {\"index.html\": ...} or ui_jsx) before opening it.".to_string()
        } else {
            format!(
                "ui/ files written: {}.",
                ui_files
                    .iter()
                    .map(|(p, _)| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        format!(
            "\nThis employee is an app. {page} It is served at /apps/{id}/ui within seconds (the loader watches the directory; no restart). The page gets `NeboAppSDK` from {}.",
            Self::APP_SDK_SCRIPT
        )
    }

    /// Workflow bindings from the tool's `automations` shape. Refuses, before
    /// anything is written, the two shapes the loader rejects on every scan
    /// afterwards: an event automation with no sources and a schedule
    /// automation with no schedule (fourteen warnings in one day's log for
    /// employees created with `sources: []`, 2026-09-05).
    fn build_agent_json_from_automations(automations: &[serde_json::Value]) -> Result<serde_json::Value, String> {
        let mut workflows = serde_json::Map::new();

        for auto in automations {
            let binding_name = auto["name"].as_str().unwrap_or("default");

            // Auto-infer trigger type from fields present — don't rely on LLM
            // setting the "trigger" field correctly when context fields exist.
            let trigger_type = if auto["schedule"].is_string() {
                "schedule"
            } else if auto["interval"].is_string() {
                "heartbeat"
            } else if !auto["sources"].is_null() {
                "event"
            } else {
                auto["trigger"].as_str().unwrap_or("manual")
            };

            // Build trigger object
            let trigger = match trigger_type {
                "schedule" => {
                    let raw = auto["schedule"].as_str().filter(|s| !s.trim().is_empty()).ok_or_else(|| {
                        format!(
                            "automation '{binding_name}': a schedule automation needs `schedule` (a cron or a phrase such as \"weekdays at 9am\"). \
                             Example: {{\"name\": \"{binding_name}\", \"schedule\": \"0 9 * * 1-5\", \"steps\": [\"...\"]}}. Nothing was written."
                        )
                    })?;
                    let cron = Self::normalize_cron(raw);
                    serde_json::json!({ "type": "schedule", "cron": cron })
                }
                "heartbeat" => {
                    let interval = auto["interval"].as_str().unwrap_or("30m");
                    let mut t = serde_json::json!({ "type": "heartbeat", "interval": interval });
                    if let Some(window) = auto["window"].as_str() {
                        t["window"] = serde_json::Value::String(window.to_string());
                    }
                    t
                }
                "event" => {
                    let sources: Vec<serde_json::Value> =
                        if let Some(arr) = auto["sources"].as_array() {
                            arr.clone()
                        } else if let Some(s) = auto["sources"].as_str() {
                            s.split(',')
                                .map(|s| serde_json::Value::String(s.trim().to_string()))
                                .collect()
                        } else {
                            vec![]
                        };
                    let sources: Vec<serde_json::Value> = sources
                        .into_iter()
                        .filter(|s| s.as_str().is_none_or(|s| !s.trim().is_empty()))
                        .collect();
                    if sources.is_empty() {
                        return Err(format!(
                            "automation '{binding_name}': an event automation needs at least one source in `sources`; the loader refuses an empty list on every scan. \
                             Example: {{\"name\": \"{binding_name}\", \"sources\": [\"email.received\"], \"steps\": [\"...\"]}}. \
                             For a duty with no trigger use \"trigger\": \"manual\". Nothing was written."
                        ));
                    }
                    serde_json::json!({ "type": "event", "sources": sources })
                }
                _ => serde_json::json!({ "type": "manual" }),
            };

            // Activities. Advanced form: a full `activities` array (multi-stage,
            // EA-manifest shape — each {id, intent, steps, skills…}) passes
            // through as-is. Simple form: `steps` become ONE activity that
            // executes them in order — the engine runs each activity as its own
            // scoped LLM execution, so splitting steps across activities would
            // produce disconnected one-line runs with no shared context.
            let activities: Vec<serde_json::Value> = if let Some(acts) = auto["activities"].as_array()
            {
                acts.clone()
            } else if let Some(steps) = auto["steps"].as_array() {
                let intent = auto["description"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(binding_name);
                vec![serde_json::json!({
                    "id": "run",
                    "intent": intent,
                    "steps": steps
                })]
            } else {
                vec![]
            };

            let mut binding = serde_json::json!({
                "trigger": trigger,
                "activities": activities
            });

            if let Some(desc) = auto["description"].as_str() {
                binding["description"] = serde_json::Value::String(desc.to_string());
            }
            if let Some(emit) = auto["emit"].as_str() {
                binding["emit"] = serde_json::Value::String(emit.to_string());
            }

            workflows.insert(binding_name.to_string(), binding);
        }

        Ok(serde_json::json!({ "workflows": workflows }))
    }

    pub(crate) async fn handle_stats(&self, input: &serde_json::Value) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");
        if name.is_empty() {
            return ToolResult::error(crate::errors::missing_param(
                "employee_stats",
                "name",
                "employee_stats(name: \"Office Manager\")",
            ));
        }

        // Resolve agent_id from DB
        let db_agent = match self.store.list_agents(500, 0) {
            Ok(agents) => {
                let lower = name.to_lowercase();
                agents
                    .into_iter()
                    .find(|r| r.name.to_lowercase() == lower || r.id == name)
            }
            Err(e) => return ToolResult::error(format!("Failed to query agents: {}", e)),
        };
        let db_agent = match db_agent {
            Some(r) => r,
            None => {
                return ToolResult::error(format!(
                    "No employee named '{}'. list_employees shows every employee here.",
                    name
                ));
            }
        };

        let agent_id = &db_agent.id;

        let stats = match self.store.agent_workflow_stats(agent_id) {
            Ok(s) => s,
            Err(e) => return ToolResult::error(format!("Failed to query stats: {}", e)),
        };

        if stats.total_runs == 0 {
            return ToolResult::ok(format!(
                "## Stats for {}\n\nNo workflow runs recorded yet.",
                db_agent.name
            ));
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // Format duration
        let duration_str = match stats.avg_duration_secs {
            Some(secs) if secs >= 60 => format!("{}m {}s", secs / 60, secs % 60),
            Some(secs) => format!("{}s", secs),
            None => "-".to_string(),
        };

        // Relative time plus the absolute timestamp it was computed from.
        let relative = |ts: Option<i64>| -> String {
            match ts {
                Some(t) => {
                    let diff = now - t;
                    let rel = if diff < 60 {
                        format!("{}s ago", diff)
                    } else if diff < 3600 {
                        format!("{}m ago", diff / 60)
                    } else if diff < 86400 {
                        format!("{}h ago", diff / 3600)
                    } else {
                        format!("{}d ago", diff / 86400)
                    };
                    match chrono::DateTime::<chrono::Utc>::from_timestamp(t, 0) {
                        Some(dt) => format!("{} ({})", rel, dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
                        None => format!("{} (unix {})", rel, t),
                    }
                }
                None => "-".to_string(),
            }
        };

        let mut out = format!(
            "## Stats for {}\n\n\
             Runs: {} total ({} completed, {} failed, {} cancelled, {} running)\n\
             Tokens: {} total\n\
             Avg duration: {}\n\
             Last run: {}",
            db_agent.name,
            stats.total_runs,
            stats.completed,
            stats.failed,
            stats.cancelled,
            stats.running,
            stats.total_tokens,
            duration_str,
            relative(stats.last_run_at),
        );

        if let Some(ref err) = stats.last_error {
            out.push_str(&format!("\nLast error: \"{}\"", err));
        }

        // Recent errors
        let errors = self
            .store
            .agent_recent_errors(agent_id, 5)
            .unwrap_or_default();
        if !errors.is_empty() {
            out.push_str("\n\n### Recent Errors");
            for (i, e) in errors.iter().enumerate() {
                let activity = e.activity_id.as_deref().unwrap_or("unknown");
                out.push_str(&format!(
                    "\n{}. [{}] activity \"{}\": {}",
                    i + 1,
                    relative(Some(e.started_at)),
                    activity,
                    e.error,
                ));
            }
        }

        ToolResult::ok(out)
    }

    pub(crate) async fn handle_setup(&self, input: &serde_json::Value) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");
        if name.is_empty() {
            return ToolResult::error(crate::errors::missing_param(
                "setup_employee",
                "name",
                "setup_employee(name: \"Office Manager\")",
            ));
        }

        let db_agent = match self.store.list_agents(500, 0) {
            Ok(agents) => {
                let lower = name.to_lowercase();
                agents
                    .into_iter()
                    .find(|r| r.name.to_lowercase() == lower || r.id == name)
            }
            Err(e) => return ToolResult::error(format!("Failed to query agents: {}", e)),
        };
        let db_agent = match db_agent {
            Some(r) => r,
            None => return ToolResult::error(format!("No employee named '{}'.", name)),
        };

        // The marker the frontend acts on rides in the structured payload channel
        // (serde-escaped), never in the model-facing text.
        ToolResult::ok(format!(
            "Setup form for '{}' opened in the Configure tab.",
            db_agent.name
        ))
        .with_payload(serde_json::json!({
            "kind": "agent_setup",
            "__agentSetup": true,
            "agentId": db_agent.id,
            "agentName": db_agent.name,
            "agentDescription": db_agent.description,
        }))
    }

    /// Find an agent by name across loader cache and DB.
    async fn find_agent(&self, name: &str) -> Option<napp::agent_loader::LoadedAgent> {
        // Check loader cache first (exact lowercase key)
        if let Some(agent) = self.agent_loader.get_by_name(name).await {
            return Some(agent);
        }
        // Try normalized form: "chief-of-staff" → "chief of staff"
        let normalized = name.to_lowercase().replace(['-', '_'], " ");
        if normalized != name.to_lowercase() {
            if let Some(agent) = self.agent_loader.get_by_name(&normalized).await {
                return Some(agent);
            }
        }

        // Fallback: check DB (agents created via REST API or marketplace install).
        // Matching uses the ONE normalizer (comm::handle::slugify) so a name
        // resolves to the same agent on every rail — the loader probes above
        // keep their space-form because that is the loader's own key format.
        let slug = comm::handle::slugify(name);
        if let Ok(db_agents) = self.store.list_agents(500, 0) {
            for r in db_agents {
                if comm::handle::slugify(&r.name) == slug || r.id == name {
                    // The body is the persona after the frontmatter, as the
                    // loader parses it; the raw file stays in `agent_md`.
                    let agent_def = napp::agent::AgentDef {
                        id: r.id.clone(),
                        name: r.name.clone(),
                        description: r.description.clone(),
                        body: Self::agent_body(&r.agent_md),
                    };
                    let config = if !r.frontmatter.is_empty() {
                        napp::agent::parse_agent_config(&r.frontmatter).ok()
                    } else {
                        None
                    };
                    // A row created in the app or by the tool lives under the
                    // user directory and records that path. Reporting it as
                    // "marketplace" at the installed root sent a live run on a
                    // forty-call hunt through the wrong tree (2026-09-05).
                    let dir = r.napp_path.clone().map(std::path::PathBuf::from);
                    // The same classification `info` prints: only a path in
                    // the installed tree (or a sealed .napp) is a marketplace
                    // install; a row with no directory is a local employee.
                    let source = if Self::source_label(
                        dir.as_deref(),
                        self.agent_loader.user_dir(),
                        self.agent_loader.installed_dir(),
                    ) == SOURCE_MARKETPLACE
                    {
                        napp::agent_loader::AgentSource::Installed
                    } else {
                        napp::agent_loader::AgentSource::User
                    };
                    // No recorded directory means the employee lives only in
                    // the database; an empty path says so to `info`.
                    let source_path = dir.clone().unwrap_or_default();
                    return Some(napp::agent_loader::LoadedAgent {
                        agent_def,
                        config,
                        source,
                        napp_path: dir,
                        source_path,
                        version: None,
                        agent_md: r.agent_md.clone(),
                        frontmatter: r.frontmatter.clone(),
                        description: r.description.clone(),
                        id: Some(r.id.clone()),
                        theme_css: None,
                        is_app: false,
                        app_ui_path: None,
                        app_binary_path: None,
                        app_window_config: None,
                    });
                }
            }
        }

        None
    }
}

/// The only shape a marketplace install code has: a 4-letter prefix and two
/// groups of 4 letters or digits. A "code" built from an employee's name
/// ("SKIL-LAW-FIRM-RECEPTIONIST", live on 2026-09-05) is refused here, before
/// any network call, with the fact that matters: codes are issued, not made.
fn install_code_shape_error(code: &str) -> Option<String> {
    // The marketplace contract: [A-Z]{4}-[A-Z0-9]{4}-[A-Z0-9]{4}. The server
    // uppercases before checking, so case is forgiven here too; the server
    // additionally limits the groups to Crockford base32 characters.
    let upper = code.to_ascii_uppercase();
    let parts: Vec<&str> = upper.split('-').collect();
    let well_formed = parts.len() == 3
        && parts[0].len() == 4
        && parts[0].chars().all(|c| c.is_ascii_uppercase())
        && parts[1..].iter().all(|g| g.len() == 4 && g.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()));
    if well_formed {
        return None;
    }
    Some(format!(
        "'{code}' is not an install code. Codes are issued by the marketplace and look like \
         PREFIX-XXXX-XXXX (four letters, then two groups of four); they are never built from a \
         name. To hire an employee, find it with find_employees — \
         the card it offers installs it. {TOOL_INSTALL_DOOR} \
         An employee that is already on the roster \
         (list_employees) needs no install: use get_employee, \
         update_employee or reload_employee on it."
    ))
}

/// Whether a [`CodeInstaller::install`] result string reports a failure. The
/// installer returns one String for both outcomes (`server::codes::handle_code_text`
/// renders errors as "Failed to install {kind}: {e}", and an unparseable code as
/// "'{code}' is not a valid install code"); the tool result must carry
/// `is_error` for those, so the model never reads a failed install as done.
fn install_text_is_failure(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with("Failed to install") || t.contains("is not a valid install code")
}

#[cfg(test)]
mod tests {

    // The owner's rule: NeboAI's employees first. The hub's own order is
    // kept inside each group, so a third-party listing never jumps ahead and
    // NeboAI's top result stays NeboAI's top result.
    #[test]
    fn neboai_listings_come_first_in_hub_order() {
        let mut items = vec![
            serde_json::json!({"slug": "a", "qualifiedName": "@acme/agents/a"}),
            serde_json::json!({"slug": "b", "qualifiedName": "@neboai/agents/b"}),
            serde_json::json!({"slug": "c", "qualifiedName": "@other/agents/c"}),
            serde_json::json!({"slug": "d", "qualifiedName": "@neboai/agents/d"}),
            serde_json::json!({"slug": "e"}),
        ];
        prefer_neboai(&mut items);
        let order: Vec<&str> = items.iter().map(|i| i["slug"].as_str().unwrap()).collect();
        assert_eq!(order, vec!["b", "d", "a", "c", "e"]);
    }

    // An employee hired by asking Nebo used to land in the roster under the
    // identifier the model passed, so "receptionist" sat next to
    // "Ecommerce Store Manager".
    #[test]
    fn slug_names_become_display_names() {
        assert_eq!(PersonaTool::display_name("receptionist"), "Receptionist");
        assert_eq!(PersonaTool::display_name("office-manager"), "Office Manager");
        assert_eq!(PersonaTool::display_name("accounts_payable_clerk"), "Accounts Payable Clerk");
    }

    // "Cfo" would be no better than "cfo".
    #[test]
    fn initialisms_stay_upper() {
        assert_eq!(PersonaTool::display_name("cfo"), "CFO");
        assert_eq!(PersonaTool::display_name("hr-coordinator"), "HR Coordinator");
        assert_eq!(PersonaTool::display_name("sdr"), "SDR");
    }

    // A caller who wrote the name themselves gets exactly what they wrote.
    #[test]
    fn deliberate_names_are_left_alone() {
        assert_eq!(PersonaTool::display_name("Ecommerce Store Manager"), "Ecommerce Store Manager");
        assert_eq!(PersonaTool::display_name("McKinsey Analyst"), "McKinsey Analyst");
        assert_eq!(PersonaTool::display_name("eBay Lister"), "eBay Lister");
    }
    use super::*;

    /// An install failure string from the installer must surface as an error
    /// result, and a success string must not.
    #[test]
    fn a_code_built_from_a_name_is_refused_before_any_install() {
        let e = install_code_shape_error("SKIL-LAW-FIRM-RECEPTIONIST").expect("refused");
        assert!(e.contains("never built from a name"), "{e}");
        assert!(e.contains("needs no install"), "{e}");
        assert!(install_code_shape_error("agnt-abcd-1234").is_none(), "the server uppercases, so must we");
        assert!(install_code_shape_error("AGNT-ABCD-12").is_some(), "short group");
        assert!(install_code_shape_error("AG1T-ABCD-1234").is_some(), "digit in the prefix");
        assert!(install_code_shape_error("AGNT-AB_D-1234").is_some(), "punctuation in a group");
        assert!(install_code_shape_error("AGNT-ABCD-1234").is_none());
        assert!(install_code_shape_error("PLUG-3TKG-GHKN").is_none(), "a real code from today");
        // The sentence pointing at the tool catalog is said once, from the
        // one constant, wherever it is still needed.
        assert!(e.contains(TOOL_INSTALL_DOOR), "{e}");
    }

    #[test]
    fn install_failure_text_is_detected() {
        assert!(install_text_is_failure("Failed to install plugin: 404 not found"));
        assert!(install_text_is_failure(
            "'XYZ' is not a valid install code — expected PREFIX-XXXX-XXXX"
        ));
        assert!(!install_text_is_failure("Installed skill 'writer' (v1.2.0)."));
        assert!(!install_text_is_failure(
            "Installed collection: 3 installed. Payment required: https://x"
        ));
    }

    #[test]
    fn update_refuses_fields_it_does_not_handle() {
        let call = serde_json::json!({"name": "Receptionist", "persona_text": "x"});
        assert_eq!(PersonaTool::unknown_update_fields(&call), vec!["persona_text".to_string()]);
        let ok = serde_json::json!({"name": "Receptionist", "instructions": "x"});
        assert!(PersonaTool::unknown_update_fields(&ok).is_empty());
    }

    /// An employee with no agent.json updates cleanly: its empty frontmatter
    /// column is an empty config, which validates; the raw empty string does not.
    #[test]
    fn empty_frontmatter_is_an_empty_config_not_an_invalid_one() {
        assert!(PersonaTool::validated_frontmatter("").is_err());
        let fm = PersonaTool::stored_frontmatter("");
        assert_eq!(fm, "{}");
        assert!(PersonaTool::validated_frontmatter(&fm).is_ok());
        assert_eq!(PersonaTool::stored_frontmatter("{\"workflows\":{}}"), "{\"workflows\":{}}");
    }

    #[test]
    fn new_instructions_replace_the_body_and_keep_the_frontmatter() {
        let md = "---\nname: receptionist\ndescription: answers calls\n---\nOld instructions.\n";
        let out = PersonaTool::replace_agent_body(md, "New instructions.");
        assert_eq!(out, "---\nname: receptionist\ndescription: answers calls\n---\nNew instructions.\n");
        assert_eq!(PersonaTool::replace_agent_body("plain prose only", "New."), "New.\n");
        assert!(napp::agent::parse_agent(&out).is_ok());
    }

    #[test]
    fn invalid_agent_json_is_refused_before_any_write() {
        assert!(PersonaTool::validated_frontmatter("{").is_err());
        assert!(PersonaTool::validated_frontmatter("{}").is_ok());
    }

    fn loaded(name: &str, agent_md: &str, napp_path: Option<&str>) -> napp::agent_loader::LoadedAgent {
        napp::agent_loader::LoadedAgent {
            agent_def: napp::agent::AgentDef {
                id: String::new(),
                name: name.to_string(),
                description: String::new(),
                body: PersonaTool::agent_body(agent_md),
            },
            config: None,
            source: napp::agent_loader::AgentSource::User,
            napp_path: napp_path.map(std::path::PathBuf::from),
            source_path: napp_path.map(std::path::PathBuf::from).unwrap_or_default(),
            version: None,
            agent_md: agent_md.to_string(),
            frontmatter: String::new(),
            description: String::new(),
            id: None,
            theme_css: None,
            is_app: false,
            app_ui_path: None,
            app_binary_path: None,
            app_window_config: None,
        }
    }

    /// The Source line is the truth about where an employee lives; a false
    /// "marketplace" sent a live run through forty calls in the wrong tree.
    #[test]
    fn info_names_the_three_sources_truthfully() {
        let user_dir = std::path::Path::new("/data/user/agents");
        let installed_dir = std::path::Path::new("/data/agents");
        let md = "---\nname: front-desk\n---\nYou answer calls.\n";
        let cases: &[(Option<&str>, &str, bool)] = &[
            (None, "Source: local employee (database only)\n", true),
            (Some("/data/user/agents/front-desk"), "Source: user-created\n", true),
            (Some("/data/agents/front-desk"), "Source: marketplace\n", false),
            (Some("/elsewhere/front-desk.napp"), "Source: marketplace\n", false),
        ];
        for (path, line, local) in cases {
            let text = PersonaTool::info_text(&loaded("front-desk", md, *path), user_dir, installed_dir);
            assert!(text.contains(line), "{path:?}: {text}");
            assert_eq!(
                text.contains("Created here, not hired from the marketplace: it has no install code."),
                *local,
                "{path:?}: {text}"
            );
        }
    }

    /// Empty facts are left out, not printed as dashes, and a body that is
    /// only frontmatter is "none yet", never the frontmatter echoed as the
    /// persona (two of three replay runs went hunting for "the real prompt").
    #[test]
    fn info_prints_no_dashes_and_no_frontmatter_as_persona() {
        let user_dir = std::path::Path::new("/data/user/agents");
        let installed_dir = std::path::Path::new("/data/agents");
        let blank = loaded("front-desk", "---\nname: front-desk\ndescription: answers calls\n---\n", None);
        let text = PersonaTool::info_text(&blank, user_dir, installed_dir);
        assert!(!text.contains("Version:"), "{text}");
        assert!(!text.contains("Description:"), "{text}");
        assert!(!text.contains(": -\n"), "{text}");
        assert!(
            text.contains("Persona: none yet. This employee has no instructions; set them with update_employee(name: \"front-desk\", instructions: \"...\")."),
            "{text}"
        );
        assert!(!text.contains("name: front-desk"), "frontmatter echoed as persona: {text}");

        let mut versioned = loaded("front-desk", "---\nname: front-desk\n---\nYou answer calls.\n", None);
        versioned.version = Some("1.2.0".into());
        versioned.agent_def.description = "Answers calls".into();
        let text = PersonaTool::info_text(&versioned, user_dir, installed_dir);
        assert!(text.contains("Version: 1.2.0\n"), "{text}");
        assert!(text.contains("Description: Answers calls\n"), "{text}");
        assert!(text.ends_with("Persona:\nYou answer calls."), "{text}");

        assert_eq!(PersonaTool::agent_body("plain prose"), "plain prose");
        assert_eq!(PersonaTool::agent_body("---\nname: x\n---\n"), "");
    }

    /// The `ui` map the call carries: every path checked by the writer's
    /// rule (a path that leaves ui/ is refused here, before a byte is
    /// written), every content a string, every file addressed relative to
    /// ui/. The rule itself is tested where the writer lives.
    #[test]
    fn the_ui_map_is_checked_before_anything_is_written() {
        let err = PersonaTool::app_ui_files(&serde_json::json!({"ui": {"../x": "boo"}}), "t").unwrap_err();
        assert!(err.contains("`..`"), "{err}");
        assert!(err.contains("nothing was written"), "{err}");
        let err = PersonaTool::app_ui_files(&serde_json::json!({"ui": {"/etc/x": "boo"}}), "t").unwrap_err();
        assert!(err.contains("absolute"), "{err}");
        let err = PersonaTool::app_ui_files(&serde_json::json!({"ui": {"a.js": 1}}), "t").unwrap_err();
        assert!(err.contains("must be a string"), "{err}");
        let err = PersonaTool::app_ui_files(&serde_json::json!({"ui": "index.html"}), "t").unwrap_err();
        assert!(err.starts_with("`ui` must be a map"), "{err}");

        let files = PersonaTool::app_ui_files(
            &serde_json::json!({"ui": {"index.html": "<p>hi</p>", "assets/app.js": "1"}}),
            "t",
        )
        .unwrap();
        let mut named: Vec<(String, String)> = files
            .iter()
            .map(|(p, c)| (p.display().to_string(), String::from_utf8_lossy(c).to_string()))
            .collect();
        named.sort();
        assert_eq!(
            named,
            vec![
                ("assets/app.js".to_string(), "1".to_string()),
                ("index.html".to_string(), "<p>hi</p>".to_string()),
            ]
        );
    }

    /// `ui_jsx` goes through the one JSX converter and comes out as an
    /// index.html that loads the app SDK; TSX compiles too; a component
    /// that does not compile is refused with the converter's reason.
    #[test]
    fn ui_jsx_compiles_to_index_html_with_the_sdk() {
        let jsx = r#"
import { useState } from "react";
export default function App() {
  const [n, setN] = useState(0);
  return <button className="p-4" onClick={() => setN(n + 1)}>Count: {n}</button>;
}
"#;
        let files = PersonaTool::app_ui_files(&serde_json::json!({"ui_jsx": jsx}), "Counter").unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, std::path::PathBuf::from("index.html"));
        let html = &String::from_utf8(files[0].1.clone()).unwrap();
        assert!(html.contains(PersonaTool::APP_SDK_SCRIPT), "sdk loaded: {html}");
        assert!(html.find(PersonaTool::APP_SDK_SCRIPT) < html.find("<body>"), "sdk in head");
        assert!(!html.contains("<button className"), "JSX transformed");
        assert!(html.contains("<title>Counter</title>"));

        let tsx = r#"
type Props = { label: string };
export default function App({ label }: Props = { label: "hi" }) { return <div>{label}</div>; }
"#;
        let html = PersonaTool::app_ui_from_jsx(tsx, "t").unwrap();
        assert!(!html.contains("type Props"), "types stripped");

        let err = PersonaTool::app_ui_from_jsx("export function App() { return <div/>; }", "t").unwrap_err();
        assert!(err.starts_with("`ui_jsx` did not compile and nothing was written"), "{err}");
        let err = PersonaTool::app_ui_files(&serde_json::json!({"ui_jsx": 5}), "t").unwrap_err();
        assert!(err.starts_with("`ui_jsx` must be a string"), "{err}");
    }

    /// The two automation shapes the loader rejects on every later scan are
    /// refused here, before anything is written, with the field named.
    #[test]
    fn automations_the_loader_would_reject_are_refused_first() {
        use serde_json::json;
        let empty_sources = PersonaTool::build_agent_json_from_automations(&[json!({
            "name": "inbox", "sources": [], "steps": ["Triage"]
        })])
        .unwrap_err();
        assert!(empty_sources.contains("`sources`"), "{empty_sources}");
        assert!(empty_sources.contains("\"sources\": [\"email.received\"]"), "{empty_sources}");
        assert!(empty_sources.contains("Nothing was written"), "{empty_sources}");

        let no_schedule = PersonaTool::build_agent_json_from_automations(&[json!({
            "name": "brief", "trigger": "schedule", "steps": ["Brief"]
        })])
        .unwrap_err();
        assert!(no_schedule.contains("`schedule`"), "{no_schedule}");

        let valid = PersonaTool::build_agent_json_from_automations(&[
            json!({"name": "inbox", "sources": ["email.received"], "steps": ["Triage"]}),
            json!({"name": "brief", "schedule": "weekdays at 9am", "steps": ["Brief"]}),
        ])
        .unwrap();
        assert_eq!(valid["workflows"]["inbox"]["trigger"]["sources"], json!(["email.received"]));
        assert_eq!(valid["workflows"]["brief"]["trigger"]["type"], "schedule");
        assert!(PersonaTool::validated_frontmatter(&valid.to_string()).is_ok());
    }

    #[test]
    fn test_truncate_str_multibyte_no_panic() {
        // Descriptions are cut at a byte length; emoji/CJK straddling the cut
        // must not panic (was `&prompt[..60]`, which panics off-boundary).
        let s = "日本語のテキスト🎉".repeat(10);
        let t = crate::truncate_str(&s, 60);
        assert!(t.len() <= 60);
        assert!(s.starts_with(t));
        // ASCII shorter than the limit passes through untouched.
        assert_eq!(crate::truncate_str("abc", 60), "abc");
    }

    #[test]
    fn test_normalize_cron_translates_numeric_dow() {
        // The `cron` crate is Quartz-style (1=Sunday, 0 invalid) while every
        // producer writes Unix-style (0=Sunday). Numeric DOW must become
        // named days or "weekdays" fires Sun–Thu and "weekends" never parses.
        assert_eq!(
            tools_dow(&PersonaTool::normalize_cron("0 7 * * 1-5")),
            "MON-FRI"
        );
        assert_eq!(
            tools_dow(&PersonaTool::normalize_cron("0 7 * * 0,6")),
            "SUN,SAT"
        );
        assert_eq!(
            tools_dow(&PersonaTool::normalize_cron("0 0 7 * * 7 *")),
            "SUN"
        );
        // Wildcards and named days pass through untouched.
        assert_eq!(tools_dow(&PersonaTool::normalize_cron("0 7 * * *")), "*");
        assert_eq!(
            tools_dow(&PersonaTool::normalize_cron("0 0 7 * * MON-FRI *")),
            "MON-FRI"
        );
    }

    /// Extract the day-of-week field from a normalized 7-field cron.
    fn tools_dow(cron: &str) -> String {
        cron.split_whitespace().nth(5).unwrap_or("").to_string()
    }

    /// 6-field crons are ambiguous. sec-first (what the schedule editors
    /// emit) must NOT get a second seconds field prepended — that shifted
    /// every field and silently broke the timer; only a trailing year means
    /// seconds are missing.
    #[test]
    fn test_normalize_cron_six_field_disambiguation() {
        // sec-first: "0 30 9 * * *" (9:30 daily, what buildSimple emits)
        // → append year, fields intact.
        assert_eq!(
            PersonaTool::normalize_cron("0 30 9 * * *"),
            "0 30 9 * * * *"
        );
        // sec-first with named DOW stays parseable and keeps its hour.
        assert_eq!(
            PersonaTool::normalize_cron("0 0 7 * * MON-FRI"),
            "0 0 7 * * MON-FRI *"
        );
        // min-first + trailing year (a one-shot missing seconds) → prepend.
        assert_eq!(
            PersonaTool::normalize_cron("48 13 27 7 * 2026"),
            "0 48 13 27 7 * 2026"
        );
    }

    #[test]
    fn test_extract_skill_name_qualified() {
        assert_eq!(
            extract_skill_name_from_ref("@nebo/skills/briefing-writer@^1.0.0"),
            "briefing-writer"
        );
        assert_eq!(
            extract_skill_name_from_ref("@acme/skills/data-analysis"),
            "data-analysis"
        );
    }

    #[test]
    fn test_extract_skill_name_install_code() {
        assert_eq!(
            extract_skill_name_from_ref("SKIL-ABCD-1234"),
            "SKIL-ABCD-1234"
        );
    }

    #[test]
    fn test_extract_skill_name_bare() {
        assert_eq!(extract_skill_name_from_ref("my-skill"), "my-skill");
    }

    #[test]
    fn test_extract_skill_name_edge_cases() {
        // Version range without caret
        assert_eq!(
            extract_skill_name_from_ref("@nebo/skills/web-search@>=2.0.0"),
            "web-search"
        );
        // No version suffix
        assert_eq!(extract_skill_name_from_ref("@org/skills/name"), "name");
    }

    #[tokio::test]
    async fn test_packaged_employee_reads_as_healthy() {
        use tempfile::TempDir;

        // An employee package declares the procedures it ships and ships them
        // inside itself. The dependency check used to compare that declaration
        // against the workforce-wide roster, where a seat's own procedures do
        // not appear — so EVERY packaged employee read as degraded.
        let installed = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();
        let agents = TempDir::new().unwrap();

        let pkg = agents.path().join("copywriter");
        std::fs::create_dir_all(pkg.join("skills").join("copy-brief")).unwrap();
        std::fs::write(pkg.join("AGENT.md"), "---\nname: copywriter\n---\n# Copywriter\n").unwrap();
        std::fs::write(
            pkg.join("manifest.json"),
            r#"{"id":"copywriter","name":"@acme/agents/copywriter","type":"agent","version":"1.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            pkg.join("skills").join("copy-brief").join("SKILL.md"),
            "---\nname: copy-brief\ndescription: Test a brief\n---\nTemplate",
        )
        .unwrap();

        let loader =
            crate::skills::Loader::new(installed.path().to_path_buf(), user.path().to_path_buf())
                .with_agent_dirs(vec![agents.path().to_path_buf()]);
        loader.load_all().await;

        let registry: AgentRegistry = Arc::new(RwLock::new(HashMap::new()));
        {
            let mut reg = registry.write().await;
            let config = Some(
                napp::agent::parse_agent_config(r#"{"skills": ["copy-brief"]}"#).unwrap(),
            );
            reg.insert("copywriter".to_string(), ActiveAgent {
                agent_id: "copywriter".to_string(),
                name: "Copywriter".to_string(),
                agent_md: String::new(),
                config: config.clone(),
                channel_id: None,
                degraded: Some("missing skills: copy-brief".to_string()),
                soul: None,
                rules: None,
            });
            // Another seat declaring the same procedure without shipping it is
            // still degraded — the fix must not make every declaration pass.
            reg.insert("closer".to_string(), ActiveAgent {
                agent_id: "closer".to_string(),
                name: "Closer".to_string(),
                agent_md: String::new(),
                config,
                channel_id: None,
                degraded: None,
                soul: None,
                rules: None,
            });
        }

        validate_agent_dependencies(&registry, &loader).await;

        let reg = registry.read().await;
        assert!(
            reg.get("copywriter").unwrap().degraded.is_none(),
            "an employee that ships the procedure it declares must read as healthy"
        );
        assert!(
            reg.get("closer")
                .unwrap()
                .degraded
                .as_deref()
                .is_some_and(|r| r.contains("copy-brief")),
            "a seat that declares a procedure nobody gave it is still degraded"
        );
    }

    #[tokio::test]
    async fn test_validate_agent_dependencies_marks_degraded() {
        use tempfile::TempDir;

        // Set up a skill loader with one skill loaded
        let installed = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();

        // Create a skill named "briefing-writer"
        let skill_dir = user.path().join("briefing-writer");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: briefing-writer\ndescription: test\n---\nTemplate",
        )
        .unwrap();

        let loader =
            crate::skills::Loader::new(installed.path().to_path_buf(), user.path().to_path_buf());
        loader.load_all().await;

        // Create agent registry with an agent that requires two skills —
        // one that exists and one that doesn't
        let registry: AgentRegistry = Arc::new(RwLock::new(HashMap::new()));
        {
            let mut reg = registry.write().await;
            reg.insert("agent-1".to_string(), ActiveAgent {
                agent_id: "agent-1".to_string(),
                name: "Test Agent".to_string(),
                agent_md: String::new(),
                config: Some(napp::agent::parse_agent_config(
                    r#"{"skills": ["@nebo/skills/briefing-writer@^1.0.0", "@nebo/skills/missing-skill@^1.0.0"]}"#
                ).unwrap()),
                channel_id: None,
                degraded: None,
                soul: None,
                rules: None,
            });
            // Agent with no config — should remain non-degraded
            reg.insert(
                "agent-2".to_string(),
                ActiveAgent {
                    agent_id: "agent-2".to_string(),
                    name: "No Config Agent".to_string(),
                    agent_md: String::new(),
                    config: None,
                    channel_id: None,
                    degraded: None,
                    soul: None,
                    rules: None,
                },
            );
        }

        // Run validation
        validate_agent_dependencies(&registry, &loader).await;

        // Agent-1 should be degraded (missing-skill not loaded)
        let reg = registry.read().await;
        let agent1 = reg.get("agent-1").unwrap();
        assert!(agent1.degraded.is_some(), "agent-1 should be degraded");
        assert!(
            agent1.degraded.as_ref().unwrap().contains("missing-skill"),
            "degraded reason should mention the missing skill"
        );

        // Agent-2 should NOT be degraded
        let agent2 = reg.get("agent-2").unwrap();
        assert!(agent2.degraded.is_none(), "agent-2 should not be degraded");
    }

    #[tokio::test]
    async fn test_validate_agent_dependencies_all_satisfied() {
        use tempfile::TempDir;

        let installed = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();

        // Create the required skill
        let skill_dir = user.path().join("briefing-writer");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: briefing-writer\ndescription: test\n---\nTemplate",
        )
        .unwrap();

        let loader =
            crate::skills::Loader::new(installed.path().to_path_buf(), user.path().to_path_buf());
        loader.load_all().await;

        let registry: AgentRegistry = Arc::new(RwLock::new(HashMap::new()));
        {
            let mut reg = registry.write().await;
            reg.insert(
                "agent-ok".to_string(),
                ActiveAgent {
                    agent_id: "agent-ok".to_string(),
                    name: "Happy Agent".to_string(),
                    agent_md: String::new(),
                    config: Some(
                        napp::agent::parse_agent_config(
                            r#"{"skills": ["@nebo/skills/briefing-writer@^1.0.0"]}"#,
                        )
                        .unwrap(),
                    ),
                    channel_id: None,
                    degraded: None,
                    soul: None,
                    rules: None,
                },
            );
        }

        validate_agent_dependencies(&registry, &loader).await;

        let reg = registry.read().await;
        let agent = reg.get("agent-ok").unwrap();
        assert!(
            agent.degraded.is_none(),
            "agent with all deps satisfied should not be degraded"
        );
    }
}
