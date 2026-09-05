use serde::{Deserialize, Serialize};

/// Payload for `steering.generate` filter hook.
#[derive(Debug, Serialize, Deserialize)]
pub struct SteeringGeneratePayload {
    pub session_id: String,
    pub iteration: usize,
}

/// Response from `steering.generate` filter hook.
#[derive(Debug, Serialize, Deserialize)]
pub struct SteeringGenerateResponse {
    #[serde(default)]
    pub directives: Vec<SteeringHookDirective>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SteeringHookDirective {
    pub content: String,
    #[serde(default = "default_label")]
    pub label: String,
    #[serde(default = "default_priority")]
    pub priority: u8,
}

fn default_label() -> String {
    "Hook".to_string()
}

fn default_priority() -> u8 {
    5
}

/// Payload for `message.pre_send` filter hook.
#[derive(Debug, Serialize, Deserialize)]
pub struct PreSendPayload {
    pub system_prompt: String,
    pub message_count: usize,
}

/// Response from `message.pre_send` filter hook.
#[derive(Debug, Serialize, Deserialize)]
pub struct PreSendResponse {
    #[serde(default)]
    pub system_prompt: Option<String>,
}

/// Payload for `message.post_receive` filter hook.
#[derive(Debug, Serialize, Deserialize)]
pub struct PostReceivePayload {
    pub response_text: String,
    pub tool_calls_count: usize,
}

/// Response from `message.post_receive` filter hook.
#[derive(Debug, Serialize, Deserialize)]
pub struct PostReceiveResponse {
    #[serde(default)]
    pub response_text: Option<String>,
}

/// Payload for `session.message_append` action hook.
#[derive(Debug, Serialize, Deserialize)]
pub struct MessageAppendPayload {
    pub session_id: String,
    pub role: String,
    pub content: String,
}

/// Payload for `agent.turn` action hook.
#[derive(Debug, Serialize, Deserialize)]
pub struct TurnPayload {
    pub session_id: String,
    pub turn: usize,
    pub tool_calls: Vec<String>,
    pub total_tool_calls: Vec<String>,
    pub has_active_task: bool,
}

/// Payload for `agent.should_continue` filter hook.
#[derive(Debug, Serialize, Deserialize)]
pub struct ShouldContinuePayload {
    pub session_id: String,
    pub turn: usize,
    pub total_tool_calls: Vec<String>,
    pub has_active_task: bool,
}

/// Response from `agent.should_continue` filter hook.
#[derive(Debug, Serialize, Deserialize)]
pub struct ShouldContinueResponse {
    #[serde(default = "default_true")]
    pub should_continue: bool,
    #[serde(default)]
    pub reason: Option<String>,
}

fn default_true() -> bool {
    true
}

/// Payload for `tool.pre_execute` filter hook.
#[derive(Debug, Serialize, Deserialize)]
pub struct ToolPreExecutePayload {
    pub tool_name: String,
    pub input: serde_json::Value,
    pub session_id: String,
    /// The reference's hook contract fields; serde defaults keep older
    /// plugin callers unchanged.
    #[serde(default)]
    pub tool_use_id: String,
    #[serde(default)]
    pub cwd: String,
    /// Present only inside a sub-agent (how a hook tells the two apart).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

/// Response from `tool.pre_execute` filter hook.
#[derive(Debug, Serialize, Deserialize)]
pub struct ToolPreExecuteResponse {
    /// If true, skip tool execution and return `blocked_message` as an error.
    #[serde(default)]
    pub blocked: bool,
    #[serde(default)]
    pub blocked_message: Option<String>,
    /// Optionally modified input to pass to the tool.
    #[serde(default)]
    pub input: Option<serde_json::Value>,
    /// A pre hook that failed (non-zero exit other than the blocking 2, or
    /// its deadline): the call still runs, and this note is attached to its
    /// result so the model knows the hook delivered no verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Payload for the `tool.post_execute` hook. Plugins subscribe as an action
/// (fire-and-forget); shell hooks subscribe as a filter and hand back a
/// `ToolPostExecuteResponse`, which is how a formatter's or test runner's
/// verdict reaches the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPostExecutePayload {
    pub tool_name: String,
    pub result: String,
    pub is_error: bool,
    pub session_id: String,
    #[serde(default)]
    pub tool_use_id: String,
    #[serde(default)]
    pub tool_input: serde_json::Value,
    #[serde(default)]
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

/// Response from the `tool.post_execute` filter: the result as the model
/// will see it. A hook that exits 2 has appended its stderr and set
/// `is_error`; a hook that exits 0 has appended its stdout as a note.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPostExecuteResponse {
    pub result: String,
    pub is_error: bool,
}
