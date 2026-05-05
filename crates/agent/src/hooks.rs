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
}

/// Payload for `tool.post_execute` action hook.
#[derive(Debug, Serialize, Deserialize)]
pub struct ToolPostExecutePayload {
    pub tool_name: String,
    pub result: String,
    pub is_error: bool,
    pub session_id: String,
}
