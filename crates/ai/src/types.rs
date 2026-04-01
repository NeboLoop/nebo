use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Rate limit metadata extracted from provider response headers.
#[derive(Debug, Clone, Default)]
pub struct RateLimitMeta {
    pub remaining_requests: Option<u64>,
    pub remaining_tokens: Option<u64>,
    pub reset_after_secs: Option<f64>,
    pub retry_after_secs: Option<u64>,
    // Janus session/weekly rate limit windows
    pub session_limit_tokens: Option<u64>,
    pub session_remaining_tokens: Option<u64>,
    pub session_reset_at: Option<String>,
    pub weekly_limit_tokens: Option<u64>,
    pub weekly_remaining_tokens: Option<u64>,
    pub weekly_reset_at: Option<String>,
}

/// Streaming event types from AI providers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamEventType {
    Text,
    ToolCall,
    ToolResult,
    Error,
    Done,
    Thinking,
    Usage,
    RateLimit,
    ApprovalRequest,
    AskRequest,
    SubagentStart,
    SubagentProgress,
    SubagentComplete,
}

/// Token usage statistics from a streaming response.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageInfo {
    pub input_tokens: i32,
    pub output_tokens: i32,
    #[serde(default)]
    pub cache_creation_input_tokens: i32,
    #[serde(default)]
    pub cache_read_input_tokens: i32,
}

/// A tool invocation from the AI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

/// A streaming event from a provider.
#[derive(Debug, Clone)]
pub struct StreamEvent {
    pub event_type: StreamEventType,
    pub text: String,
    pub tool_call: Option<ToolCall>,
    pub error: Option<String>,
    pub usage: Option<UsageInfo>,
    pub rate_limit: Option<RateLimitMeta>,
    pub widgets: Option<serde_json::Value>,
    /// Provider metadata from Janus for tool stickiness routing.
    pub provider_metadata: Option<HashMap<String, String>>,
}

impl StreamEvent {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            event_type: StreamEventType::Text,
            text: text.into(),
            tool_call: None,
            error: None,
            usage: None,
            rate_limit: None,
            widgets: None,
            provider_metadata: None,
        }
    }

    pub fn thinking(text: impl Into<String>) -> Self {
        Self {
            event_type: StreamEventType::Thinking,
            text: text.into(),
            tool_call: None,
            error: None,
            usage: None,
            rate_limit: None,
            widgets: None,
            provider_metadata: None,
        }
    }

    pub fn tool_call(tc: ToolCall) -> Self {
        Self {
            event_type: StreamEventType::ToolCall,
            text: String::new(),
            tool_call: Some(tc),
            error: None,
            usage: None,
            rate_limit: None,
            widgets: None,
            provider_metadata: None,
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            event_type: StreamEventType::Error,
            text: String::new(),
            tool_call: None,
            error: Some(msg.into()),
            usage: None,
            rate_limit: None,
            widgets: None,
            provider_metadata: None,
        }
    }

    pub fn done() -> Self {
        Self {
            event_type: StreamEventType::Done,
            text: String::new(),
            tool_call: None,
            error: None,
            usage: None,
            rate_limit: None,
            widgets: None,
            provider_metadata: None,
        }
    }

    pub fn usage(info: UsageInfo) -> Self {
        Self {
            event_type: StreamEventType::Usage,
            text: String::new(),
            tool_call: None,
            error: None,
            usage: Some(info),
            rate_limit: None,
            widgets: None,
            provider_metadata: None,
        }
    }

    pub fn rate_limit_info(meta: RateLimitMeta) -> Self {
        Self {
            event_type: StreamEventType::RateLimit,
            text: String::new(),
            tool_call: None,
            error: None,
            usage: None,
            rate_limit: Some(meta),
            widgets: None,
            provider_metadata: None,
        }
    }

    pub fn approval_request(tc: ToolCall) -> Self {
        Self {
            event_type: StreamEventType::ApprovalRequest,
            text: String::new(),
            tool_call: Some(tc),
            error: None,
            usage: None,
            rate_limit: None,
            widgets: None,
            provider_metadata: None,
        }
    }

    pub fn ask_request(
        question_id: impl Into<String>,
        prompt: impl Into<String>,
        widgets: Option<serde_json::Value>,
    ) -> Self {
        Self {
            event_type: StreamEventType::AskRequest,
            text: prompt.into(),
            tool_call: None,
            error: Some(question_id.into()), // reuse error field for question_id
            usage: None,
            rate_limit: None,
            widgets,
            provider_metadata: None,
        }
    }
}

/// Describes a tool available to the AI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// Image content for vision messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageContent {
    pub media_type: String,
    pub data: String,
}

/// A message in a conversation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_results: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<ImageContent>>,
}

/// A request to an AI provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
    #[serde(default)]
    pub max_tokens: i32,
    #[serde(default)]
    pub temperature: f64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub system: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub static_system: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(default)]
    pub enable_thinking: bool,
    /// Provider metadata echoed back for Janus tool stickiness routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
    /// Byte offsets into the system prompt where cache boundaries should be
    /// placed.  Providers that support prompt caching (e.g. Anthropic) will
    /// split the system prompt at these offsets and mark the prefix blocks
    /// with `cache_control: { type: "ephemeral" }`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cache_breakpoints: Vec<usize>,
}

/// Sender half of a streaming event channel.
pub type EventSender = mpsc::Sender<StreamEvent>;
/// Receiver half of a streaming event channel.
pub type EventReceiver = mpsc::Receiver<StreamEvent>;

/// AI provider trait. All providers implement this.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Provider identifier (e.g., "anthropic", "openai", "ollama").
    fn id(&self) -> &str;

    /// Auth profile ID for usage tracking. Empty for providers without profiles.
    fn profile_id(&self) -> &str {
        ""
    }

    /// Whether this provider executes tools itself (e.g., CLI providers via MCP).
    fn handles_tools(&self) -> bool {
        false
    }

    /// Whether this provider supports images in tool result content blocks.
    /// When true, the runner will pass screenshot images directly to the model
    /// instead of converting them to text via the sidecar vision model.
    fn supports_tool_result_images(&self) -> bool {
        false
    }

    /// Send a request and return a channel of streaming events.
    async fn stream(
        &self,
        req: &ChatRequest,
    ) -> Result<EventReceiver, ProviderError>;
}

/// Optional trait for providers that support HTTP/2 connection reset recovery.
/// Implemented by providers that use persistent HTTP/2 connections which can
/// enter a poisoned state (GOAWAY frames, connection exhaustion).
pub trait ConnectionResetter {
    /// Reset all idle HTTP connections. Call when GOAWAY or connection errors
    /// are detected to force new connections on the next request.
    fn reset_connections(&self);
}

/// Optional trait for providers that track auth profile usage for billing.
pub trait ProfileTracker {
    /// Record successful usage (tokens consumed) against the auth profile.
    fn record_usage(&self, input_tokens: i32, output_tokens: i32);
    /// Record an error with a cooldown hint string (e.g., "rate_limit:60s").
    fn record_error(&self, cooldown: &str);
}

/// Error from an AI provider.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ProviderError {
    #[error("{message}")]
    Api {
        code: String,
        message: String,
        retryable: bool,
    },

    #[error("context overflow")]
    ContextOverflow,

    #[error("rate limit exceeded")]
    RateLimit,

    #[error("authentication error: {0}")]
    Auth(String),

    #[error("request error: {0}")]
    Request(String),

    #[error("stream error: {0}")]
    Stream(String),
}

impl ProviderError {
    /// Whether this error is retryable.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ProviderError::RateLimit
                | ProviderError::Api { retryable: true, .. }
                | ProviderError::Stream(_)
        )
    }
}

/// Check if an error indicates context window overflow.
pub fn is_context_overflow(err: &ProviderError) -> bool {
    matches!(err, ProviderError::ContextOverflow)
        || matches!(err, ProviderError::Api { code, message, .. }
            if code == "context_length_exceeded"
                || (message.contains("context") && message.contains("exceeded")))
}

/// Check if an error is a transient network issue safe to retry.
pub fn is_transient_error(err: &ProviderError) -> bool {
    if let ProviderError::Stream(msg) | ProviderError::Request(msg) = err {
        let lower = msg.to_lowercase();
        let keywords = [
            "stream error",
            "connection reset",
            "connection refused",
            "broken pipe",
            "eof",
            "tls handshake",
            "timeout",
            "no such host",
        ];
        keywords.iter().any(|kw| lower.contains(kw))
    } else {
        false
    }
}

/// Check if an error is due to message role ordering issues.
pub fn is_role_ordering_error(err: &ProviderError) -> bool {
    let msg = err.to_string().to_lowercase();
    let keywords = [
        "roles must alternate",
        "incorrect role information",
        "expected alternating",
        "must be followed by",
    ];
    keywords.iter().any(|kw| msg.contains(kw))
}

/// Classify an error reason for cooldown duration.
pub fn classify_error_reason(err: &ProviderError) -> &str {
    match err {
        ProviderError::RateLimit => "rate_limit",
        ProviderError::Auth(_) => "auth",
        ProviderError::ContextOverflow => "context_overflow",
        ProviderError::Api { code, message, .. } => {
            let lower_msg = message.to_lowercase();
            let lower_code = code.to_lowercase();
            if lower_code.contains("rate_limit") || lower_msg.contains("rate limit") || lower_msg.contains("429") {
                "rate_limit"
            } else if lower_code.contains("auth") || lower_msg.contains("unauthorized") || lower_msg.contains("api key") {
                "auth"
            } else if lower_msg.contains("billing") || lower_msg.contains("quota") || lower_msg.contains("payment") {
                "billing"
            } else if lower_msg.contains("timeout") || lower_msg.contains("timed out") {
                "timeout"
            } else {
                "other"
            }
        }
        ProviderError::Request(msg) | ProviderError::Stream(msg) => {
            let lower = msg.to_lowercase();
            if lower.contains("rate limit") || lower.contains("429") {
                "rate_limit"
            } else if lower.contains("billing") || lower.contains("quota") || lower.contains("payment") {
                "billing"
            } else if lower.contains("provider error") || lower.contains("upstream") {
                "provider"
            } else if lower.contains("timeout") || lower.contains("timed out") {
                "timeout"
            } else {
                "other"
            }
        }
    }
}

/// Provider configuration for constructing providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider_type: String,
    pub api_key: String,
    #[serde(default)]
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub extra: HashMap<String, String>,
}

/// Wraps a Provider with auth profile tracking.
pub struct ProfiledProvider {
    pub inner: Arc<dyn Provider>,
    profile_id: String,
}

impl ProfiledProvider {
    pub fn new(inner: Arc<dyn Provider>, profile_id: String) -> Self {
        Self { inner, profile_id }
    }
}

#[async_trait]
impl Provider for ProfiledProvider {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn profile_id(&self) -> &str {
        &self.profile_id
    }

    fn handles_tools(&self) -> bool {
        self.inner.handles_tools()
    }

    async fn stream(
        &self,
        req: &ChatRequest,
    ) -> Result<EventReceiver, ProviderError> {
        self.inner.stream(req).await
    }
}
