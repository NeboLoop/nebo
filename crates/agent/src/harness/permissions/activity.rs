//! The activity record: every decision the check makes, with why.

use types::permissions::{Decision, Target};

use super::CheckCx;

/// Record one decision. `activity` is the owner-facing line for the call.
pub fn record(
    store: &db::Store,
    cx: &CheckCx<'_>,
    t: &Target,
    activity: String,
    d: &Decision,
    ask_id: Option<&str>,
) -> Result<(), types::NeboError> {
    let (decision, why) = match d {
        Decision::Allow { why } => ("allow", serde_json::to_string(why)),
        Decision::Deny { why, .. } => ("deny", serde_json::to_string(why)),
        Decision::Ask { case } => ("ask", serde_json::to_string(case)),
    };
    store.record_permission_activity(&db::PermissionActivityRow {
        agent_id: cx.grant.agent_id.clone(),
        session_key: cx.ctx.session_key.clone(),
        door: cx.ctx.door.label().to_string(),
        tool: t.tool.clone(),
        rule_key: t.key.clone(),
        activity,
        decision: decision.to_string(),
        why: why.unwrap_or_default(),
        ask_id: ask_id.map(str::to_string),
        created_at: chrono::Utc::now().timestamp(),
    })
}
