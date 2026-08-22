//! Owner notification emission — the ONE persist+broadcast sequence.
//!
//! Eleven hand-built copies existed (audit 2026-08-22) and had drifted on
//! three axes: payload field names (`actionUrl` vs `link`, `body` vs
//! `message`), payload completeness (`agentId`/`readAt`/`createdAt` present
//! at some sites only), and idempotency (deterministic ids written with the
//! non-deduped insert hard-error on retry). The comments at two of the copies
//! record the same empty-row bug being fixed one copy at a time.
//!
//! The TWO event names are deliberate and stay: they are the client's
//! loudness contract, not drift —
//! - `notification` (loud): Inbox row + toast + native OS banner
//!   (`listeners.ts` fires the Tauri notification client-side).
//! - `notification_created` (quiet): Inbox row only (the bell).

use serde_json::json;

/// One owner notification. `id` should be deterministic when the event is
/// retryable (e.g. `wf-approval:{run_id}`) — emission is idempotent.
pub struct OwnerNotification<'a> {
    pub id: &'a str,
    /// Inbox row type: "info" | "warning" | "error" | "approval" | ...
    pub kind: &'a str,
    pub title: &'a str,
    pub body: Option<&'a str>,
    pub action_url: Option<&'a str>,
    pub agent_id: Option<&'a str>,
    /// `true` = bell + toast + native OS banner; `false` = bell only.
    pub loud: bool,
}

/// Persist the Inbox row (idempotent) and broadcast the canonical payload.
/// Persistence is best-effort — the live broadcast is what surfaces the
/// notification, so a failed row write is logged, never fatal.
pub fn emit(
    store: &db::Store,
    broadcast: Option<&dyn Fn(&str, serde_json::Value)>,
    n: &OwnerNotification,
) {
    // notifications FK to users(id) — resolve the real local user ("" violates it).
    let user_id = store.ensure_local_user_id().unwrap_or_default();
    if let Err(e) = store.create_notification_if_not_exists(
        n.id,
        &user_id,
        n.kind,
        n.title,
        n.body,
        n.action_url,
        None,
        n.agent_id,
    ) {
        tracing::warn!(id = %n.id, error = %e, "owner notification row not persisted; broadcasting anyway");
    }

    if let Some(broadcast) = broadcast {
        broadcast(
            if n.loud { "notification" } else { "notification_created" },
            json!({
                "id": n.id,
                "type": n.kind,
                "title": n.title,
                "body": n.body,
                "actionUrl": n.action_url,
                "agentId": n.agent_id,
                "readAt": null,
                "createdAt": chrono::Utc::now().timestamp(),
            }),
        );
    }
}
