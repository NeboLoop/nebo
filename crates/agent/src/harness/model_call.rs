//! One provider call with its retry, overflow and output-cutoff ladders.
//! WP1.1 moves the body here from `runner.rs`. Items narrow to `pub(crate)`
//! once the turn driver calls them (WP2.3).

use tokio::sync::mpsc;

use super::prompt::SystemPrompt;
use super::turn::TurnContext;

/// One request to the model.
pub struct ModelCall {
    pub system: SystemPrompt,
    pub messages: Vec<ai::Message>,
    pub tools: Vec<ai::ToolDefinition>,
    pub model: String,
    pub max_tokens: i32,
    pub tool_choice: ai::ToolChoice,
    pub trace: ai::RequestTrace,
}

/// What a call came back with.
pub enum CallOutcome {
    Reply(ModelReply),
    Overflow,
    Transient(String),
    Refused(String),
    Failed(ai::ProviderError),
}

/// The model's reply.
pub struct ModelReply {
    pub text: String,
    pub tool_calls: Vec<ai::ToolCall>,
    pub stop: Option<String>,
    pub usage: ai::UsageInfo,
    pub block_order: Vec<String>,
}

/// Make one call, streaming its events on `tx`.
pub async fn call_model(
    _cx: &TurnContext,
    _call: ModelCall,
    _tx: &mpsc::Sender<ai::StreamEvent>,
) -> CallOutcome {
    unimplemented!("WP1.1")
}
