//! The activity record: every decision the check makes, with why, and the
//! judgement's verdict when a judge was asked. A call neither judge could
//! answer is flagged unreviewed ("the permission check couldn't run").
//! A shadow verdict still being judged when the call ran lands on its
//! record when it comes ([`judged_later`]).

use std::sync::Arc;

use types::permissions::{Decision, JudgementMode, Target, Verdict};

use super::{CheckCx, Judged};

/// One decision, as it is recorded.
pub struct Entry<'a> {
    /// The owner-facing line for the call.
    pub activity: String,
    pub decision: &'a Decision,
    pub ask_id: Option<&'a str>,
    pub judged: Option<&'a Judged>,
}

/// Record one decision; returns its record's id.
pub fn record(store: &db::Store, cx: &CheckCx<'_>, t: &Target, e: &Entry<'_>) -> Result<i64, types::NeboError> {
    let (decision, why) = match e.decision {
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
        activity: e.activity.clone(),
        decision: decision.to_string(),
        why: why.unwrap_or_default(),
        ask_id: e.ask_id.map(str::to_string),
        unreviewed: e.judged.is_some_and(Judged::unreviewed),
        judgement: e.judged.map(judgement_json),
        created_at: chrono::Utc::now().timestamp(),
    })
}

/// The shadow verdict for the decision recorded as `id`, written when the
/// judges answer. Never awaited by the call: it has already run on the
/// code's answer. A judgement dropped without a verdict is unjudged.
pub fn judged_later(store: Arc<db::Store>, id: i64, mut verdict: tokio::sync::watch::Receiver<Option<Verdict>>) {
    tokio::spawn(async move {
        let verdict = match verdict.wait_for(Option::is_some).await {
            Ok(v) => v.clone().unwrap_or(Verdict::Unjudged),
            Err(_) => Verdict::Unjudged,
        };
        let judged = Judged { mode: JudgementMode::Shadow, verdict };
        if let Err(e) = store.set_permission_activity_judgement(id, &judgement_json(&judged), judged.unreviewed()) {
            tracing::warn!(id, error = %e, "shadow judgement not recorded");
        }
    });
}

/// `{mode, verdict, by, reason}`: which judge gave which verdict, and
/// whether it decided the call (enforce) or was only recorded (shadow).
fn judgement_json(j: &Judged) -> String {
    let (verdict, by, reason) = match &j.verdict {
        Verdict::Allow { by, reason } => ("allow", Some(by.as_str()), reason.as_str()),
        Verdict::Ask { by, reason, .. } => ("ask", Some(by.as_str()), reason.as_str()),
        Verdict::Unjudged => ("unjudged", None, types::permissions::UNREVIEWED_REASON),
    };
    serde_json::json!({ "mode": j.mode.as_str(), "verdict": verdict, "by": by, "reason": reason }).to_string()
}
