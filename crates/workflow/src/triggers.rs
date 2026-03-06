use db::Store;
use tracing::{info, warn};

use crate::parser::{Trigger, WorkflowDef};

/// Register triggers for a workflow definition.
pub fn register_triggers(def: &WorkflowDef, store: &Store) {
    for trigger in &def.triggers {
        match trigger {
            Trigger::Schedule { cron } => {
                let name = format!("workflow-{}", def.id);
                match store.upsert_cron_job(
                    &name,
                    cron,
                    &def.id,         // command = workflow ID
                    "workflow",       // task_type
                    None,             // message
                    None,             // deliver
                    None,             // instructions
                    true,             // enabled
                ) {
                    Ok(_) => info!(
                        workflow = def.id.as_str(),
                        cron = cron.as_str(),
                        "registered schedule trigger"
                    ),
                    Err(e) => warn!(
                        workflow = def.id.as_str(),
                        error = %e,
                        "failed to register schedule trigger"
                    ),
                }
            }
            Trigger::Event { event } => {
                // Events system not yet ported — stub for future
                info!(
                    workflow = def.id.as_str(),
                    event = event.as_str(),
                    "event trigger registered (stub)"
                );
            }
            Trigger::Manual => {
                // Manual triggers don't need registration
            }
        }
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
