use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// The bearer an HTTP provider presents, resolved on every request. A fixed
/// key (a user's own OpenAI key) converts from `String`; a key that rotates
/// (the NeboAI token Janus takes, which the hub rotates on every comms
/// connect) is `ApiKey::live`, so a provider built once never presents a
/// token that has since been rotated out.
#[derive(Clone)]
pub struct ApiKey(Arc<dyn Fn() -> String + Send + Sync>);

impl ApiKey {
    pub fn live(resolve: impl Fn() -> String + Send + Sync + 'static) -> Self {
        Self(Arc::new(resolve))
    }

    /// The key to present right now.
    pub fn current(&self) -> String {
        (self.0)()
    }
}

impl From<String> for ApiKey {
    fn from(key: String) -> Self {
        Self::live(move || key.clone())
    }
}

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
    /// One whole thinking block, as the provider must get it back with the
    /// assistant turn it belongs to ([`StreamEvent::thinking_block`]).
    ThinkingBlock,
    Usage,
    RateLimit,
    ApprovalRequest,
    AskRequest,
    SubagentStart,
    SubagentProgress,
    SubagentComplete,
    ToolSummary,
    /// Run-control status from the runner (spiral backstop, circuit breaker,
    /// terminal tool error). Rendered as a status/notice in the UI — NEVER
    /// accumulated into reply text (`text` is the human-readable status line;
    /// `stop_reason` is the typed machine reason, e.g. "max_steps").
    ControlNotice,
    /// Whether the text segment the next tool call closed stays in the reply
    /// or folds into the turn's work (`text`: "shown" | "folded"; `payload`:
    /// `{"segment": n}`, the segment's index in the turn). Sent by the
    /// harness, never by a provider ([`StreamEvent::text_verdict`]).
    TextVerdict,
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
    /// What this request cost, in microdollars, as reported by the provider
    /// (Janus prices the model it actually routed to). `None` when the
    /// provider does not say — then the local price table is the only
    /// estimate, and for a routed alias it has nothing to say.
    #[serde(default)]
    pub cost_microdollars: Option<i64>,
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
    /// Structured rendering payload from ToolResult.payload (ToolResult events
    /// only) — forwarded to the app so known kinds render as rich cards. On a
    /// `terminal_tool_error` ControlNotice: the refusing tool's
    /// `types::OwnerNeed`, when it named one ([`StreamEvent::owner_need`]).
    pub payload: Option<serde_json::Value>,
    /// Engine-stamped provenance classes of the run (Done events only) — the
    /// final taint set the runner accumulated. Consumed by the coworker rail
    /// to stamp reply envelopes; never model-writable.
    pub provenance: Option<Vec<types::provenance::ProvenanceClass>>,
}

impl StreamEvent {
    /// Carry what only the owner can supply on a `terminal_tool_error`
    /// ControlNotice (the refusing tool named it).
    pub fn with_owner_need(mut self, need: Option<types::OwnerNeed>) -> Self {
        self.payload = need.and_then(|n| serde_json::to_value(n).ok());
        self
    }

    /// The owner need a `terminal_tool_error` ControlNotice carries.
    pub fn owner_need(&self) -> Option<types::OwnerNeed> {
        if self.event_type != StreamEventType::ControlNotice {
            return None;
        }
        self.payload.clone().and_then(|p| serde_json::from_value(p).ok())
    }

    /// A whole thinking block (ThinkingBlock events), carried in `payload`.
    pub fn thinking_block(block: ThinkingBlock) -> Self {
        let mut event = Self::thinking("");
        event.event_type = StreamEventType::ThinkingBlock;
        event.payload = serde_json::to_value(block).ok();
        event
    }

    /// The verdict on the turn's text segment `segment` ("shown" | "folded").
    pub fn text_verdict(segment: usize, fold: &str) -> Self {
        let mut event = Self::text(fold);
        event.event_type = StreamEventType::TextVerdict;
        event.payload = Some(serde_json::json!({ "segment": segment }));
        event
    }

    /// The block a ThinkingBlock event carries.
    pub fn block(&self) -> Option<ThinkingBlock> {
        if self.event_type != StreamEventType::ThinkingBlock {
            return None;
        }
        self.payload.clone().and_then(|p| serde_json::from_value(p).ok())
    }

    /// Attach the run's final provenance classes (Done events).
    pub fn with_provenance(mut self, classes: Vec<types::provenance::ProvenanceClass>) -> Self {
        self.provenance = Some(classes);
        self
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self { payload: None,
            provenance: None,
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
        Self { payload: None,
            provenance: None,
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

    /// Run-control status: `text` is the user-facing status line, `stop_reason`
    /// the typed reason ("max_steps", "user_requested_stop",
    /// "terminal_tool_error"). Consumers surface it as status — reply
    /// accumulators must ignore it by type.
    pub fn control_notice(text: impl Into<String>, stop_reason: impl Into<String>) -> Self {
        Self { payload: None,
            provenance: None,
            event_type: StreamEventType::ControlNotice,
            text: text.into(),
            tool_call: None,
            error: None,
            usage: None,
            rate_limit: None,
            widgets: None,
            provider_metadata: None,
            stop_reason: Some(stop_reason.into()),
            image_url: None,
        }
    }

    pub fn tool_call(tc: ToolCall) -> Self {
        Self { payload: None,
            provenance: None,
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
        Self { payload: None,
            provenance: None,
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
        Self { payload: None,
            provenance: None,
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
        Self { payload: None,
            provenance: None,
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
        Self { payload: None,
            provenance: None,
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
        Self { payload: None,
            provenance: None,
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
        Self { payload: None,
            provenance: None,
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
        Self { payload: None,
            provenance: None,
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

    /// One card for several gated calls in one batch: the first call rides in
    /// `tool_call` (its id is the request id the answer comes back on) and the
    /// whole list rides in `widgets.batch`. One decision applies to all.
    pub fn approval_request_batch(calls: &[ToolCall]) -> Self {
        let mut ev = Self::approval_request(calls[0].clone());
        ev.widgets = Some(serde_json::json!({
            "batch": calls.iter().map(|c| serde_json::json!({"id": c.id, "tool": c.name, "input": c.input})).collect::<Vec<_>>()
        }));
        ev
    }

    pub fn approval_request(tc: ToolCall) -> Self {
        Self { payload: None,
            provenance: None,
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
        Self { payload: None,
            provenance: None,
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
        Self { payload: None,
            provenance: None,
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

/// A block of the model's thinking in an assistant turn. A provider that
/// signs its thinking (Anthropic) refuses a tool loop with thinking on unless
/// each block comes back unchanged with the turn it belongs to, and a
/// signature is bound to the model that wrote it (Claude Code keeps them in
/// the assistant message and strips them when the model changes:
/// `src/utils/messages.ts:5066` `stripSignatureBlocks`, `src/query.ts:924-929`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThinkingBlock {
    Thinking { thinking: String, signature: String },
    RedactedThinking { data: String },
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
    /// An assistant turn's thinking blocks, in the order they came, when the
    /// request goes to the model that wrote them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub thinking: Vec<ThinkingBlock>,
}

/// What an LLM request is for and which Nebo run produced it. Every
/// `ChatRequest` carries one, so a call that does not name its purpose does
/// not compile. The Janus provider emits it as `X-Purpose` plus
/// `X-Agent-ID`/`X-Run-ID`/`X-Workflow-ID`/`X-Action-ID`/`X-Step-ID` headers so
/// usage can be grouped by purpose and attributed per agent/workflow/action/
/// step. `agent_id` is the rollup key that disambiguates the same workflow run
/// by different agents; chat runs set `agent_id` + `run_id`. Empty ids stay off
/// the wire. Never serialized into the request body.
#[derive(Debug, Clone)]
pub struct RequestTrace {
    /// Short stable name of the call site's job (`agent_turn`,
    /// `memory_extract`, `compaction`, ...). The grouping key in Janus usage.
    pub purpose: &'static str,
    pub agent_id: String,
    pub run_id: String,
    pub workflow_id: String,
    pub action_id: String,
    pub step_id: String,
}

impl RequestTrace {
    /// A trace naming `purpose`, with no ids. Set the ids that are in scope
    /// with struct update syntax: `RequestTrace { agent_id, ..RequestTrace::new("title") }`.
    pub fn new(purpose: &'static str) -> Self {
        Self {
            purpose,
            agent_id: String::new(),
            run_id: String::new(),
            workflow_id: String::new(),
            action_id: String::new(),
            step_id: String::new(),
        }
    }

    /// The attribution headers for this trace. The one place they are
    /// written; every Janus request path inserts these.
    pub fn headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        for (name, val) in [
            ("x-purpose", self.purpose),
            ("x-agent-id", self.agent_id.as_str()),
            ("x-run-id", self.run_id.as_str()),
            ("x-workflow-id", self.workflow_id.as_str()),
            ("x-action-id", self.action_id.as_str()),
            ("x-step-id", self.step_id.as_str()),
        ] {
            if !val.is_empty()
                && let Ok(hv) = val.parse()
            {
                headers.insert(reqwest::header::HeaderName::from_static(name), hv);
            }
        }
        headers
    }
}

/// A request to an AI provider. There is no `Default`: `trace` is required,
/// so build one from [`ChatRequest::new`] or name every field.
#[derive(Debug, Clone, Serialize)]
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
    /// What this request is for and the Nebo run/workflow/action/step behind
    /// it. Consumed by the Janus provider to emit usage-attribution headers;
    /// never serialized.
    #[serde(skip)]
    pub trace: RequestTrace,
    /// The run's tool credential, for a provider that runs tools itself over
    /// the server's MCP endpoint (the CLI providers): its tool calls carry it
    /// back so they execute as this run. Never serialized.
    #[serde(skip)]
    pub tool_credential: Option<String>,
    /// The Nebo conversation this turn belongs to (the chat row's id), for a
    /// provider that keeps one remote conversation per Nebo chat (the linked
    /// provider). Empty for calls that are not a conversation's turn. Never
    /// serialized.
    #[serde(skip)]
    pub chat_id: String,
    /// The run's tool-approval channels — `tools::ApprovalChannels`, the ONE
    /// tool-approval pathway — for a `handles_tools` provider whose runtime
    /// stops for the owner's decision (the linked provider): it registers the
    /// runtime's request under the same map the runner's own gate uses, so
    /// the ApprovalGate, the phone and the comm relay all answer it. Never
    /// serialized.
    #[serde(skip)]
    pub approval_channels: Option<ApprovalChannels>,
}

/// The run's tool-approval channels, keyed by request id; the value is the
/// decision (`"once"`, `"always"`, `"deny"`). The same type as
/// `tools::ApprovalChannels`, spelled here because `tools` depends on this
/// crate.
pub type ApprovalChannels = Arc<
    tokio::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<String>>>,
>;

impl ChatRequest {
    /// An empty request for `trace`. Fill the rest with struct update syntax:
    /// `ChatRequest { messages, ..ChatRequest::new(RequestTrace::new("title")) }`.
    pub fn new(trace: RequestTrace) -> Self {
        Self {
            messages: Vec::new(),
            tools: Vec::new(),
            tool_choice: ToolChoice::default(),
            max_tokens: 0,
            temperature: 0.0,
            system: String::new(),
            model: String::new(),
            enable_thinking: false,
            metadata: None,
            cache_breakpoints: Vec::new(),
            cancel_token: None,
            trace,
            tool_credential: None,
            chat_id: String::new(),
            approval_channels: None,
        }
    }
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

    /// Whether the runner may send this provider a call it did not build for
    /// it: a failed call again, or a call another provider failed. False for
    /// a provider whose runtime keeps the conversation and answers only for
    /// the agent addressed (the linked provider): a message is delivered once,
    /// so the runner shows its error as it is and ends the step, and never
    /// rotates another employee's failed call onto it.
    fn retryable(&self) -> bool {
        true
    }

    /// Whether this provider supports images in tool result content blocks.
    /// When true, the runner will pass screenshot images directly to the model
    /// instead of converting them to text via the sidecar vision model.
    fn supports_tool_result_images(&self) -> bool {
        false
    }

    /// Whether this provider puts `Message::images` on the wire. When false the
    /// runner describes attached images through the sidecar first — a provider
    /// that ignores the field would otherwise drop the user's image in silence
    /// and answer as if nothing had been attached.
    fn supports_vision(&self) -> bool {
        false
    }

    /// Send a request and return a channel of streaming events.
    async fn stream(&self, req: &ChatRequest) -> Result<EventReceiver, ProviderError>;
}

/// The first provider, in priority order, that takes any call: where a call
/// no employee's model names goes (a summary, a compaction, a review). Never
/// the linked provider (`retryable` = false), which answers only for the
/// agent it is addressed to.
pub fn default_provider(providers: &[Arc<dyn Provider>]) -> Option<Arc<dyn Provider>> {
    providers.iter().find(|p| p.retryable()).cloned()
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

    /// A 429. `retry_after_secs` is the provider's `Retry-After`, when it
    /// sent one — the runner waits that long instead of its own backoff.
    #[error("rate limit exceeded")]
    RateLimit { retry_after_secs: Option<u64> },

    #[error("authentication error: {0}")]
    Auth(String),

    #[error("request error: {0}")]
    Request(String),

    #[error("stream error: {0}")]
    Stream(String),
}

impl ProviderError {
    /// Whether this error is retryable.
    ///
    /// `Request` is a transport-level send failure (connection reset, stale
    /// pooled keepalive, momentary network blip) — the request never reached
    /// the server, making it the safest class to retry. The runner bounds
    /// retries (MAX_RETRYABLE_RETRIES + backoff), so deterministic Request
    /// constructors (serialize errors) just fail identically and stop.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ProviderError::RateLimit { .. }
                | ProviderError::Api {
                    retryable: true,
                    ..
                }
                | ProviderError::Stream(_)
                | ProviderError::Request(_)
        )
    }
}

/// Check if an error indicates context window overflow.
/// Resolve a tool-result image reference into `(media_type, base64_data)` for
/// provider payloads. Accepts a `data:` URI or a local image file path (format
/// sniffed from magic bytes, never the extension). Returns None for anything
/// unreadable, non-image, or over the 5MB provider base64 cap — callers omit
/// the image block instead of sending garbage bytes labeled image/png.
pub fn image_source_to_base64(raw: &str) -> Option<(String, String)> {
    use base64::Engine;
    const MAX_BASE64_LEN: usize = 5 * 1024 * 1024; // Anthropic hard limit
    if let Some(rest) = raw.strip_prefix("data:") {
        let (header, data) = rest.split_once(',')?;
        if data.len() > MAX_BASE64_LEN {
            return None;
        }
        let media_type = header.strip_suffix(";base64").unwrap_or(header);
        return Some((media_type.to_string(), data.to_string()));
    }
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return None;
    }
    let bytes = std::fs::read(raw).ok()?;
    let media_type = sniff_image_mime(&bytes)?;
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
    if data.len() > MAX_BASE64_LEN {
        return None;
    }
    Some((media_type.to_string(), data))
}

/// Identify an image from its magic bytes. Returns None for anything that is
/// not an image a provider will accept — never trust a declared MIME type.
pub fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

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
            // Mid-stream cut: the byte stream ended without a finish_reason /
            // [DONE] (e.g. an edge/proxy idle timeout during a long tool call).
            // Retrying re-runs the turn rather than silently dropping it.
            "truncated",
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

/// Check if an error is a refusal the provider will repeat for the same
/// request — an invalid parameter, an unknown model, an unsupported feature.
///
/// These arrive as ordinary stream errors, and `ProviderError::Stream` is
/// blanket-retryable, so without this the retry ladder re-sends the identical
/// payload until the budget runs out: the owner waits through five round trips
/// to be told the same thing five times. Nothing about the request changes
/// between attempts, so the first answer is the final one.
///
/// Capacity and quota refusals are deliberately excluded — they arrive with
/// similar shapes but do clear on their own, and belong to the retry ladder.
pub fn is_deterministic_request_error(err: &ProviderError) -> bool {
    if let ProviderError::Stream(msg) | ProviderError::Request(msg) = err {
        let lower = msg.to_lowercase();
        let clears_on_its_own = [
            "rate limit",
            "rate_limit",
            "429",
            "quota",
            "billing",
            "payment",
            "overloaded",
            "capacity",
        ];
        if clears_on_its_own.iter().any(|kw| lower.contains(kw)) {
            return false;
        }
        let refusals = [
            "invalid_parameter_error",
            "invalid_request_error",
            "model_not_found",
            "unsupported parameter",
            "is not supported for",
            "does not support",
            "unknown model",
        ];
        refusals.iter().any(|kw| lower.contains(kw))
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
        ProviderError::RateLimit { .. } => "rate_limit",
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

    fn retryable(&self) -> bool {
        self.inner.retryable()
    }

    async fn stream(&self, req: &ChatRequest) -> Result<EventReceiver, ProviderError> {
        self.inner.stream(req).await
    }
}

#[cfg(test)]
mod image_source_tests {
    use super::*;

    #[test]
    fn data_uri_passes_through_and_junk_is_rejected() {
        let (mt, data) = image_source_to_base64("data:image/jpeg;base64,/9j/AAAA").unwrap();
        assert_eq!(mt, "image/jpeg");
        assert_eq!(data, "/9j/AAAA");
        // Non-data, non-file strings must be dropped, not sent as fake base64 PNG
        assert!(image_source_to_base64("https://example.com/x.png").is_none());
        assert!(image_source_to_base64("/nonexistent/path.png").is_none());
        // A real file that isn't an image must be rejected by magic-byte sniff
        let tmp = std::env::temp_dir().join("nebo_img_norm_test.png");
        std::fs::write(&tmp, b"definitely not a png").unwrap();
        assert!(image_source_to_base64(tmp.to_str().unwrap()).is_none());
        // A real PNG-magic file round-trips to a proper data pair
        std::fs::write(&tmp, b"\x89PNG\r\n\x1a\nrest-of-file").unwrap();
        let (mt, _) = image_source_to_base64(tmp.to_str().unwrap()).unwrap();
        assert_eq!(mt, "image/png");
        let _ = std::fs::remove_file(&tmp);
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
            ..ChatRequest::new(RequestTrace::new("test"))
        };
        let v = serde_json::to_value(&auto).unwrap();
        assert!(
            v.get("tool_choice").is_none(),
            "Auto tool_choice must be omitted"
        );

        // Non-Auto is serialized (the per-provider adapter then maps it).
        let forced = ChatRequest {
            tool_choice: ToolChoice::Tool("StructuredOutput".to_string()),
            ..ChatRequest::new(RequestTrace::new("test"))
        };
        let v2 = serde_json::to_value(&forced).unwrap();
        assert!(
            v2.get("tool_choice").is_some(),
            "non-Auto tool_choice must be present"
        );

        assert_eq!(ToolChoice::default(), ToolChoice::Auto);
    }
}

#[cfg(test)]
mod deterministic_refusal_tests {
    use super::*;

    /// The refusal that took the owner's employee down: dashscope rejecting
    /// the temperature for the model behind "Nebo 1 Pro". It arrives as a
    /// plain stream error, and `Stream(_)` is blanket-retryable, so without
    /// this predicate the runner re-sent the same payload five times.
    #[test]
    fn an_unsupported_parameter_is_never_retried() {
        let err = ProviderError::Stream(
            "Provider dashscope error: OpenAI API error (HTTP 400): \
             [invalid_parameter_error] <400> InternalError.Algo.InvalidParameter: \
             Parameter 'temperature'=0.699999988079071 is not supported for kimi-k3 model."
                .to_string(),
        );
        assert!(is_deterministic_request_error(&err));
        // The blanket-retryable classification is exactly what this guards.
        assert!(err.is_retryable());
    }

    #[test]
    fn an_unknown_model_is_never_retried() {
        for msg in [
            "model_not_found: no such model",
            "invalid_request_error: unknown model nebo-1-high",
            "This endpoint does not support streaming",
        ] {
            assert!(
                is_deterministic_request_error(&ProviderError::Stream(msg.to_string())),
                "should be deterministic: {msg}"
            );
        }
    }

    /// Anything that clears on its own must stay on the retry ladder — the
    /// predicate narrows what gets retried, it must not empty it.
    #[test]
    fn refusals_that_clear_on_their_own_still_retry() {
        for msg in [
            "429 rate limit exceeded",
            "insufficient quota for this request",
            "billing: payment required",
            "upstream is overloaded, try again",
            "connection reset by peer",
            "stream error: unexpected EOF",
        ] {
            assert!(
                !is_deterministic_request_error(&ProviderError::Stream(msg.to_string())),
                "must stay retryable: {msg}"
            );
        }
    }

    /// A rate limit whose body happens to mention an invalid parameter must
    /// still be treated as transient — the self-clearing check runs first.
    #[test]
    fn self_clearing_wins_over_a_refusal_keyword() {
        let err = ProviderError::Stream(
            "429 rate limit: invalid_request_error while shedding load".to_string(),
        );
        assert!(!is_deterministic_request_error(&err));
    }

    #[test]
    fn other_error_kinds_are_left_alone() {
        assert!(!is_deterministic_request_error(&ProviderError::RateLimit { retry_after_secs: None }));
        assert!(!is_deterministic_request_error(&ProviderError::Auth(
            "bad key".into()
        )));
    }
}
