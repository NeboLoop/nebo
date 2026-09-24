//! Memory in the turn: the employee-memory section of the per-session
//! prompt (WP2.2) and the relevant-memories event from the recall prefetch
//! (WP2.7).

use crate::db_context::{self, InheritScope};

/// What the employee knows at the start of a turn: the identity slice of
/// its memory (profile, owner, preferences, learned personality, the
/// always-on memories) plus the owner's configured inputs.
pub struct EmployeeMemory {
    /// The `employee_memory` section, placed after the cache boundary.
    pub section: String,
    /// The owner's IANA timezone, when set: the environment's date is
    /// computed in it.
    pub timezone: Option<String>,
}

/// Load the employee-memory section once per turn. `user_id` and
/// `inherit_scopes` come from the seat's memory scope, so an isolated seat
/// never sees a sibling's memories.
pub fn load_employee_memory(
    store: &db::Store,
    user_id: &str,
    agent_id: &str,
    inherit_scopes: &[InheritScope],
    agent_name: &str,
) -> EmployeeMemory {
    let ctx = db_context::load_db_context(store, user_id, agent_id, inherit_scopes);
    let timezone = ctx.user.as_ref().and_then(|u| u.timezone.clone()).filter(|tz| !tz.is_empty());
    let mut section = db_context::format_for_system_prompt(&ctx, agent_name);
    let inputs = if agent_id.is_empty() {
        None
    } else {
        store
            .get_agent(agent_id)
            .ok()
            .flatten()
            .and_then(|a| db_context::format_configured_inputs(&a.input_values))
    };
    if let Some(inputs) = inputs {
        section.push_str("\n\n---\n\n");
        section.push_str(&inputs);
    }
    EmployeeMemory { section, timezone }
}
