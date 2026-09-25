//! Coworker message rail — the seam between the `message` tool and the server's
//! dispatch pipeline (`server::coworker::CoworkerRailImpl`).
//!
//! Addressing determines mechanism: a NAMED employee is always reached by
//! message — delivered into their own lane, run under their own identity
//! (persona, memory scope, connected accounts, run receipt), visible as a
//! thread on both sides. There is no second way to reach a named agent;
//! anonymous parallel labor is a helper (`delegate`).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// One intra-bot coworker message (the local leg of the A2A envelope).
#[derive(Debug, Clone)]
pub struct CoworkerMessage {
    /// Sending agent id (empty = the main companion).
    pub from_agent_id: String,
    /// The sender's session key: the reply comes back to it as a
    /// notification.
    pub sender_session_key: String,
    /// Target employee, by name or id. Resolved strictly by the rail against
    /// the installed roster — an unknown name is an error, never a minted
    /// identity.
    pub to: String,
    pub text: String,
    /// The requester's resolved memory scope, copied VERBATIM from
    /// `ToolContext.user_id` (the runner's canonical derivation). The rail
    /// extracts the matter (`:ctx:`) from it via `agent::memory` — carrying
    /// the matter on the envelope is what keys the target's thread and scope
    /// per-matter instead of pooling every matter into one default context
    /// (isolation audit 2026-08-22, leak #6). The tool never re-derives
    /// scopes.
    pub requester_scope: String,
    /// Agent-to-agent hop count (from `ToolContext.handoff_depth`). The rail
    /// enforces the chain cap so enlistment chains and A↔B cycles stay
    /// bounded.
    pub handoff_depth: u8,
    /// Engine-stamped provenance of the SENDER's run at send time (copied
    /// verbatim from `ToolContext.run_taint`). The rail seeds the target run
    /// with it, so multi-hop chains carry the union by construction.
    pub provenance: Vec<types::provenance::ProvenanceClass>,
    /// Set when this message is a team post being delivered to a member: the
    /// target's thread is the team thread (`agent:<to>:coworker:team:<id>`),
    /// the briefing names the team, and the reply is posted back into the
    /// team instead of returned to the sender. `act` = false delivers the
    /// post as context only (no run).
    pub team: Option<TeamDelivery>,
}

/// The team leg of a coworker message (see `CoworkerMessage::team`).
#[derive(Debug, Clone)]
pub struct TeamDelivery {
    pub team_id: String,
    /// Whether the member is asked to act (run) or only receives the post.
    pub act: bool,
    /// The session that posted, when one did: the member's reply comes back
    /// to it as a notification as well as into the team.
    pub reply_to: Option<String>,
}

/// Delivery acknowledgment — a message is never silently dropped: either this
/// is returned (the message is persisted in the target's thread and their run
/// is enqueued) or the send errors. It never carries the reply: that comes
/// back to the sender's session as a notification.
#[derive(Debug, Clone)]
pub struct CoworkerDelivery {
    pub to_agent_id: String,
    pub to_name: String,
    /// The target-side thread (session key) the message was delivered into.
    pub thread_key: String,
}

/// One post into a team (`crate::team`). The rail appends it to the team's
/// local thread (`team:<id>`), fans it out to the members through the ONE
/// coworker pipeline, and mirrors it to the team's hub channel when there
/// is one.
#[derive(Debug, Clone)]
pub struct TeamPost {
    /// Local team id (`db::Team::id`).
    pub team_id: String,
    /// Posting agent id (empty = the owner).
    pub from_agent_id: String,
    pub text: String,
    /// Uploaded files riding the post (the app's composer). The post core
    /// saves them locally and notes the paths in the text, the same way a
    /// direct chat does, so every member can open them.
    pub attachments: Vec<comm::wire::Attachment>,
    /// Members explicitly asked to act (local agent ids). Mentions written in
    /// the text (`<@id>` / `@Name`) count too; the rail unions both.
    pub mention: Vec<String>,
    /// Agent-to-agent hop count of the post (`ToolContext.handoff_depth`).
    /// Every delivery this post causes carries it; the rail's depth limit
    /// refuses past it.
    pub handoff_depth: u8,
    /// Engine-stamped provenance of the posting run.
    pub provenance: Vec<types::provenance::ProvenanceClass>,
    /// True when this post is a member's reply to a delivery (set by the
    /// rail's collector), false for a deliberate post (tool or app). A reply
    /// asks nobody on its own — even the lead's; its mentions still ask, and
    /// the lead's @everyone still summons the team.
    pub is_reply: bool,
    /// The session that posted (`None` for the owner in the app, who reads
    /// the team thread): every reply the post causes comes back to it as a
    /// notification. A reply carries its post's.
    pub reply_to: Option<String>,
}

/// Receipt for a team post: the post is in the thread and every listed
/// member has been asked to act (the rest received it as context).
#[derive(Debug, Clone)]
pub struct TeamPostReceipt {
    pub team_id: String,
    pub team_name: String,
    pub message_id: String,
    /// Display names of the members whose runs were started by this post.
    pub asked: Vec<String>,
}

/// Implemented by the server (`CoworkerRailImpl`), consumed by
/// `send_message` (coworkers and teams) and the escalation up a reporting line. `Pin<Box<dyn Future>>` for object safety — same seam
/// shape as `SubAgentOrchestrator`.
pub trait CoworkerRail: Send + Sync {
    fn send(
        &self,
        msg: CoworkerMessage,
    ) -> Pin<Box<dyn Future<Output = Result<CoworkerDelivery, String>> + Send + '_>>;

    /// Post into a team: record + fan-out + optional hub mirror.
    fn post_team(
        &self,
        post: TeamPost,
    ) -> Pin<Box<dyn Future<Output = Result<TeamPostReceipt, String>> + Send + '_>>;
}

/// Late-bound cell: tool registration runs before `AppState` exists, so the
/// server fills this after startup — same pattern as `code_installer` and
/// `notify_fn`.
pub type CoworkerRailCell = Arc<std::sync::RwLock<Option<Arc<dyn CoworkerRail>>>>;

/// Create a new empty rail cell.
pub fn new_rail_cell() -> CoworkerRailCell {
    Arc::new(std::sync::RwLock::new(None))
}

/// Turn a tool's context plus "who and what" into one delivery — the ONE place
/// the envelope is built from a [`crate::origin::ToolContext`].
///
/// Two callers, deliberately sharing this rather than each assembling the
/// envelope: `send_message` to a coworker, and the escalation that takes
/// work a seat cannot finish up its reporting line. Every field a run's
/// identity, isolation, taint and hop budget depend on is derived here, once —
/// a second copy is exactly how a caller forgets `requester_scope` and pools an
/// isolated employee's matters into one context.
pub async fn deliver(
    rail: &Arc<dyn CoworkerRail>,
    ctx: &crate::origin::ToolContext,
    to: &str,
    text: &str,
) -> Result<CoworkerDelivery, String> {
    rail.send(CoworkerMessage {
        from_agent_id: types::keyparser::extract_agent_id(&ctx.session_key),
        sender_session_key: ctx.session_key.clone(),
        to: to.to_string(),
        text: text.to_string(),
        // Verbatim resolved scope — the rail derives the matter from it; a tool
        // never re-derives scopes (the canonical derivation lives in
        // agent::memory::resolve_memory_scope and the runner).
        requester_scope: ctx.user_id.clone(),
        handoff_depth: ctx.handoff_depth,
        provenance: ctx.run_taint.clone(),
        team: None,
    })
    .await
}
