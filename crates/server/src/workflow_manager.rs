//! WorkflowManagerImpl — implements tools::workflows::WorkflowManager trait.
//!
//! Bridges workflow lifecycle operations (DB queries, marketplace install) with
//! workflow execution (spawned via tokio::spawn, using the workflow engine).

use std::sync::Arc;

use tokio::sync::RwLock;
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
}

impl WorkflowManagerImpl {
    pub fn new(
        store: Arc<db::Store>,
        providers: Arc<RwLock<Vec<Arc<dyn Provider>>>>,
        tools: Arc<tools::Registry>,
        hub: Arc<ClientHub>,
        config: config::Config,
    ) -> Self {
        Self {
            store,
            providers,
            tools,
            hub,
            config,
        }
    }

    fn build_api_client(&self) -> Result<comm::api::NeboLoopApi, String> {
        let bot_id = config::read_bot_id()
            .ok_or_else(|| "no bot_id configured".to_string())?;
        let profiles = match self.store.list_active_auth_profiles_by_provider("neboloop") {
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
        let (trigger_count, activity_count) = match workflow::parser::parse_workflow(&wf.definition) {
            Ok(def) => (def.triggers.len(), def.activities.len()),
            Err(_) => (0, 0),
        };
        WorkflowInfo {
            id: wf.id.clone(),
            name: wf.name.clone(),
            version: wf.version.clone(),
            description: String::new(),
            is_enabled: wf.is_enabled != 0,
            trigger_count,
            activity_count,
        }
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

            // The response id is the workflow ID from NeboLoop
            // Look up the workflow in DB (handle_work_code in codes.rs already stored it)
            // If not found, the install may have been handled via the codes path
            match self.store.get_workflow(&resp.id) {
                Ok(Some(wf)) => Ok(self.workflow_to_info(&wf)),
                _ => Err(format!("workflow installed but not found in local DB (id: {})", resp.id)),
            }
        })
    }

    fn uninstall<'a>(&'a self, id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            // Unregister triggers
            if let Ok(Some(wf)) = self.store.get_workflow(id) {
                if let Ok(def) = workflow::parser::parse_workflow(&wf.definition) {
                    workflow::triggers::unregister_triggers(&def.id, &self.store);
                }
            }

            // Delete bindings then workflow
            if let Err(e) = self.store.delete_workflow_bindings(id) {
                warn!(workflow_id = %id, error = %e, "failed to delete workflow bindings");
            }
            self.store.delete_workflow(id)
                .map_err(|e| format!("delete_workflow: {}", e))
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

            let def = workflow::parser::parse_workflow(&wf.definition)
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

            // Clone Arcs for the spawned task
            let store = self.store.clone();
            let providers = self.providers.clone();
            let tools_registry = self.tools.clone();
            let hub = self.hub.clone();
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

                match workflow::engine::execute_workflow(
                    &def,
                    inputs,
                    &trigger,
                    None,
                    &store,
                    &*provider,
                    &resolved_tools,
                ).await {
                    Ok(_engine_run_id) => {
                        // Engine creates its own run record; update ours to completed
                        if let Err(e) = store.update_workflow_run(
                            &run_id_clone,
                            Some("completed"),
                            None,
                            None,
                            None,
                            None,
                        ) {
                            warn!(run_id = %run_id_clone, error = %e, "failed to mark workflow run completed");
                        }
                        hub.broadcast(
                            "workflow_run_completed",
                            serde_json::json!({
                                "workflowId": wf_id,
                                "runId": run_id_clone,
                                "name": wf_name,
                            }),
                        );
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
                        warn!(workflow = %wf_id, run_id = %run_id_clone, error = %err_msg, "workflow failed");
                    }
                }
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
