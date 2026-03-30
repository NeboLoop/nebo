use db::Store;
use tracing::{info, warn};

/// Register schedule triggers for a workflow from cron_jobs in the DB.
///
/// Triggers are now owned by Agents (via agent.json). This function only
/// handles schedule-based cron job registration when called explicitly
/// (e.g., from agent install cascade).
pub fn register_schedule_trigger(workflow_id: &str, cron: &str, store: &Store) {
    let name = format!("workflow-{}", workflow_id);
    match store.upsert_cron_job(
        &name,
        cron,
        workflow_id,      // command = workflow ID
        "workflow",       // task_type
        None,             // message
        None,             // deliver
        None,             // instructions
        true,             // enabled
    ) {
        Ok(_) => info!(
            workflow = workflow_id,
            cron,
            "registered schedule trigger"
        ),
        Err(e) => warn!(
            workflow = workflow_id,
            error = %e,
            "failed to register schedule trigger"
        ),
    }
}

/// Register all triggers from an agent's workflow bindings.
///
/// For schedule triggers: creates cron_job with name `agent-{agent_id}-{binding}`.
/// The command field stores `agent:{agent_id}:{binding_name}` so the scheduler
/// can resolve the inline definition at execution time.
/// For event triggers: stored in agent_workflows table, consumed by EventDispatcher.
pub fn register_agent_triggers(agent_id: &str, bindings: &[db::models::AgentWorkflow], store: &Store) {
    for binding in bindings {
        if binding.trigger_type == "schedule" && binding.is_active == 1 {
            let name = format!("agent-{}-{}", agent_id, binding.binding_name);
            // Command encodes agent_id:binding_name for scheduler to resolve inline def
            let command = format!("agent:{}:{}", agent_id, binding.binding_name);
            match store.upsert_cron_job(
                &name,
                &binding.trigger_config,
                &command,
                "agent_workflow",
                None,
                None,
                None,
                true,
            ) {
                Ok(_) => info!(
                    agent = agent_id,
                    binding = %binding.binding_name,
                    cron = %binding.trigger_config,
                    "registered agent schedule trigger"
                ),
                Err(e) => warn!(
                    agent = agent_id,
                    binding = %binding.binding_name,
                    error = %e,
                    "failed to register agent schedule trigger"
                ),
            }
        }
        // Event triggers are stored in agent_workflows and consumed by EventDispatcher
    }
}

/// Unregister a single agent trigger (cron job named `agent-{agent_id}-{binding_name}`).
pub fn unregister_single_agent_trigger(agent_id: &str, binding_name: &str, store: &Store) {
    let name = format!("agent-{}-{}", agent_id, binding_name);
    match store.delete_cron_job_by_name(&name) {
        Ok(count) => {
            if count > 0 {
                info!(agent = agent_id, binding = binding_name, "unregistered single agent trigger");
            }
        }
        Err(e) => warn!(
            agent = agent_id,
            binding = binding_name,
            error = %e,
            "failed to unregister single agent trigger"
        ),
    }
}

/// Unregister all triggers for an agent (cron jobs with agent-{agent_id} prefix).
pub fn unregister_agent_triggers(agent_id: &str, store: &Store) {
    let prefix = format!("agent-{}-", agent_id);
    match store.delete_cron_jobs_by_prefix(&prefix) {
        Ok(count) => {
            if count > 0 {
                info!(agent = agent_id, deleted = count, "unregistered agent triggers");
            }
        }
        Err(e) => warn!(
            agent = agent_id,
            error = %e,
            "failed to unregister agent triggers"
        ),
    }
}

/// Unregister all triggers for a workflow.
pub fn unregister_triggers(workflow_id: &str, store: &Store) {
    let name = format!("workflow-{}", workflow_id);
    match store.delete_cron_job_by_name(&name) {
        Ok(count) => {
            if count > 0 {
                info!(workflow = workflow_id, "unregistered schedule trigger");
            }
        }
        Err(e) => warn!(
            workflow = workflow_id,
            error = %e,
            "failed to unregister triggers"
        ),
    }
}
