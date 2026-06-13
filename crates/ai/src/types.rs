use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Rate limit metadata extracted from provider response headers.
#[derive(Debug, Clone, Default)]
pub struct RateLimitMeta {
    pub remaining_requests: Option<u64>,
    pub remaining_tokens: Option<u64>,
    pub reset_after_secs: Option<f64>,
    pub retry_after_secs: Option<u64>,
    // Janus session/weekly rate limit windows (values are microdollars)
    pub session_limit_credits: Option<u64>,
    pub session_remaining_credits: Option<u64>,
    pub session_reset_at: Option<String>,
    pub weekly_limit_credits: Option<u64>,
    pub weekly_remaining_credits: Option<u64>,
    pub weekly_reset_at: Option<String>,
    // Janus budget pool headers
    pub budget_free_available: Option<u64>,
    pub budget_gift_available: Option<u64>,
    pub budget_credits_cents: Option<u64>,
    pub budget_active_pool: Option<String>,
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
    PlanApproval,
    SubagentStart,
    SubagentProgress,
    SubagentComplete,
    ToolSummary,
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
    /// System prompt + tool-schema tokens (estimate). Populated by the runner so the
    /// UI can subtract fixed overhead and show conversation-only input tokens.
    #[serde(default)]
    pub overhead_tokens: i32,
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
    /// Stop reason from the provider: "end_turn", "max_tokens", "length", "tool_use", etc.
    pub stop_reason: Option<String>,
    /// File/image artifact produced by a tool (ToolResult events only).
    /// Either a `data:` URI (inline base64), a `/api/v1/files/<name>` local URL,
    /// or a local filesystem path under `<data_dir>/files/`. Used by chat_dispatch
    /// to auto-attach run-produced files to outbound comm replies.
    pub image_url: Option<String>,
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
            stop_reason: None,
            image_url: None,
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
            stop_reason: None,
            image_url: None,
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
            stop_reason: None,
            image_url: None,
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
            stop_reason: None,
            image_url: None,
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
            stop_reason: None,
            image_url: None,
        }
    }

    /// A sub-agent (or harness node) started. `id` is a stable node id; `description` is the
    /// human label the UI renders. Callers (e.g. the orchestrator) may add extra `widgets`
    /// fields (agent_type, total_count) after construction.
    pub fn subagent_start(id: impl Into<String>, description: impl Into<String>) -> Self {
        let id = id.into();
        let description = description.into();
        Self {
            event_type: StreamEventType::SubagentStart,
            text: description.clone(),
            tool_call: None,
            error: Some(id.clone()),
            usage: None,
            rate_limit: None,
            widgets: Some(serde_json::json!({ "task_id": id, "description": description })),
            provider_metadata: None,
            stop_reason: None,
            image_url: None,
        }
    }

    /// A sub-agent (or harness node) finished. `success` flags whether it produced a result.
    pub fn subagent_complete(
        id: impl Into<String>,
        description: impl Into<String>,
        success: bool,
    ) -> Self {
        let id = id.into();
        let description = description.into();
        Self {
            event_type: StreamEventType::SubagentComplete,
            text: description.clone(),
            tool_call: None,
            error: Some(id.clone()),
            usage: None,
            rate_limit: None,
            widgets: Some(
                serde_json::json!({ "task_id": id, "description": description, "success": success }),
            ),
            provider_metadata: None,
            stop_reason: None,
            image_url: None,
        }
    }

    pub fn done_with_reason(reason: impl Into<String>) -> Self {
        Self {
            event_type: StreamEventType::Done,
            text: String::new(),
            tool_call: None,
            error: None,
            usage: None,
            rate_limit: None,
            widgets: None,
            provider_metadata: None,
            stop_reason: Some(reason.into()),
            image_url: None,
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
            stop_reason: None,
            image_url: None,
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
            stop_reason: None,
            image_url: None,
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
            stop_reason: None,
            image_url: None,
        }
    }

    pub fn tool_summary(text: impl Into<String>) -> Self {
        Self {
            event_type: StreamEventType::ToolSummary,
            text: text.into(),
            tool_call: None,
            error: None,
            usage: None,
            rate_limit: None,
            widgets: None,
            provider_metadata: None,
            stop_reason: None,
            image_url: None,
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
            stop_reason: None,
            image_url: None,
        }
    }

    /// Plan approval request: sends plan text and proposed tool names to the frontend.
    pub fn plan_approval_request(
        request_id: impl Into<String>,
        plan: impl Into<String>,
        tools: Vec<String>,
    ) -> Self {
        Self {
            event_type: StreamEventType::PlanApproval,
            text: plan.into(),
            tool_call: None,
            error: Some(request_id.into()),
            usage: None,
            rate_limit: None,
            widgets: Some(serde_json::json!(tools)),
            provider_metadata: None,
            stop_reason: None,
            image_url: None,
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

/// How the model may use the offered tools on a request.
/// `Auto` (default) is omitted on the wire, so existing requests stay byte-identical.
/// `Any`/`Tool`/`None` are mapped per-provider; providers that can't force tool calls
/// (ollama/local/cli) treat non-`Auto` as a best-effort no-op.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    /// Model decides whether to call a tool.
    #[default]
    Auto,
    /// Model MUST call some tool (any of the offered ones).
    Any,
    /// Model MUST call the named tool.
    Tool(String),
    /// Model must NOT call any tool.
    None,
}

/// Serde helper: `tool_choice` is omitted when it's the default `Auto`.
fn is_auto(tc: &ToolChoice) -> bool {
    *tc == ToolChoice::Auto
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
    /// How the model may use the offered tools. `Auto` (default) is omitted on the wire.
    #[serde(default, skip_serializing_if = "is_auto")]
    pub tool_choice: ToolChoice,
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
    /// Cancellation token for cooperative shutdown. CLI providers use this to
    /// kill their child process when the user hits stop.
    #[serde(skip)]
    pub cancel_token: Option<CancellationToken>,
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

    /// Human-readable name for UI display. Defaults to `id()`.
    fn display_name(&self) -> &str {
        self.id()
    }

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
    async fn stream(&self, req: &ChatRequest) -> Result<EventReceiver, ProviderError>;
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
                | ProviderError::Api {
                    retryable: true,
                    ..
                }
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

/// Check if an error indicates the model is overloaded (HTTP 529 or "overloaded" in message).
pub fn is_overloaded(err: &ProviderError) -> bool {
    match err {
        ProviderError::Api { code, message, .. } => {
            code == "529" || message.to_lowercase().contains("overloaded")
        }
        ProviderError::Stream(msg) | ProviderError::Request(msg) => {
            let lower = msg.to_lowercase();
            lower.contains("529") || lower.contains("overloaded")
        }
        _ => false,
    }
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
            // Upstream LLM hiccups that clear on retry — e.g. dashscope/Janus
            // returning a completion with no text and no tool calls.
            "empty response",
            "empty completion",
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
            if lower_code.contains("rate_limit")
                || lower_msg.contains("rate limit")
                || lower_msg.contains("429")
            {
                "rate_limit"
            } else if lower_code.contains("auth")
                || lower_msg.contains("unauthorized")
                || lower_msg.contains("api key")
            {
                "auth"
            } else if lower_msg.contains("billing")
                || lower_msg.contains("quota")
                || lower_msg.contains("payment")
            {
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
            } else if lower.contains("billing")
                || lower.contains("quota")
                || lower.contains("payment")
            {
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

    fn display_name(&self) -> &str {
        self.inner.display_name()
    }

    fn profile_id(&self) -> &str {
        &self.profile_id
    }

    fn handles_tools(&self) -> bool {
        self.inner.handles_tools()
    }

    async fn stream(&self, req: &ChatRequest) -> Result<EventReceiver, ProviderError> {
        self.inner.stream(req).await
    }
}

#[cfg(test)]
mod transient_tests {
    use super::*;

    #[test]
    fn empty_response_is_transient_and_retried() {
        // Upstream "empty response" (e.g. dashscope via Janus) must self-recover.
        let err = ProviderError::Stream(
            "Provider dashscope returned empty response (finish_reason=)".to_string(),
        );
        assert!(is_transient_error(&err), "empty response should be transient");
    }

    #[test]
    fn unrelated_stream_error_not_transient() {
        let err = ProviderError::Stream("invalid request: bad tool schema".to_string());
        assert!(!is_transient_error(&err));
    }

    #[test]
    fn tool_choice_auto_omitted_non_auto_wired() {
        // Auto must be omitted on the wire → existing requests stay byte-identical.
        let auto = ChatRequest {
            tool_choice: ToolChoice::Auto,
            ..Default::default()
        };
        let v = serde_json::to_value(&auto).unwrap();
        assert!(
            v.get("tool_choice").is_none(),
            "Auto tool_choice must be omitted"
        );

        // Non-Auto is serialized (the per-provider adapter then maps it).
        let forced = ChatRequest {
            tool_choice: ToolChoice::Tool("StructuredOutput".to_string()),
            ..Default::default()
        };
        let v2 = serde_json::to_value(&forced).unwrap();
        assert!(
            v2.get("tool_choice").is_some(),
            "non-Auto tool_choice must be present"
        );

        assert_eq!(ToolChoice::default(), ToolChoice::Auto);
    }
}
