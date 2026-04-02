# Provider System: Complete Architecture Reference

**Crates:** `crates/ai/`, `crates/agent/`, `crates/config/`, `crates/db/`, `crates/server/` | **Status:** Implemented (Rust)

This document is the authoritative reference for the Nebo AI provider system as implemented in Rust. It covers the universal Provider trait, all six provider implementations, model selection and routing, configuration, database schema, error handling, embeddings, and hot reload. For Janus-specific migration notes and Go source references, see `janus-and-providers.md`.

---

## Table of Contents

1. [System Overview](#1-system-overview)
2. [Provider Trait and Core Types](#2-provider-trait-and-core-types)
3. [Anthropic Provider](#3-anthropic-provider)
4. [OpenAI Provider (OpenAI / Janus / Deepseek)](#4-openai-provider)
5. [Gemini Provider](#5-gemini-provider)
6. [Ollama Provider](#6-ollama-provider)
7. [Local Provider (GGUF / llama.cpp)](#7-local-provider)
8. [CLI Provider (Claude Code / Gemini CLI / Codex)](#8-cli-provider)
9. [Embedding System](#9-embedding-system)
10. [SSE Parser](#10-sse-parser)
11. [Model Selection and Task Routing](#11-model-selection-and-task-routing)
12. [Configuration Layer](#12-configuration-layer)
13. [Database Layer](#13-database-layer)
14. [Provider Lifecycle and Hot Reload](#14-provider-lifecycle-and-hot-reload)
15. [Error Handling and Retry Logic](#15-error-handling-and-retry-logic)
16. [Server HTTP API](#16-server-http-api)
17. [Key Files Reference](#17-key-files-reference)

---

## 1. System Overview

### 1.1 Architecture Diagram

```
┌──────────────────────────────────────────────────────────────────────┐
│                    Server HTTP API Layer                              │
│  POST/PUT/DELETE /api/v1/providers → reload_providers()              │
│  GET /api/v1/models → catalog + routing config                       │
├──────────────────────────────────────────────────────────────────────┤
│                    Agent Runner (crates/agent/)                       │
│  providers: Arc<RwLock<Vec<Arc<dyn Provider>>>>                      │
│  Retry loop: 10 transient, 5 retryable, 3 auto-continuations        │
├──────────────────────────────────────────────────────────────────────┤
│                    Model Selector (crates/agent/)                     │
│  classify task → route by config → check usable → fallback chain     │
│  fuzzy resolution: "sonnet" → "anthropic/claude-sonnet-4-6"          │
├──────────────────────────────────────────────────────────────────────┤
│                    Provider Trait (crates/ai/)                        │
│  stream(&ChatRequest) → Result<EventReceiver, ProviderError>         │
├────────┬──────────┬─────────┬─────────┬──────────┬──────────────────┤
│Anthropic│  OpenAI  │ Gemini  │ Ollama  │  Local   │      CLI        │
│  (SSE)  │(SSE/raw) │  (SSE)  │(NDJSON) │  (GGUF)  │ (stdio/JSON)    │
│         │ +Janus   │         │         │  gated   │ handles_tools() │
│         │ +Deepseek│         │         │          │                 │
└────────┴──────────┴─────────┴─────────┴──────────┴──────────────────┘
```

### 1.2 Data Flow

```
ChatRequest
  → Provider.stream()
    → HTTP request (provider-specific format)
    → tokio::spawn(stream handler)
      → Parse response (SSE / NDJSON / JSON lines)
      → Emit StreamEvents via mpsc channel
    → Return EventReceiver to caller
```

### 1.3 Provider Priority Order

Provider ordering in the `providers` vec (used by runner fallback logic):

```
1. Direct API providers (Anthropic, OpenAI, Gemini, DeepSeek, Ollama via API keys)
2. CLI providers (claude, codex, gemini binaries -- use user's own subscription)
3. Janus gateway (NeboLoop -- consumes Nebo credits, always LAST)
4. Local models (GGUF -- always-available fallback, feature-gated)
```

**Rationale:** CLI providers don't burn Nebo credits (they use the user's existing subscription), so they take priority over Janus. Direct API keys take priority over everything.

---

## 2. Provider Trait and Core Types

**File:** `crates/ai/src/types.rs`

### 2.1 Provider Trait

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    /// Provider identifier: "anthropic", "openai", "janus", "deepseek", "gemini", "ollama", "local", "cli"
    fn id(&self) -> &str;

    /// Auth profile ID for billing tracking (optional)
    fn profile_id(&self) -> &str { "" }

    /// True for CLI providers that execute tools autonomously via MCP
    fn handles_tools(&self) -> bool { false }

    /// Whether this provider supports images in tool result content blocks.
    /// When true, the runner passes screenshot images directly to the model
    /// instead of converting them to text via the sidecar vision model.
    /// Returns true for Anthropic (which supports image blocks in tool_result).
    fn supports_tool_result_images(&self) -> bool { false }

    /// Send a chat request and return a streaming event receiver
    async fn stream(&self, req: &ChatRequest) -> Result<EventReceiver, ProviderError>;
}
```

### 2.2 Optional Traits

```rust
/// Reset HTTP/2 persistent connections on GOAWAY frames
pub trait ConnectionResetter {
    fn reset_connections(&self);
}

/// Track auth profile usage for billing
pub trait ProfileTracker {
    fn record_usage(&self, input_tokens: i32, output_tokens: i32);
    fn record_error(&self, cooldown: &str);  // e.g., "rate_limit:60s"
}

/// Wrapper that adds profile ID tracking to any Provider
pub struct ProfiledProvider {
    pub inner: Arc<dyn Provider>,
    profile_id: String,
}
```

### 2.3 ChatRequest

The universal request envelope sent to every provider:

```rust
pub struct ChatRequest {
    pub messages: Vec<Message>,                       // Conversation history
    pub tools: Vec<ToolDefinition>,                   // Available tools/functions
    pub max_tokens: i32,                              // Max output tokens
    pub temperature: f64,                             // Sampling temperature
    pub system: String,                               // Dynamic system prompt
    pub static_system: String,                        // Static system (Anthropic legacy cache)
    pub model: String,                                // Model ID (e.g., "claude-sonnet-4-6")
    pub enable_thinking: bool,                        // Extended thinking (Anthropic)
    pub metadata: Option<HashMap<String, String>>,    // Janus tool stickiness routing (echoed back)
    pub cache_breakpoints: Vec<usize>,                // Byte offsets for Anthropic prompt caching
    pub cancel_token: Option<CancellationToken>,      // Cooperative shutdown (CLI providers)
}
```

### 2.4 Message

```rust
pub struct Message {
    pub role: String,                              // "user", "assistant", "tool", "system"
    pub content: String,                           // Text content
    pub tool_calls: Option<serde_json::Value>,     // SessionToolCall[] (assistant)
    pub tool_results: Option<serde_json::Value>,   // SessionToolResult[] (user)
    pub images: Option<Vec<ImageContent>>,          // Vision (user)
}

pub struct ImageContent {
    pub media_type: String,  // "image/jpeg", "image/png"
    pub data: String,        // Base64 encoded
}
```

### 2.5 StreamEvent

Events emitted by providers through the mpsc channel:

```rust
pub struct StreamEvent {
    pub event_type: StreamEventType,
    pub text: String,
    pub tool_call: Option<ToolCall>,
    pub error: Option<String>,
    pub usage: Option<UsageInfo>,
    pub rate_limit: Option<RateLimitMeta>,
    pub widgets: Option<serde_json::Value>,                 // UI widgets (ask_request)
    pub provider_metadata: Option<HashMap<String, String>>, // Janus tool stickiness (on Done)
    pub stop_reason: Option<String>,                        // "end_turn", "max_tokens", "tool_use"
}

pub enum StreamEventType {
    Text,             // Incremental text output
    ToolCall,         // Tool/function call request
    ToolResult,       // Tool execution result
    Error,            // Provider error
    Done,             // Stream complete
    Thinking,         // Extended thinking output (Anthropic)
    Usage,            // Token usage report
    RateLimit,        // Rate limit metadata
    ApprovalRequest,  // Tool approval request (CLI providers)
    AskRequest,       // User input request (CLI providers)
    SubagentStart,    // Subagent execution started
    SubagentProgress, // Subagent progress update
    SubagentComplete, // Subagent execution finished
}
```

### 2.6 ToolCall and ToolDefinition

```rust
pub struct ToolCall {
    pub id: String,                // Unique call ID
    pub name: String,              // Tool name
    pub input: serde_json::Value,  // Tool arguments (JSON)
}

pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,  // JSON Schema
}
```

### 2.7 UsageInfo

```rust
pub struct UsageInfo {
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub cache_creation_input_tokens: i32,  // Anthropic prompt cache tokens created
    pub cache_read_input_tokens: i32,      // Anthropic prompt cache tokens read
}
```

### 2.8 RateLimitMeta

```rust
pub struct RateLimitMeta {
    pub remaining_requests: Option<u64>,
    pub remaining_tokens: Option<u64>,
    pub reset_after_secs: Option<f64>,
    pub retry_after_secs: Option<u64>,
    // Janus session/weekly rate limit windows
    pub session_limit_tokens: Option<u64>,
    pub session_remaining_tokens: Option<u64>,
    pub session_reset_at: Option<String>,      // ISO8601 timestamp
    pub weekly_limit_tokens: Option<u64>,
    pub weekly_remaining_tokens: Option<u64>,
    pub weekly_reset_at: Option<String>,       // ISO8601 timestamp
}
```

### 2.9 ProviderError

```rust
pub enum ProviderError {
    Api { code: String, message: String, retryable: bool },
    ContextOverflow,
    RateLimit,
    Auth(String),
    Request(String),
    Stream(String),
}
```

### 2.10 EventReceiver

Type alias for the streaming channel:

```rust
pub type EventReceiver = tokio::sync::mpsc::Receiver<StreamEvent>;
```

---

## 3. Anthropic Provider

**File:** `crates/ai/src/providers/anthropic.rs`

### 3.1 Configuration

```rust
pub struct AnthropicProvider {
    client: Client,
    api_key: String,
    model: String,
    base_url: String,  // Default: "https://api.anthropic.com"
}
```

### 3.2 Key Features

| Feature | Implementation |
|---------|---------------|
| **Streaming** | Raw HTTP SSE via reqwest `bytes_stream()` |
| **Vision** | Base64 image `ContentBlock::Image` in user messages |
| **Tool calling** | `ContentBlock::ToolUse` (call) + `ContentBlock::ToolResult` (response) |
| **Tool result images** | `supports_tool_result_images() = true` — images passed inline in tool results |
| **Extended thinking** | `enable_thinking` flag → `thinking_delta` events (budget_tokens: 10000) |
| **Prompt caching** | 3-tier: `cache_breakpoints` offsets, `static_system` fallback, or single-block |
| **Message caching** | Last 3 messages get `cache_control: "ephemeral"` on their last content block |
| **Max tokens** | Default: 8192; with thinking: 16384 |
| **Rate limits** | `anthropic-ratelimit-{requests,tokens}-remaining` headers |

### 3.3 Message Compilation

Anthropic has the most complex message building:

1. **System prompt caching** (3-tier):
   - **Primary:** `cache_breakpoints` (byte offsets into `system_prompt`) — splits prompt and marks prefix blocks with `cache_control: "ephemeral"`. The tail (dynamic suffix) gets no cache control.
   - **Fallback 1:** `static_system` prefix — splits at static/dynamic boundary. Static prefix is cached, dynamic suffix is not.
   - **Fallback 2:** Single block with `cache_control: "ephemeral"` (whole prompt cached).

2. **User messages**: `ContentBlock::Text` or `ContentBlock::Image` (vision). Tool results go as `ContentBlock::ToolResult` with `tool_use_id` and optional `is_error`. Tool results with `image_url` get multi-block content (text + image).

3. **Assistant messages**: `ContentBlock::Text` for text, `ContentBlock::ToolUse` for tool calls. Only tool calls that have corresponding results are included (orphan filtering).

4. **Conversation caching**: The last content block of the last 3 messages gets `cache_control: "ephemeral"`. The last tool definition also gets cache control.

### 3.4 SSE Event Handling

```
message_start      → Extract usage from response metadata
content_block_start → Track block type (text, tool_use, thinking)
content_block_delta → Emit Text/ToolCall/Thinking events
  - text_delta      → StreamEvent::text()
  - input_json_delta → Accumulate tool call JSON
  - thinking_delta   → StreamEvent::thinking() (if enable_thinking)
content_block_stop  → Emit accumulated tool call
message_delta       → Extract stop_reason, final usage
message_stop        → StreamEvent::done()
error               → StreamEvent::error()
```

### 3.5 Rate Limit Headers

```
anthropic-ratelimit-requests-remaining
anthropic-ratelimit-tokens-remaining
anthropic-ratelimit-requests-reset
```

Extracted from initial HTTP response headers before SSE body, emitted as `StreamEvent::rate_limit_info()`.

---

## 4. OpenAI Provider

**File:** `crates/ai/src/providers/openai.rs`

### 4.1 Configuration

```rust
pub struct OpenAIProvider {
    api_key: String,
    model: String,
    base_url: String,           // Customizable for compatible APIs
    provider_id: String,        // "openai", "janus", "deepseek"
    bot_id: Option<String>,     // X-Bot-ID header (Janus per-bot billing)
    lane: Option<String>,       // X-Lane header (Janus routing)
    http_client: RwLock<Client>, // For connection reset on GOAWAY
}
```

### 4.2 Triple-Duty Design

The OpenAI provider serves three different backends via configuration:

| Backend | base_url | provider_id | Extra Headers |
|---------|----------|-------------|---------------|
| **OpenAI** | `https://api.openai.com/v1` | `"openai"` | None |
| **Deepseek** | `https://api.deepseek.com/v1` | `"deepseek"` | None |
| **Janus** | `{janus_url}/v1` | `"janus"` | `X-Bot-ID`, `X-Lane` |

### 4.3 Raw reqwest Streaming

Uses raw `reqwest::Client` with `bytes_stream()` instead of `reqwest-eventsource` crate. Reason: `reqwest-eventsource` implements automatic SSE reconnection per W3C spec, causing infinite retries on 502 errors from Janus.

### 4.4 Four Janus Streaming Quirks

All four workarounds activate based on stream behavior (backward-compatible with standard OpenAI):

**Quirk 1 -- Tool Name Duplication**: Janus sends tool name in every chunk. Tracked via `HashSet<u32>` of seen indices; duplicates after first are ignored.

**Quirk 2 -- Complete JSON Arguments**: Janus sends complete JSON args in one chunk, then repeats. Detection: attempt `serde_json::from_str()` -- if valid JSON, mark as seen via `HashSet<u32>` and skip subsequent chunks. Falls through to standard incremental `push_str` accumulation for OpenAI.

**Quirk 3 -- Missing `[DONE]` Sentinel**: Janus may not send `data: [DONE]` after `finish_reason`. The stream breaks immediately on any non-empty `finish_reason` rather than waiting for the sentinel.

**Quirk 4 -- Non-Null Content for Gemini Backends**: When content is empty but tool_calls exist, content is set to `" "` (single space) to satisfy Gemini backend requirements behind Janus.

### 4.5 Tool Call Emission Strategy

Tool calls are accumulated in `HashMap<u32, AccumulatedToolCall>` keyed by tool index. They are emitted at stream END (after `finish_reason` or `[DONE]`), not inline during streaming. This ensures correct handling of both Janus (single-chunk) and OpenAI (incremental) tool call patterns.

Deduplication: `HashSet<String>` of emitted tool call IDs prevents double-emission.

### 4.6 Mid-Stream Error Handling

Detects `{"error": {"message": "...", "type": "...", "code": "..."}}` objects in the SSE stream (Janus error format), emits as `StreamEvent::error()`, and terminates the stream.

### 4.7 Connection Reset

Implements `ConnectionResetter` trait:

```rust
fn reset_connections(&self) {
    *self.http_client.write().unwrap() = reqwest::Client::new();
}
```

Called by the runner on GOAWAY or persistent connection errors.

### 4.8 Message Building

- Orphan tool calls (no matching response) are filtered out
- Tool call IDs are deduplicated across history
- Assistant messages with empty content but tool_calls get content `" "` (Gemini backend compat)
- Uses `async-openai` SDK types for request construction but raw reqwest for execution

### 4.9 Provider Metadata (Tool Stickiness)

Janus includes `provider_metadata` in SSE stream data. The OpenAI provider extracts this and attaches it to the `StreamEvent::Done` event via `provider_metadata` field. On the next request, it's echoed back via `ChatRequest.metadata` → serialized as `"metadata"` in the JSON body.

### 4.10 Rate Limit Headers

```
# Standard OpenAI:
x-ratelimit-remaining-requests
x-ratelimit-remaining-tokens
x-ratelimit-reset-requests
# Janus session window:
x-ratelimit-session-limit-tokens
x-ratelimit-session-remaining-tokens
x-ratelimit-session-reset              # ISO8601 timestamp
# Janus weekly window:
x-ratelimit-weekly-limit-tokens
x-ratelimit-weekly-remaining-tokens
x-ratelimit-weekly-reset               # ISO8601 timestamp
```

The `remaining_tokens` field in `RateLimitMeta` uses `session_remaining` if available (tighter constraint), otherwise falls back to standard `remaining_tokens`.

---

## 5. Gemini Provider

**File:** `crates/ai/src/providers/gemini.rs`

### 5.1 Configuration

```rust
pub struct GeminiProvider {
    api_key: String,
    model: String,  // Default: "gemini-2.0-flash"
}
```

### 5.2 Key Features

| Feature | Implementation |
|---------|---------------|
| **Streaming** | REST API with `?alt=sse` parameter |
| **Turn alternation** | STRICT -- user/model must alternate. Auto-merges consecutive same-role messages |
| **Tool calling** | Function Declarations with Gemini-specific schema format |
| **Tool IDs** | Sequential: `gemini-call-{counter}` (Gemini doesn't provide IDs) |

### 5.3 Schema Conversion

JSON Schema types are mapped to Gemini types:

```
string  → STRING
number  → NUMBER
integer → INTEGER
boolean → BOOLEAN
array   → ARRAY
object  → OBJECT
```

Nested properties and array items are converted recursively. Enum values and required fields are preserved.

### 5.4 Message Normalization

1. Ensures history starts with a user message (prepends `"Continue."` if needed)
2. Merges consecutive same-role messages by concatenating content with `\n\n`
3. Maps roles: `"assistant"` → `"model"`, `"tool"` → wrapped in `"user"` with functionResponse
4. Tool calls go as `functionCall` parts in model messages
5. Tool results go as `functionResponse` parts in user messages

### 5.5 Finish Reasons

- `STOP` → normal completion
- `MAX_TOKENS` → context overflow
- `SAFETY` → safety filter triggered

---

## 6. Ollama Provider

**File:** `crates/ai/src/providers/ollama.rs`

### 6.1 Configuration

```rust
pub struct OllamaProvider {
    client: Client,      // 5-minute timeout for local inference
    base_url: String,    // Default: "http://localhost:11434"
    model: String,       // Default: "qwen3:4b"
}
```

### 6.2 Key Features

| Feature | Implementation |
|---------|---------------|
| **Streaming** | NDJSON (newline-delimited JSON), NOT SSE |
| **Termination** | `done: true` field, NOT `[DONE]` sentinel |
| **Tool calling** | Complete tool call objects (not streamed incrementally) |
| **Tool IDs** | Synthetic: `ollama-call-{counter}` |
| **Auth** | None required (local) |

### 6.3 Utility Functions

```rust
/// Health check with 2-second timeout
pub async fn check_ollama_available(base_url: &str) -> bool;

/// List all locally available models
pub async fn list_ollama_models(base_url: &str) -> Result<Vec<String>>;

/// Pull model if not present (30-minute timeout for large models)
pub async fn ensure_ollama_model(base_url: &str, model: &str) -> Result<()>;
```

### 6.4 Message Building

- Standard role/content/tool_calls structure
- Tool calls go in assistant message as `tool_calls` array
- Tool results go in `"tool"` role messages with `content` field

---

## 7. Local Provider

**File:** `crates/ai/src/providers/local.rs`, `crates/ai/src/providers/local_ffi.rs`

### 7.1 Configuration

```rust
pub struct LocalProvider {
    model_path: String,     // Path to GGUF file
    model_name: String,
    mu: Mutex<()>,          // Serialize inference (llama.cpp is NOT thread-safe)
}
```

### 7.2 Feature Gate

The local provider is behind the `local-inference` Cargo feature flag. When disabled, `stream()` returns `ProviderError::Request("Local inference not available")`.

### 7.3 Tool Calling via Prompt Engineering

Since llama.cpp has no native function calling, tools are handled via prompt injection and XML extraction:

**Injection**: Tool definitions are injected into the system prompt describing each tool's name, description, and parameters.

**Extraction**: The model responds with `<tool_call>` XML blocks:

```xml
<tool_call>
{"name": "file", "arguments": {"action": "read", "path": "/tmp/test.txt"}}
</tool_call>
```

`extract_tool_calls()` parses these blocks and validates tool existence.

### 7.4 GGUF Inference (local_ffi.rs)

- Uses `llama-cpp-2` crate for FFI to llama.cpp C library
- Loads GGUF model files, tokenizes with BOS marker
- Runs on blocking thread (sync operation)
- Streams tokens via mpsc channel
- Mutex-serialized (one inference at a time)

---

## 8. CLI Provider

**File:** `crates/ai/src/providers/cli.rs`

### 8.1 Configuration

```rust
pub struct CLIProvider {
    name: String,           // "claude-code", "gemini-cli", "codex-cli"
    command: String,        // Binary name: "claude", "gemini", "codex"
    args: Vec<String>,      // CLI arguments (built by constructors)
}
```

### 8.2 Key Design Decision

`handles_tools() = true` -- CLI providers execute tools autonomously. The runner does NOT manage tool calls for CLI providers; the CLI tool itself handles them.

### 8.3 Claude Code Integration

```rust
CLIProvider::new_claude_code(max_turns: u32, server_port: u16) -> Self
```

- Connects to Nebo agent MCP server at `http://localhost:{server_port}/agent/mcp`
- Disables ALL built-in Claude Code tools (`--tools ""`)
- Only allows `mcp__nebo-agent__*` tools (Nebo's tool set)
- Configurable max turns via `--max-turns` (0 = unlimited)
- Effort levels: `"high"` (thinking enabled), `"low"` (disabled)
- Strict MCP config enabled (`--strict-mcp-config`)
- Output format: `--output-format stream-json --include-partial-messages`
- Cancellation: listens to `req.cancel_token`, kills child process group on cancel

### 8.4 Other CLI Wrappers

```rust
CLIProvider::new_gemini_cli() -> Self   // Wraps `gemini` binary, reads from stdin
CLIProvider::new_codex_cli() -> Self    // Wraps `codex` binary with --full-auto
```

### 8.5 Stream Processing

- Reads stdout line-by-line (stream-json format)
- Unwraps `stream_event` envelope if present
- Intercepts `content_block_start/delta/stop` events for tool tracking
- Accumulates tool input JSON via `input_json_delta`
- Emits `tool_call` events on `content_block_stop`
- Handles `text_delta` → StreamEvent::text(), `thinking_delta` → StreamEvent::thinking()
- Handles `result` type: subtype "success" or "error_max_turns" → done
- Handles `error` type → StreamEvent::error()
- Process group setup on Unix for clean shutdown (SIGTERM → SIGKILL)
- Windows: `CREATE_NO_WINDOW` flag to suppress console flash

### 8.6 Prompt Building

- Merges consecutive same-role messages
- Prefixes with `[User]`, `[Assistant]`, `[System]`
- Inlines tool results as `"[Tool Result: {id}]\n{content}"`

---

## 9. Embedding System

**File:** `crates/ai/src/embedding.rs`

### 9.1 EmbeddingProvider Trait

```rust
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn id(&self) -> &str;
    fn dimensions(&self) -> usize;
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ProviderError>;
}
```

### 9.2 Implementations

**OpenAIEmbeddingProvider:**
- Default model: `text-embedding-3-small` (1536 dimensions)
- Custom base URL support (Janus-routed embeddings)
- Configurable model and dimensions
- 3-retry pattern: 500ms → 2s → 8s delays
- Auth error detection (401/403 -- no retry)

**OllamaEmbeddingProvider:**
- Local embeddings via Ollama API (`POST /api/embed`)
- Configurable model and dimensions
- Same 3-retry pattern

### 9.3 CachedEmbeddingProvider

Wraps any `EmbeddingProvider` with SHA256-keyed database caching:

```rust
pub struct CachedEmbeddingProvider {
    inner: Box<dyn EmbeddingProvider>,
    store: Arc<db::Store>,
}
```

- Cache key: `SHA256(text)` → lookup in `embedding_cache` table
- Avoids re-embedding identical inputs
- Stores vectors as byte arrays with dimension metadata

### 9.4 Embedding Priority

```
1. Janus (centralized) → OpenAIEmbeddingProvider with janus base_url
2. OpenAI (direct)     → OpenAIEmbeddingProvider with default base_url
3. Ollama (local)      → OllamaEmbeddingProvider
```

### 9.5 Utility Functions

```rust
/// Convert f32 vector to bytes for DB storage
pub fn f32_to_bytes(vec: &[f32]) -> Vec<u8>;

/// Convert bytes back to f32 vector
pub fn bytes_to_f32(bytes: &[u8]) -> Vec<f32>;
```

---

## 10. SSE Parser

**File:** `crates/ai/src/sse.rs`

Shared by Anthropic, OpenAI, and Gemini providers:

```rust
pub enum SseEvent {
    Data(String),      // "data: {...}"  → parsed JSON payload
    Done,              // "data: [DONE]" → stream termination sentinel
    Event(String),     // "event: ..."   → event type (Anthropic)
    Skip,              // Empty lines, comments
}

pub fn parse_sse_line(line: &str) -> SseEvent;
```

---

## 11. Model Selection and Task Routing

**File:** `crates/agent/src/selector.rs`

### 11.1 ModelSelector

```rust
pub struct ModelSelector {
    config: ModelRoutingConfig,
    cooldowns: RwLock<HashMap<String, CooldownState>>,  // Exponential backoff
    excluded: RwLock<HashMap<String, bool>>,             // Failed models
    fuzzy: RwLock<Option<FuzzyMatcher>>,                 // Alias resolution
    loaded_providers: RwLock<Vec<String>>,                // Active provider IDs
}
```

### 11.2 Task Classification

Messages are classified by keyword detection on the last user message:

| Task Type | Keywords |
|-----------|----------|
| **Vision** | `data:image/`, `"type":"image"` references |
| **Audio** | `data:audio/`, `"type":"audio"` references |
| **Reasoning** | think through, analyze, prove, step by step, mathematical proof, logical reasoning, derive, theorem, hypothesis, contradict, paradox, evaluate the, compare and contrast, pros and cons, trade-offs, implications |
| **Code** | code, function, implement, refactor, debug, python, javascript, typescript, react, rust, golang, java, swift, kotlin, sql, api, endpoint, database, algorithm, compile, syntax, variable, class |
| **General** | Default (no keyword match) |

### 11.3 Selection Algorithm (`select_for_task()`)

```
1. Classify task from user message keywords
2. Look up task_routing[task_type] (e.g., "anthropic/claude-opus-4-6" for reasoning)
3. Check usability:
   a. Provider is loaded (has a running Provider instance)
   b. Model not in exclusion list
   c. Model not in cooldown (exponential backoff)
4. If not usable, try fallbacks for that task type
5. Fall back to task_routing["general"]
6. Fall back to defaults.primary
7. Any active non-gateway model from loaded providers
8. If CLI provider is loaded → return empty string (runner defers to index 0 = CLI)
9. True last resort: any gateway (Janus) model
10. Final fallback: default_model regardless of usability
```

### 11.4 Fuzzy Resolution

User-friendly aliases resolve to full model IDs (examples from current catalog):

```
"sonnet"   → "anthropic/claude-sonnet-4-6"
"opus"     → "anthropic/claude-opus-4-6"
"haiku"    → "anthropic/claude-haiku-4-5-20251001"
"gpt"      → "openai/gpt-5.4"
```

Resolution uses `FuzzyMatcher` (in `crates/agent/src/fuzzy.rs`) built from `ModelsConfig.providers` and user-defined aliases.

### 11.5 Failure Tracking

```rust
/// Mark a model as failed with exponential backoff
pub fn mark_failed(&self, model_id: &str);
// Backoff: 5s → 10s → 20s → 40s → ... → 1 hour max

/// Check remaining cooldown duration (returns Duration::ZERO if not in cooldown)
pub fn get_cooldown_remaining(&self, model_id: &str) -> Duration;

/// Clear all failures and cooldowns
pub fn clear_failed(&self);

/// Rebuild fuzzy matcher (e.g., after provider reload)
pub fn rebuild_fuzzy(&self, user_aliases: &HashMap<String, String>);
```

### 11.6 Lane Routing

Different execution lanes can use different models:

```yaml
lane_routing:
  heartbeat: "anthropic/claude-haiku-4-5"
  events: "openai/gpt-5-nano"
  comm: "janus"
  subagent: "janus"
```

Lane names are defined in `crates/types/src/constants.rs`: `main`, `events`, `subagent`, `heartbeat`, `comm`.

---

## 12. Configuration Layer

**File:** `crates/config/src/models.rs`, `crates/config/src/models.yaml`

### 12.1 ModelsConfig

```rust
pub struct ModelsConfig {
    pub version: String,                                   // "1.0"
    pub providers: HashMap<String, Vec<ModelDef>>,         // Provider → models
    pub defaults: Option<Defaults>,                        // Primary + fallbacks
    pub task_routing: Option<TaskRouting>,                  // Task → model mapping
    pub lane_routing: Option<LaneRouting>,                  // Lane → model mapping
    pub aliases: Vec<ModelAlias>,                           // User-friendly name mappings
    pub cli_providers: Vec<CliProviderDef>,                 // CLI tool definitions
}
```

### 12.2 ModelDef

```rust
pub struct ModelDef {
    pub id: String,                     // "claude-sonnet-4-6"
    pub display_name: String,           // "Claude Sonnet 4.6"
    pub context_window: i64,            // 1000000
    pub capabilities: Vec<String>,      // ["vision", "tools", "streaming", "code", "reasoning", "thinking"]
    pub kind: Vec<String>,              // ["smart", "fast", "code"]
    pub preferred: bool,                // User preference flag
    pub pricing: Option<ModelPricing>,  // input/output/cachedInput per million tokens
    pub active: Option<bool>,           // Active by default (defaults to true if None)
}

pub struct ModelPricing {
    pub input: f64,         // Per million tokens
    pub output: f64,
    pub cached_input: f64,
}

pub struct ModelAlias {
    pub alias: String,      // e.g., "sonnet"
    pub model_id: String,   // e.g., "claude-sonnet-4-6"
}
```

### 12.3 Defaults and Routing

```rust
pub struct Defaults {
    pub primary: String,            // "anthropic/claude-sonnet-4-6"
    pub fallbacks: Vec<String>,     // Ordered fallback chain
}

pub struct TaskRouting {
    pub vision: String,
    pub audio: String,
    pub reasoning: String,
    pub code: String,
    pub general: String,
    pub fallbacks: HashMap<String, Vec<String>>,  // Per-task fallbacks
}

pub struct LaneRouting {
    pub heartbeat: String,
    pub events: String,
    pub comm: String,
    pub subagent: String,
}
```

### 12.4 Embedded models.yaml

The default model catalog is embedded in the binary at `crates/config/src/models.yaml`. Users can override with a `models.yaml` in their data directory (`~/Library/Application Support/Nebo/models.yaml` on macOS).

**Configured providers:**

| Provider | Models |
|----------|--------|
| `anthropic` | Opus 4.6 (1M ctx), Sonnet 4.6 (1M ctx), Haiku 4.5 (200K ctx) |
| `openai` | GPT-5.4 (400K ctx), GPT-5.4 Mini (400K ctx), GPT-5.4 Nano (128K ctx), Codex (256K ctx) |
| `google` | Gemini 3.1 Pro (1M ctx), Gemini 3 Flash (1M ctx), Gemini 2.5 Flash (1M ctx) |
| `deepseek` | DeepSeek Chat (128K ctx), DeepSeek Reasoner (128K ctx) |
| `janus` | Nebo 1 (200K ctx, server-side routing), Nebo Embeddings Small/Large (8K ctx) |
| `ollama` | Auto-discovered at runtime (not listed in YAML) |

**Default task routing:**

```yaml
defaults:
  primary: anthropic/claude-sonnet-4-6
  fallbacks: [anthropic/claude-haiku-4-5-20251001]

task_routing:
  vision: anthropic/claude-sonnet-4-6
  audio: openai/gpt-5.4
  reasoning: anthropic/claude-opus-4-6
  code: anthropic/claude-sonnet-4-6
  general: anthropic/claude-sonnet-4-6
```

### 12.5 CLI Provider Definitions

```rust
pub struct CliProviderDef {
    pub id: String,              // "claude-code", "codex-cli", "gemini-cli"
    pub display_name: String,    // "Claude Agent", "OpenAI Codex CLI", "Gemini CLI"
    pub command: String,         // Binary name: "claude", "codex", "gemini"
    pub install_hint: String,    // e.g., "brew install claude-code"
    pub models: Vec<String>,     // Available models for this CLI
    pub default_model: String,   // Default model selection
    pub active: Option<bool>,    // Active (defaults to false if None)
}
```

### 12.6 Data Directory

```rust
// crates/config/src/defaults.rs
pub fn data_dir() -> PathBuf {
    // macOS:   ~/Library/Application Support/Nebo/
    // Windows: %AppData%\Nebo\
    // Linux:   ~/.config/nebo/
    // Override: NEBO_DATA_DIR environment variable
}
```

Bot ID: `<data_dir>/bot_id` -- immutable UUID generated on first startup, used for Janus `X-Bot-ID` header.

---

## 13. Database Layer

**Files:** `crates/db/migrations/0010_auth_profiles.sql`, `crates/db/migrations/0047_model_catalog.sql`, `crates/db/src/queries/auth_profiles.rs`, `crates/db/src/queries/provider_models.rs`

### 13.1 auth_profiles Table

```sql
CREATE TABLE auth_profiles (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    provider TEXT NOT NULL,           -- "anthropic", "openai", "google", "deepseek", "ollama", "neboloop"
    api_key TEXT NOT NULL,            -- Encrypted
    model TEXT,                       -- Model override for this profile
    base_url TEXT,                    -- Custom API base URL (Ollama, Deepseek)
    priority INTEGER DEFAULT 0,       -- Higher = preferred
    is_active INTEGER DEFAULT 1,
    cooldown_until INTEGER,           -- Unix timestamp for rate limit backoff
    last_used_at INTEGER,
    usage_count INTEGER DEFAULT 0,
    error_count INTEGER DEFAULT 0,    -- Consecutive errors
    metadata TEXT,                    -- JSON: {"janus_provider": "true", "org_id": "..."}
    auth_type TEXT,                   -- "token", "oauth", "local"
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX idx_auth_profiles_provider ON auth_profiles(provider, is_active);
CREATE INDEX idx_auth_profiles_priority ON auth_profiles(provider, priority DESC, is_active);
```

### 13.2 Key Auth Profile Queries

```rust
/// Get the best available profile for a provider
/// Sort: oauth > token > local, priority DESC, last_used ASC
/// Excludes: is_active=0, cooldown_until > now()
pub fn get_best_auth_profile(provider: &str) -> Option<AuthProfile>;

/// List all active profiles for a provider, respecting cooldowns
pub fn list_active_auth_profiles_by_provider(provider: &str) -> Vec<AuthProfile>;

/// Update usage tracking after successful request
pub fn update_auth_profile_usage(id: &str);  // Increments usage_count, resets error_count

/// Update error tracking after failed request
pub fn update_auth_profile_error(id: &str, cooldown: Option<i64>);  // Increments error_count

/// Set rate limit cooldown
pub fn set_auth_profile_cooldown(id: &str, cooldown_until: i64);

/// Toggle active state
pub fn toggle_auth_profile(id: &str, active: bool);
```

### 13.3 provider_models Table

```sql
CREATE TABLE provider_models (
    id TEXT PRIMARY KEY,              -- Composite: "anthropic/claude-sonnet-4-5"
    provider TEXT NOT NULL,
    model_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    is_active INTEGER DEFAULT 1,      -- User can disable models
    is_default INTEGER DEFAULT 0,     -- One per provider
    context_window INTEGER,
    input_price REAL,                 -- Per million tokens
    output_price REAL,
    capabilities TEXT,                -- JSON: ["vision", "tools", "streaming"]
    kind TEXT,                        -- JSON: ["fast", "smart", "code"]
    preferred INTEGER DEFAULT 0,      -- User preference
    seeded_version TEXT,              -- App version that seeded this row
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(provider, model_id)
);
```

### 13.4 Model Seeding

`seed_models_from_catalog()` runs at startup:
1. Reads all models from embedded `models.yaml`
2. Upserts into `provider_models` table
3. Preserves user's `is_active`, `is_default`, `preferred` choices
4. Marks old catalog models (not in current yaml) as inactive
5. Tracks `seeded_version` to avoid re-seeding

---

## 14. Provider Lifecycle and Hot Reload

**Files:** `crates/server/src/lib.rs`, `crates/server/src/handlers/provider.rs`, `crates/agent/src/runner.rs`

### 14.1 Initialization (Server Startup)

```
1. Load auth_profiles from DB
2. Load models.yaml (embedded + user override, merge missing sections)
3. seed_models_from_catalog() → upsert provider_models table
4. reload_providers() → create Arc<dyn Provider> instances:
   - anthropic → AnthropicProvider
   - openai    → OpenAIProvider
   - deepseek  → OpenAIProvider(base_url=deepseek, provider_id="deepseek")
   - google    → GeminiProvider
   - ollama    → OllamaProvider
   - neboloop + metadata.janus_provider="true"
              → OpenAIProvider(base_url=janus, provider_id="janus", bot_id)
5. Live-detect CLI providers (PATH scan for claude, codex, gemini)
6. Order: [direct API providers] + [CLI providers] + [Janus gateway]
7. Create ModelSelector with routing config
8. Create Runner with providers + selector
```

### 14.2 reload_providers() Provider Mapping

```rust
match profile.provider.as_str() {
    "anthropic" => AnthropicProvider::new(api_key, model),
    "openai"    => OpenAIProvider::new(api_key, model),
    "deepseek"  => {
        let mut p = OpenAIProvider::with_base_url(api_key, model, base_url_or_default);
        p.set_provider_id("deepseek");
        p
    },
    "google"    => GeminiProvider::new(api_key, model),
    "ollama"    => OllamaProvider::new(base_url, model),
    "neboloop"  => {
        // Only if metadata contains janus_provider: "true"
        // Skip if no active chat-capable Janus models in catalog
        let api_key = if profile.api_key.is_empty() { bot_id.clone() } else { profile.api_key };
        let mut p = OpenAIProvider::with_base_url(api_key, model, format!("{}/v1", janus_url));
        p.set_provider_id("janus");
        p.set_bot_id(bot_id);
        p  // → gateway_providers (appended LAST)
    },
}
// CLI providers: live-detected via config::detect_all_clis()
// Final order: [direct API] + [CLI] + [Janus]
```

### 14.3 Hot Reload on Auth Profile Changes

```
1. User creates/updates/deletes auth profile via REST API
2. Handler updates database
3. Handler calls reload_providers(state: &AppState):
   a. Re-reads auth_profiles from DB (list_auth_profiles)
   b. Builds providers from profiles (match on provider type)
   c. Live-detects CLI providers (detect_all_clis)
   d. Orders: direct API → CLI → Janus
   e. Calls state.runner.reload_providers(providers).await:
      - Swaps providers: *self.providers.write().await = providers
      - Updates selector: self.selector.set_loaded_providers(provider_ids)
      - Rebuilds fuzzy matcher: self.selector.rebuild_fuzzy(&HashMap::new())
```

**Note:** `clear_failed()` is NOT called during reload — cooldowns persist across reloads.

### 14.4 Runner Provider Management

```rust
pub struct Runner {
    providers: Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    selector: Arc<ModelSelector>,
    // ...
}

impl Runner {
    pub async fn reload_providers(&self, providers: Vec<Arc<dyn Provider>>) {
        let loaded_ids: Vec<String> = providers.iter().map(|p| p.id().to_string()).collect();
        *self.providers.write().await = providers;
        self.selector.set_loaded_providers(loaded_ids);
        self.selector.rebuild_fuzzy(&HashMap::new());
    }
}
```

---

## 15. Error Handling and Retry Logic

**Files:** `crates/ai/src/types.rs`, `crates/agent/src/runner.rs`

### 15.1 Error Classification Functions

```rust
/// Classify error for cooldown/retry behavior
pub fn classify_error_reason(err: &ProviderError) -> &str {
    // "rate_limit"        → 429 or rate limit messages
    // "auth"              → 401/403 or API key errors
    // "billing"           → Billing/quota exceeded
    // "timeout"           → Request timeout
    // "provider"          → Upstream provider error
    // "context_overflow"  → Context window exceeded
    // "other"             → Generic errors
}

/// Check if error is a network-level transient error (safe to retry)
pub fn is_transient_error(err: &ProviderError) -> bool {
    // Only matches Stream/Request variants. Keywords:
    // stream error, connection reset, connection refused, broken pipe,
    // eof, tls handshake, timeout, no such host
}

/// Check if context window was exceeded
pub fn is_context_overflow(err: &ProviderError) -> bool {
    // "context_length_exceeded" or "context exceeded"
}

/// Check if message role ordering is invalid
pub fn is_role_ordering_error(err: &ProviderError) -> bool {
    // "roles must alternate", "incorrect role information",
    // "expected alternating", "must be followed by"
}
```

### 15.2 Runner Retry Constants

```rust
const MAX_TRANSIENT_RETRIES: usize = 10;   // Network errors (connection reset, timeout, DNS)
const MAX_RETRYABLE_RETRIES: usize = 5;    // Rate limit, auth, billing, 429
const MAX_AUTO_CONTINUATIONS: usize = 3;   // Agent stops mid-response
```

### 15.3 Retry Strategy

```
On each provider.stream() failure:
  1. Classify error (classify_error_reason)
  2. If transient (is_transient_error):
     - Retry up to 10 times
     - If ConnectionResetter, reset connections
  3. If retryable (rate_limit, auth):
     - Retry up to 5 times
     - Update auth profile error count + cooldown
  4. If context_overflow:
     - Prune messages (remove oldest non-system messages)
     - Retry
  5. If fatal:
     - Return error to user
```

### 15.4 Auth Profile Error Tracking

After each failed request:

```rust
// Increment error count, optionally set cooldown
db.update_auth_profile_error(profile_id, Some(cooldown_until));

// On successful request, reset error count
db.update_auth_profile_usage(profile_id);
```

### 15.5 Model Selector Cooldowns

Exponential backoff per model in `ModelSelector`:

```
1st failure:  5 seconds
2nd failure:  10 seconds
3rd failure:  20 seconds
4th failure:  40 seconds
...
Maximum:      1 hour
```

Cooldowns are NOT automatically cleared on `reload_providers()` — they persist until they expire or `clear_failed()` is called explicitly.

---

## 16. Server HTTP API

**File:** `crates/server/src/handlers/provider.rs`

### 16.1 Provider CRUD

| Method | Endpoint | Purpose |
|--------|----------|---------|
| `GET` | `/api/v1/providers` | List all auth profiles |
| `POST` | `/api/v1/providers` | Create new auth profile → reload |
| `GET` | `/api/v1/providers/:id` | Get single profile |
| `PUT` | `/api/v1/providers/:id` | Update profile → reload |
| `DELETE` | `/api/v1/providers/:id` | Delete profile → reload |
| `POST` | `/api/v1/providers/:id/test` | Test connection (minimal "Say OK" request, 15s timeout) |

**Create/Update body:**

```json
{
  "name": "My Claude",
  "provider": "anthropic",
  "apiKey": "sk-...",
  "model": "claude-opus-4-6",
  "baseUrl": null,
  "priority": 50,
  "authType": "token",
  "metadata": {}
}
```

### 16.2 Models API

| Method | Endpoint | Purpose |
|--------|----------|---------|
| `GET` | `/api/v1/models` | Model catalog + routing config + CLI statuses |
| `PUT` | `/api/v1/models/{provider}/{modelId}` | Toggle model active/preferred (Janus cascade) |
| `PUT` | `/api/v1/models/config` | Update defaults (primary + fallbacks) |
| `PUT` | `/api/v1/models/task-routing` | Update task routing + lane routing |
| `PUT` | `/api/v1/models/cli/{cliId}` | Enable/disable CLI provider |
| `GET` | `/api/v1/local-models/status` | Ollama availability + installed models |

**Janus cascade rule:** When the last chat-capable Janus model is disabled, ALL Janus models (including embeddings) are auto-disabled. On `reload_providers()`, Janus provider is skipped if no active chat models remain.

**GET /api/v1/models response:**

```json
{
  "models": {
    "anthropic": [{ "id": "claude-opus-4-6", "displayName": "Claude Opus 4.6", ... }],
    "openai": [...]
  },
  "taskRouting": {
    "vision": "anthropic/claude-sonnet-4-6",
    "reasoning": "anthropic/claude-opus-4-6",
    "code": "anthropic/claude-sonnet-4-6",
    "general": "anthropic/claude-sonnet-4-6",
    "audio": "openai/gpt-5.4",
    "fallbacks": { "vision": ["anthropic/claude-opus-4-6", "openai/gpt-5.4"], ... }
  },
  "laneRouting": { "heartbeat": "janus", "events": "janus" },
  "aliases": [{ "alias": "sonnet", "modelId": "claude-sonnet-4-6" }],
  "availableCLIs": { "claude": true, "codex": false, "gemini": false },
  "cliStatuses": {
    "claude": { "installed": true, "authenticated": true, "version": "..." },
    "codex": { "installed": false, "authenticated": false, "version": null },
    "gemini": { "installed": false, "authenticated": false, "version": null }
  },
  "cliProviders": [
    { "id": "claude-code", "displayName": "Claude Agent", "command": "claude",
      "installHint": "brew install claude-code", "models": ["opus","sonnet","haiku"],
      "defaultModel": "opus", "active": false }
  ]
}
```

---

## 17. Key Files Reference

### Core Provider System

| File | Purpose |
|------|---------|
| `crates/ai/src/types.rs` | Provider trait, ChatRequest, StreamEvent, ProviderError, UsageInfo, RateLimitMeta |
| `crates/ai/src/sse.rs` | SSE line parser (shared by Anthropic, OpenAI, Gemini) |
| `crates/ai/src/lib.rs` | Public API re-exports |
| `crates/ai/Cargo.toml` | Dependencies: reqwest, async-openai, serde, tokio, llama-cpp-2 (optional) |

### Provider Implementations

| File | Provider | Transport |
|------|----------|-----------|
| `crates/ai/src/providers/anthropic.rs` | Anthropic (Claude) | SSE |
| `crates/ai/src/providers/openai.rs` | OpenAI + Janus + Deepseek | SSE (raw reqwest) |
| `crates/ai/src/providers/gemini.rs` | Google Gemini | SSE |
| `crates/ai/src/providers/ollama.rs` | Ollama (local) | NDJSON |
| `crates/ai/src/providers/local.rs` | GGUF (llama.cpp) | Direct FFI |
| `crates/ai/src/providers/local_ffi.rs` | llama.cpp FFI bindings | Direct |
| `crates/ai/src/providers/cli.rs` | Claude Code / Gemini CLI / Codex | stdio JSON lines |

### Embedding System

| File | Purpose |
|------|---------|
| `crates/ai/src/embedding.rs` | EmbeddingProvider trait, OpenAI/Ollama/Cached implementations |

### Model Selection

| File | Purpose |
|------|---------|
| `crates/agent/src/selector.rs` | ModelSelector: task classification, routing, cooldowns |
| `crates/agent/src/fuzzy.rs` | FuzzyMatcher: alias resolution ("sonnet" → full model ID) |
| `crates/agent/src/runner.rs` | Runner: provider management, retry loop, auto-continuation |

### Configuration

| File | Purpose |
|------|---------|
| `crates/config/src/models.rs` | ModelsConfig, ModelDef, TaskRouting, LaneRouting, CliProviderDef |
| `crates/config/src/models.yaml` | Embedded model catalog (all providers + routing defaults) |
| `crates/config/src/defaults.rs` | Data directory paths, bot ID management |

### Database

| File | Purpose |
|------|---------|
| `crates/db/migrations/0010_auth_profiles.sql` | Auth profiles schema |
| `crates/db/migrations/0047_model_catalog.sql` | Provider models catalog schema |
| `crates/db/src/queries/auth_profiles.rs` | Profile CRUD, best profile selection, cooldown management |
| `crates/db/src/queries/provider_models.rs` | Model catalog queries, seeding, activation |

### Server Integration

| File | Purpose |
|------|---------|
| `crates/server/src/lib.rs` | build_providers(), seed_models_from_catalog() |
| `crates/server/src/handlers/provider.rs` | REST API, reload_providers(), test_provider_connection() |

### Documentation

| File | Purpose |
|------|---------|
| `docs/sme/janus-and-providers.md` | Janus gateway migration guide (Go → Rust), local inference |
| `docs/sme/provider-system.md` | This document -- full Rust provider system reference |

---

*Generated: 2026-03-06 | Validated: 2026-04-02*
