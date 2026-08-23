//! Workroom creation — the ONE core both doors share.
//!
//! A workroom is a loop channel plus a local registry row. Rooms are opened
//! by whichever employee owns the task (the loop tool's `workroom` resource);
//! the REST handler is the platform API over the same function. Neither door
//! carries its own logic — find-or-create the channel on the hub, register
//! the room, return the row.

use std::sync::Arc;

use comm::CommPlugin;
use db::{Store, Workroom};

/// WS event announcing a new room; the sidebar refreshes its list on it.
pub const WORKROOM_CREATED_EVENT: &str = "workroom_created";

/// Create a workroom. `member_agent_ids` are LOCAL agent ids — resolve names
/// before calling; the creator comes first (it is the organizer).
///
/// Two policies live HERE, the one enforcement point for both doors:
/// - A room is a collaboration: it takes at least two employees — the
///   organizer plus the coworkers it names. No solo rooms.
/// - Rooms are never reused. The hub finds channels by name, so a repeated
///   name would return the existing channel — and re-registering it would
///   silently replace the mission and throw the previous members out. A
///   taken name is an error, never an overwrite.
pub async fn create(
    comm: &Arc<dyn CommPlugin>,
    store: &Store,
    name: &str,
    mission: &str,
    member_agent_ids: &[String],
) -> Result<Workroom, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("workroom name required".to_string());
    }

    let mut members: Vec<String> = Vec::new();
    for id in member_agent_ids {
        if !id.is_empty() && !members.iter().any(|m| m == id) {
            members.push(id.clone());
        }
    }
    if members.len() < 2 {
        return Err(
            "A workroom needs at least two employees — the organizer plus the coworkers \
             it names. Name the coworkers (agents: [\"Coworker Name\", …]) and create again."
                .to_string(),
        );
    }

    let channel_id = comm
        .ensure_channel(name, (!mission.is_empty()).then_some(mission))
        .await
        .map_err(|e| format!("create workroom channel: {e}"))?;
    if let Some(existing) = store
        .get_workroom(&channel_id)
        .map_err(|e| format!("check workroom: {e}"))?
    {
        return Err(format!(
            "A workroom named \"{}\" already exists (channel_id: {}). Rooms are never \
             reused — post into it with loop(resource: \"channel\", action: \"send\", \
             channel_id: \"{}\", text: \"...\"), or create a room with a new, distinct name.",
            existing.name, existing.channel_id, existing.channel_id
        ));
    }
    store
        .create_workroom(&channel_id, name, mission, &members)
        .map_err(|e| format!("register workroom: {e}"))?;
    store
        .get_workroom(&channel_id)
        .map_err(|e| format!("load workroom: {e}"))?
        .ok_or_else(|| "workroom vanished after registration".to_string())
}
