# Janus Gateway and Provider Architecture

**Source:** `nebo/docs/sme/JANUS_GATEWAY.md`, `nebo/docs/sme/LOCAL_INFERENCE.md` | **Target:** `nebo-rs/crates/ai/`, `nebo-rs/crates/server/`, `nebo-rs/crates/config/` | **Status:** Draft

This document consolidates the Go Janus Gateway and Local Inference SME references into a single Rust migration guide. It covers the managed AI gateway, local model inference, provider loading, and streaming quirks -- everything needed to reimplement or extend the provider stack in nebo-rs without referencing Go source.

---

## Table of Contents

1. [Janus Gateway Architecture](#1-janus-gateway-architecture)
2. [Activation Flow](#2-activation-flow)
3. [Request/Response Headers](#3-requestresponse-headers)
4. [Four Streaming Quirks](#4-four-streaming-quirks)
5. [Steering Integration](#5-steering-integration)
6. [Local Inference Architecture](#6-local-inference-architecture)
7. [Model Download Pipeline](#7-model-download-pipeline)
8. [Tool Calling via Prompt Engineering](#8-tool-calling-via-prompt-engineering)
9. [Provider Priority Order](#9-provider-priority-order)
10. [Rust Implementation Status](#10-rust-implementation-status)

---

## 1. Janus Gateway Architecture

**File(s):** `crates/ai/src/providers/openai.rs`, `crates/server/src/lib.rs`, `crates/server/src/handlers/provider.rs`

### 1.1 What Janus Is

Janus is NeboLoop's managed AI gateway -- an OpenAI-compatible HTTP proxy at `https://janus.neboloop.com` that routes requests to upstream LLM providers (Anthropic, OpenAI, Google Gemini, etc.) so non-technical users never need to obtain or manage API keys.

Nebo's ICP (realtors, lawyers, freelancers) will NOT get an Anthropic API key. Janus is the "one click, no API keys" onramp: user signs up for NeboLoop, opts in to Janus, done.

**Zero public documentation exists for Janus.** Everything in this document is derived from the Nebo and nebo-rs codebases.

### 1.2 Network Topology

```
Nebo (desktop / headless)
  +-- OpenAI SDK types ---> https://janus.neboloop.com/v1
                              |
                              +-- Routes to Anthropic
                              +-- Routes to OpenAI
                              +-- Routes to Gemini
                              +-- (model selection is server-side -- Nebo sends model="janus")
```

Nebo creates a standard `OpenAIProvider` with `provider_id="janus"` and base URL `janus.neboloop.com/v1`. The request carries **no model routing hint** -- Janus decides routing internally based on the model name "janus".

### 1.3 Why OpenAI-Compatible

Janus exposes the OpenAI chat completions API (`POST /v1/chat/completions`). This means:

- Nebo reuses the EXACT same `OpenAIProvider` implementation for Janus and direct OpenAI.
- The only differences are: (a) base URL, (b) auth token is a NeboLoop JWT instead of an OpenAI API key, (c) extra headers (`X-Bot-ID`, `X-Lane`), (d) streaming quirks documented in section 4.

### 1.4 Configuration

The Janus URL is NOT stored in auth profiles. It comes from the server configuration:

```yaml
# etc/nebo.yaml
NeboLoop:
  JanusURL: https://janus.neboloop.com
```

Overridable via the `NEBOLOOP_JANUS_URL` environment variable.

### 1.5 Models

| ID | Purpose | Context | Capabilities |
|----|---------|---------|--------------|
| `janus` | Chat completion (server-side routing) | 200k | vision, tools, streaming, code, reasoning |
| `text-embedding-small` | Embeddings for memory/search | 8,191 | embeddings |
| `text-embedding-large` | Higher-quality embeddings | 8,191 | embeddings |

All Janus models are `active: true` by default. Other providers ship as `active: false` until the user adds API keys.

### 1.6 Go File Map (Reference)

| Layer | Go File | Purpose |
|-------|---------|---------|
| Config | `internal/config/config.go:104-105` | Default URL: `https://janus.neboloop.com` |
| URL bootstrap | `cmd/nebo/root.go:166` | `SetJanusURL()` at startup |
| Provider loading | `cmd/nebo/providers.go:262-304` | Creates `OpenAIProvider` with Janus base URL |
| Wire protocol | `internal/agent/ai/api_openai.go` | OpenAI SDK with Janus-specific middleware |
| Rate limit capture | `api_openai.go:62-103` | HTTP middleware parses 6 `X-RateLimit-*` headers |
| In-memory store | `internal/svc/servicecontext.go:85` | `JanusUsage atomic.Pointer[ai.RateLimitInfo]` |
| Usage API | `internal/handler/neboloop/handlers.go:227-259` | `GET /api/v1/neboloop/janus/usage` |
| Steering | `internal/agent/steering/generators.go:250-288` | Quota warning at >80% usage |

### 1.7 Rust Provider Construction

In Rust, the Janus provider is constructed identically to a direct OpenAI provider, with three customizations:

```rust
// crates/server/src/lib.rs -- initial load
// crates/server/src/handlers/provider.rs -- hot reload

let janus_url = &cfg.neboloop.janus_url;
let model = profile.model.clone().unwrap_or_else(|| "janus".into());
let bot_id = config::read_bot_id().unwrap_or_default();

let mut p = ai::OpenAIProvider::with_base_url(
    profile.api_key.clone(),       // NeboLoop JWT
    model,                         // "janus"
    format!("{}/v1", janus_url),   // "https://janus.neboloop.com/v1"
);
p.set_provider_id("janus");
if !bot_id.is_empty() {
    p.set_bot_id(bot_id);
}
```

The provider ID is set to `"janus"` (NOT `"openai"`) so that:
- Lane headers (`X-Lane`) are only sent for Janus requests.
- Streaming quirk workarounds activate correctly.
- The frontend can distinguish Janus from direct OpenAI in the providers list.

### 1.8 Embedding via Janus

The `OpenAIEmbeddingProvider` supports custom base URLs, enabling Janus-routed embeddings:

```rust
// crates/ai/src/embedding.rs
let embed_provider = OpenAIEmbeddingProvider::with_base_url(
    jwt.clone(),
    format!("{}/v1", janus_url),
).with_model("text-embedding-small".into(), 1536);
```

Embedding provider priority in Go: **Janus (centralized) -> OpenAI (direct) -> Ollama (local)**.

### 1.9 GatewayService Proto (App Platform)

The Go codebase defines a generalized gRPC interface for gateway apps in `proto/apps/v0/gateway.proto`:

```protobuf
service GatewayService {
  rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);
  rpc Stream(GatewayRequest) returns (stream GatewayEvent);
  rpc Poll(PollRequest) returns (PollResponse);
  rpc Cancel(CancelRequest) returns (CancelResponse);
  rpc Configure(SettingsMap) returns (Empty);
}
```

- `GatewayRequest`: messages, tools, max_tokens, temperature, system, user context. **No model field.**
- `GatewayEvent` types: `text`, `tool_call`, `thinking`, `error`, `done`
- Current Janus uses OpenAI REST directly; this proto is for future app-platform gateway apps.

This proto is NOT yet implemented in Rust.

---

## 2. Activation Flow

**File(s):** `crates/server/src/handlers/neboloop.rs`

### 2.1 OAuth Opt-In

Janus activation is tied to the NeboLoop OAuth flow. The user does NOT configure Janus separately -- it piggybacks on the NeboLoop sign-up.

**Step-by-step flow:**

1. Frontend initiates OAuth with `?janus=true` query parameter.
2. `oauth_start()` handler reads the parameter and stores `janus_provider: true` in the in-memory `OAuthFlowState`.
3. OAuth completes via the callback handler.
4. The callback stores an `auth_profiles` row with `metadata = {"janus_provider": "true"}`.
5. On provider load, the neboloop profile is found, metadata checked for `janus_provider=true`.
6. Creates `OpenAIProvider` with Janus base URL, `provider_id="janus"`, and `X-Bot-ID`.

### 2.2 Rust OAuth Flow State

```rust
// crates/server/src/handlers/neboloop.rs
struct OAuthFlowState {
    code_verifier: String,
    created_at: Instant,
    completed: bool,
    error: String,
    email: String,
    display_name: String,
    janus_provider: bool,        // <-- Janus opt-in flag
}
```

The `janus` query parameter is parsed from `OAuthStartParams`:

```rust
#[derive(serde::Deserialize)]
pub struct OAuthStartParams {
    pub janus: Option<String>,
}

// In oauth_start():
let janus_provider = params.janus.as_deref() == Some("true");
```

### 2.3 Carry-Forward Safety

Re-authentication MUST preserve `janus_provider=true` from the existing profile so the opt-in is never lost. In Go this is handled by `storeNeboLoopProfile()` which reads existing metadata before writing. The Rust implementation must replicate this behavior in the callback handler.

### 2.4 Account Status API

```
GET /api/v1/neboloop/account/status
```

Response includes `janusProvider: bool` so the frontend knows whether to show Janus-specific UI (usage bars, toggle).

### 2.5 Bot ID

The `X-Bot-ID` header value comes from `plugin_settings` in Go. In Rust, it is read via `config::read_bot_id()` which reads from the data directory. This is an immutable UUID generated on first startup, used for per-bot billing on the Janus side.

---

## 3. Request/Response Headers

**File(s):** `crates/ai/src/providers/openai.rs`

### 3.1 Request Headers (Nebo -> Janus)

| Header | Value | Purpose |
|--------|-------|---------|
| `Authorization` | `Bearer <NeboLoop JWT>` | Auth (via OpenAI SDK wire format) |
| `X-Bot-ID` | UUID | Per-bot billing (immutable, from config) |
| `X-Lane` | `main`/`events`/`subagent`/`heartbeat`/`comm` | Request routing/analytics |

**CRITICAL:** `X-Lane` is ONLY sent when `provider_id == "janus"`. Sending lane information to OpenAI or other providers would be meaningless and could leak internal architecture details.

### 3.2 Rust Header Construction

```rust
// crates/ai/src/providers/openai.rs -- stream() method
let mut headers = reqwest::header::HeaderMap::new();
headers.insert(
    reqwest::header::AUTHORIZATION,
    format!("Bearer {}", self.api_key).parse().expect("valid auth header"),
);
if let Some(ref bot_id) = self.bot_id {
    headers.insert(
        reqwest::header::HeaderName::from_static("x-bot-id"),
        bot_id.parse().expect("valid X-Bot-ID header"),
    );
}
```

**Current gap:** The `X-Lane` header is NOT yet sent in the Rust implementation. The `ChatRequest` struct does not carry lane context. This requires threading the lane name through the request pipeline.

### 3.3 Response Headers (Janus -> Nebo)

Janus returns six rate limit headers on every response:

| Header | Example | Purpose |
|--------|---------|---------|
| `X-RateLimit-Session-Limit-Tokens` | `500000` | Max tokens per session window |
| `X-RateLimit-Session-Remaining-Tokens` | `350000` | Tokens remaining in session |
| `X-RateLimit-Session-Reset` | RFC3339 timestamp | Session window reset time |
| `X-RateLimit-Weekly-Limit-Tokens` | `5000000` | Max tokens per billing week |
| `X-RateLimit-Weekly-Remaining-Tokens` | `4200000` | Tokens remaining this week |
| `X-RateLimit-Weekly-Reset` | RFC3339 timestamp | Weekly window reset time |

### 3.4 Rate Limit Capture -- Go Implementation

```go
// internal/agent/ai/api_openai.go:62-103
func captureRateLimitHeaders(resp *http.Response) *RateLimitInfo {
    // Parse all 6 X-RateLimit-* headers
    // Store as atomic.Pointer[ai.RateLimitInfo] on ServiceContext
}
```

**Callback chain:**
1. `captureRateLimitHeaders` middleware parses headers -> stores on `OpenAIProvider.rateLimit`
2. Runner reads via `provider.GetRateLimit()` after each turn
3. Runner calls `rateLimitStore` callback -> `svcCtx.JanusUsage.Store(rl)`
4. Frontend polls `GET /api/v1/neboloop/janus/usage` which reads from that pointer

### 3.5 Rate Limit Storage

**Go approach:** `atomic.Pointer[ai.RateLimitInfo]` -- in-memory only, lost on restart, repopulated on next Janus API response. A TODO exists to persist to `<data_dir>/janus_usage.json`.

```go
type RateLimitInfo struct {
    SessionLimitTokens     int64
    SessionRemainingTokens int64
    SessionResetAt         time.Time
    WeeklyLimitTokens      int64
    WeeklyRemainingTokens  int64
    WeeklyResetAt          time.Time
    UpdatedAt              time.Time
}
```

**Rust approach (proposed):** Use `Arc<RwLock<Option<RateLimitInfo>>>` on `AppState`. The `handle_stream` method already has access to response headers before the SSE body begins, making it the natural capture point. Persistence to `janus_usage.json` can use `tokio::fs::write` with serde_json serialization.

```rust
// Proposed Rust struct
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitInfo {
    pub session_limit_tokens: i64,
    pub session_remaining_tokens: i64,
    pub session_reset_at: chrono::DateTime<chrono::Utc>,
    pub weekly_limit_tokens: i64,
    pub weekly_remaining_tokens: i64,
    pub weekly_reset_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}
```

### 3.6 Usage API

```
GET /api/v1/neboloop/janus/usage
```

Returns:

```json
{
  "session": {
    "limitTokens": 500000,
    "remainingTokens": 350000,
    "usedTokens": 150000,
    "percentUsed": 30,
    "resetAt": "2026-03-04T12:00:00Z"
  },
  "weekly": {
    "limitTokens": 5000000,
    "remainingTokens": 4200000,
    "usedTokens": 800000,
    "percentUsed": 16,
    "resetAt": "2026-03-10T00:00:00Z"
  }
}
```

**Current gap:** This endpoint is NOT yet implemented in Rust.

### 3.7 Go Usage Types (Reference)

```go
type NeboLoopJanusWindowUsage struct {
    LimitTokens     int64  `json:"limitTokens"`
    RemainingTokens int64  `json:"remainingTokens"`
    UsedTokens      int64  `json:"usedTokens"`
    PercentUsed     int    `json:"percentUsed"`
    ResetAt         string `json:"resetAt,omitempty"`
}

type NeboLoopJanusUsageResponse struct {
    Session NeboLoopJanusWindowUsage `json:"session"`
    Weekly  NeboLoopJanusWindowUsage `json:"weekly"`
}
```

---

## 4. Four Streaming Quirks

**File(s):** `crates/ai/src/providers/openai.rs` -- `handle_stream()`

Janus is an OpenAI-compatible proxy, but its SSE stream deviates from the OpenAI specification in four documented ways. All four workarounds are implemented in both Go and Rust.

### 4.1 Tool Name Duplication

**Problem:** Janus sends the tool name in EVERY chunk of a tool call stream. The standard OpenAI API sends the name only in the first chunk. Without deduplication, the SDK accumulator concatenates names -> `"agentagentagent..."`.

**Go workaround** (`api_openai.go:356-365`): Track seen names per tool index; clear duplicates after the first occurrence.

**Rust workaround:**

```rust
// crates/ai/src/providers/openai.rs
let mut seen_tool_name: HashSet<u32> = HashSet::new();

// Inside the tool call accumulation loop:
if let Some(name) = func.name.as_deref() {
    if !name.is_empty() && !seen_tool_name.contains(&idx) {
        entry.name = name.to_string();
        seen_tool_name.insert(idx);
    }
}
```

### 4.2 Complete JSON Arguments in One Chunk

**Problem:** Standard OpenAI streams tool call arguments incrementally (partial JSON across many chunks). Janus sends the COMPLETE JSON arguments in a single chunk, then repeats them. Without deduplication, the accumulator doubles/triples the JSON.

**Go workaround** (`api_openai.go:366-374`): Detect valid JSON in the arguments field; mark as seen; clear subsequent chunks for that tool index.

**Rust workaround:**

```rust
// crates/ai/src/providers/openai.rs
let mut seen_tool_args: HashSet<u32> = HashSet::new();

if !args.is_empty() {
    if seen_tool_args.contains(&idx) {
        // Already have complete args, skip duplicate
    } else if serde_json::from_str::<serde_json::Value>(args).is_ok() {
        // Complete JSON in one chunk (Janus style)
        entry.arguments = args.to_string();
        seen_tool_args.insert(idx);
    } else {
        // Partial JSON (standard OpenAI streaming)
        entry.arguments.push_str(args);
    }
}
```

This approach is backward-compatible: if the arguments are NOT valid JSON (standard OpenAI incremental streaming), the code falls through to the `push_str` accumulator.

### 4.3 Missing SSE `[DONE]` Sentinel

**Problem:** The OpenAI SSE spec requires a `data: [DONE]` line after the last chunk. Janus does NOT always send this sentinel after `finish_reason`. Without early termination, `stream.Next()` blocks until TCP timeout (~120 seconds).

**Go workaround** (`api_openai.go:410-417`): Break on any non-empty `finish_reason`, do NOT wait for `[DONE]`.

**Rust workaround:**

```rust
// crates/ai/src/providers/openai.rs
// Check finish reason -- break immediately.
// Janus may not send [DONE] sentinel after finish_reason,
// which would block until TCP timeout (~120s).
if let Some(ref reason) = choice.finish_reason {
    debug!(
        finish_reason = ?reason,
        text_chunks,
        chunk_count,
        "stream finished"
    );
    finished = true;
    break 'outer;
}
```

### 4.4 Non-Null Content for Gemini Backends

**Problem:** When Janus routes to Gemini backends, the API rejects messages with `null` content + `tool_calls`. Gemini requires a non-null content field even when the assistant message is purely tool calls.

**Go workaround** (`api_openai.go:288-293`): Set content to `" "` (single space) when empty but tool_calls are present.

**Rust workaround:**

```rust
// crates/ai/src/providers/openai.rs -- build_messages()
// Some gateways reject null content with tool_calls
let content = if msg.content.is_empty() && !tool_calls.is_empty() {
    Some(ChatCompletionRequestAssistantMessageContent::Text(
        " ".to_string(),
    ))
} else if !msg.content.is_empty() {
    Some(ChatCompletionRequestAssistantMessageContent::Text(
        msg.content.clone(),
    ))
} else {
    None
};
```

### 4.5 Error Handling in SSE Stream

In addition to the four quirks above, the Rust implementation handles mid-stream error objects from Janus:

```rust
// Check for OpenAI-compatible error responses (e.g. from Janus)
// These have {"error":{"message":"...","type":"...","code":"..."}}
if let Ok(val) = serde_json::from_str::<serde_json::Value>(&data) {
    if let Some(err_obj) = val.get("error") {
        let msg = err_obj
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown provider error");
        warn!(error = msg, "provider returned error in SSE stream");
        let _ = tx.send(StreamEvent::error(msg.to_string())).await;
        finished = true;
        break 'outer;
    }
}
```

### 4.6 Raw reqwest Instead of reqwest-eventsource

The Rust `OpenAIProvider` deliberately uses raw reqwest byte streaming instead of the `reqwest-eventsource` crate. Reason: `reqwest-eventsource` implements automatic SSE reconnection per the W3C spec, which causes infinite retries on 502 errors from Janus. The raw approach gives full control over error handling and stream termination.

### 4.7 Tool Call Emission Strategy

Tools are accumulated in a `HashMap<u32, AccumulatedToolCall>` keyed by tool index. They are emitted at the END of the stream (after `finish_reason` or `[DONE]`), NOT inline during streaming:

```rust
// Emit accumulated tool calls (fallback for Janus single-chunk tool calls)
for tc in tool_calls.values() {
    if !tc.id.is_empty() && !tc.name.is_empty() && !emitted_tool_calls.contains(&tc.id) {
        emitted_tool_calls.insert(tc.id.clone());
        let input: serde_json::Value = serde_json::from_str(&tc.arguments)
            .unwrap_or(serde_json::Value::Object(Default::default()));
        let _ = tx.send(StreamEvent::tool_call(ToolCall {
            id: tc.id.clone(),
            name: tc.name.clone(),
            input,
        })).await;
    }
}
```

This ensures that even when Janus sends tool call data in a single chunk without a clean separation, the tool calls are still emitted correctly.

---

## 5. Steering Integration

**File(s):** Go: `internal/agent/steering/generators.go:250-288`, `internal/agent/steering/templates.go:71-73`

### 5.1 janusQuotaWarning Generator

The `janusQuotaWarning` steering generator fires when the Janus rate limit budget is running low. It is one of ~10 generators in the steering pipeline.

**Trigger conditions:**
- Either the session OR weekly window is >80% consumed (`remaining/limit < 0.20`)
- Fires ONCE per session (tracked in `janusQuotaWarnedSessions` sync.Map in Go)

**Steering message template:**

```
Your NeboLoop Janus token budget is %d%% used (%s window running low).
Warn the user that their AI usage quota is running low. Suggest shorter prompts
or upgrading their plan.
You can open the billing page with: agent(resource: profile, action: open_billing)
```

### 5.2 Janus-Quota Skill

In addition to the steering generator, there is a `janus-quota` skill (priority: 95, max_turns: 1) that handles quota-related trigger phrases:

**Triggers:** quota, tokens, limit exceeded, ran out of tokens, out of credits, upgrade plan, weekly limit, can't respond, something went wrong

**Purpose:** Gracefully handle AI token quota warnings and exhaustion. Progressive warning thresholds at ~80%, ~90%, ~95%+, and full exhaustion. Directs users to Settings > NeboLoop or the upgrade link. CRITICAL: never mentions "Janus" to users -- just says "AI tokens" or "weekly budget". Tone: matter-of-fact, like a fuel gauge.

### 5.3 Integration Points

The quota warning depends on:
1. Rate limit headers being captured from Janus responses (section 3.3).
2. The captured data being accessible to the steering pipeline.
3. A per-session deduplication mechanism to avoid repeating the warning.

### 5.4 Rust Status

The steering pipeline exists in concept (see `misc-systems.md` section on steering generators) but the `janusQuotaWarning` generator is NOT yet implemented in Rust. Prerequisites:
- Rate limit header capture must be implemented first (section 3.5).
- The steering pipeline must have access to the rate limit store.
- Session-scoped deduplication (`HashSet<String>` per runner instance) is trivial.

---

## 6. Local Inference Architecture

**File(s):** Go: `internal/agent/ai/api_local.go`, `internal/agent/ai/local_models.go`

### 6.1 Overview

Nebo can run LLMs locally using llama.cpp via Go bindings (gollama). No external runtime (Ollama, Python, etc.) is required. Models download automatically after onboarding and serve as the always-available fallback when cloud providers are unavailable.

### 6.2 Architecture Diagram

```
User finishes onboarding
  -> Background download starts (both models in parallel)
  -> 0.8B (~600MB) finishes first -> immediate fallback available
  -> 9B (~5.5GB) finishes later -> full local capability

Provider selection order:
  1. Cloud APIs (Anthropic, OpenAI, Gemini via API keys)
  2. Janus (NeboLoop gateway)
  3. Ollama (if installed)
  4. CLI providers (claude, codex, gemini)
  5. Local models (ALWAYS LAST -- built-in fallback)
```

### 6.3 Default Models

| Model | File | Size | Priority | Purpose |
|-------|------|------|----------|---------|
| Qwen 3.5 0.8B | `qwen3.5-0.8b-q4_k_m.gguf` | ~600MB | 0 (immediate) | Fast fallback, basic chat |
| Qwen 3.5 9B | `qwen3.5-9b-q4_k_m.gguf` | ~5.5GB | 1 (background) | Full capability, tool use |

Both are natively multimodal (vision) and support 256K context.

Models are stored in `<data_dir>/models/` (e.g., `~/Library/Application Support/Nebo/models/`).

### 6.4 Go Key Files (Reference)

| File | Purpose |
|------|---------|
| `internal/agent/ai/api_local.go` | LocalProvider -- implements Provider interface via gollama |
| `internal/agent/ai/local_models.go` | ModelDownloader -- download, verify, and manage GGUF files |
| `internal/handler/provider/localmodelshandler.go` | HTTP endpoints: status check + SSE download progress |
| `cmd/nebo/providers.go` | Provider loading -- `loadLocalProviders()` + `StartLocalModelDownload()` |

### 6.5 Model Loading

- **Lazy-loaded** on first `Stream()` call, NOT at startup.
- GPU offload: all layers to Metal (macOS) / CUDA (NVIDIA) by default.
- Context: 8192 tokens with q8_0 KV cache (50% VRAM savings).
- Thread-safe: model is shared, contexts are per-request.

### 6.6 Build Requirements (Go)

gollama uses CGO to link llama.cpp. Required build flags:
- `CGO_ENABLED=1` (already required for desktop builds)
- `CGO_LDFLAGS_ALLOW='-Wl,-rpath,.*'` (llama.cpp rpath for prebuilt libs)
- `GOFLAGS=-mod=mod` (vendor dir doesn't include C source files)

### 6.7 Rust Approach -- Two Paths

Rust does NOT use gollama. The local inference story for nebo-rs has two paths:

**Path 1: Ollama delegation (current).** The `OllamaProvider` in `crates/ai/src/providers/ollama.rs` connects to a running Ollama instance. This is already implemented and tested. Ollama handles model management, GPU offload, and inference:

```rust
// crates/ai/src/providers/ollama.rs
pub struct OllamaProvider {
    client: Client,
    base_url: String,  // Default: http://localhost:11434
    model: String,     // Default: qwen3:4b
}
```

Ollama utilities already implemented:
- `check_ollama_available(base_url)` -- probes `GET /api/tags` with 2s timeout
- `list_ollama_models(base_url)` -- lists all available models
- `ensure_ollama_model(base_url, model)` -- pulls a model if NOT present locally (30min timeout)

**Path 2: Native llama.cpp bindings (future).** A Rust crate such as `llama-cpp-rs` or `llm` could provide direct GGUF model loading without an external Ollama daemon. This is NOT yet implemented.

### 6.8 Ollama Provider Implementation Details

The Ollama provider uses NDJSON streaming (NOT SSE like OpenAI):

```rust
// Each line from Ollama is a complete JSON object
let resp: OllamaStreamResponse = serde_json::from_str(&line)?;

// Text content streamed incrementally
if !resp.message.content.is_empty() {
    let _ = tx.send(StreamEvent::text(&resp.message.content)).await;
}

// Tool calls arrive as complete objects (NOT streamed incrementally)
if let Some(ref tool_calls) = resp.message.tool_calls {
    for tc in tool_calls {
        // Generate synthetic IDs since Ollama doesn't provide them
        let _ = tx.send(StreamEvent::tool_call(ToolCall {
            id: format!("ollama-call-{}", counter),
            name: tc.function.name.clone(),
            input: serde_json::to_value(&tc.function.arguments).unwrap_or_default(),
        })).await;
    }
}

// Stream termination via "done" field (NOT [DONE] sentinel)
if resp.done {
    let _ = tx.send(StreamEvent::done()).await;
    return;
}
```

Key differences from OpenAI/Janus streaming:
- NDJSON (newline-delimited JSON) instead of SSE
- Tool calls are complete objects, NOT streamed incrementally
- Ollama generates NO tool call IDs -- Rust generates synthetic `ollama-call-N` IDs
- Stream terminates via `done: true` field, NOT a `[DONE]` sentinel
- 5-minute timeout configured on the HTTP client for large model inference

---

## 7. Model Download Pipeline

**File(s):** Go: `internal/agent/ai/local_models.go`, `internal/handler/provider/localmodelshandler.go`

### 7.1 Download Strategy

Downloads are background and parallel:
- Both models (0.8B and 9B) begin downloading simultaneously after onboarding.
- The 0.8B model (~600MB) finishes first, providing immediate fallback capability.
- The 9B model (~5.5GB) finishes later, enabling full local capability with tool use.

### 7.2 API Endpoints (Go)

**GET /api/v1/local-models/status**

```json
{
  "ready": true,
  "models": [
    {"name": "qwen3.5-9b-q4_k_m", "path": "/path/to/file.gguf", "size": 5800000000}
  ],
  "available": {"qwen3.5-0.8b": true, "qwen3.5-9b": true},
  "ollama_models": ["qwen3.5:9b", "llama3:8b", "mistral:7b"]
}
```

**POST /api/v1/local-models/download** -- SSE stream with progress events:

```
data: {"model_name":"qwen3.5-0.8b","downloaded":150000000,"total":600000000,"percent":25,"bytes_per_sec":50000000}
data: {"model_name":"qwen3.5-0.8b","downloaded":600000000,"total":600000000,"percent":100,"bytes_per_sec":0}
data: {"ready":true}
```

### 7.3 Download Pipeline Details

1. **URL resolution:** GGUF file URLs point to HuggingFace (to be confirmed when Qwen publishes).
2. **HTTP download:** Streamed with progress tracking (bytes downloaded, total size, bytes/sec).
3. **Verification:** File size check post-download (SHA256 verification is a future enhancement).
4. **Storage:** Written to `<data_dir>/models/<filename>.gguf`.
5. **Progress broadcast:** SSE events streamed to the frontend in real-time.
6. **Error recovery:** Failed downloads can be retried via the same endpoint.

### 7.4 Ollama Auto-Detection

The status endpoint also probes for a running Ollama instance and lists available models. These show as badges in the UI under "Also available via Ollama". This is informational -- Ollama models are managed via the Ollama provider, NOT the local provider.

### 7.5 Rust Status

The Rust implementation has a simplified `local_models_status` endpoint that checks Ollama availability. The full GGUF download pipeline (background download, progress SSE, model management) is NOT yet implemented.

### 7.6 Future Work (Both Codebases)

- Wire `StartLocalModelDownload()` into onboarding completion handler
- Speculative decoding: use 0.8B as draft model for 9B (2-3x speedup)
- Download progress via WebSocket events (not just SSE)
- Model management UI (delete, re-download, add custom GGUF)
- Verify actual HuggingFace GGUF URLs once Qwen publishes them
- Context window auto-sizing based on available RAM
- Native tool calling when gollama/llama-cpp-rs adds support

---

## 8. Tool Calling via Prompt Engineering

**File(s):** Go: `internal/agent/ai/api_local.go`

### 8.1 The Problem

gollama (the Go llama.cpp binding) does NOT have native tool call support. Small local models also lack robust function-calling capabilities even when the runtime supports it. The Go implementation solves this via prompt engineering.

### 8.2 Approach

1. **Tool definitions** are injected into the system prompt as structured text, describing each tool's name, description, and parameters.

2. **The model responds** with `<tool_call>` XML blocks containing JSON:

```xml
<tool_call>
{"name": "file", "arguments": {"action": "read", "path": "/tmp/test.txt"}}
</tool_call>
```

3. **`extractToolCalls()`** parses these XML blocks from the response text and converts them to standard `ToolCall` structs.

4. **The runner** executes tools normally -- it does NOT know or care whether tool calls came from native function calling or prompt engineering extraction.

### 8.3 Message Format

- Session messages are converted to `llama.ChatMessage{Role, Content}`.
- Tool results are wrapped as `<tool_result name="...">` in user messages.
- Tool calls from the assistant are included inline for context continuity.

### 8.4 Rust Applicability

If nebo-rs implements native llama.cpp bindings (section 6.7, path 2), the same prompt engineering approach applies. The XML extraction logic is straightforward:

```rust
// Proposed Rust implementation for future native local provider
fn extract_tool_calls(text: &str) -> Vec<ToolCall> {
    let mut calls = Vec::new();
    let mut remaining = text;

    while let Some(start) = remaining.find("<tool_call>") {
        if let Some(end) = remaining[start..].find("</tool_call>") {
            let json_str = remaining[start + 11..start + end].trim();
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                let name = val["name"].as_str().unwrap_or("").to_string();
                let arguments = val.get("arguments").cloned()
                    .unwrap_or(serde_json::Value::Object(Default::default()));
                calls.push(ToolCall {
                    id: format!("local-call-{}", calls.len() + 1),
                    name,
                    input: arguments,
                });
            }
            remaining = &remaining[start + end + 12..];
        } else {
            break;
        }
    }

    calls
}
```

This is NOT yet needed since nebo-rs delegates to Ollama, which has its own native tool calling support via the Ollama API.

### 8.5 Contrast: Ollama vs gollama Tool Calling

| Aspect | gollama (Go local) | Ollama (Rust current) |
|--------|--------------------|-----------------------|
| Tool call mechanism | Prompt engineering (`<tool_call>` XML) | Native API support |
| Tool call IDs | Generated as `local-call-N` | Generated as `ollama-call-N` |
| Reliability | Depends on model following XML format | Native structured output |
| Model compatibility | Any GGUF model | Models with tool calling support |
| External daemon | None required | Ollama must be running |

---

## 9. Provider Priority Order

**File(s):** `crates/server/src/lib.rs`, `crates/server/src/handlers/provider.rs`

### 9.1 Detection Priority

Provider detection follows a cascading priority system:

| Priority | Source | Mechanism |
|----------|--------|-----------|
| 1 (highest) | Database | API keys from UI (Settings > Providers) stored in `auth_profiles` table |
| 2 | Config file | `models.yaml` credentials section (env var expansion via `os.ExpandEnv` in Go) |
| 3 (lowest) | CLI auto-discovery | PATH scan for `claude`, `codex`, `gemini` binaries |

### 9.2 Provider Type Resolution

When loading from `auth_profiles`, the provider type determines which implementation is constructed:

| `auth_profiles.provider` | Rust Implementation | Default Model |
|--------------------------|---------------------|---------------|
| `anthropic` | `AnthropicProvider::new(api_key, model)` | `claude-sonnet-4-20250514` |
| `openai` | `OpenAIProvider::new(api_key, model)` | `gpt-4o` |
| `deepseek` | `OpenAIProvider::with_base_url(...)` | `deepseek-chat` |
| `google` | `OpenAIProvider::with_base_url(...)` | `gemini-2.0-flash` |
| `ollama` | `OllamaProvider::new(base_url, model)` | `llama3.2` |
| `neboloop` | `OpenAIProvider::with_base_url(...)` + janus config | `janus` |

### 9.3 Provider Selection at Runtime

The runner maintains an ordered list of providers. When a request fails:

1. Try the primary provider.
2. On failure (rate limit, auth error, timeout), try the next provider in the list.
3. Continue until a provider succeeds or all providers are exhausted.
4. Exponential backoff on repeated failures per provider.

Error classification drives retry behavior:

```rust
// crates/ai/src/types.rs
pub fn classify_error_reason(err: &ProviderError) -> &str {
    match err {
        ProviderError::RateLimit => "rate_limit",
        ProviderError::Auth(_) => "auth",
        ProviderError::ContextOverflow => "context_overflow",
        // ... billing, timeout, other
    }
}
```

### 9.4 Embedding Provider Priority

Embedding requests follow a separate priority chain:

1. **Janus** (centralized, if available) -- uses `text-embedding-small` via Janus URL
2. **OpenAI** (direct) -- uses `text-embedding-3-small` via OpenAI API
3. **Ollama** (local) -- uses whatever embedding model is configured

The `CachedEmbeddingProvider` wraps any embedding provider with SHA256-keyed caching in the `embedding_cache` database table, avoiding re-embedding identical text:

```rust
// crates/ai/src/embedding.rs
pub struct CachedEmbeddingProvider {
    inner: Box<dyn EmbeddingProvider>,
    store: std::sync::Arc<db::Store>,
}

// Cache lookup: SHA256(text) -> embedding_cache table
fn content_hash(text: &str) -> String {
    let hash = Sha256::digest(text.as_bytes());
    hex::encode(hash)
}
```

### 9.5 Hot Reload

Providers are reloaded dynamically when auth profiles change. In Rust, the `reload_providers()` function in `crates/server/src/handlers/provider.rs` rebuilds the provider list from `auth_profiles` and calls `state.runner.reload_providers(providers).await`.

This happens automatically on:
- `POST /api/v1/providers` (create)
- `PUT /api/v1/providers/:id` (update)
- `DELETE /api/v1/providers/:id` (delete)

### 9.6 CLI Provider Detection

The Rust implementation detects CLI providers at startup and exposes their status via the models API:

```rust
// crates/server/src/handlers/provider.rs -- list_models()
let cli_statuses = serde_json::json!({
    "claude": {
        "installed": cli.claude.installed,
        "authenticated": cli.claude.authenticated,
        "version": cli.claude.version,
    },
    "codex": { ... },
    "gemini": { ... },
});
```

CLI providers are configured in `models.yaml` (NOT `auth_profiles`) and toggled via `PUT /api/v1/models/cli/{cliId}`.

### 9.7 Task-Based Model Routing

The models config supports task-based routing where different task types (vision, audio, reasoning, code, general) can be routed to specific models:

```yaml
task_routing:
  vision: "claude-sonnet-4-20250514"
  reasoning: "claude-sonnet-4-20250514"
  code: "claude-sonnet-4-20250514"
  general: "claude-sonnet-4-20250514"
  fallbacks:
    claude-sonnet-4-20250514: ["gpt-4o", "janus"]
```

Lane routing allows different lanes to use different models:

```yaml
lane_routing:
  heartbeat: "janus"
  events: "janus"
  comm: "janus"
  subagent: "janus"
```

### 9.8 Provider Test Endpoint

The `POST /api/v1/providers/:id/test` endpoint creates a temporary provider instance and sends a minimal chat request ("Say OK", max_tokens=16, 15s timeout) to verify connectivity:

```rust
// crates/server/src/handlers/provider.rs
async fn test_provider_connection(provider: &dyn ai::Provider) -> Result<String, String> {
    let req = ai::ChatRequest {
        messages: vec![ai::Message {
            role: "user".into(),
            content: "Say OK".into(),
            tool_calls: None,
            tool_results: None,
        }],
        max_tokens: 16,
        temperature: 0.0,
        // ...
    };
    match tokio::time::timeout(Duration::from_secs(15), provider.stream(&req)).await {
        // ...
    }
}
```

This works for ALL provider types including Janus -- the Janus test uses the same JWT and bot ID as production requests.

---

## 10. Rust Implementation Status

### 10.1 Provider Implementation Status

| Component | Go | Rust | Notes |
|-----------|:---:|:----:|-------|
| OpenAI provider | Y | Y | Full streaming, tool calls, error mapping |
| Anthropic provider | Y | Y | Full streaming, thinking mode, cache control |
| Ollama provider | Y | Y | NDJSON streaming, tool calls, model management |
| Janus provider | Y | Y | Via OpenAIProvider with custom base URL + bot ID |
| DeepSeek provider | Y | Y | Via OpenAIProvider with custom base URL |
| Google/Gemini provider | Y | Y | Via OpenAIProvider with Google base URL |
| CLI providers (claude, codex, gemini) | Y | P | Detection done; MCP-based execution NOT yet wired |
| Local provider (gollama) | Y | N | Delegates to Ollama instead |

### 10.2 Janus Feature Status

| Feature | Go | Rust | Notes |
|---------|:---:|:----:|-------|
| Provider construction | Y | Y | Matching implementation |
| X-Bot-ID header | Y | Y | Sent on every Janus request |
| X-Lane header | Y | N | ChatRequest does not carry lane context |
| Rate limit header capture | Y | N | Response headers not parsed in handle_stream |
| Rate limit in-memory store | Y | N | No RateLimitInfo struct or storage |
| Rate limit persistence (janus_usage.json) | N | N | TODO in both codebases |
| Usage API endpoint | Y | N | GET /api/v1/neboloop/janus/usage not implemented |
| Account status (janusProvider bool) | Y | P | OAuth flow stores the flag; status endpoint partial |
| OAuth ?janus=true activation | Y | Y | Matching implementation |
| Carry-forward on re-auth | Y | P | Needs verification in callback handler |
| Streaming quirk: tool name dedup | Y | Y | HashSet per stream |
| Streaming quirk: complete JSON args | Y | Y | JSON parse detection |
| Streaming quirk: missing [DONE] | Y | Y | Break on finish_reason |
| Streaming quirk: non-null content | Y | Y | Space string for empty content |
| Mid-stream error handling | Y | Y | Error object detection in SSE |
| Raw reqwest (no auto-reconnect) | N/A | Y | Go uses OpenAI SDK; Rust avoids reqwest-eventsource |

### 10.3 Local Inference Feature Status

| Feature | Go | Rust | Notes |
|---------|:---:|:----:|-------|
| llama.cpp via gollama | Y | N | Not applicable -- different approach |
| Ollama integration | Y | Y | Full provider with NDJSON streaming |
| Ollama auto-detection | Y | Y | check_ollama_available() + list_ollama_models() |
| Ollama model pull | Y | Y | ensure_ollama_model() with 30min timeout |
| GGUF model download pipeline | Y | N | Background download with SSE progress |
| Download progress SSE endpoint | Y | N | POST /api/v1/local-models/download |
| Local model status endpoint | Y | P | Delegates to Ollama only |
| Lazy model loading | Y | N/A | Ollama handles its own model lifecycle |
| GPU offload configuration | Y | N/A | Ollama handles GPU offload |
| Tool calling via prompt engineering | Y | N | Ollama has native tool calling |
| Model management UI support | Y | P | Status endpoint exists; no download/delete |

### 10.4 Steering and Quota Status

| Feature | Go | Rust | Notes |
|---------|:---:|:----:|-------|
| Steering pipeline | Y | P | Framework exists; generators incomplete |
| janusQuotaWarning generator | Y | N | Requires rate limit capture first |
| janus-quota skill | Y | P | Skill system exists; this specific skill needs porting |
| Per-session deduplication | Y | N | Trivial once rate limit data is available |

### 10.5 Configuration Status

| Feature | Go | Rust | Notes |
|---------|:---:|:----:|-------|
| NeboLoop.JanusURL in config | Y | Y | etc/nebo.yaml |
| NEBOLOOP_JANUS_URL env override | Y | Y | Environment variable expansion |
| Bot ID read | Y | Y | config::read_bot_id() |
| models.yaml Janus model defs | Y | Y | janus provider section with migration |
| Janus model ID migration (strip prefix) | Y | Y | Strips "janus/" prefix from model IDs |
| Provider hot reload | Y | Y | reload_providers() on CRUD |
| Task routing config | Y | Y | models.yaml task_routing section |
| Lane routing config | Y | Y | models.yaml lane_routing section |
| CLI provider detection | Y | Y | Startup PATH scan with status tracking |

### 10.6 Migration Priority

Ranked by user impact:

1. **Rate limit header capture** -- Without this, users have no visibility into Janus quota usage. Blocks steering integration.
2. **X-Lane header** -- Analytics and routing. Thread lane name through ChatRequest.
3. **Usage API endpoint** -- Frontend needs this for progress bars and warnings.
4. **janusQuotaWarning steering** -- Prevents surprise quota exhaustion.
5. **GGUF download pipeline** -- Enables offline-first experience without Ollama.
6. **Carry-forward verification** -- Ensure re-auth preserves janus_provider metadata.

--

### 10.7 Frontend Integration Points

The frontend depends on these APIs for Janus/provider functionality:

| Endpoint | Status | Used By |
|----------|:------:|---------|
| `GET /api/v1/providers` | Y | ProvidersSection.svelte |
| `POST /api/v1/providers` | Y | Add provider dialog |
| `PUT /api/v1/providers/:id` | Y | Edit provider |
| `DELETE /api/v1/providers/:id` | Y | Remove provider |
| `POST /api/v1/providers/:id/test` | Y | Test connection button |
| `GET /api/v1/models` | Y | Model catalog + routing config |
| `PUT /api/v1/models/{provider}/{modelId}` | Y | Toggle model active/preferred |
| `PUT /api/v1/models/config` | Y | Set default model |
| `PUT /api/v1/models/task-routing` | Y | Task/lane routing config |
| `PUT /api/v1/models/cli/{cliId}` | Y | Toggle CLI provider |
| `GET /api/v1/local-models/status` | P | Local model detection (Ollama only) |
| `POST /api/v1/local-models/download` | N | GGUF download with SSE progress |
| `GET /api/v1/neboloop/account/status` | P | NeboLoop account page |
| `GET /api/v1/neboloop/janus/usage` | N | Usage bars on providers/account pages |
| `GET /api/v1/neboloop/oauth/start` | Y | OAuth initiation with ?janus=true |

--

*Generated: 2026-03-04*
