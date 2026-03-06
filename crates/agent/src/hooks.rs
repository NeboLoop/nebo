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
    pub messages: Vec<SteeringHookMessage>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SteeringHookMessage {
    pub content: String,
    #[serde(default = "default_end")]
    pub position: String,
}

fn default_end() -> String {
    "end".to_string()
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
