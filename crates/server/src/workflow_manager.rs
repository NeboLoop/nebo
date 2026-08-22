//! WorkflowManagerImpl — implements tools::workflows::WorkflowManager trait.
//!
//! Bridges workflow lifecycle operations (DB queries, marketplace install) with
//! workflow execution (spawned via tokio::spawn, using the workflow engine).

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{RwLock, Semaphore};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Max concurrent workflow runs per agent+binding. Events beyond this limit are
/// queued by the semaphore and execute as earlier runs complete.
/// Rust+Tokio overhead is ~500KB per in-flight task — 100 concurrent ≈ 50MB.
/// The real ceiling is LLM provider rate limits, not local resources.
/// How many runs of ONE binding may execute at once.
///
/// 1 — serial per binding. An event binding shares mutable state across its
/// runs: one mailbox, one OAuth token (each plugin call is a separate process,
/// so parallel runs raced the token refresh into `invalid_grant`), and one
/// memory record per lead. Running six replies concurrently produced exactly
/// the failures you would predict: dropped sends, contradictory memory writes,
/// and duplicate outreach. Events are not lost when serialized — they queue on
/// the semaphore and run in order, which is also what the human this replaces
/// would do.
///
/// Raise deliberately per deployment with NEBO_BINDING_CONCURRENCY when a
/// binding is genuinely stateless (was effectively 100 = unbounded before).
fn binding_concurrency() -> usize {
    std::env::var("NEBO_BINDING_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(1)
}

use ai::Provider;
use tools::origin::ToolContext;
use tools::registry::{DynTool, ToolResult};
use tools::workflows::{WorkflowInfo, WorkflowManager, WorkflowRunInfo};

use crate::handlers::ws::ClientHub;

/// Concrete implementation of WorkflowManager.
pub struct WorkflowManagerImpl {
    store: Arc<db::Store>,
    providers: Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    tools: Arc<tools::Registry>,
    hub: Arc<ClientHub>,
    config: config::Config,
    /// Active run cancellation tokens, keyed by run_id.
    active_runs: Arc<std::sync::Mutex<HashMap<String, CancellationToken>>>,
    /// Maps agent_id → list of active run_ids, for cancelling all runs when an agent stops.
    agent_runs: Arc<std::sync::Mutex<HashMap<String, Vec<String>>>>,
    /// Event bus for emitting workflow lifecycle events.
    event_bus: Option<tools::EventBus>,
    /// Skill loader for resolving skill_content in workflow execution.
    skill_loader: Option<Arc<tools::skills::Loader>>,
    /// Per-binding concurrency semaphores. Key: "agent:{agent_id}:{binding}".
    /// Allows up to binding_concurrency() concurrent runs; additional events wait.
    binding_semaphores: Arc<std::sync::Mutex<HashMap<String, Arc<Semaphore>>>>,
    /// Consecutive failure counts per (agent_id, binding_name) for unattended
    /// (non-manual) automations. The owner is notified once, at the 2nd
    /// consecutive failure — a lone blip stays quiet, a broken automation
    /// doesn't fail silently for days. Reset on success.
    /// ponytail: in-memory — an app restart re-arms the 2-count; fine.
    failure_counts: Arc<std::sync::Mutex<HashMap<(String, String), u32>>>,
    /// Late-injected agent worker registry. The registry is constructed AFTER
    /// this manager (it depends on us), so it's wired in via `set_agent_workers`
    /// once both exist. Used by `create` to restart the owning agent's worker so
    /// a new binding's live triggers (event/heartbeat/watch/folder) register
    /// immediately — schedule triggers fire via the cron scheduler regardless.
    agent_workers: std::sync::OnceLock<Arc<agent::AgentWorkerRegistry>>,
}

impl WorkflowManagerImpl {
    pub fn new(
        store: Arc<db::Store>,
        providers: Arc<RwLock<Vec<Arc<dyn Provider>>>>,
        tools: Arc<tools::Registry>,
        hub: Arc<ClientHub>,
        config: config::Config,
        event_bus: Option<tools::EventBus>,
        skill_loader: Option<Arc<tools::skills::Loader>>,
    ) -> Self {
        Self {
            store,
            providers,
            tools,
            hub,
            config,
            active_runs: Arc::new(std::sync::Mutex::new(HashMap::new())),
            agent_runs: Arc::new(std::sync::Mutex::new(HashMap::new())),
            event_bus,
            skill_loader,
            binding_semaphores: Arc::new(std::sync::Mutex::new(HashMap::new())),
            failure_counts: Arc::new(std::sync::Mutex::new(HashMap::new())),
            agent_workers: std::sync::OnceLock::new(),
        }
    }

    /// Wire in the agent worker registry once it has been constructed (it
    /// depends on this manager, so it can't be passed to `new`).
    pub fn set_agent_workers(&self, workers: Arc<agent::AgentWorkerRegistry>) {
        let _ = self.agent_workers.set(workers);
    }

    /// Cancel a running workflow by run_id.
    pub async fn cancel_run(&self, run_id: &str) -> Result<(), String> {
        let token = {
            let runs = self.active_runs.lock().unwrap();
            runs.get(run_id).cloned()
        };
        match token {
            Some(t) => {
                t.cancel();
                // Update DB status
                if let Err(e) = self.store.update_workflow_run(
                    run_id,
                    Some("cancelled"),
                    None,
                    None,
                    Some("cancelled by user"),
                    None,
                ) {
                    warn!(run_id, error = %e, "failed to update cancelled run status");
                }
                self.hub.broadcast(
                    "workflow_run_cancelled",
                    serde_json::json!({ "runId": run_id }),
                );
                info!(run_id, "workflow run cancelled");
                Ok(())
            }
            None => Err(format!("no active run found: {}", run_id)),
        }
    }

    /// Cancel all running workflows associated with an agent.
    async fn cancel_runs_for_agent_impl(&self, agent_id: &str) {
        let run_ids = {
            let runs = self.agent_runs.lock().unwrap();
            runs.get(agent_id).cloned().unwrap_or_default()
        };
        for run_id in &run_ids {
            if let Err(e) = self.cancel_run(run_id).await {
                warn!(agent_id, run_id = %run_id, error = %e, "failed to cancel agent workflow run");
            }
        }
        if !run_ids.is_empty() {
            info!(
                agent_id,
                count = run_ids.len(),
                "cancelled running workflows for agent"
            );
        }
    }

    fn build_api_client(&self) -> Result<comm::api::NeboAIApi, String> {
        let bot_id = config::read_bot_id().ok_or_else(|| "no bot_id configured".to_string())?;
        let profiles = match self
            .store
            .list_all_active_auth_profiles_by_provider("neboai")
        {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, "failed to list auth profiles for neboai");
                return Err("failed to query auth profiles".to_string());
            }
        };
        let profile = profiles
            .first()
            .ok_or_else(|| "not connected to NeboAI".to_string())?;
        let api_server = self.config.neboai.api_url.clone();
        Ok(comm::api::NeboAIApi::new(
            api_server,
            bot_id,
            profile.api_key.clone(),
        ))
    }

    fn workflow_to_info(&self, wf: &db::models::Workflow) -> WorkflowInfo {
        let activity_count = match self.load_workflow_def(wf) {
            Ok(def) => def.activities.len(),
            Err(_) => 0,
        };

        // Description lives in manifest.json, not workflow.json (per packaging spec)
        let description = wf
            .manifest
            .as_deref()
            .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
            .and_then(|v| v["description"].as_str().map(String::from))
            .unwrap_or_default();

        WorkflowInfo {
            id: wf.id.clone(),
            name: wf.name.clone(),
            version: wf.version.clone(),
            description,
            is_enabled: wf.is_enabled != 0,
            trigger_count: 0, // Triggers are now agent-owned
            activity_count,
        }
    }

    /// Load workflow definition from filesystem directory or fall back to DB.
    fn load_workflow_def(
        &self,
        wf: &db::models::Workflow,
    ) -> Result<workflow::WorkflowDef, String> {
        // Try loading from napp_path first (always a directory after migration)
        if let Some(ref napp_path) = wf.napp_path {
            let path = std::path::Path::new(napp_path);
            if path.is_dir() {
                let json_path = path.join("workflow.json");
                if json_path.exists() {
                    let json = std::fs::read_to_string(&json_path).map_err(|e| e.to_string())?;
                    return workflow::parser::parse_workflow(&json).map_err(|e| e.to_string());
                }
            }
        }

        // Fall back to definition stored in DB
        workflow::parser::parse_workflow(&wf.definition).map_err(|e| e.to_string())
    }

    /// Expand `${NEBO_DATA_DIR}` / `${NEBO_SKILL_DIR}` (and the rest of the
    /// skill template variables) inside `command` activities' `params.command`.
    /// A command node names its skill via `params.skill`; expansion uses the
    /// SAME `SkillLoader::expand_template` context the skill body itself gets,
    /// so a script path written once in SKILL.md and once in agent.json resolve
    /// identically. Done here, before the definition reaches the engine, so
    /// the graph node stays a pure "run this string" executor.
    ///
    /// Fails LOUD, never soft: a command that names a skill which isn't
    /// installed, or that still carries a `${...}` placeholder after
    /// expansion, is refused before the run starts. Left to the shell, an
    /// unknown `${VAR}` silently expands to "" and the operator gets a
    /// misleading "python3: can't open file '/scripts/x.py'" three layers
    /// below the actual cause (observed live).
    async fn expand_command_params(
        &self,
        def: &mut workflow::WorkflowDef,
        store: &db::Store,
    ) -> Result<(), String> {
        for activity in def.activities.iter_mut() {
            if activity.activity_type != "command" {
                continue;
            }
            let Some(params) = activity.params.as_mut().and_then(|p| p.as_object_mut()) else {
                continue;
            };
            let skill_name = params
                .get("skill")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let Some(command) = params.get("command").and_then(|v| v.as_str()) else { continue };
            let mut expanded = command.to_string();
            if !skill_name.is_empty() {
                let Some(loader) = self.skill_loader.as_ref() else {
                    return Err(format!(
                        "command activity '{}' names skill '{}' but no skill loader is available",
                        activity.id, skill_name
                    ));
                };
                let Some(mut skill) = loader.get(&skill_name, None).await else {
                    return Err(format!(
                        "command activity '{}' references skill '{}', which is not installed on this instance",
                        activity.id, skill_name
                    ));
                };
                // expand_template expands the skill's own body; reuse its
                // context by temporarily making the command the body.
                skill.template = command.to_string();
                expanded = loader.expand_template(&skill, Some(store));
            }
            if let Some(start) = expanded.find("${") {
                let end = expanded[start..].find('}').map(|i| start + i + 1).unwrap_or(expanded.len());
                return Err(format!(
                    "command activity '{}' still contains an unexpanded template variable {} \
                     — set params.skill to the skill that defines it, or remove the placeholder",
                    activity.id,
                    &expanded[start..end]
                ));
            }
            params.insert("command".into(), serde_json::Value::String(expanded));
        }
        Ok(())
    }

    fn run_to_info(run: &db::models::WorkflowRun) -> WorkflowRunInfo {
        WorkflowRunInfo {
            id: run.id.clone(),
            workflow_id: run.workflow_id.clone(),
            status: run.status.clone(),
            trigger_type: run.trigger_type.clone(),
            total_tokens_used: run.total_tokens_used,
            error: run.error.clone(),
            started_at: run.started_at,
            completed_at: run.completed_at,
        }
    }

    /// Fire an agent workflow binding on demand (work-tool dispatch through
    /// `run()` with a binding-scoped id). Resolves the binding definition from
    /// the agent's frontmatter — the same prep the cron scheduler
    /// (`scheduler::execute_agent_workflow_task`) and the manual-run HTTP
    /// endpoint (`handlers::agents::run_agent_workflow`) perform — then fires
    /// it through `run_inline`, the one firing body every binding trigger uses.
    async fn run_binding(
        &self,
        agent_id: &str,
        binding_name: &str,
        inputs: serde_json::Value,
        trigger_type: &str,
    ) -> Result<String, String> {
        let agent_rec = self
            .store
            .get_agent(agent_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("agent not found: {}", agent_id))?;

        // Enabled state lives on the agent_workflows row (the panel toggle).
        let row = self
            .store
            .list_agent_workflows(agent_id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|b| b.binding_name == binding_name)
            .ok_or_else(|| format!("workflow binding not found: {}", binding_name))?;
        if row.is_active == 0 {
            return Err(format!(
                "workflow {:?} is disabled — toggle it on before running",
                binding_name
            ));
        }

        let config = napp::agent::parse_agent_config(&agent_rec.frontmatter)
            .map_err(|e| format!("parse agent config: {}", e))?;
        let binding = config
            .workflows
            .get(binding_name)
            .ok_or_else(|| format!("workflow binding not found: {}", binding_name))?;
        if !binding.has_activities() {
            return Err(format!("workflow {:?} has no activities", binding_name));
        }
        let def_json = binding.to_workflow_json(binding_name);

        // Binding default inputs, overlaid with caller-supplied inputs.
        let mut merged = serde_json::to_value(&binding.inputs).unwrap_or_default();
        if !merged.is_object() {
            merged = serde_json::json!({});
        }
        if let (Some(base), Some(extra)) = (merged.as_object_mut(), inputs.as_object()) {
            for (k, v) in extra {
                base.insert(k.clone(), v.clone());
            }
        }

        // Synthesized envelope — same shape the EventDispatcher builds for
        // real events, via the same helper (workflow::events).
        workflow::events::insert_event_envelope(
            &mut merged,
            "manual",
            serde_json::json!({
                "note": "manual invocation — there is no triggering event, so no event payload is available"
            }),
            &format!("agent:{}", agent_id),
        );

        let emit_source = binding.emit.as_ref().map(|emit_name| {
            let slug = agent_rec.name.to_lowercase().replace(' ', "-");
            format!("{}.{}", slug, emit_name)
        });

        self.run_inline(
            def_json,
            merged,
            trigger_type,
            Some(binding_name.to_string()),
            agent_id,
            emit_source,
        )
        .await
    }
}

/// Split a binding-scoped workflow id (`agent:{agent_id}:{binding_name}`) —
/// the id shape `resolve()` returns for agent bindings, matching the command
/// format the cron scheduler already uses for them.
fn split_binding_id(id: &str) -> Option<(&str, &str)> {
    id.strip_prefix("agent:")?.split_once(':')
}

/// Map an agent-owned binding row to the tool-facing WorkflowInfo shape.
fn agent_workflow_to_info(wf: &db::models::AgentWorkflow) -> WorkflowInfo {
    let activity_count = wf
        .activities
        .as_ref()
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    WorkflowInfo {
        id: wf.binding_name.clone(),
        name: wf.binding_name.clone(),
        version: "1.0".to_string(),
        description: wf.description.clone().unwrap_or_default(),
        is_enabled: wf.is_active != 0,
        trigger_count: if wf.trigger_type == "manual" { 0 } else { 1 },
        activity_count,
    }
}

/// Slugify a display name into a binding key: lowercase, spaces→hyphens,
/// keep alphanumerics and hyphens, collapse repeats.
fn slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_dash = false;
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if ch.is_whitespace() || ch == '-' || ch == '_' {
            if !prev_dash && !out.is_empty() {
                out.push('-');
                prev_dash = true;
            }
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

impl WorkflowManager for WorkflowManagerImpl {
    fn list<'a>(
        &'a self,
        agent_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<WorkflowInfo>> + Send + 'a>> {
        Box::pin(async move {
            let mut out = Vec::new();

            // The agent's own bindings — the canonical store the panel reads.
            if !agent_id.is_empty() {
                match self.store.list_agent_workflows(agent_id) {
                    Ok(bindings) => out.extend(bindings.iter().map(agent_workflow_to_info)),
                    Err(e) => warn!(agent_id, error = %e, "failed to list agent workflows"),
                }
            }

            // Standalone marketplace-installed workflows (shared, not agent-owned).
            match self.store.list_workflows(100, 0) {
                Ok(workflows) => out.extend(workflows.iter().map(|wf| self.workflow_to_info(wf))),
                Err(e) => warn!(error = %e, "failed to list workflows"),
            }

            out
        })
    }

    fn install<'a>(
        &'a self,
        code: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<WorkflowInfo, String>> + Send + 'a>,
    > {
        Box::pin(async move {
            let api = self.build_api_client()?;
            let resp = api
                .install_workflow(code)
                .await
                .map_err(|e| format!("install_workflow: {}", e))?;

            // The response artifact.id is the workflow ID from NeboAI
            // Look up the workflow in DB (handle_work_code in codes.rs already stored it)
            // If not found, the install may have been handled via the codes path
            match self.store.get_workflow(&resp.artifact.id) {
                Ok(Some(wf)) => Ok(self.workflow_to_info(&wf)),
                _ => Err(format!(
                    "workflow installed but not found in local DB (id: {})",
                    resp.artifact.id
                )),
            }
        })
    }

    fn uninstall<'a>(
        &'a self,
        id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            // Resolve workflow — try by ID first, then by name
            let wf = self
                .store
                .get_workflow(id)
                .ok()
                .flatten()
                .or_else(|| {
                    // Try resolving by name if ID lookup failed
                    self.store.list_workflows(500, 0).ok().and_then(|wfs| {
                        let lower = id.to_lowercase();
                        wfs.into_iter().find(|w| w.name.to_lowercase() == lower)
                    })
                })
                .ok_or_else(|| format!("workflow '{}' not found", id))?;

            let wf_id = wf.id.clone();
            let napp_path = wf.napp_path.clone();

            // Unregister triggers while we have the workflow
            if let Ok(def) = self.load_workflow_def(&wf) {
                workflow::triggers::unregister_triggers(&def.id, &self.store);
            }

            // Delete runs, bindings, then workflow
            if let Err(e) = self.store.delete_workflow_runs(&wf_id) {
                warn!(workflow_id = %wf_id, error = %e, "failed to delete workflow runs");
            }
            if let Err(e) = self.store.delete_workflow_bindings(&wf_id) {
                warn!(workflow_id = %wf_id, error = %e, "failed to delete workflow bindings");
            }
            self.store
                .delete_workflow(&wf_id)
                .map_err(|e| format!("delete_workflow: {}", e))?;

            // Clean up filesystem directory if it exists
            if let Some(ref path_str) = napp_path {
                let path = std::path::Path::new(path_str);
                if path.exists() {
                    if let Err(e) = std::fs::remove_dir_all(path) {
                        warn!(workflow_id = %id, path = %path_str, error = %e, "failed to remove workflow directory");
                    }
                }
            }

            Ok(())
        })
    }

    fn resolve<'a>(
        &'a self,
        agent_id: &'a str,
        name_or_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<WorkflowInfo, String>> + Send + 'a>,
    > {
        Box::pin(async move {
            // The calling agent's own bindings first — the same store list()
            // reads, so anything list() shows resolves. On a name collision
            // with a standalone workflow, the agent's own binding wins.
            if !agent_id.is_empty() {
                match self.store.list_agent_workflows(agent_id) {
                    Ok(bindings) => {
                        let key = slug(name_or_id);
                        if let Some(b) = bindings
                            .iter()
                            .find(|b| b.binding_name == name_or_id || b.binding_name == key)
                        {
                            let mut info = agent_workflow_to_info(b);
                            // Binding-scoped id so run()/list_runs()/toggle()
                            // route to the binding, not the workflows table.
                            info.id = format!("agent:{}:{}", agent_id, b.binding_name);
                            return Ok(info);
                        }
                    }
                    Err(e) => warn!(agent_id, error = %e, "failed to list agent workflows"),
                }
            }

            // Try the standalone workflows table by ID
            if let Ok(Some(wf)) = self.store.get_workflow(name_or_id) {
                return Ok(self.workflow_to_info(&wf));
            }

            // Search by name
            match self.store.list_workflows(100, 0) {
                Ok(workflows) => {
                    let lower = name_or_id.to_lowercase();
                    for wf in &workflows {
                        if wf.name.to_lowercase() == lower || wf.id == name_or_id {
                            return Ok(self.workflow_to_info(wf));
                        }
                    }
                    Err(format!("no workflow found matching {:?}", name_or_id))
                }
                Err(e) => Err(format!("failed to search workflows: {}", e)),
            }
        })
    }

    fn run<'a>(
        &'a self,
        id: &'a str,
        inputs: serde_json::Value,
        trigger_type: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        Box::pin(async move {
            // Agent binding dispatch — resolve() returns bindings with a
            // binding-scoped id; fire them through run_inline, never the
            // standalone path below.
            if let Some((agent_id, binding_name)) = split_binding_id(id) {
                return self
                    .run_binding(agent_id, binding_name, inputs, trigger_type)
                    .await;
            }

            let wf = self
                .store
                .get_workflow(id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("workflow not found: {}", id))?;

            if wf.is_enabled == 0 {
                return Err("workflow is disabled".into());
            }

            let mut def = self
                .load_workflow_def(&wf)
                .map_err(|e| format!("parse error: {}", e))?;
            self.expand_command_params(&mut def, &self.store).await?;

            // Create run record
            let run_id = uuid::Uuid::new_v4().to_string();
            let session_key = format!("workflow-{}-{}", id, run_id);
            self.store
                .create_workflow_run(
                    &run_id,
                    id,
                    trigger_type,
                    None,
                    Some(&inputs.to_string()),
                    Some(&session_key),
                )
                .map_err(|e| format!("create_workflow_run: {}", e))?;

            // Create cancellation token
            let cancel_token = CancellationToken::new();
            {
                let mut runs = self.active_runs.lock().unwrap();
                runs.insert(run_id.clone(), cancel_token.clone());
            }

            // Clone Arcs for the spawned task
            let store = self.store.clone();
            let providers = self.providers.clone();
            let tools_registry = self.tools.clone();
            let hub = self.hub.clone();
            let active_runs = self.active_runs.clone();
            let event_bus = self.event_bus.clone();
            let skill_loader = self.skill_loader.clone();
            let run_id_clone = run_id.clone();
            let wf_id = id.to_string();
            let wf_name = wf.name.clone();
            let trigger = trigger_type.to_string();

            tokio::spawn(async move {
                // Get first available provider
                let provider = {
                    let lock = providers.read().await;
                    lock.first().cloned()
                };
                let provider = match provider {
                    Some(p) => p,
                    None => {
                        if let Err(e) = store.update_workflow_run(
                            &run_id_clone,
                            Some("failed"),
                            None,
                            None,
                            Some("no AI provider available"),
                            None,
                        ) {
                            warn!(run_id = %run_id_clone, error = %e, "failed to update workflow run status");
                        }
                        notify_workflow_failure(
                            &store,
                            &hub,
                            &wf_id,
                            &run_id_clone,
                            &wf_id,
                            "no AI provider available",
                        );
                        hub.broadcast(
                            "workflow_run_failed",
                            serde_json::json!({
                                "workflowId": wf_id,
                                "runId": run_id_clone,
                                "error": "no AI provider available",
                            }),
                        );
                        return;
                    }
                };

                // Build tool wrappers from the registry snapshot
                let tool_defs = tools_registry.list().await;
                let resolved_tools: Vec<Box<dyn DynTool>> = tool_defs
                    .iter()
                    .map(|td| {
                        Box::new(RegistryTool {
                            tool_name: td.name.clone(),
                            tool_desc: td.description.clone(),
                            tool_schema: td.input_schema.clone(),
                            registry: tools_registry.clone(),
                        }) as Box<dyn DynTool>
                    })
                    .collect();

                info!(
                    workflow = %wf_id,
                    run_id = %run_id_clone,
                    trigger = %trigger,
                    tools = resolved_tools.len(),
                    "executing workflow in background"
                );

                // Load skill content for activities that reference skills
                // Template variables are expanded at activation time.
                let skill_content = if let Some(ref loader) = skill_loader {
                    let mut map = HashMap::new();
                    for activity in &def.activities {
                        for skill_name in &activity.skills {
                            if !map.contains_key(skill_name) {
                                // Global scope: Learned skills are per-employee
                                // and not declarable in workflow activities.
                                if let Some(skill) = loader.get(skill_name, None).await {
                                    if !skill.template.is_empty() {
                                        let expanded = loader.expand_template(&skill, Some(&store));
                                        map.insert(skill_name.clone(), expanded);
                                    }
                                }
                            }
                        }
                    }
                    if map.is_empty() { None } else { Some(map) }
                } else {
                    None
                };

                // Deferred tool names (MCP proxies etc.): activities only get
                // their schemas when they declare or reference them.
                let deferred_names = tools_registry.get_deferred_names().await;
                let (wf_memory_user_id, wf_memory_writes_disabled) =
                    workflow_memory_scope(&store, "");
                match workflow::engine::execute_workflow(
                    &def,
                    "", // standalone workflow run — not bound to an agent
                    &wf_memory_user_id,
                    wf_memory_writes_disabled,
                    inputs,
                    &trigger,
                    None,
                    &store,
                    &*provider,
                    &resolved_tools,
                    Some(&deferred_names),
                    Some(&run_id_clone),
                    Some(&cancel_token),
                    skill_content.as_ref(),
                    event_bus.as_ref(),
                    None,
                    None,
                    None, // standalone run — no per-employee approval policy
                    None,
                )
                .await
                {
                    Ok((_engine_run_id, _output)) => {
                        // Engine already called complete_workflow_run with output
                        hub.broadcast(
                            "workflow_run_completed",
                            serde_json::json!({
                                "workflowId": wf_id,
                                "runId": run_id_clone,
                                "name": wf_name,
                            }),
                        );
                        // Emit system event
                        if let Some(ref bus) = event_bus {
                            bus.emit(tools::Event {
                                source: format!("workflow.{}.completed", wf_id),
                                payload: serde_json::json!({ "runId": run_id_clone, "name": wf_name }),
                                origin: format!("workflow:{}:{}", wf_id, run_id_clone),
                                timestamp: std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_secs(),
                            });
                        }
                        info!(workflow = %wf_id, run_id = %run_id_clone, "workflow completed");
                    }
                    Err(e) => {
                        let err_msg = e.to_string();
                        if let Err(e) = store.update_workflow_run(
                            &run_id_clone,
                            Some("failed"),
                            None,
                            None,
                            Some(&err_msg),
                            None,
                        ) {
                            warn!(run_id = %run_id_clone, error = %e, "failed to mark workflow run failed");
                        }
                        notify_workflow_failure(
                            &store,
                            &hub,
                            &wf_id,
                            &run_id_clone,
                            &wf_id,
                            &err_msg,
                        );
                        hub.broadcast(
                            "workflow_run_failed",
                            serde_json::json!({
                                "workflowId": wf_id,
                                "runId": run_id_clone,
                                "error": err_msg,
                            }),
                        );
                        // Emit system event
                        if let Some(ref bus) = event_bus {
                            bus.emit(tools::Event {
                                source: format!("workflow.{}.failed", wf_id),
                                payload: serde_json::json!({ "runId": run_id_clone, "error": err_msg }),
                                origin: format!("workflow:{}:{}", wf_id, run_id_clone),
                                timestamp: std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_secs(),
                            });
                        }
                        warn!(workflow = %wf_id, run_id = %run_id_clone, error = %err_msg, "workflow failed");
                    }
                }

                // Remove from active runs
                let mut runs = active_runs.lock().unwrap();
                runs.remove(&run_id_clone);
            });

            Ok(run_id)
        })
    }

    fn run_status<'a>(
        &'a self,
        run_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<WorkflowRunInfo, String>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.store
                .get_workflow_run(run_id)
                .map_err(|e| e.to_string())?
                .map(|r| Self::run_to_info(&r))
                .ok_or_else(|| format!("run not found: {}", run_id))
        })
    }

    fn list_runs<'a>(
        &'a self,
        workflow_id: &'a str,
        limit: i64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<WorkflowRunInfo>> + Send + 'a>>
    {
        Box::pin(async move {
            // Binding-scoped id: runs are recorded under "agent:{agent_id}"
            // with the binding name (or "binding:event.source") in
            // trigger_detail — fetch the agent's runs and filter down.
            if let Some((agent_id, binding_name)) = split_binding_id(workflow_id) {
                let parent = format!("agent:{}", agent_id);
                return match self.store.list_workflow_runs(&parent, 200, 0) {
                    Ok(runs) => runs
                        .iter()
                        .filter(|r| {
                            r.trigger_detail.as_deref().is_some_and(|d| {
                                d == binding_name
                                    || d.strip_prefix(binding_name)
                                        .is_some_and(|rest| rest.starts_with(':'))
                            })
                        })
                        .take(limit.max(0) as usize)
                        .map(Self::run_to_info)
                        .collect(),
                    Err(e) => {
                        warn!(workflow_id = %workflow_id, error = %e, "failed to list binding runs");
                        Vec::new()
                    }
                };
            }

            match self.store.list_workflow_runs(workflow_id, limit, 0) {
                Ok(runs) => runs.iter().map(Self::run_to_info).collect(),
                Err(e) => {
                    warn!(workflow_id = %workflow_id, error = %e, "failed to list workflow runs");
                    Vec::new()
                }
            }
        })
    }

    fn toggle<'a>(
        &'a self,
        id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + Send + 'a>>
    {
        Box::pin(async move {
            // Binding-scoped id: enabled state lives on the agent_workflows row.
            if let Some((agent_id, binding_name)) = split_binding_id(id) {
                return self
                    .store
                    .toggle_agent_workflow(agent_id, binding_name)
                    .map_err(|e| format!("toggle: {}", e));
            }

            self.store
                .toggle_workflow(id)
                .map_err(|e| format!("toggle: {}", e))?;
            let wf = self
                .store
                .get_workflow(id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "workflow not found after toggle".to_string())?;
            Ok(wf.is_enabled != 0)
        })
    }

    fn resolve_agent<'a>(
        &'a self,
        agent_ref: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        Box::pin(async move {
            if let Ok(Some(_)) = self.store.get_agent(agent_ref) {
                return Ok(agent_ref.to_string());
            }
            let agents = self
                .store
                .list_agents(500, 0)
                .map_err(|e| format!("list_agents: {}", e))?;
            let names: Vec<(&str, &str)> = agents
                .iter()
                .map(|a| (a.id.as_str(), a.name.as_str()))
                .collect();
            match find_agent_by_name(&names, agent_ref) {
                Some(id) => Ok(id.to_string()),
                None => Err(format!(
                    "no agent matching '{}' — available: {}. To create a NEW agent \
                     with duties, use agent(resource: \"registry\", action: \"create\", \
                     name: \"...\", automations: [...]) first — workflows can only \
                     attach to an agent that exists.",
                    agent_ref,
                    names
                        .iter()
                        .map(|(_, n)| *n)
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
        })
    }

    fn create<'a>(
        &'a self,
        agent_id: &'a str,
        name: &'a str,
        definition: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<WorkflowInfo, String>> + Send + 'a>,
    > {
        Box::pin(save_binding(self, agent_id, name, definition, false))
    }

    fn update<'a>(
        &'a self,
        agent_id: &'a str,
        name: &'a str,
        definition: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<WorkflowInfo, String>> + Send + 'a>,
    > {
        Box::pin(save_binding(self, agent_id, name, definition, true))
    }

    fn tuning_sweep<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            workflow_tuning_sweep(
                &self.store,
                &self.providers,
                (&self.tools, &self.hub),
                &self.config.neboai.api_url,
            )
            .await;
        })
    }

    fn delete<'a>(
        &'a self,
        agent_id: &'a str,
        binding_name: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            use crate::handlers::agents::write_agent_json_to_fs;

            let agent = self
                .store
                .get_agent(agent_id)
                .map_err(|e| format!("get_agent: {}", e))?
                .ok_or_else(|| format!("agent '{}' not found", agent_id))?;

            // Verify the binding exists in the tracking store so a typo'd name
            // errors instead of reporting a successful no-op delete.
            let known = self
                .store
                .list_agent_workflows(agent_id)
                .map_err(|e| format!("list_agent_workflows: {}", e))?;
            if !known.iter().any(|w| w.binding_name == binding_name) {
                let names: Vec<&str> = known.iter().map(|w| w.binding_name.as_str()).collect();
                return Err(format!(
                    "no workflow named '{}' — this agent owns: {}",
                    binding_name,
                    if names.is_empty() { "(none)".to_string() } else { names.join(", ") }
                ));
            }

            // Remove from frontmatter (source of truth) and persist to disk.
            let mut fm: serde_json::Value =
                serde_json::from_str(&agent.frontmatter).unwrap_or_else(|_| serde_json::json!({}));
            if let Some(workflows) = fm.get_mut("workflows").and_then(|w| w.as_object_mut()) {
                workflows.remove(binding_name);
            }
            self.store
                .update_agent(
                    agent_id,
                    &agent.name,
                    &agent.description,
                    &agent.agent_md,
                    &fm.to_string(),
                    agent.pricing_model.as_deref(),
                    agent.pricing_cost,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .map_err(|e| format!("update_agent: {}", e))?;

            self.store
                .delete_single_agent_workflow(agent_id, binding_name)
                .map_err(|e| format!("delete_single_agent_workflow: {}", e))?;
            workflow::triggers::unregister_single_agent_trigger(agent_id, binding_name, &self.store);
            write_agent_json_to_fs(&agent.napp_path, &fm);

            // Restart the worker so live (event/heartbeat/watch) triggers for
            // the deleted binding tear down immediately.
            if let Some(workers) = self.agent_workers.get() {
                workers.start_agent(agent_id, &agent.name, None).await;
            }

            self.hub.broadcast(
                "agent_workflow_deleted",
                serde_json::json!({ "agentId": agent_id, "binding": binding_name }),
            );
            info!(agent_id, binding = %binding_name, "agent workflow deleted via tool");
            Ok(())
        })
    }

    fn run_inline<'a>(
        &'a self,
        definition_json: String,
        inputs: serde_json::Value,
        trigger_type: &'a str,
        trigger_detail: Option<String>,
        agent_id: &'a str,
        emit_source: Option<String>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        Box::pin(async move {
            // A call tree is a phone line's declarative config — the voice
            // session consumes it live; the engine must NEVER execute it as
            // a graph. Any pathway that lands one here (cron drift, the work
            // tool, a manual run) is a bug, refused loudly.
            if serde_json::from_str::<serde_json::Value>(&definition_json)
                .ok()
                .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(|t| t == "call_tree"))
                .unwrap_or(false)
            {
                return Err(
                    "call trees are phone-line configuration, not runnable workflows".into(),
                );
            }

            let mut def = workflow::parser::parse_workflow(&definition_json)
                .map_err(|e| format!("parse inline workflow: {}", e))?;
            self.expand_command_params(&mut def, &self.store).await?;

            // Merge agent-level input_values into workflow inputs
            let inputs = {
                let mut merged = inputs;
                if let Ok(Some(agent_rec)) = self.store.get_agent(agent_id) {
                    if let Ok(agent_inputs) =
                        serde_json::from_str::<serde_json::Value>(&agent_rec.input_values)
                    {
                        if let (Some(m), Some(r)) =
                            (merged.as_object_mut(), agent_inputs.as_object())
                        {
                            for (k, v) in r {
                                m.entry(k.clone()).or_insert_with(|| v.clone());
                            }
                        }
                    }
                }
                merged
            };

            // Approval checkpoint context (the employee's per-operation policy)
            // and, for a post-approval re-entry, the durable resume state loaded
            // from the run's suspension row (Temporal semantics: rehydrate and
            // continue AT the blocked call — never re-run). The approval
            // endpoint passes the reserved `_resume_run` input with the parked
            // run's id; it is stripped before the model ever sees inputs.
            let (inputs, checkpoint_ctx, resume_state, resume_run_id) = {
                let mut inputs = inputs;
                let resume_run: Option<String> = inputs
                    .as_object_mut()
                    .and_then(|m| m.remove("_resume_run"))
                    .and_then(|v| v.as_str().map(|s| s.to_string()));
                let resume_state = match &resume_run {
                    Some(rid) => {
                        let row = self
                            .store
                            .get_workflow_suspension(rid)
                            .map_err(|e| format!("load suspension: {e}"))?
                            .ok_or_else(|| format!("no suspension for run {rid}"))?;
                        let (_agent, _binding, activity_id, iteration, step_index, messages_json, pending_json, _op, _display) = row;
                        let messages: Vec<ai::Message> = serde_json::from_str(&messages_json)
                            .map_err(|e| format!("suspension messages corrupt: {e}"))?;
                        let pending: ai::ToolCall = serde_json::from_str(&pending_json)
                            .map_err(|e| format!("suspension pending call corrupt: {e}"))?;
                        Some(workflow::engine::ResumeState {
                            activity_id,
                            iteration,
                            step_index,
                            messages,
                            pending,
                        })
                    }
                    None => None,
                };
                let policy = self
                    .store
                    .get_entity_config("agent", agent_id)
                    .ok()
                    .flatten()
                    .and_then(|c| c.operation_policy)
                    .map(|j| tools::policy::OperationPolicy::from_json(Some(&j)));
                let binding = trigger_detail
                    .as_deref()
                    .map(|d| d.split(':').next().unwrap_or(d).to_string())
                    .unwrap_or_default();
                let ctx = policy.map(|p| workflow::engine::CheckpointCtx {
                    operation_policy: Some(p),
                    binding_name: binding,
                });
                (inputs, ctx, resume_state, resume_run)
            };

            // Resolve the concurrency semaphore for this binding. The permit
            // itself is acquired INSIDE the spawned task — acquiring here
            // would block the caller (the EventDispatcher consumer loop runs
            // run_inline serially, so one saturated binding would stall event
            // dispatch for every agent). Events still wait, never dropped.
            let binding_sem = trigger_detail.as_ref().map(|detail| {
                let binding_name = detail.split(':').next().unwrap_or(detail);
                let sem_key = format!("agent:{}:{}", agent_id, binding_name);
                let mut sems = self.binding_semaphores.lock().unwrap();
                sems.entry(sem_key)
                    .or_insert_with(|| Arc::new(Semaphore::new(binding_concurrency())))
                    .clone()
            });

            // Create run record using agent_id for tracking. A resumed run
            // keeps ITS OWN id — the parked row flips back to running; no new
            // run row, no re-created history.
            let run_id = resume_run_id
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            // Canonical agent session key — tools parse the `agent:<id>:` prefix
            // to resolve per-agent plugin accounts and memory scope. The old
            // dash format resolved nothing (briefings ran with no account).
            let session_key = tools::workflow_session_key(agent_id, &run_id);
            if resume_run_id.is_some() {
                // Consume the suspension (the signal is being handled) and
                // wake the parked run.
                let _ = self.store.delete_workflow_suspension(&run_id);
                self.store
                    .update_workflow_run(&run_id, Some("running"), None, None, None, None)
                    .map_err(|e| format!("resume run: {e}"))?;
            } else {
                self.store
                    .create_workflow_run(
                        &run_id,
                        &format!("agent:{}", agent_id),
                        trigger_type,
                        trigger_detail.as_deref(),
                        Some(&inputs.to_string()),
                        Some(&session_key),
                    )
                    .map_err(|e| format!("create_workflow_run: {}", e))?;
            }

            // Create cancellation token
            let cancel_token = CancellationToken::new();
            {
                let mut runs = self.active_runs.lock().unwrap();
                runs.insert(run_id.clone(), cancel_token.clone());
            }
            {
                let mut agent_map = self.agent_runs.lock().unwrap();
                agent_map
                    .entry(agent_id.to_string())
                    .or_default()
                    .push(run_id.clone());
            }

            // Clone Arcs for the spawned task
            let store = self.store.clone();
            let providers = self.providers.clone();
            let tools_registry = self.tools.clone();
            let hub = self.hub.clone();
            let active_runs = self.active_runs.clone();
            let agent_runs = self.agent_runs.clone();
            let event_bus = self.event_bus.clone();
            let skill_loader = self.skill_loader.clone();
            let failure_counts = self.failure_counts.clone();
            let neboai_api_url = self.config.neboai.api_url.clone();
            let run_id_clone = run_id.clone();
            let agent_id_owned = agent_id.to_string();
            let trigger = trigger_type.to_string();
            let binding_name = def.name.clone();

            tokio::spawn(async move {
                // Acquire and hold the binding's concurrency permit for the
                // lifetime of this run; waiting happens in THIS task so the
                // dispatcher stays free. Dropped automatically on completion.
                let _permit = match binding_sem {
                    Some(sem) => match sem.acquire_owned().await {
                        Ok(permit) => Some(permit),
                        Err(e) => {
                            // Semaphores are never closed; degrade to unpermitted
                            // rather than silently dropping the queued event.
                            tracing::warn!(run_id = %run_id_clone, error = %e, "binding semaphore closed, running unpermitted");
                            None
                        }
                    },
                    None => None,
                };

                // Session key for posting chat messages to the agent's conversation
                let chat_session = format!("agent:{}:web", agent_id_owned);

                let provider = {
                    let lock = providers.read().await;
                    lock.first().cloned()
                };
                let provider = match provider {
                    Some(p) => p,
                    None => {
                        let _ = store.update_workflow_run(
                            &run_id_clone,
                            Some("failed"),
                            None,
                            None,
                            Some("no AI provider available"),
                            None,
                        );
                        // Post failure to agent chat
                        post_automation_message(
                            &store,
                            &hub,
                            &chat_session,
                            &format!(
                                "**Automation failed** — {} ({}): no AI provider available",
                                binding_name, trigger
                            ),
                        );

                        // Notify with the same consecutive-failure policy as
                        // engine failures — a missing provider hits every
                        // scheduled run and would otherwise ping on each one.
                        if record_failure_should_notify(
                            &failure_counts,
                            &agent_id_owned,
                            &binding_name,
                            &trigger,
                        ) {
                            notify_workflow_failure(
                                &store,
                                &hub,
                                &agent_id_owned,
                                &run_id_clone,
                                &binding_name,
                                "no AI provider available",
                            );
                        }

                        hub.broadcast(
                            "workflow_run_failed",
                            serde_json::json!({
                                "agentId": agent_id_owned,
                                "runId": run_id_clone,
                                "bindingName": binding_name,
                                "error": "no AI provider available",
                            }),
                        );
                        let now = chrono::Utc::now().to_rfc3339();
                        let _ = store.update_agent_workflow_last_fired(
                            &agent_id_owned,
                            &binding_name,
                            &now,
                        );
                        return;
                    }
                };

                let tool_defs = tools_registry.list().await;
                let resolved_tools: Vec<Box<dyn tools::registry::DynTool>> = tool_defs
                    .iter()
                    .map(|td| {
                        Box::new(RegistryTool {
                            tool_name: td.name.clone(),
                            tool_desc: td.description.clone(),
                            tool_schema: td.input_schema.clone(),
                            registry: tools_registry.clone(),
                        }) as Box<dyn tools::registry::DynTool>
                    })
                    .collect();

                info!(
                    role = %agent_id_owned,
                    run_id = %run_id_clone,
                    trigger = %trigger,
                    tools = resolved_tools.len(),
                    "executing inline workflow in background"
                );

                // Post "started" message to agent chat
                post_automation_message(
                    &store,
                    &hub,
                    &chat_session,
                    &format!("**Automation started** — {} ({})", binding_name, trigger),
                );

                // Record last_fired timestamp
                let now = chrono::Utc::now().to_rfc3339();
                let _ =
                    store.update_agent_workflow_last_fired(&agent_id_owned, &binding_name, &now);

                hub.broadcast(
                    "workflow_run_started",
                    serde_json::json!({
                        "agentId": agent_id_owned,
                        "runId": run_id_clone,
                        "bindingName": binding_name,
                        "triggerType": trigger,
                    }),
                );

                let skill_content = if let Some(ref loader) = skill_loader {
                    let mut map = HashMap::new();
                    for activity in &def.activities {
                        for skill_name in &activity.skills {
                            if !map.contains_key(skill_name) {
                                // Global scope: Learned skills are per-employee
                                // and not declarable in workflow activities.
                                if let Some(skill) = loader.get(skill_name, None).await {
                                    if !skill.template.is_empty() {
                                        let expanded = loader.expand_template(&skill, Some(&store));
                                        map.insert(skill_name.clone(), expanded);
                                    }
                                }
                            }
                        }
                    }
                    if map.is_empty() { None } else { Some(map) }
                } else {
                    None
                };

                // Create progress channel for live activity + task updates
                let (progress_tx, mut progress_rx) =
                    tokio::sync::mpsc::unbounded_channel::<workflow::WorkflowProgress>();
                {
                    let hub = hub.clone();
                    let agent_id_for_progress = agent_id_owned.clone();
                    let run_id = run_id_clone.clone();
                    let binding = binding_name.clone();
                    tokio::spawn(async move {
                        while let Some(progress) = progress_rx.recv().await {
                            match progress {
                                workflow::WorkflowProgress::ActivityStarted {
                                    activity_id,
                                    activity_index,
                                    total_activities,
                                } => {
                                    hub.broadcast(
                                        "workflow_activity_update",
                                        serde_json::json!({
                                            "agentId": agent_id_for_progress,
                                            "runId": run_id,
                                            "bindingName": binding,
                                            "activityId": activity_id,
                                            "step": activity_index + 1,
                                            "totalSteps": total_activities,
                                        }),
                                    );
                                }
                                workflow::WorkflowProgress::TaskUpdated {
                                    list_id,
                                    task_id,
                                    seq,
                                    status,
                                } => {
                                    hub.broadcast(
                                        "task_updated",
                                        serde_json::json!({
                                            "listId": list_id,
                                            "taskId": task_id,
                                            "seq": seq,
                                            "status": status,
                                        }),
                                    );
                                }
                            }
                        }
                    });
                }

                // Deferred tool names (MCP proxies etc.): activities only get
                // their schemas when they declare or reference them.
                let deferred_names = tools_registry.get_deferred_names().await;
                let (wf_memory_user_id, wf_memory_writes_disabled) =
                    workflow_memory_scope(&store, &agent_id_owned);
                match workflow::engine::execute_workflow(
                    &def,
                    &agent_id_owned,
                    &wf_memory_user_id,
                    wf_memory_writes_disabled,
                    inputs,
                    &trigger,
                    None,
                    &store,
                    &*provider,
                    &resolved_tools,
                    Some(&deferred_names),
                    Some(&run_id_clone),
                    Some(&cancel_token),
                    skill_content.as_ref(),
                    event_bus.as_ref(),
                    emit_source,
                    Some(progress_tx),
                    checkpoint_ctx.as_ref(),
                    resume_state,
                )
                .await
                {
                    Ok((_engine_run_id, output)) => {
                        // Engine already called complete_workflow_run with output

                        // A working automation resets its consecutive-failure count
                        failure_counts
                            .lock()
                            .unwrap()
                            .remove(&(agent_id_owned.clone(), binding_name.clone()));

                        // Post completion message with output to agent chat
                        let summary = if output.is_empty() {
                            format!("**Automation completed** — {} ({})", binding_name, trigger)
                        } else {
                            // Truncate output to ~4000 chars to keep chat messages reasonable
                            let truncated = if output.len() > 4000 {
                                let mut end = 4000;
                                while !output.is_char_boundary(end) {
                                    end -= 1;
                                }
                                &output[..end]
                            } else {
                                &output
                            };
                            format!(
                                "**Automation completed** — {} ({})\n\n{}",
                                binding_name, trigger, truncated
                            )
                        };
                        post_automation_message(&store, &hub, &chat_session, &summary);

                        hub.broadcast(
                            "workflow_run_completed",
                            serde_json::json!({
                                "agentId": agent_id_owned,
                                "runId": run_id_clone,
                                "bindingName": binding_name,
                            }),
                        );
                        record_run_outcome(&store, &agent_id_owned, &binding_name, "completed", &output);
                        info!(role = %agent_id_owned, run_id = %run_id_clone, "inline workflow completed");
                    }
                    Err(workflow::WorkflowError::AwaitingApproval { operation, display }) => {
                        // Not a failure: the run is parked (engine persisted the
                        // suspension + awaiting_approval status). Tell the owner
                        // in chat + Inbox; the approval endpoint resumes/denies.
                        post_automation_message(
                            &store,
                            &hub,
                            &chat_session,
                            &format!(
                                "**Automation paused for your approval** — {} ({}): {}",
                                binding_name, trigger, display
                            ),
                        );
                        notify_workflow_approval(
                            &store,
                            &hub,
                            &neboai_api_url,
                            &agent_id_owned,
                            &run_id_clone,
                            &binding_name,
                            &display,
                        );
                        hub.broadcast(
                            "workflow_run_awaiting_approval",
                            serde_json::json!({
                                "agentId": agent_id_owned,
                                "runId": run_id_clone,
                                "bindingName": binding_name,
                                "operation": operation,
                                "display": display,
                            }),
                        );
                        info!(role = %agent_id_owned, run_id = %run_id_clone, %operation, "inline workflow awaiting approval");
                    }
                    Err(e) => {
                        let err_msg = e.to_string();
                        let _ = store.update_workflow_run(
                            &run_id_clone,
                            Some("failed"),
                            None,
                            None,
                            Some(&err_msg),
                            None,
                        );

                        // Post failure message to agent chat
                        post_automation_message(
                            &store,
                            &hub,
                            &chat_session,
                            &format!(
                                "**Automation failed** — {} ({}): {}",
                                binding_name, trigger, err_msg
                            ),
                        );

                        // Notify the owner — manual runs immediately, unattended
                        // runs at the 2nd consecutive failure (see
                        // record_failure_should_notify).
                        if record_failure_should_notify(
                            &failure_counts,
                            &agent_id_owned,
                            &binding_name,
                            &trigger,
                        ) {
                            notify_workflow_failure(
                                &store,
                                &hub,
                                &agent_id_owned,
                                &run_id_clone,
                                &binding_name,
                                &err_msg,
                            );
                        }

                        hub.broadcast(
                            "workflow_run_failed",
                            serde_json::json!({
                                "agentId": agent_id_owned,
                                "runId": run_id_clone,
                                "bindingName": binding_name,
                                "error": err_msg,
                            }),
                        );
                        // Outcome history: exits are informative ("nothing to
                        // do today"), cancellations are owner actions, not
                        // outcomes worth remembering.
                        match &e {
                            workflow::WorkflowError::Cancelled => {}
                            workflow::WorkflowError::Exited(reason) => {
                                record_run_outcome(&store, &agent_id_owned, &binding_name, "exited", reason);
                            }
                            _ => {
                                record_run_outcome(&store, &agent_id_owned, &binding_name, "failed", &err_msg);
                                // Workflow review fork (fork-lite): genuine
                                // failures may carry a durable lesson. Same
                                // learning-mode gate as the chat fork.
                                let mode = learning_mode_for(&store, &agent_id_owned);
                                if mode == "auto" || mode == "staged" {
                                    tokio::spawn(review_failed_workflow_run(
                                        providers.clone(),
                                        tools_registry.clone(),
                                        agent_id_owned.clone(),
                                        binding_name.clone(),
                                        definition_json.clone(),
                                        err_msg.clone(),
                                        mode == "staged",
                                    ));
                                }
                            }
                        }
                        warn!(role = %agent_id_owned, run_id = %run_id_clone, error = %err_msg, "inline workflow failed");
                    }
                }

                // Clean up from active_runs and agent_runs
                {
                    let mut runs = active_runs.lock().unwrap();
                    runs.remove(&run_id_clone);
                }
                {
                    let mut agent_map = agent_runs.lock().unwrap();
                    if let Some(ids) = agent_map.get_mut(&agent_id_owned) {
                        ids.retain(|id| id != &run_id_clone);
                        if ids.is_empty() {
                            agent_map.remove(&agent_id_owned);
                        }
                    }
                }
            });

            Ok(run_id)
        })
    }

    fn cancel<'a>(
        &'a self,
        run_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move { self.cancel_run(run_id).await })
    }

    fn cancel_runs_for_agent<'a>(
        &'a self,
        agent_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move { self.cancel_runs_for_agent_impl(agent_id).await })
    }
}

/// Registry-backed tool wrapper for workflow execution.
///
/// Snapshots tool metadata at construction time and delegates execution to the
/// shared Registry. This avoids holding the Registry's RwLock across await points.
struct RegistryTool {
    tool_name: String,
    tool_desc: String,
    tool_schema: serde_json::Value,
    registry: Arc<tools::Registry>,
}

impl DynTool for RegistryTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> String {
        self.tool_desc.clone()
    }

    fn schema(&self) -> serde_json::Value {
        self.tool_schema.clone()
    }

    fn requires_approval(&self) -> bool {
        false // Workflows run headless
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move { self.registry.execute(ctx, &self.tool_name, input).await })
    }
}

/// Record a failure for (agent, binding) and decide whether to notify the owner.
///
/// Manual runs always notify — the user is watching. Unattended runs
/// (schedule/heartbeat/event/watch) notify exactly once, at the 2nd consecutive
/// failure: a lone blip stays quiet, and a persistently broken automation can't
/// fail silently for days. Stays quiet past 2 until a success resets the count.
fn record_failure_should_notify(
    counts: &std::sync::Mutex<HashMap<(String, String), u32>>,
    agent_id: &str,
    binding_name: &str,
    trigger: &str,
) -> bool {
    if trigger == "manual" {
        return true;
    }
    let mut map = counts.lock().unwrap();
    let count = map
        .entry((agent_id.to_string(), binding_name.to_string()))
        .or_insert(0);
    *count += 1;
    *count == 2
}

/// Create an in-app notification for a workflow run failure, deep-linked to the run.
/// Owner notification for a run parked at the approval checkpoint. Same
/// Inbox + broadcast pathway as failure notifications; type "approval" so the
/// Inbox can render Approve/Deny affordances against the approval endpoint.
fn notify_workflow_approval(
    store: &db::Store,
    hub: &ClientHub,
    api_url: &str,
    agent_id: &str,
    run_id: &str,
    binding_name: &str,
    display: &str,
) {
    let notif_id = format!("wf-approval:{}", run_id);
    let title = format!("{} needs your approval", binding_name);
    let action_url = format!("/{}/runs/{}", agent_id, run_id);
    let user_id = store.ensure_local_user_id().unwrap_or_default();
    if let Err(e) = store.create_notification(
        &notif_id,
        &user_id,
        "approval",
        &title,
        Some(display),
        Some(&action_url),
        None,
        Some(agent_id),
    ) {
        warn!(run_id = %run_id, error = %e, "failed to create workflow approval notification");
    } else {
        hub.broadcast(
            "notification_created",
            serde_json::json!({
                "id": notif_id,
                "type": "approval",
                "title": title,
                "body": display,
                "actionUrl": action_url,
                "agentId": agent_id,
                "readAt": null,
            }),
        );
        // Mirror to the owner's unified inbox at neboai.com/app. The item is
        // self-describing: the buttons carry tunnel-relative resolve calls, so
        // the hub renders and proxies them without learning bot route shapes.
        let approval_path = format!("/api/v1/agents/workflow-runs/{}/approval", run_id);
        crate::codes::push_inbox_via(
            store,
            api_url,
            serde_json::json!({
                "id": notif_id,
                "type": "approval",
                "title": title,
                "body": display,
                "link": action_url,
                "actions": {
                    "buttons": [
                        {"label": "Approve", "style": "primary", "method": "POST",
                         "path": approval_path, "body": {"approved": true}},
                        {"label": "Deny", "style": "danger", "method": "POST",
                         "path": approval_path, "body": {"approved": false}},
                    ],
                    "status": {"method": "GET", "path": approval_path},
                },
            }),
        );
    }
}

fn notify_workflow_failure(
    store: &db::Store,
    hub: &ClientHub,
    agent_id: &str,
    run_id: &str,
    binding_name: &str,
    error: &str,
) {
    let notif_id = format!("wf-fail:{}", run_id);
    let title = format!("{} failed", binding_name);
    // Truncate error to keep the notification body concise
    let body = if error.len() > 200 {
        &error[..error.ceil_char_boundary(200)]
    } else {
        error
    };
    let action_url = format!("/{}/runs/{}", agent_id, run_id);

    let user_id = store.ensure_local_user_id().unwrap_or_default();
    if let Err(e) = store.create_notification(
        &notif_id,
        &user_id,
        "error",
        &title,
        Some(body),
        Some(&action_url),
        None,
        Some(agent_id),
    ) {
        warn!(run_id = %run_id, error = %e, "failed to create workflow failure notification");
    } else {
        hub.broadcast(
            "notification_created",
            serde_json::json!({
                "id": notif_id,
                "type": "error",
                "title": title,
                "body": body,
                "actionUrl": action_url,
                "agentId": agent_id,
                "readAt": null,
                "createdAt": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            }),
        );
    }
}

/// Post an automation lifecycle message to an agent's chat session.
/// The agent's learning mode, lowercased: "auto" (Learn freely), "staged"
/// (Ask me first), or "" / "off" (does not learn). Single read shared by the
/// outcome recorder and the workflow review fork so the gates can't drift.
fn learning_mode_for(store: &db::Store, agent_id: &str) -> String {
    store
        .get_entity_config("agent", agent_id)
        .ok()
        .flatten()
        .and_then(|c| c.learning_mode)
        .map(|m| m.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Record a one-line outcome memory in the agent's scope after a workflow run:
/// one row per binding (upsert), value = latest date + status + gist. This is
/// history, not a lesson — the next run sees it via the engine's recent-memory
/// slice, which is what makes dedup ("already published today") and error
/// avoidance possible. Honors learning modes: written for auto and staged,
/// skipped for off — an agent set to Off does not accumulate anything.
/// Fail-closed read of an agent's `memory.context_isolated` flag: an empty
/// frontmatter is a legitimate default (not isolated), but config that exists
/// and cannot be parsed — or an agent row that cannot be read — counts as
/// isolated. "Couldn't read the isolation flag" must never mean "not isolated"
/// (isolation audit 2026-08-22, fail-open class).
pub(crate) fn agent_context_isolated(store: &db::Store, agent_id: &str) -> bool {
    if agent_id.is_empty() {
        return false;
    }
    match store.get_agent(agent_id) {
        Ok(Some(a)) if a.frontmatter.is_empty() => false,
        Ok(Some(a)) => napp::agent::parse_agent_config(&a.frontmatter)
            .map(|c| c.memory.context_isolated)
            .unwrap_or(true),
        _ => true,
    }
}

/// Memory scope a workflow run executes tools under. Workflow runs have no
/// matter/chat context, so for context-isolated agents the scope stays the
/// agent base and WRITES ARE DISABLED (fail closed) — reads still serve the
/// agent scope.
fn workflow_memory_scope(store: &db::Store, agent_id: &str) -> (String, bool) {
    let owner = store.ensure_local_user_id().unwrap_or_default();
    if agent_id.is_empty() {
        return (owner, false);
    }
    (
        agent::memory::agent_memory_scope(&owner, agent_id),
        agent_context_isolated(store, agent_id),
    )
}

fn record_run_outcome(store: &db::Store, agent_id: &str, binding: &str, status: &str, detail: &str) {
    let learning_mode = learning_mode_for(store, agent_id);
    if learning_mode != "auto" && learning_mode != "staged" {
        return;
    }

    let owner = store.ensure_local_user_id().unwrap_or_default();
    if owner.is_empty() {
        return;
    }
    let scope = agent::memory::agent_memory_scope(&owner, agent_id);

    // Context-isolated agents keep case/matter data sealed per context. The
    // outcome row lives in the SHARED agent scope, so for isolated agents it
    // carries status only — run output could name a client or matter.
    let context_isolated = agent_context_isolated(store, agent_id);
    let detail: &str = if context_isolated { "" } else { detail };

    let date = chrono::Utc::now().format("%Y-%m-%d %H:%M UTC");
    let mut gist = detail.replace('\n', " ");
    if gist.len() > 240 {
        let mut end = 240;
        while !gist.is_char_boundary(end) {
            end -= 1;
        }
        gist.truncate(end);
        gist.push('…');
    }
    let value = format!("{} run {}: {}", date, status, gist);
    if let Err(e) = store.upsert_memory("project/workflow-history", binding, &value, None, None, &scope)
    {
        warn!(agent = agent_id, binding, error = %e, "failed to record run outcome memory");
    }
}

/// Workflow-side entry to the SAME lesson store the chat review fork writes.
///
/// On a genuinely failed run, one aux LLM call reads the run report and either
/// stays silent or produces ONE durable lesson, committed through the skill
/// tool's learned pathway — so the learning modes gate identically to chat:
/// auto commits to the agent's learned tree, staged becomes a pending write in
/// the owner's Inbox, off never reaches here. Duplicate lesson names are
/// rejected by the tool (natural dedup for recurring failures); refining an
/// existing lesson is the chat fork's / curator's job, not this one's.
async fn review_failed_workflow_run(
    providers: Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    registry: Arc<tools::Registry>,
    agent_id: String,
    binding: String,
    def_json: String,
    err_msg: String,
    staged: bool,
) {
    let provider = {
        let guard = providers.read().await;
        match guard.first() {
            Some(p) => p.clone(),
            None => return,
        }
    };

    const REVIEW_SYSTEM: &str = "You review a FAILED scheduled workflow run and decide whether there is \
        ONE durable lesson worth saving for this agent. Most failures teach nothing durable — transient \
        network errors, provider hiccups, rate limits, one-off bad data: answer null for those. A lesson \
        is durable only if it would change how the NEXT run should behave: a tool quirk, a required \
        format, a wrong assumption baked into the steps. Never store secrets, credentials, tokens, or \
        personal data. Lessons are shared across every context this agent serves, so they must contain \
        ZERO client-, case-, matter-, or engagement-specific information — general procedure only; if \
        the failure cannot be expressed without naming a client or case, answer null. \
        Respond with STRICT JSON only, no prose, no code fences: \
        {\"lesson\": null} or {\"lesson\": {\"name\": \"kebab-case-name\", \"content\": \"---\\nname: <name>\\ndescription: <one line>\\n---\\n<what to do differently next run, concrete and specific>\"}}";

    let mut def_excerpt = def_json.clone();
    if def_excerpt.len() > 4000 {
        let mut end = 4000;
        while !def_excerpt.is_char_boundary(end) {
            end -= 1;
        }
        def_excerpt.truncate(end);
    }
    let report = format!(
        "Workflow binding: {}\n\nDefinition:\n{}\n\nFailure:\n{}",
        binding, def_excerpt, err_msg
    );

    let req = ai::ChatRequest {
        tool_choice: Default::default(),
        messages: vec![ai::Message {
            role: "user".into(),
            content: report,
            ..Default::default()
        }],
        tools: vec![],
        max_tokens: 700,
        temperature: 0.0,
        system: REVIEW_SYSTEM.to_string(),
        static_system: String::new(),
        model: String::new(),
        enable_thinking: false,
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: None,
        trace: None,
    };
    let mut rx = match provider.stream(&req).await {
        Ok(rx) => rx,
        Err(e) => {
            warn!(agent = %agent_id, binding = %binding, error = %e, "workflow review call failed");
            return;
        }
    };
    let mut text = String::new();
    while let Some(event) = rx.recv().await {
        match event.event_type {
            ai::StreamEventType::Text => text.push_str(&event.text),
            ai::StreamEventType::Error => {
                warn!(agent = %agent_id, binding = %binding, "workflow review stream error");
                return;
            }
            ai::StreamEventType::Done => break,
            _ => {}
        }
    }

    let trimmed = text.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim();
    let parsed: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => {
            info!(agent = %agent_id, binding = %binding, "workflow review produced no parseable verdict");
            return;
        }
    };
    let Some(lesson) = parsed.get("lesson").filter(|l| !l.is_null()) else {
        info!(agent = %agent_id, binding = %binding, "workflow review: no durable lesson");
        return;
    };
    let (Some(name), Some(content)) = (
        lesson.get("name").and_then(|v| v.as_str()),
        lesson.get("content").and_then(|v| v.as_str()),
    ) else {
        return;
    };

    // Commit through the one learned-skill pathway; the tool enforces owner
    // scoping, staging, frontmatter validation, and duplicate rejection.
    let mut ctx = ToolContext::new(tools::Origin::System)
        .with_session(format!("agent:{}:workflow-review", agent_id), format!("wfreview-{}", uuid::Uuid::new_v4()));
    ctx.learned_write_agent = Some(agent_id.clone());
    ctx.learned_write_staged = staged;
    ctx.tool_whitelist = Some(std::collections::HashSet::from(["skill".to_string()]));

    let input = serde_json::json!({
        "action": "create",
        "name": name,
        "content": content,
    });
    let result = registry.execute(&ctx, "skill", input).await;
    if result.is_error {
        info!(agent = %agent_id, binding = %binding, result = %result.content.chars().take(160).collect::<String>(), "workflow lesson not saved");
    } else {
        info!(agent = %agent_id, binding = %binding, lesson = %name, staged, "workflow review saved lesson");
    }
}

/// Match an agent reference against (id, name) pairs: exact name
/// (case-insensitive) first, then slug equality ("Content Creator Agent"
/// matches "content-creator-agent"). Pure core of `resolve_agent`.
fn find_agent_by_name<'a>(agents: &[(&'a str, &str)], agent_ref: &str) -> Option<&'a str> {
    if let Some((id, _)) = agents
        .iter()
        .find(|(_, n)| n.eq_ignore_ascii_case(agent_ref))
    {
        return Some(id);
    }
    let want = slug(agent_ref);
    if want.is_empty() {
        return None;
    }
    agents
        .iter()
        .find(|(_, n)| slug(n) == want)
        .map(|(id, _)| *id)
}

/// Resolve a tool-authored workflow definition's trigger. Accepts a `trigger`
/// object ({type, ...}) or a top-level `schedule` (cron string, or a
/// {cron: "..."} map). A trigger that is PRESENT but malformed is a hard
/// error, never a silent downgrade to manual — that downgrade is how an agent
/// shipped seven never-firing workflows while reporting "active with proper
/// schedules" (2026-08-01). Manual stays the default only when no trigger was
/// asked for at all. Schedule crons must parse once normalized; human phrases
/// ("weekdays at 9am") are fine — that's what normalize_cron is for.
fn resolve_tool_trigger(
    def: &serde_json::Value,
) -> Result<(String, serde_json::Value), String> {
    const TRIGGER_TYPES: &[&str] =
        &["schedule", "heartbeat", "event", "watch", "folder", "manual"];
    let (trigger_type, mut trigger_config): (String, serde_json::Value) =
        if let Some(t) = def.get("trigger") {
            let ty = t
                .as_object()
                .and_then(|o| o.get("type"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    format!(
                        "trigger must be an object with a \"type\" of: {} — got: {}",
                        TRIGGER_TYPES.join(", "),
                        t
                    )
                })?;
            if !TRIGGER_TYPES.contains(&ty) {
                return Err(format!(
                    "unknown trigger type '{}' — valid: {}",
                    ty,
                    TRIGGER_TYPES.join(", ")
                ));
            }
            (ty.to_string(), t.clone())
        } else if let Some(s) = def.get("schedule") {
            let cron = s
                .as_str()
                .or_else(|| s.get("cron").and_then(|v| v.as_str()))
                .ok_or_else(|| {
                    format!(
                        "schedule must be a cron string like \"0 9 * * MON-FRI\" \
                         (or {{\"cron\": \"...\"}}) — got: {}",
                        s
                    )
                })?;
            if cron.is_empty() {
                ("manual".to_string(), serde_json::json!({}))
            } else {
                ("schedule".to_string(), serde_json::json!({ "cron": cron }))
            }
        } else {
            ("manual".to_string(), serde_json::json!({}))
        };

    if trigger_type == "schedule" {
        if trigger_config.get("cron").and_then(|v| v.as_str()).is_none() {
            // Trigger-object form may carry the expression in the human
            // `schedule` field — promote it so storage normalizes it.
            if let Some(s) = trigger_config
                .get("schedule")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
            {
                trigger_config["cron"] = serde_json::json!(s);
            }
        }
        let raw = trigger_config
            .get("cron")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if raw.is_empty() {
            return Err(
                "schedule trigger requires a cron expression, e.g. \
                 {\"type\": \"schedule\", \"cron\": \"0 9 * * MON-FRI\"}"
                    .to_string(),
            );
        }
        let normalized = tools::PersonaTool::normalize_cron(raw);
        if normalized.parse::<cron::Schedule>().is_err() {
            return Err(format!(
                "invalid cron expression {:?} (normalized to {:?}) — use standard \
                 5-field cron like \"0 9 * * MON-FRI\" or a phrase like \
                 \"weekdays at 9am\"",
                raw, normalized
            ));
        }
    }
    Ok((trigger_type, trigger_config))
}

/// Shared create/update body for caller-owned workflow bindings: parse the
/// agent-authored definition, build the binding, persist it everywhere
/// (frontmatter, tracking row, triggers, agent.json), and go live. The
/// `must_exist` flag splits the verbs: create is new-only (no silent clobber
/// of a live automation), update is full-replacement and requires the binding
/// to exist (typo-safe, mirrors delete). Run history is keyed by binding name,
/// so updates keep it attached.
async fn save_binding(
    mgr: &WorkflowManagerImpl,
    agent_id: &str,
    name: &str,
    definition: &str,
    must_exist: bool,
) -> Result<WorkflowInfo, String> {
    use crate::handlers::agents::{build_trigger_json, flatten_trigger_config, write_agent_json_to_fs};

    if agent_id.is_empty() {
        return Err("workflow creation must be scoped to an agent".to_string());
    }
    {
            // The definition is agent-authored JSON; accept it loosely and pull
            // out the binding fields. (parse_workflow is too strict — it drops
            // unknown fields silently, which is how orphans were born.)
            let mut def: serde_json::Value = serde_json::from_str(definition)
                .map_err(|e| format!("invalid workflow definition (not JSON): {}", e))?;

            // Convenience: a top-level `steps` array becomes ONE activity that
            // executes them in order — same simple-form semantics as agent
            // registry automations.
            if def.get("activities").is_none()
                && let Some(steps) = def.get("steps").cloned()
                && steps.is_array()
            {
                let intent = def
                    .get("description")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or(name)
                    .to_string();
                def["activities"] =
                    serde_json::json!([{ "id": "run", "intent": intent, "steps": steps }]);
            }

            // Reject hollow definitions loudly — a workflow with no activities
            // can never execute, and silently storing one reads as success.
            let has_runnable_activity = def
                .get("activities")
                .and_then(|v| v.as_array())
                .is_some_and(|acts| {
                    !acts.is_empty()
                        && acts.iter().any(|a| {
                            a.get("steps").and_then(|s| s.as_array()).is_some_and(|s| !s.is_empty())
                                || a.get("intent").and_then(|i| i.as_str()).is_some_and(|i| !i.is_empty())
                        })
                });
            if !has_runnable_activity {
                return Err(
                    "workflow definition has no runnable activities — it would never execute. \
                     Shape: {\"trigger\": {\"type\": \"schedule\", \"cron\": \"0 9 * * MON-FRI\"}, \
                     \"activities\": [{\"id\": \"run\", \"intent\": \"what this accomplishes\", \
                     \"steps\": [\"concrete step 1\", \"concrete step 2\"]}]} — or pass a top-level \
                     \"steps\" array for the simple form."
                        .to_string(),
                );
            }

            let (trigger_type, trigger_config) = resolve_tool_trigger(&def)?;

            let binding_name = slug(name);
            if binding_name.is_empty() {
                return Err("name must contain at least one alphanumeric character".to_string());
            }

            let existing = mgr
                .store
                .list_agent_workflows(agent_id)
                .map_err(|e| format!("list_agent_workflows: {}", e))?;
            let exists = existing.iter().any(|w| w.binding_name == binding_name);
            if must_exist && !exists {
                let names: Vec<&str> = existing.iter().map(|w| w.binding_name.as_str()).collect();
                return Err(format!(
                    "no workflow named '{}' — this agent owns: {}. Use create for a new workflow.",
                    binding_name,
                    if names.is_empty() { "(none)".to_string() } else { names.join(", ") }
                ));
            }
            if !must_exist && exists {
                return Err(format!(
                    "workflow '{}' already exists. Use update to modify it (full replacement), or pick a new name.",
                    binding_name
                ));
            }


            let agent = mgr
                .store
                .get_agent(agent_id)
                .map_err(|e| format!("get_agent: {}", e))?
                .ok_or_else(|| format!("agent '{}' not found", agent_id))?;

            // Merge the binding into the agent's frontmatter (source of truth).
            let mut fm: serde_json::Value =
                serde_json::from_str(&agent.frontmatter).unwrap_or_else(|_| serde_json::json!({}));
            if fm.get("workflows").is_none() {
                fm["workflows"] = serde_json::json!({});
            }
            let mut binding_val = serde_json::json!({
                "trigger": build_trigger_json(&trigger_type, &trigger_config),
            });
            if let Some(d) = def.get("description") {
                binding_val["description"] = d.clone();
            }
            if let Some(i) = def.get("inputs") {
                binding_val["inputs"] = i.clone();
            }
            if let Some(a) = def.get("activities") {
                binding_val["activities"] = a.clone();
            }
            if let Some(c) = def.get("connections") {
                binding_val["connections"] = c.clone();
            }
            if let Some(e) = def.get("emit") {
                binding_val["emit"] = e.clone();
            }
            fm["workflows"][binding_name.as_str()] = binding_val;

            mgr.store
                .update_agent(
                    agent_id,
                    &agent.name,
                    &agent.description,
                    &agent.agent_md,
                    &fm.to_string(),
                    agent.pricing_model.as_deref(),
                    agent.pricing_cost,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .map_err(|e| format!("update_agent: {}", e))?;

            // Tracking row — the canonical store the UI panel reads.
            let desc = def.get("description").and_then(|v| v.as_str());
            let inputs_json = def.get("inputs").and_then(|v| serde_json::to_string(v).ok());
            let emit = def.get("emit").and_then(|v| v.as_str());
            let activities_json = def
                .get("activities")
                .and_then(|v| serde_json::to_string(v).ok());
            let connections_json = def
                .get("connections")
                .and_then(|v| serde_json::to_string(v).ok());
            mgr.store
                .upsert_agent_workflow(
                    agent_id,
                    &binding_name,
                    &trigger_type,
                    &flatten_trigger_config(&trigger_type, &trigger_config),
                    desc,
                    inputs_json.as_deref(),
                    emit,
                    activities_json.as_deref(),
                    connections_json.as_deref(),
                )
                .map_err(|e| format!("upsert_agent_workflow: {}", e))?;

            // Register schedule cron rows now so the scheduler fires them within
            // a minute, even if the worker isn't restarted below.
            if let Ok(bindings) = mgr.store.list_agent_workflows(agent_id) {
                workflow::triggers::register_agent_triggers(agent_id, &bindings, &mgr.store);
            }

            write_agent_json_to_fs(&agent.napp_path, &fm);

            // Restart the owning agent's worker so live triggers (event/heartbeat/
            // watch/folder) register immediately. Schedule triggers already went
            // live via register_agent_triggers above.
            if let Some(workers) = mgr.agent_workers.get() {
                workers.start_agent(agent_id, &agent.name, None).await;
            }

            mgr.hub.broadcast(
                "agent_workflow_created",
                serde_json::json!({ "agentId": agent_id, "binding": binding_name }),
            );
            info!(agent_id, binding = %binding_name, trigger = %trigger_type, "agent workflow created via tool");

            let activity_count = def
                .get("activities")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            Ok(WorkflowInfo {
                id: binding_name.clone(),
                name: name.to_string(),
                version: "1.0".to_string(),
                description: desc.unwrap_or("").to_string(),
                is_enabled: true,
                trigger_count: if trigger_type == "manual" { 0 } else { 1 },
                activity_count,
            })
    }
}

/// Apply a full workflow binding definition to an agent: frontmatter merge,
/// tracking row, trigger registration, and on-disk agent.json — the same
/// primitives the REST create/update handlers use. Shared by the tuning pass
/// (auto mode) and the Inbox approval handler (staged mode).
pub(crate) fn apply_workflow_binding(
    store: &db::Store,
    agent_id: &str,
    binding_name: &str,
    binding_val: &serde_json::Value,
) -> Result<(), String> {
    use crate::handlers::agents::{flatten_trigger_config, write_agent_json_to_fs};

    let agent = store
        .get_agent(agent_id)
        .map_err(|e| format!("get_agent: {}", e))?
        .ok_or_else(|| format!("agent '{}' not found", agent_id))?;

    let mut fm: serde_json::Value =
        serde_json::from_str(&agent.frontmatter).unwrap_or_else(|_| serde_json::json!({}));
    if fm.get("workflows").is_none() {
        fm["workflows"] = serde_json::json!({});
    }
    fm["workflows"][binding_name] = binding_val.clone();

    store
        .update_agent(
            agent_id,
            &agent.name,
            &agent.description,
            &agent.agent_md,
            &fm.to_string(),
            agent.pricing_model.as_deref(),
            agent.pricing_cost,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .map_err(|e| format!("update_agent: {}", e))?;

    let trigger = binding_val.get("trigger").cloned().unwrap_or(serde_json::json!({"type": "manual"}));
    let trigger_type = trigger
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("manual")
        .to_string();
    let desc = binding_val.get("description").and_then(|v| v.as_str());
    let inputs_json = binding_val.get("inputs").and_then(|v| serde_json::to_string(v).ok());
    let emit = binding_val.get("emit").and_then(|v| v.as_str());
    let activities_json = binding_val
        .get("activities")
        .and_then(|v| serde_json::to_string(v).ok());
    let connections_json = binding_val
        .get("connections")
        .and_then(|v| serde_json::to_string(v).ok());
    store
        .upsert_agent_workflow(
            agent_id,
            binding_name,
            &trigger_type,
            &flatten_trigger_config(&trigger_type, &trigger),
            desc,
            inputs_json.as_deref(),
            emit,
            activities_json.as_deref(),
            connections_json.as_deref(),
        )
        .map_err(|e| format!("upsert_agent_workflow: {}", e))?;

    if let Ok(bindings) = store.list_agent_workflows(agent_id) {
        workflow::triggers::register_agent_triggers(agent_id, &bindings, store);
    }
    write_agent_json_to_fs(&agent.napp_path, &fm);
    Ok(())
}

/// The current binding definition JSON from the agent's frontmatter, used as
/// the conflict token for staged tuning proposals: if the binding changed
/// between staging and approval, the proposal is discarded, not blind-applied.
pub(crate) fn current_binding_json(store: &db::Store, agent_id: &str, binding: &str) -> String {
    store
        .get_agent(agent_id)
        .ok()
        .flatten()
        .and_then(|a| serde_json::from_str::<serde_json::Value>(&a.frontmatter).ok())
        .and_then(|fm| fm.get("workflows").and_then(|w| w.get(binding)).cloned())
        .map(|b| b.to_string())
        .unwrap_or_default()
}

/// Weekly workflow tuning pass. For each agent with learning enabled and
/// clear evidence of trouble (2+ failed runs in the last 7 days), an aux
/// model reads the stats, recent errors, definitions, and outcome history,
/// and proposes AT MOST ONE minimal edit to ONE binding — the owner's own
/// single-variable SHIP/REVERT doctrine. Honors learning modes:
///   auto   → the edit is applied, with an audit row + owner notification
///   staged → the edit becomes a pending write in the owner's Inbox
///   off    → the agent is skipped entirely
/// Anti-spam: at most one proposal per agent per 7 days, counting rejected
/// ones — a rejection is an answer, not an invitation to re-ask.
async fn workflow_tuning_sweep(
    store: &Arc<db::Store>,
    providers: &Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    registry_hub: (&Arc<tools::Registry>, &Arc<ClientHub>),
    neboai_api_url: &str,
) {
    let (_registry, hub) = registry_hub;
    const WEEK_SECS: i64 = 7 * 24 * 3600;

    let agents = match store.list_agents(500, 0) {
        Ok(a) => a,
        Err(_) => return,
    };

    for agent in agents.iter().filter(|a| a.id != "assistant") {
        let mode = learning_mode_for(store, &agent.id);
        if mode != "auto" && mode != "staged" {
            continue;
        }
        if store
            .has_recent_pending_write(&agent.id, "workflow", WEEK_SECS)
            .unwrap_or(true)
        {
            continue;
        }
        // Evidence gate: 2+ failed runs in the last 7 days.
        let errors = store.agent_recent_errors(&agent.id, 10).unwrap_or_default();
        let week_ago = chrono::Utc::now().timestamp() - WEEK_SECS;
        let recent_failures = errors
            .iter()
            .filter(|e| e.started_at >= week_ago)
            .count();
        if recent_failures < 2 {
            continue;
        }

        let stats = match store.agent_workflow_stats(&agent.id) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let workflows_json = serde_json::from_str::<serde_json::Value>(&agent.frontmatter)
            .ok()
            .and_then(|fm| fm.get("workflows").cloned())
            .unwrap_or(serde_json::json!({}));
        let error_lines: Vec<String> = errors
            .iter()
            .take(5)
            .map(|e| {
                format!(
                    "- [{}] {}",
                    e.activity_id.as_deref().unwrap_or("?"),
                    e.error
                )
            })
            .collect();
        let history: Vec<String> = store
            .recent_memories_for_agent(&agent.id, 6)
            .unwrap_or_default()
            .into_iter()
            .filter(|m| m.namespace == "project/workflow-history")
            .map(|m| format!("- {}: {}", m.key, m.value))
            .collect();

        const TUNER_SYSTEM: &str = "You tune an AI employee's scheduled workflows based on run evidence. \
            Propose AT MOST ONE minimal edit to ONE workflow binding — the smallest change most likely to \
            fix the observed failures. Prefer clarifying or fixing step text; change triggers only when the \
            schedule itself is the failure; never invent new duties. If the evidence is unclear or the \
            failures look transient (network, rate limits, provider errors), propose nothing. \
            Workflow definitions are shared across every context this agent serves: the proposed steps \
            must contain ZERO client-, case-, or matter-specific information — procedure only. \
            Respond with STRICT JSON only, no prose, no code fences: {\"proposal\": null} or \
            {\"proposal\": {\"binding\": \"<name>\", \"reason\": \"<one line>\", \"workflow\": {\"trigger\": {...}, \"description\": \"...\", \"activities\": [{\"id\": \"...\", \"intent\": \"...\", \"steps\": [\"...\"]}]}}} \
            — `workflow` is the complete corrected binding, not a diff.";

        let report = format!(
            "Agent: {}\n\nRun stats: {} total, {} completed, {} failed\n\nRecent errors:\n{}\n\nOutcome history:\n{}\n\nCurrent workflow bindings:\n{}",
            agent.name,
            stats.total_runs,
            stats.completed,
            stats.failed,
            error_lines.join("\n"),
            if history.is_empty() { "- none".to_string() } else { history.join("\n") },
            serde_json::to_string_pretty(&workflows_json).unwrap_or_default()
        );

        let provider = {
            let guard = providers.read().await;
            match guard.first() {
                Some(p) => p.clone(),
                None => return,
            }
        };
        let req = ai::ChatRequest {
            tool_choice: Default::default(),
            messages: vec![ai::Message {
                role: "user".into(),
                content: report,
                ..Default::default()
            }],
            tools: vec![],
            max_tokens: 1500,
            temperature: 0.0,
            system: TUNER_SYSTEM.to_string(),
            static_system: String::new(),
            model: String::new(),
            enable_thinking: false,
            metadata: None,
            cache_breakpoints: vec![],
            cancel_token: None,
            trace: None,
        };
        let mut rx = match provider.stream(&req).await {
            Ok(rx) => rx,
            Err(e) => {
                warn!(agent = %agent.name, error = %e, "tuning pass call failed");
                continue;
            }
        };
        let mut text = String::new();
        while let Some(event) = rx.recv().await {
            match event.event_type {
                ai::StreamEventType::Text => text.push_str(&event.text),
                ai::StreamEventType::Error | ai::StreamEventType::Done => break,
                _ => {}
            }
        }
        let trimmed = text
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();
        let parsed: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let Some(proposal) = parsed.get("proposal").filter(|p| !p.is_null()) else {
            info!(agent = %agent.name, "tuning pass: no proposal");
            continue;
        };
        let (Some(binding), Some(reason), Some(workflow_val)) = (
            proposal.get("binding").and_then(|v| v.as_str()),
            proposal.get("reason").and_then(|v| v.as_str()),
            proposal.get("workflow").filter(|w| w.is_object()),
        ) else {
            continue;
        };
        // The proposal must target an existing binding — tuning edits, it
        // never creates new duties.
        if workflows_json.get(binding).is_none() {
            info!(agent = %agent.name, binding, "tuning pass proposed unknown binding; discarded");
            continue;
        }

        let pending_id = uuid::Uuid::new_v4().to_string();
        let gist = format!("Tune workflow '{}': {}", binding, reason);
        let conflict_token = current_binding_json(store, &agent.id, binding);
        if let Err(e) = store.create_pending_write(
            &pending_id,
            &agent.id,
            "workflow",
            "update",
            binding,
            Some(&workflow_val.to_string()),
            &gist,
            &conflict_token,
        ) {
            warn!(agent = %agent.name, error = %e, "tuning pass: failed to record proposal");
            continue;
        }

        let user_id = store.ensure_local_user_id().unwrap_or_default();
        if mode == "auto" {
            match apply_workflow_binding(store, &agent.id, binding, workflow_val) {
                Ok(()) => {
                    let _ = store.resolve_pending_write(&pending_id, "approved");
                    let _ = store.create_notification_if_not_exists(
                        &format!("learn:{}", pending_id),
                        &user_id,
                        "info",
                        &format!("{} tuned its own workflow", agent.name),
                        Some(&gist),
                        None,
                        None,
                        Some(&agent.id),
                    );
                    // Full payload — an {id}-only broadcast used to land as an
                    // empty row in the Inbox store.
                    hub.broadcast(
                        "notification_created",
                        serde_json::json!({
                            "id": format!("learn:{}", pending_id),
                            "type": "info",
                            "title": format!("{} tuned its own workflow", agent.name),
                            "body": gist,
                            "agentId": agent.id,
                            "readAt": null,
                        }),
                    );
                    info!(agent = %agent.name, binding, "tuning pass applied edit (auto)");
                }
                Err(e) => {
                    let _ = store.resolve_pending_write(&pending_id, "conflict");
                    warn!(agent = %agent.name, binding, error = %e, "tuning pass apply failed");
                }
            }
        } else {
            let _ = store.create_notification_if_not_exists(
                &format!("learn:{}", pending_id),
                &user_id,
                "approval",
                &format!("{} proposes a workflow change", agent.name),
                Some(&gist),
                None,
                None,
                Some(&agent.id),
            );
            // Full payload — an {id}-only broadcast used to land as an empty
            // row in the Inbox store.
            hub.broadcast(
                "notification_created",
                serde_json::json!({
                    "id": format!("learn:{}", pending_id),
                    "type": "approval",
                    "title": format!("{} proposes a workflow change", agent.name),
                    "body": gist,
                    "agentId": agent.id,
                    "readAt": null,
                }),
            );
            // Mirror the staged proposal to the owner's web inbox with its
            // self-describing resolve contract.
            let resolve_path = format!("/api/v1/agents/learnings/{}/resolve", pending_id);
            crate::codes::push_inbox_via(
                store,
                neboai_api_url,
                serde_json::json!({
                    "id": format!("learn:{}", pending_id),
                    "type": "approval",
                    "title": format!("{} proposes a workflow change", agent.name),
                    "body": gist,
                    "actions": {
                        "buttons": [
                            {"label": "Approve", "style": "primary", "method": "POST",
                             "path": resolve_path, "body": {"approved": true}},
                            {"label": "Reject", "style": "danger", "method": "POST",
                             "path": resolve_path, "body": {"approved": false}},
                        ],
                    },
                }),
            );
            info!(agent = %agent.name, binding, "tuning pass staged proposal to Inbox");
        }
    }
}

fn post_automation_message(store: &db::Store, hub: &ClientHub, session_key: &str, content: &str) {
    let msg_id = uuid::Uuid::new_v4().to_string();
    match store.create_chat_message_for_runner(
        &msg_id,
        session_key,
        "assistant",
        content,
        None,
        None,
        None,
        None,
        None,
    ) {
        Ok(_msg) => {
            // Broadcast as chat_complete so the chat UI picks it up in real time
            hub.broadcast(
                "chat_complete",
                serde_json::json!({
                    "chatId": session_key,
                    "content": content,
                    "role": "assistant",
                }),
            );
        }
        Err(e) => {
            warn!(session = %session_key, error = %e, "failed to post automation message to chat");
        }
    }
}

#[cfg(test)]
mod trigger_tests {
    use super::{find_agent_by_name, resolve_tool_trigger};

    #[test]
    fn agent_ref_matches_name_and_slug() {
        let agents = [("id-1", "Content Creator Agent"), ("id-2", "Map Master")];
        assert_eq!(find_agent_by_name(&agents, "content creator agent"), Some("id-1"));
        assert_eq!(find_agent_by_name(&agents, "content-creator-agent"), Some("id-1"));
        assert_eq!(find_agent_by_name(&agents, "Map Master"), Some("id-2"));
        assert_eq!(find_agent_by_name(&agents, "no-such-agent"), None);
        assert_eq!(find_agent_by_name(&agents, ""), None);
    }

    #[test]
    fn schedule_map_form_is_accepted_not_degraded() {
        // The Dexter incident shape: schedule passed as a map
        let def = serde_json::json!({"schedule": {"cron": "0 14,16,18 * * 6"}});
        let (ty, cfg) = resolve_tool_trigger(&def).unwrap();
        assert_eq!(ty, "schedule");
        assert_eq!(cfg["cron"], "0 14,16,18 * * 6");
    }

    #[test]
    fn malformed_schedule_is_a_hard_error() {
        let def = serde_json::json!({"schedule": {"at": "8am"}});
        assert!(resolve_tool_trigger(&def).is_err());
        let def = serde_json::json!({"schedule": 7});
        assert!(resolve_tool_trigger(&def).is_err());
    }

    #[test]
    fn trigger_without_type_is_a_hard_error() {
        let def = serde_json::json!({"trigger": {"cron": "0 9 * * *"}});
        assert!(resolve_tool_trigger(&def).is_err());
        let def = serde_json::json!({"trigger": {"type": "chron"}});
        assert!(resolve_tool_trigger(&def).is_err());
    }

    #[test]
    fn schedule_trigger_requires_parseable_cron() {
        let def = serde_json::json!({"trigger": {"type": "schedule"}});
        assert!(resolve_tool_trigger(&def).is_err());
        let def = serde_json::json!({"trigger": {"type": "schedule", "cron": "not a cron"}});
        assert!(resolve_tool_trigger(&def).is_err());
        // Human phrase normalizes and passes
        let def = serde_json::json!({"trigger": {"type": "schedule", "schedule": "weekdays at 9am"}});
        let (ty, cfg) = resolve_tool_trigger(&def).unwrap();
        assert_eq!(ty, "schedule");
        assert!(cfg["cron"].as_str().is_some());
    }

    #[test]
    fn no_trigger_stays_manual_and_valid_forms_pass() {
        let (ty, _) = resolve_tool_trigger(&serde_json::json!({})).unwrap();
        assert_eq!(ty, "manual");
        let def = serde_json::json!({"schedule": "0 9 * * MON-FRI"});
        let (ty, _) = resolve_tool_trigger(&def).unwrap();
        assert_eq!(ty, "schedule");
        let def = serde_json::json!({"trigger": {"type": "heartbeat", "interval": "30m"}});
        let (ty, _) = resolve_tool_trigger(&def).unwrap();
        assert_eq!(ty, "heartbeat");
    }
}
