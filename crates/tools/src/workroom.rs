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

/// Create (or refresh) a workroom. `member_agent_ids` are LOCAL agent ids —
/// resolve names before calling. Idempotent by channel name on the hub side
/// and by channel id in the registry.
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
    let channel_id = comm
        .ensure_channel(name, (!mission.is_empty()).then_some(mission))
        .await
        .map_err(|e| format!("create workroom channel: {e}"))?;
    store
        .create_workroom(&channel_id, name, mission, member_agent_ids)
        .map_err(|e| format!("register workroom: {e}"))?;
    store
        .get_workroom(&channel_id)
        .map_err(|e| format!("load workroom: {e}"))?
        .ok_or_else(|| "workroom vanished after registration".to_string())
}
