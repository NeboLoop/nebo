//! Plan mode: read and plan only. A call that changes something doesn't
//! run; the plan approval card ends the mode.

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use ai::StreamEvent;

/// What a change hears in Plan mode.
pub const REFUSAL: &str =
    "Plan mode: this changes something, so it didn't run. Include this step in the plan instead.";

/// How the owner answered the plan card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanAnswer {
    Approved,
    Rejected,
    /// The run was cancelled while the card was up.
    Cancelled,
}

/// Show the plan card and wait for the owner's answer. `plan` is the
/// model's own words for what it will do; `tools` the calls it proposes.
pub async fn approve_plan(
    ask_channels: &tools::AskChannels,
    tx: &mpsc::Sender<StreamEvent>,
    cancel: &CancellationToken,
    session_id: &str,
    plan: &str,
    tools: Vec<String>,
) -> PlanAnswer {
    let request_id = uuid::Uuid::new_v4().to_string();
    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
    ask_channels.lock().await.insert(request_id.clone(), resp_tx);
    let _ = tx.send(StreamEvent::plan_approval_request(&request_id, plan, tools)).await;
    tracing::info!(session_id, request_id = %request_id, "plan mode: waiting for the owner");
    tokio::select! {
        _ = cancel.cancelled() => {
            ask_channels.lock().await.remove(&request_id);
            PlanAnswer::Cancelled
        }
        answer = resp_rx => match answer {
            Ok(v) if matches!(v.to_lowercase().as_str(), "approve" | "approved" | "yes" | "true") => PlanAnswer::Approved,
            _ => PlanAnswer::Rejected,
        }
    }
}
