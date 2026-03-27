use db::Store;
use tracing::{info, warn};

/// Register schedule triggers for a workflow from cron_jobs in the DB.
///
/// Triggers are now owned by Roles (via role.json). This function only
/// handles schedule-based cron job registration when called explicitly
/// (e.g., from role install cascade).
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

/// Register all triggers from a role's workflow bindings.
///
/// For schedule triggers: creates cron_job with name `role-{role_id}-{binding}`.
/// The command field stores `role:{role_id}:{binding_name}` so the scheduler
/// can resolve the inline definition at execution time.
/// For event triggers: stored in role_workflows table, consumed by EventDispatcher.
pub fn register_role_triggers(role_id: &str, bindings: &[db::models::RoleWorkflow], store: &Store) {
    for binding in bindings {
        if binding.trigger_type == "schedule" && binding.is_active == 1 {
            let name = format!("role-{}-{}", role_id, binding.binding_name);
            // Command encodes role_id:binding_name for scheduler to resolve inline def
            let command = format!("role:{}:{}", role_id, binding.binding_name);
            match store.upsert_cron_job(
                &name,
                &binding.trigger_config,
                &command,
                "role_workflow",
                None,
                None,
                None,
                true,
            ) {
                Ok(_) => info!(
                    role = role_id,
                    binding = %binding.binding_name,
                    cron = %binding.trigger_config,
                    "registered role schedule trigger"
                ),
                Err(e) => warn!(
                    role = role_id,
                    binding = %binding.binding_name,
                    error = %e,
                    "failed to register role schedule trigger"
                ),
            }
        }
        // Event triggers are stored in role_workflows and consumed by EventDispatcher
    }
}

/// Unregister a single role trigger (cron job named `role-{role_id}-{binding_name}`).
pub fn unregister_single_role_trigger(role_id: &str, binding_name: &str, store: &Store) {
    let name = format!("role-{}-{}", role_id, binding_name);
    match store.delete_cron_job_by_name(&name) {
        Ok(count) => {
            if count > 0 {
                info!(role = role_id, binding = binding_name, "unregistered single role trigger");
            }
        }
        Err(e) => warn!(
            role = role_id,
            binding = binding_name,
            error = %e,
            "failed to unregister single role trigger"
        ),
    }
}

/// Unregister all triggers for a role (cron jobs with role-{role_id} prefix).
pub fn unregister_role_triggers(role_id: &str, store: &Store) {
    let prefix = format!("role-{}-", role_id);
    match store.delete_cron_jobs_by_prefix(&prefix) {
        Ok(count) => {
            if count > 0 {
                info!(role = role_id, deleted = count, "unregistered role triggers");
            }
        }
        Err(e) => warn!(
            role = role_id,
            error = %e,
            "failed to unregister role triggers"
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
