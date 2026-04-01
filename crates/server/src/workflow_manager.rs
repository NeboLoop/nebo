//! WorkflowManagerImpl — implements tools::workflows::WorkflowManager trait.
//!
//! Bridges workflow lifecycle operations (DB queries, marketplace install) with
//! workflow execution (spawned via tokio::spawn, using the workflow engine).

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use ai::Provider;
use tools::registry::{DynTool, ToolResult};
use tools::origin::ToolContext;
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
        }
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
            info!(agent_id, count = run_ids.len(), "cancelled running workflows for agent");
        }
    }

    fn build_api_client(&self) -> Result<comm::api::NeboLoopApi, String> {
        let bot_id = config::read_bot_id()
            .ok_or_else(|| "no bot_id configured".to_string())?;
        let profiles = match self.store.list_all_active_auth_profiles_by_provider("neboloop") {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, "failed to list auth profiles for neboloop");
                return Err("failed to query auth profiles".to_string());
            }
        };
        let profile = profiles
            .first()
            .ok_or_else(|| "not connected to NeboLoop".to_string())?;
        let api_server = self.config.neboloop.api_url.clone();
        Ok(comm::api::NeboLoopApi::new(api_server, bot_id, profile.api_key.clone()))
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
    fn load_workflow_def(&self, wf: &db::models::Workflow) -> Result<workflow::WorkflowDef, String> {
        // Try loading from napp_path first (always a directory after migration)
        if let Some(ref napp_path) = wf.napp_path {
            let path = std::path::Path::new(napp_path);
            if path.is_dir() {
                let json_path = path.join("workflow.json");
                if json_path.exists() {
                    let json = std::fs::read_to_string(&json_path)
                        .map_err(|e| e.to_string())?;
                    return workflow::parser::parse_workflow(&json)
                        .map_err(|e| e.to_string());
                }
            }
        }

        // Fall back to definition stored in DB
        workflow::parser::parse_workflow(&wf.definition)
            .map_err(|e| e.to_string())
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
}

impl WorkflowManager for WorkflowManagerImpl {
    fn list(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<WorkflowInfo>> + Send + '_>> {
        Box::pin(async move {
            match self.store.list_workflows(100, 0) {
                Ok(workflows) => workflows.iter().map(|wf| self.workflow_to_info(wf)).collect(),
                Err(e) => {
                    warn!(error = %e, "failed to list workflows");
                    Vec::new()
                }
            }
        })
    }

    fn install<'a>(&'a self, code: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<WorkflowInfo, String>> + Send + 'a>> {
        Box::pin(async move {
            let api = self.build_api_client()?;
            let resp = api
                .install_workflow(code)
                .await
                .map_err(|e| format!("install_workflow: {}", e))?;

            // The response artifact.id is the workflow ID from NeboLoop
            // Look up the workflow in DB (handle_work_code in codes.rs already stored it)
            // If not found, the install may have been handled via the codes path
            match self.store.get_workflow(&resp.artifact.id) {
                Ok(Some(wf)) => Ok(self.workflow_to_info(&wf)),
                _ => Err(format!("workflow installed but not found in local DB (id: {})", resp.artifact.id)),
            }
        })
    }

    fn uninstall<'a>(&'a self, id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            // Resolve workflow — try by ID first, then by name
            let wf = self.store.get_workflow(id)
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
            self.store.delete_workflow(&wf_id)
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

    fn resolve<'a>(&'a self, name_or_id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<WorkflowInfo, String>> + Send + 'a>> {
        Box::pin(async move {
            // Try by ID first
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
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            let wf = self.store.get_workflow(id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("workflow not found: {}", id))?;

            if wf.is_enabled == 0 {
                return Err("workflow is disabled".into());
            }

            let def = self.load_workflow_def(&wf)
                .map_err(|e| format!("parse error: {}", e))?;

            // Create run record
            let run_id = uuid::Uuid::new_v4().to_string();
            let session_key = format!("workflow-{}-{}", id, run_id);
            self.store.create_workflow_run(
                &run_id,
                id,
                trigger_type,
                None,
                Some(&inputs.to_string()),
                Some(&session_key),
            ).map_err(|e| format!("create_workflow_run: {}", e))?;

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
                                if let Some(skill) = loader.get(skill_name).await {
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

                match workflow::engine::execute_workflow(
                    &def,
                    inputs,
                    &trigger,
                    None,
                    &store,
                    &*provider,
                    &resolved_tools,
                    Some(&run_id_clone),
                    Some(&cancel_token),
                    skill_content.as_ref(),
                    event_bus.as_ref(),
                    None,
                    None,
                ).await {
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

    fn run_status<'a>(&'a self, run_id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<WorkflowRunInfo, String>> + Send + 'a>> {
        Box::pin(async move {
            self.store.get_workflow_run(run_id)
                .map_err(|e| e.to_string())?
                .map(|r| Self::run_to_info(&r))
                .ok_or_else(|| format!("run not found: {}", run_id))
        })
    }

    fn list_runs<'a>(&'a self, workflow_id: &'a str, limit: i64) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<WorkflowRunInfo>> + Send + 'a>> {
        Box::pin(async move {
            match self.store.list_workflow_runs(workflow_id, limit, 0) {
                Ok(runs) => runs.iter().map(Self::run_to_info).collect(),
                Err(e) => {
                    warn!(workflow_id = %workflow_id, error = %e, "failed to list workflow runs");
                    Vec::new()
                }
            }
        })
    }

    fn toggle<'a>(&'a self, id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + Send + 'a>> {
        Box::pin(async move {
            self.store.toggle_workflow(id)
                .map_err(|e| format!("toggle: {}", e))?;
            let wf = self.store.get_workflow(id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "workflow not found after toggle".to_string())?;
            Ok(wf.is_enabled != 0)
        })
    }

    fn create<'a>(
        &'a self,
        name: &'a str,
        definition: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<WorkflowInfo, String>> + Send + 'a>> {
        Box::pin(async move {
            // Validate the workflow definition
            let _def = workflow::parser::parse_workflow(definition)
                .map_err(|e| format!("invalid workflow definition: {}", e))?;

            let id = uuid::Uuid::new_v4().to_string();

            let wf = self.store
                .create_workflow(&id, None, name, "1.0", definition, None, None)
                .map_err(|e| format!("create_workflow: {}", e))?;

            // Write workflow.json to user/workflows/{name}/ for filesystem-based loading
            if let Ok(user_dir) = config::user_dir() {
                let wf_dir = user_dir.join("workflows").join(name);
                if std::fs::create_dir_all(&wf_dir).is_ok() {
                    let json_path = wf_dir.join("workflow.json");
                    if std::fs::write(&json_path, definition).is_ok() {
                        let _ = self.store.set_workflow_napp_path(&id, &wf_dir.to_string_lossy());
                    }
                }
            }

            self.hub.broadcast(
                "workflow_installed",
                serde_json::json!({ "workflowId": wf.id, "name": wf.name }),
            );

            info!(workflow_id = %wf.id, name = %wf.name, "workflow created via agent");

            Ok(self.workflow_to_info(&wf))
        })
    }

    fn run_inline<'a>(
        &'a self,
        definition_json: String,
        inputs: serde_json::Value,
        trigger_type: &'a str,
        agent_id: &'a str,
        emit_source: Option<String>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            let def = workflow::parser::parse_workflow(&definition_json)
                .map_err(|e| format!("parse inline workflow: {}", e))?;

            // Merge agent-level input_values into workflow inputs
            let inputs = {
                let mut merged = inputs;
                if let Ok(Some(agent_rec)) = self.store.get_agent(agent_id) {
                    if let Ok(agent_inputs) = serde_json::from_str::<serde_json::Value>(&agent_rec.input_values) {
                        if let (Some(m), Some(r)) = (merged.as_object_mut(), agent_inputs.as_object()) {
                            for (k, v) in r {
                                m.entry(k.clone()).or_insert_with(|| v.clone());
                            }
                        }
                    }
                }
                merged
            };

            // Create run record using agent_id for tracking
            let run_id = uuid::Uuid::new_v4().to_string();
            let session_key = format!("agent-{}-{}", agent_id, run_id);
            self.store.create_workflow_run(
                &run_id,
                &format!("agent:{}", agent_id),
                trigger_type,
                None,
                Some(&inputs.to_string()),
                Some(&session_key),
            ).map_err(|e| format!("create_workflow_run: {}", e))?;

            // Create cancellation token
            let cancel_token = CancellationToken::new();
            {
                let mut runs = self.active_runs.lock().unwrap();
                runs.insert(run_id.clone(), cancel_token.clone());
            }
            {
                let mut agent_map = self.agent_runs.lock().unwrap();
                agent_map.entry(agent_id.to_string()).or_default().push(run_id.clone());
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
            let run_id_clone = run_id.clone();
            let agent_id_owned = agent_id.to_string();
            let trigger = trigger_type.to_string();
            let binding_name = def.name.clone();

            tokio::spawn(async move {
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
                            &run_id_clone, Some("failed"), None, None,
                            Some("no AI provider available"), None,
                        );
                        // Post failure to agent chat
                        post_automation_message(
                            &store, &hub, &chat_session,
                            &format!("**Automation failed** — {} ({}): no AI provider available", binding_name, trigger),
                        );
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
                        let _ = store.update_agent_workflow_last_fired(&agent_id_owned, &binding_name, &now);
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
                    &store, &hub, &chat_session,
                    &format!("**Automation started** — {} ({})", binding_name, trigger),
                );

                // Record last_fired timestamp
                let now = chrono::Utc::now().to_rfc3339();
                let _ = store.update_agent_workflow_last_fired(&agent_id_owned, &binding_name, &now);

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
                                if let Some(skill) = loader.get(skill_name).await {
                                    if !skill.template.is_empty() {
                                        map.insert(skill_name.clone(), skill.template.clone());
                                    }
                                }
                            }
                        }
                    }
                    if map.is_empty() { None } else { Some(map) }
                } else {
                    None
                };

                // Create progress channel for live activity updates
                let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<workflow::WorkflowProgress>();
                {
                    let hub = hub.clone();
                    let agent_id_for_progress = agent_id_owned.clone();
                    let run_id = run_id_clone.clone();
                    let binding = binding_name.clone();
                    tokio::spawn(async move {
                        while let Some(progress) = progress_rx.recv().await {
                            hub.broadcast(
                                "workflow_activity_update",
                                serde_json::json!({
                                    "agentId": agent_id_for_progress,
                                    "runId": run_id,
                                    "bindingName": binding,
                                    "activityId": progress.activity_id,
                                    "step": progress.activity_index + 1,
                                    "totalSteps": progress.total_activities,
                                }),
                            );
                        }
                    });
                }

                match workflow::engine::execute_workflow(
                    &def, inputs, &trigger, None, &store, &*provider,
                    &resolved_tools, Some(&run_id_clone), Some(&cancel_token),
                    skill_content.as_ref(), event_bus.as_ref(),
                    emit_source, Some(progress_tx),
                ).await {
                    Ok((_engine_run_id, output)) => {
                        // Engine already called complete_workflow_run with output

                        // Post completion message with output to agent chat
                        let summary = if output.is_empty() {
                            format!("**Automation completed** — {} ({})", binding_name, trigger)
                        } else {
                            // Truncate output to ~4000 chars to keep chat messages reasonable
                            let truncated = if output.len() > 4000 {
                                let mut end = 4000;
                                while !output.is_char_boundary(end) { end -= 1; }
                                &output[..end]
                            } else { &output };
                            format!("**Automation completed** — {} ({})\n\n{}", binding_name, trigger, truncated)
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
                        info!(role = %agent_id_owned, run_id = %run_id_clone, "inline workflow completed");
                    }
                    Err(e) => {
                        let err_msg = e.to_string();
                        let _ = store.update_workflow_run(
                            &run_id_clone, Some("failed"), None, None, Some(&err_msg), None,
                        );

                        // Post failure message to agent chat
                        post_automation_message(
                            &store, &hub, &chat_session,
                            &format!("**Automation failed** — {} ({}): {}", binding_name, trigger, err_msg),
                        );

                        hub.broadcast(
                            "workflow_run_failed",
                            serde_json::json!({
                                "agentId": agent_id_owned,
                                "runId": run_id_clone,
                                "bindingName": binding_name,
                                "error": err_msg,
                            }),
                        );
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

    fn cancel<'a>(&'a self, run_id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.cancel_run(run_id).await
        })
    }

    fn cancel_runs_for_agent<'a>(&'a self, agent_id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.cancel_runs_for_agent_impl(agent_id).await
        })
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
        Box::pin(async move {
            self.registry.execute(ctx, &self.tool_name, input).await
        })
    }
}

/// Post an automation lifecycle message to an agent's chat session.
fn post_automation_message(
    store: &db::Store,
    hub: &ClientHub,
    session_key: &str,
    content: &str,
) {
    let msg_id = uuid::Uuid::new_v4().to_string();
    match store.create_chat_message_for_runner(&msg_id, session_key, "assistant", content, None, None, None, None) {
        Ok(_msg) => {
            // Broadcast as chat_complete so the chat UI picks it up in real time
            hub.broadcast("chat_complete", serde_json::json!({
                "chatId": session_key,
                "content": content,
                "role": "assistant",
            }));
        }
        Err(e) => {
            warn!(session = %session_key, error = %e, "failed to post automation message to chat");
        }
    }
}
