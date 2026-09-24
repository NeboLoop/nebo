//! Parking a call on the owner. Only that step waits: the ask is written
//! and the model hears at once that the call is waiting, so it carries on
//! with everything else. The card, the answers, the resume and the expiry
//! build on this row (WP2.13).

use tools::ResolvedCall;
use types::permissions::AskCase;

use super::CheckCx;

/// How long an ask waits for the owner before it expires as a No.
pub const EXPIRES_AFTER_SECS: i64 = 72 * 3600;

/// Write the ask for `call` and return its id.
pub fn park(cx: &CheckCx<'_>, call: &ResolvedCall<'_>, case: &AskCase) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();
    let row = db::PermissionAskRow {
        id: id.clone(),
        agent_id: cx.grant.agent_id.clone(),
        session_key: cx.ctx.session_key.clone(),
        chat_id: None,
        door: serde_json::to_string(&cx.ctx.door).unwrap_or_default(),
        ask_case: serde_json::to_string(case).unwrap_or_default(),
        sentence: call.tool.activity(call.input),
        target: serde_json::to_string(&call.target).unwrap_or_default(),
        call: serde_json::json!({ "name": call.name(), "input": call.input }).to_string(),
        seat: serde_json::to_string(cx.grant).unwrap_or_default(),
        status: "open".to_string(),
        created_at: now,
        expires_at: now + EXPIRES_AFTER_SECS,
    };
    if let Err(e) = cx.store.insert_permission_ask(&row) {
        tracing::warn!(tool = %call.name(), error = %e, "ask not written");
    }
    id
}

/// What the model hears for a parked call.
pub fn parked_text(sentence: &str, case: &AskCase) -> String {
    let why = match case {
        AskCase::Money { .. } => " It is over this employee's money limit.",
        AskCase::OutsideJob { .. } => " It is outside this employee's job.",
        AskCase::UntrustedInput { .. } => " It acts on words that came from outside.",
        _ => "",
    };
    format!(
        "Waiting for the owner to allow: {sentence}.{why} Carry on with anything else; the answer \
         arrives as a notification. Don't retry this action."
    )
}
