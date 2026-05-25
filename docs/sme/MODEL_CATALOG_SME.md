# Model Catalog & Selection System — SME Reference

> Last updated: May 2026. Covers `crates/ai/`, `crates/config/src/models.rs`,
> `crates/agent/src/selector.rs`, `crates/agent/src/fuzzy.rs`,
> `crates/agent/src/runner.rs`, `crates/agent/src/sidecar.rs`,
> `crates/agent/src/pruning.rs`, `crates/server/src/lib.rs`.

---

## 1. Architecture Overview

```
  ┌──────────────────────────────────────────────────────────────────────┐
  │                        User Request Flow                            │
  │                                                                     │
  │  User ─► WebSocket ─► chat_dispatch ─► Runner.run() ─► run_loop()  │
  │                                             │                       │
  │                     ┌───────────────────────┼──────────────────┐    │
  │                     │                       ▼                  │    │
  │                     │   ┌──────────────────────────────┐       │    │
  │                     │   │        ModelSelector          │       │    │
  │                     │   │  1. classify_task(messages)   │       │    │
  │                     │   │  2. select_for_task(task)     │       │    │
  │                     │   │  3. fuzzy resolve overrides   │       │    │
  │                     │   │  4. cooldown/exclusion check  │       │    │
  │                     │   └──────────┬───────────────────┘       │    │
  │                     │              │ "provider/model-id"        │    │
  │                     │              ▼                            │    │
  │                     │   ┌──────────────────────────────┐       │    │
  │                     │   │    Provider Resolution        │       │    │
  │                     │   │  parse "anthropic/claude-..."  │       │    │
  │                     │   │  find matching Provider in     │       │    │
  │                     │   │  providers[] by .id()          │       │    │
  │                     │   └──────────┬───────────────────┘       │    │
  │                     │              │                            │    │
  │                     │              ▼                            │    │
  │                     │   ┌──────────────────────────────┐       │    │
  │                     │   │    Provider.stream(req)       │       │    │
  │                     │   │  Anthropic / OpenAI / Gemini  │       │    │
  │                     │   │  Ollama / CLI / Local / Janus │       │    │
  │                     │   └──────────────────────────────┘       │    │
  │                     │                                          │    │
  │                     │  Agent Runner (crates/agent/src/runner.rs)│    │
  │                     └──────────────────────────────────────────┘    │
  └──────────────────────────────────────────────────────────────────────┘
```

### Data Flow: Model Selection Pipeline

```
  models.yaml (embedded in binary)
       │
       ├─── User override: ~/.nebo/data/models.yaml
       │
       ▼
  ModelsConfig::load()
       │
       ▼
  ModelRoutingConfig::from_models_config(
      models_cfg,
      active_provider_ids,    ◄── DB: auth_profiles (who has API keys?)
      model_overrides          ◄── DB: per-model active toggles
  )
       │
       ▼
  ModelSelector::new(routing_config)
       │
       ├── FuzzyMatcher (aliases, typo tolerance, kind tags)
       ├── CooldownState (per-model exponential backoff)
       ├── loaded_providers (runtime filter)
       └── runtime_models (Ollama auto-discovery)
```

---

## 2. Model Catalog (models.yaml)

The model catalog is defined in `crates/config/src/models.yaml`, embedded into the
binary at compile time via `include_str!`. Users can override it by placing a modified
copy at `~/.nebo/data/models.yaml`. On load, missing sections are backfilled from the
embedded defaults.

### Top-Level Structure

| Section          | Purpose                                              |
|------------------|------------------------------------------------------|
| `version`        | Schema version ("1.0")                               |
| `defaults`       | Primary model and ordered fallback chain              |
| `task_routing`   | Per-task-type model assignments + fallback overrides  |
| `lane_routing`   | Per-lane model preferences (heartbeat, comm, etc.)    |
| `aliases`        | User-defined friendly names for models                |
| `providers`      | Provider -> list of ModelDef (the core catalog)       |
| `cli_providers`  | CLI tool definitions (claude, codex, gemini)          |

### Key Config Struct — `ModelsConfig`

```rust
// crates/config/src/models.rs
pub struct ModelsConfig {
    pub version: String,
    pub defaults: Option<Defaults>,
    pub task_routing: Option<TaskRouting>,
    pub lane_routing: Option<LaneRouting>,
    pub aliases: Vec<ModelAlias>,
    pub providers: HashMap<String, Vec<ModelDef>>,
    pub cli_providers: Vec<CliProviderDef>,
}
```

### Model Definition — `ModelDef`

```rust
pub struct ModelDef {
    pub id: String,                     // "claude-sonnet-4-6"
    pub display_name: String,           // "Claude Sonnet 4.6"
    pub context_window: i64,            // 1_000_000
    pub pricing: Option<ModelPricing>,  // input/output per million tokens
    pub capabilities: Vec<String>,      // ["vision", "tools", "streaming", "code", "thinking"]
    pub kind: Vec<String>,              // ["smart", "fast", "cheap", "reasoning"]
    pub preferred: bool,                // preferred model within its kind
    pub active: Option<bool>,           // user toggle (defaults to true)
}
```

### Supported Models (as of models.yaml)

```
Provider    Model ID                       Context   In/Out $/M   Capabilities
─────────── ────────────────────────────── ───────── ──────────── ─────────────────────
anthropic   claude-opus-4-6                1,000,000  $5/$25      vision,tools,streaming,code,reasoning,thinking
anthropic   claude-sonnet-4-6              1,000,000  $3/$15      vision,tools,streaming,code,thinking
anthropic   claude-haiku-4-5-20251001        200,000  $1/$5       vision,tools,streaming,code

openai      gpt-5.4                          400,000  -/-         vision,tools,streaming,code,reasoning
openai      gpt-5.4-mini                     400,000  -/-         vision,tools,streaming,code,reasoning
openai      gpt-5.4-nano                     128,000  -/-         vision,tools,streaming,code
openai      codex-mini-latest                256,000  -/-         tools,streaming,code,reasoning

google      gemini-3.1-pro-preview         1,000,000  $1.25/$5    vision,tools,streaming,code,reasoning,thinking
google      gemini-3-flash-preview         1,000,000  $0.10/$0.40 vision,tools,streaming,code
google      gemini-2.5-flash               1,000,000  $0.15/$0.60 vision,tools,streaming,code

deepseek    deepseek-chat                    128,000  -/-         tools,streaming,code
deepseek    deepseek-reasoner                128,000  -/-         reasoning,code

janus       nebo-1                           200,000  free*       vision,tools,streaming,code,reasoning
janus       nebo-embed-small                   8,191  free*       embeddings
janus       nebo-embed-large                   8,191  free*       embeddings
```

\* Janus gateway: free tier with session/weekly credit limits. Server-side routing
picks the best upstream model. Costs are metered via microdollar budget pools
(free, gift, credits).

### CLI Providers

CLI providers wrap locally-installed CLI tools. They require no API keys -- they use
the user's existing subscription auth (Claude subscription, ChatGPT account, etc.).

| CLI ID       | Command  | Install                      | Default Model             |
|-------------|----------|------------------------------|---------------------------|
| claude-code | `claude` | `brew install claude-code`   | opus                      |
| codex-cli   | `codex`  | `npm i -g @openai/codex`     | gpt-5.4                   |
| gemini-cli  | `gemini` | `npm i -g @google/gemini-cli`| gemini-3.1-pro-preview    |

CLI providers set `handles_tools() = true`, meaning they execute tools autonomously
via MCP. The runner skips its own tool execution loop when a CLI provider is active.

### Local GGUF Models

```
Model                 Filename                     Size       Priority
───────────────────── ──────────────────────────── ────────── ────────
qwen3.5-0.8b          Qwen3.5-0.8B-Q4_K_M.gguf    533 MB    0 (download first)
qwen3.5-2b            Qwen3.5-2B-Q4_K_M.gguf      1.6 GB    1
qwen3.5-4b            Qwen3.5-4B-Q4_K_M.gguf      2.7 GB    1
qwen3.5-9b            Qwen3.5-9B-Q4_K_M.gguf      5.7 GB    2 (download last)
```

Managed by `ModelDownloader` in `crates/ai/src/local_models.rs`. Priority 0 models
download sequentially first (immediate fallback), priority 1+ download in parallel.
Local models use `<tool_call>` XML tags for tool invocation since GGUF models lack
native tool-use support.

---

## 3. Provider System

### Provider Trait

```rust
// crates/ai/src/types.rs
#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &str;                           // "anthropic", "openai", "janus", "ollama", etc.
    fn display_name(&self) -> &str;                 // Human-readable name for UI
    fn profile_id(&self) -> &str;                   // Auth profile tracking
    fn handles_tools(&self) -> bool;                // CLI providers handle tools autonomously
    fn supports_tool_result_images(&self) -> bool;  // Anthropic + Gemini support inline images
    async fn stream(&self, req: &ChatRequest) -> Result<EventReceiver, ProviderError>;
}
```

### Provider Implementations

```
                         ┌─────────────────────────────────┐
                         │          Provider Trait          │
                         └───────────────┬─────────────────┘
                                         │
          ┌──────────────┬───────────────┼───────────────┬──────────────┬────────────┐
          ▼              ▼               ▼               ▼              ▼            ▼
  ┌──────────────┐ ┌──────────┐  ┌──────────────┐ ┌──────────┐ ┌───────────┐ ┌──────────┐
  │  Anthropic   │ │  OpenAI  │  │   Gemini     │ │  Ollama  │ │   CLI     │ │  Local   │
  │  Provider    │ │ Provider │  │  Provider    │ │ Provider │ │ Provider  │ │ Provider │
  │              │ │          │  │              │ │          │ │           │ │          │
  │ Raw HTTP SSE │ │ Raw HTTP │  │ REST SSE    │ │ NDJSON   │ │ stdin/    │ │ GGUF FFI │
  │ Anthropic API│ │ OpenAI   │  │ Gemini API  │ │ Ollama   │ │ stdout    │ │ llama.cpp│
  │              │ │ compat   │  │             │ │ API      │ │ streaming │ │          │
  │ Also:       │ │          │  │             │ │          │ │           │ │ Tool use │
  │ cache_ctrl  │ │ Also:    │  │ Also:       │ │          │ │ claude    │ │ via XML  │
  │ 4 breakpts  │ │ Janus    │  │ turn norm.  │ │ Auto-pull│ │ codex     │ │ tags     │
  │ thinking    │ │ DeepSeek │  │ schema conv │ │ 5min TMO │ │ gemini    │ │          │
  └──────────────┘ └──────────┘  └──────────────┘ └──────────┘ └───────────┘ └──────────┘
```

#### OpenAI Provider — Multi-Purpose

The `OpenAIProvider` serves multiple backends via its OpenAI-compatible API:

1. **Direct OpenAI** — `api.openai.com/v1`
2. **DeepSeek** — `api.deepseek.com/v1` (via `set_provider_id("deepseek")`)
3. **Janus Gateway** — NeboLoop's AI gateway (via `set_provider_id("janus")` + `set_bot_id()`)

Janus-specific features:
- `X-Bot-ID` header for per-bot billing
- `X-Lane` header for routing
- Tool stickiness via `provider_metadata` echo
- Budget pool headers (`x-budget-free-available`, `x-budget-gift-available`, etc.)
- Session and weekly rate limit headers

#### Connection Resilience

`OpenAIProvider` implements `ConnectionResetter`:

```rust
pub trait ConnectionResetter {
    fn reset_connections(&self);  // Replace HTTP client to recover from GOAWAY/poison
}
```

The HTTP client is wrapped in `RwLock<reqwest::Client>` so it can be atomically
replaced when persistent HTTP/2 connections become poisoned.

---

## 4. Model Selection Logic

### Selection Pipeline (per iteration of run_loop)

```
  1. model_override / model_preference
     │  (explicit user request or per-entity preference)
     │  Fuzzy-resolved via FuzzyMatcher
     │
     ├── If set: use it directly
     │
     └── If empty:
         │
         2. ModelSelector.select(window_messages)
            │
            ├── classify_task(messages) → TaskType
            │   │  Keywords in last user message:
            │   │  - data:image/  → Vision
            │   │  - data:audio/  → Audio
            │   │  - "step by step", "analyze", "prove"  → Reasoning
            │   │  - "code", "function", "implement", "rust"  → Code
            │   │  - otherwise  → General
            │   │
            │   ▼
            ├── select_for_task(task, exclusions)
            │   │
            │   │  Resolution order:
            │   │  1. task_routing[task_type] (if usable)
            │   │  2. task_fallbacks[task_type] (iterate)
            │   │  3. task_routing["general"] (cross-task fallback)
            │   │  4. defaults.primary
            │   │  5. Any non-gateway active model (static + runtime)
            │   │  6. Empty string if CLI providers loaded (defer to CLI)
            │   │  7. Any gateway model (Janus — last resort)
            │   │  8. defaults.primary (absolute fallback)
            │   │
            │   │  "Usable" check:
            │   │  - Not in exclusion list
            │   │  - Provider is in loaded_providers set
            │   │  - Not in cooldown (or cooldown expired)
            │   │
            │   ▼
            └── Returns "provider/model-id" string
```

### Provider Resolution (run_loop)

Once the selector returns a `"provider/model-id"` string:

```rust
// Parse provider and model portions
let (selected_provider_id, selected_model_name) = selector::parse_model_id(&selected_model);

// Find the provider instance by matching .id()
let idx = if provider_idx > 0 {
    provider_idx % prov_lock.len()     // Round-robin after retries
} else if !selected_provider_id.is_empty() {
    prov_lock.iter()
        .position(|p| p.id() == selected_provider_id)
        .unwrap_or(0)                  // Fallback to index 0
} else {
    0                                  // Default: first provider
};
```

The `model` field in `ChatRequest` is set to `selected_model_name` (just the model
portion without the provider prefix). Each provider's `stream()` method uses this
model if non-empty, otherwise falls back to the provider's default model.

---

## 5. Fuzzy Model Resolution

### FuzzyMatcher (`crates/agent/src/fuzzy.rs`)

Resolves informal model names to canonical `"provider/model-id"` strings.

**Alias Sources (priority order):**
1. User-configured aliases (`settings.json` or models.yaml `aliases:` section)
2. Provider names ("anthropic" -> first active model for that provider)
3. Model IDs ("claude-sonnet-4-6" -> "anthropic/claude-sonnet-4-6")
4. Display names ("Claude Sonnet 4.6" -> "anthropic/claude-sonnet-4-6")
5. Short-form parts from model ID split on `-`, `_`, `.` (e.g., "sonnet" -> "anthropic/claude-sonnet-4-6")
6. Kind tags ("smart", "fast", "cheap", "reasoning")
7. "api" -> first provider with credentials

**Scoring System (0-300+ scale, threshold=50):**

| Match Type                  | Score |
|-----------------------------|-------|
| Exact match                 | +300  |
| Normalized exact            | +250  |
| Prefix match                | +140-150 |
| Normalized prefix           | +120-130 |
| Contains match              | +90-100 |
| Normalized contains         | +70-80 |
| Word match (alias/model/provider) | +40-120 |
| Levenshtein distance <= 3   | +50-200 (scaled) |
| Variant token match         | +60 per match |
| Variant mismatch penalty    | -15 to -30 |

**Variant Tokens:** `lightning`, `preview`, `mini`, `fast`, `turbo`, `lite`, `beta`,
`small`, `nano`, `instant`, `pro`, `thinking`

These variant tokens handle requests like "sonnet fast" vs "sonnet" -- variant tokens
must match or incur a penalty.

### Natural Language Model Switching

```rust
// crates/agent/src/fuzzy.rs
pub fn parse_model_request(input: &str) -> Option<String> {
    // Recognizes: "use sonnet", "switch to opus", "change to gpt-4o", "try claude"
    let patterns = ["use ", "switch to ", "change to ", "try ", "with "];
    // Strips: " model", " please", " for this"
}
```

---

## 6. Provider Construction & Loading

### build_providers() — Server Startup

```rust
// crates/server/src/lib.rs
pub fn build_providers(store, cfg, cli_statuses) -> Vec<Arc<dyn Provider>>
```

**Provider Loading Order:**

```
  1. DB auth_profiles (active=1)
     │
     ├── anthropic → AnthropicProvider::new(api_key, model)
     ├── openai    → OpenAIProvider::new(api_key, model)
     ├── deepseek  → OpenAIProvider::with_base_url(...) + set_provider_id("deepseek")
     ├── google    → GeminiProvider::new(api_key, model)
     ├── ollama    → OllamaProvider::new(base_url, model)
     └── neboloop  → OpenAIProvider::with_base_url(janus_url) + set_provider_id("janus")
                      └── Only if metadata.janus_provider="true"
                      └── Only if has active chat models in catalog
                      └── Deferred to END of list (gateway = last resort)

  2. Auto-create Ollama (if running, has active models, no auth_profile)

  3. CLI providers from models.yaml (if active && installed)
     ├── claude → CLIProvider::new_claude_code(0, port)
     ├── codex  → CLIProvider::new_codex_cli()
     └── gemini → CLIProvider::new_gemini_cli()

  4. Gateway providers appended LAST
     └── Janus goes at the end of the list
```

**Critical ordering rule:** Gateway providers (Janus) are always appended last.
This ensures direct API keys and CLI providers take priority, preventing accidental
Nebo credit consumption for operations that could use the user's own API keys.

### Hot Reload — `Runner.reload_providers()`

When API keys are added/removed at runtime:

```rust
pub async fn reload_providers(&self, providers: Vec<Arc<dyn Provider>>) {
    let loaded_ids = providers.iter().map(|p| p.id().to_string()).collect();
    *self.providers.write().await = providers;
    self.selector.set_loaded_providers(loaded_ids);  // Sync selector filter
    self.selector.rebuild_fuzzy(&HashMap::new());     // Rebuild alias table
}
```

---

## 7. Context Window Management

### Token Estimation

```rust
// crates/agent/src/pruning.rs
const CHARS_PER_TOKEN: usize = 4;
const IMAGE_CHAR_ESTIMATE: usize = 8000;

fn estimate_message_tokens(msg: &ChatMessage) -> usize {
    (msg.content.len() + tc.len() + tr.len() + image_overhead) / CHARS_PER_TOKEN
}
```

### Sliding Window

The runner uses a sliding window approach to prevent context overflow:

```
  Full message history
  ┌──────────────────────────────────────────────────────────┐
  │ [evicted...]  │  [window_messages]                       │
  │               │  ← max_tokens budget                     │
  │               │  ← MAX_MESSAGE_COUNT (80)                │
  │               │  ← current-run messages never evicted    │
  └──────────────────────────────────────────────────────────┘
```

**Key parameters:**
- `DEFAULT_WINDOW_MAX_TOKENS`: 40,000 tokens
- `MAX_MESSAGE_COUNT`: 80 messages hard cap
- `DEFAULT_CONTEXT_TOKEN_LIMIT`: 80,000 tokens (fallback)

### Graduated Context Thresholds

```rust
pub struct ContextThresholds {
    pub warning: usize,       // Trigger micro-compaction
    pub error: usize,         // Log warnings
    pub auto_compact: usize,  // Trigger full compaction
}

// Computed from model's context_window minus prompt overhead:
// auto_compact = min(effective, 500_000)
// error = auto_compact - 10,000
// warning = auto_compact - 20,000
// Minimums: warning=40K, error=50K
```

### Micro-Compaction

When context approaches the warning threshold, tool results older than the
`MICRO_COMPACT_KEEP_RECENT` (3 most recent) are truncated. If more than
`MICRO_COMPACT_COUNT_TRIGGER` (4) compactable results exist, all but the most
recent are stripped. Time-based micro-compaction fires after 5 minutes of
inactivity to avoid re-processing stale results at full input cost.

### Prompt Caching

Both Anthropic and OpenAI providers support multi-breakpoint prompt caching:

```
  System prompt layout:
  ┌────────────────────────┬───────────────────────┬──────────────────────┐
  │  Stable identity/soul  │  Semi-dynamic: skills,│  Dynamic: STRAP,    │
  │  (CACHE_BOUNDARY)      │  model aliases        │  tools, steering    │
  │  ← breakpoint 1        │  ← breakpoint 2       │  (no cache)         │
  │  cache_control:        │  cache_control:        │                     │
  │    ephemeral           │    ephemeral           │                     │
  └────────────────────────┴───────────────────────┴──────────────────────┘
```

Anthropic budgets 4 total cache-control blocks:
- System blocks (breakpoints) + last tool definition + last N messages

---

## 8. Fallback & Error Recovery

### Fallback Chain (defaults section)

```yaml
defaults:
  primary: janus/nebo-1
  fallbacks:
    - anthropic/claude-sonnet-4-6
    - anthropic/claude-haiku-4-5-20251001
```

### Error-Driven Fallback

```
  Provider Error
       │
       ├── ContextOverflow
       │   └── Reduce window size, retry same provider
       │
       ├── Transient (connection reset, timeout, EOF, broken pipe)
       │   ├── mark_failed(model) → exponential cooldown
       │   ├── Try next provider (round-robin)
       │   │   └── BUT: never silently fall from CLI to Janus
       │   ├── Sleep 2s between retries
       │   └── Max MAX_TRANSIENT_RETRIES (10) total
       │
       ├── Retryable (rate_limit, 5xx, billing)
       │   ├── mark_failed(model) → exponential cooldown
       │   ├── Try next provider
       │   │   └── BUT: never silently fall from CLI to Janus
       │   ├── Sleep 2s between retries
       │   └── Max MAX_RETRYABLE_RETRIES (5) total
       │
       ├── Overloaded (HTTP 529)
       │   └── After MAX_OVERLOADS_BEFORE_FALLBACK (3):
       │       └── Switch to cheapest_model
       │
       ├── Auth error → immediate abort
       │
       └── Role ordering error → message history repair
```

### Cooldown System (Exponential Backoff)

```rust
// Backoff: 5s, 10s, 20s, 40s, ... capped at 3600s (1 hour)
let backoff_secs = min(5 * 2^(failure_count - 1), 3600);
```

A model in cooldown is excluded from selection. Cooldowns are checked at selection
time and the model becomes usable again once `cooldown_until` is in the past.

### Non-Gateway Provider Preference

```rust
// crates/agent/src/runner.rs
pub(crate) fn prefer_non_gateway(providers: &[Arc<dyn Provider>]) -> Option<Arc<dyn Provider>> {
    providers
        .iter()
        .find(|p| p.id() != "janus")
        .cloned()
        .or_else(|| providers.first().cloned())
}
```

Used by background operations (memory extraction, compaction, summarization) to avoid
burning Janus credits when a CLI or direct-API provider is available. Falls back to
Janus only when no other option exists.

### Cheapest Model Selection

```rust
pub fn get_cheapest_model(&self) -> String {
    // Cost formula: input_price + output_price * 2.0
    // Filter: active models from credentialed providers
    // Fallback: models with kind "cheap" or "fast"
    // Final fallback: default_model
}
```

Used for:
- Sidecar tasks (screenshot verification)
- Large input summarization
- Background memory operations
- Post-overload fallback

---

## 9. Pricing Awareness

### Cost-Based Model Ranking

The `ModelInfo.input_price` and `ModelInfo.output_price` fields (per million tokens)
directly influence selection:

1. **`get_cheapest_model()`** — ranks by `input_price + output_price * 2.0` (output
   weighted double since output tokens are typically more expensive)
2. **Sidecar model** — `defaults.fallbacks[0]` (the cheapest fallback)
3. **Gateway (Janus) ordering** — always last in provider list

### Janus Budget Tracking

Rate limit headers from Janus provide real-time budget visibility:

```rust
pub struct RateLimitMeta {
    // Standard rate limits
    pub remaining_requests: Option<u64>,
    pub remaining_tokens: Option<u64>,

    // Session window (per-conversation)
    pub session_limit_credits: Option<u64>,
    pub session_remaining_credits: Option<u64>,
    pub session_reset_at: Option<String>,

    // Weekly window
    pub weekly_limit_credits: Option<u64>,
    pub weekly_remaining_credits: Option<u64>,
    pub weekly_reset_at: Option<String>,

    // Budget pools (microdollars)
    pub budget_free_available: Option<u64>,
    pub budget_gift_available: Option<u64>,
    pub budget_credits_cents: Option<u64>,
    pub budget_active_pool: Option<String>,  // "free", "gift", "credits"
}
```

Quota warnings fire at 80% usage for both session and weekly windows, visible to
the user via WebSocket events and injected into steering directives.

---

## 10. Task-Based Routing

### Task Types

```rust
pub enum TaskType {
    Vision,     // Image analysis
    Audio,      // Audio processing
    Reasoning,  // Complex analysis, proofs, step-by-step
    Code,       // Programming tasks
    General,    // Everything else
}
```

### Classification Keywords

| Task Type   | Detection Keywords                                                    |
|-------------|-----------------------------------------------------------------------|
| Vision      | `data:image/`, `"type":"image"`                                       |
| Audio       | `data:audio/`, `"type":"audio"`                                       |
| Reasoning   | "think through", "analyze", "prove", "step by step", "theorem", etc.  |
| Code        | "code", "function", "implement", "refactor", "debug", "python", etc.  |
| General     | Fallback for anything not matching above                              |

### Default Task Routing (models.yaml)

```yaml
task_routing:
  vision: janus/nebo-1       # Server-side picks best vision model
  audio: janus/nebo-1
  reasoning: janus/nebo-1
  code: janus/nebo-1
  general: janus/nebo-1
  fallbacks:
    vision:     [anthropic/claude-sonnet-4-6, openai/gpt-5.4]
    reasoning:  [anthropic/claude-opus-4-6, openai/gpt-5.4]
    code:       [anthropic/claude-sonnet-4-6, openai/gpt-5.4]
    general:    [anthropic/claude-haiku-4-5-20251001, openai/gpt-5.4-nano]
```

### Thinking Mode

Extended thinking is automatically enabled when:
1. The task classifies as `Reasoning`
2. The selected model supports thinking (checked via `supports_thinking()`)

```rust
pub fn supports_thinking(&self, model_id: &str) -> bool {
    // Check capabilities: ["thinking", "reasoning", "extended_thinking"]
    // Name-based fallback: contains "opus", "o1", or "o3"
}
```

When enabled, Anthropic's thinking budget is set to 10,000 tokens, and Claude CLI
uses `--effort high`.

---

## 11. Lane Routing

### Lane-Specific Models

```rust
pub struct LaneRouting {
    pub heartbeat: String,   // Proactive tick model
    pub events: String,      // Event-triggered workflow model
    pub comm: String,        // NeboLoop message model
    pub subagent: String,    // Sub-agent spawn model
}
```

Lane routing allows background lanes (heartbeat, events) to use cheaper models
than the interactive chat lane, reducing costs for autonomous operations.

### Sidecar Model

The sidecar model (`defaults.fallbacks[0]`) is used for lightweight auxiliary tasks:

```rust
// crates/agent/src/sidecar.rs
fn sidecar_model() -> String {
    config::ModelsConfig::load()
        .sidecar_model()
        .unwrap_or_else(|| "claude-haiku-4-5-20251001".into())
}
```

Currently used for browser automation screenshot verification (150 max tokens,
temperature 0.0).

---

## 12. Runtime Model Discovery

### Ollama Auto-Discovery

Ollama models are not listed in models.yaml. Instead, they're discovered at runtime:

```rust
// crates/ai/src/providers/ollama.rs
pub async fn list_ollama_models(base_url: &str) -> Result<Vec<String>, ProviderError>
pub async fn check_ollama_available(base_url: &str) -> bool
pub async fn ensure_ollama_model(base_url: &str, model: &str) -> Result<(), ProviderError>
```

Discovered models are injected into the selector:

```rust
selector.inject_provider_models("ollama", models);
selector.rebuild_fuzzy(&user_aliases);
```

### Local GGUF Model Discovery

```rust
// crates/ai/src/local_models.rs
pub fn find_local_models(dir: &Path) -> Vec<LocalModelInfo>
// Scans for .gguf files, sorted by size descending (largest first for auto-select)
```

---

## 13. Error Classification

### ProviderError Variants

```rust
pub enum ProviderError {
    Api { code, message, retryable },   // Generic API error
    ContextOverflow,                     // Context window exceeded
    RateLimit,                           // 429 Too Many Requests
    Auth(String),                        // 401 Unauthorized
    Request(String),                     // Network/connection error
    Stream(String),                      // SSE stream error
}
```

### Error Classification Functions

```rust
pub fn is_context_overflow(err) -> bool     // Context window exceeded
pub fn is_overloaded(err) -> bool           // HTTP 529 / "overloaded"
pub fn is_transient_error(err) -> bool      // Connection reset, timeout, EOF, etc.
pub fn is_role_ordering_error(err) -> bool  // Message role alternation violation
pub fn classify_error_reason(err) -> &str   // "rate_limit"|"auth"|"billing"|"timeout"|"other"
```

### Error-to-Cooldown Mapping

```
Error Reason     Cooldown Category    Retry Behavior
──────────────── ──────────────────── ─────────────────────────────
rate_limit       Exponential backoff  Try next provider, max 5
auth             No retry             Immediate abort
billing          No retry             Immediate abort
context_overflow No backoff           Reduce window, same provider
timeout          Backoff + next prov  Max 10 transient retries
provider (5xx)   Backoff + next prov  Max 5 retryable retries
overloaded (529) After 3: cheapest    Switch to get_cheapest_model
```

---

## 14. Key Function Signatures

### Model Selection

```rust
// crates/agent/src/selector.rs
impl ModelSelector {
    pub fn new(config: ModelRoutingConfig) -> Self;
    pub fn select(&self, messages: &[ChatMessage]) -> String;
    pub fn select_with_exclusions(&self, messages: &[ChatMessage], exclude: &[String]) -> String;
    pub fn classify_task(&self, messages: &[ChatMessage]) -> TaskType;
    pub fn resolve_fuzzy(&self, input: &str) -> Option<String>;
    pub fn get_cheapest_model(&self) -> String;
    pub fn supports_thinking(&self, model_id: &str) -> bool;
    pub fn mark_failed(&self, model_id: &str);
    pub fn clear_failed(&self);
    pub fn get_cooldown_remaining(&self, model_id: &str) -> Duration;
    pub fn set_loaded_providers(&self, provider_ids: Vec<String>);
    pub fn inject_provider_models(&self, provider: &str, models: Vec<ModelInfo>);
    pub fn rebuild_fuzzy(&self, user_aliases: &HashMap<String, String>);
}
```

### Fuzzy Matching

```rust
// crates/agent/src/fuzzy.rs
impl FuzzyMatcher {
    pub fn new(provider_models, user_aliases, provider_credentials) -> Self;
    pub fn resolve(&self, input: &str) -> Option<String>;
    pub fn add_alias(&mut self, alias: &str, model_id: &str);
    pub fn get_aliases_text(&self) -> String;  // For system prompt injection
}
pub fn parse_model_request(input: &str) -> Option<String>;
```

### Provider Construction

```rust
// crates/ai/src/providers/
AnthropicProvider::new(api_key: String, model: String) -> Self
OpenAIProvider::new(api_key: String, model: String) -> Self
OpenAIProvider::with_base_url(api_key, model, base_url) -> Self
GeminiProvider::new(api_key: String, model: String) -> Self
OllamaProvider::new(base_url: String, model: String) -> Self
CLIProvider::new_claude_code(max_turns: u32, server_port: u16) -> Self
CLIProvider::new_codex_cli() -> Self
CLIProvider::new_gemini_cli() -> Self
LocalProvider::new(model_path: &str, model_name: &str) -> Self
```

### Configuration

```rust
// crates/config/src/models.rs
impl ModelsConfig {
    pub fn load() -> Self;                               // Load with embedded fallback
    pub fn save(&self) -> Result<(), String>;            // Save to ~/.nebo/data/
    pub fn update_model(&mut self, provider, id, update); // Toggle active/kind/preferred
    pub fn default_model_for_provider(&self, provider) -> Option<String>;
    pub fn model_for_task(&self, task: &str) -> Option<String>;
    pub fn sidecar_model(&self) -> Option<String>;
}
```

---

## 15. Configuration Options

### Environment Variables

| Variable              | Purpose                              |
|-----------------------|--------------------------------------|
| `ANTHROPIC_API_KEY`   | Anthropic API key                    |
| `OPENAI_API_KEY`      | OpenAI API key                       |
| `GOOGLE_API_KEY`      | Google/Gemini API key                |
| `DEEPSEEK_API_KEY`    | DeepSeek API key                     |
| `NEBOLOOP_JANUS_URL`  | Override Janus gateway URL           |

### UI Configuration (Settings > Providers)

- Add/remove API keys (stored encrypted in SQLite)
- Toggle individual models active/inactive
- Configure task routing overrides
- Enable/disable CLI providers
- Set lane-specific model preferences

### ChatRequest Parameters

```rust
pub struct ChatRequest {
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub max_tokens: i32,              // Default: 16,384 (escalated: 65,536)
    pub temperature: f64,             // Default: 0.7
    pub system: String,               // Full system prompt (static + dynamic)
    pub static_system: String,        // Stable prefix for cache splitting
    pub model: String,                // Model name (without provider prefix)
    pub enable_thinking: bool,        // Extended thinking mode
    pub metadata: Option<HashMap>,    // Janus tool stickiness metadata
    pub cache_breakpoints: Vec<usize>,// Byte offsets for prompt caching
    pub cancel_token: Option<CancellationToken>,
}
```

### Runner Constants

```rust
const DEFAULT_MAX_ITERATIONS: usize = 100;
const EXTENDED_MAX_ITERATIONS: usize = 200;
const DEFAULT_CONTEXT_TOKEN_LIMIT: usize = 80_000;
const MAX_TRANSIENT_RETRIES: usize = 10;
const MAX_RETRYABLE_RETRIES: usize = 5;
const MAX_OVERLOADS_BEFORE_FALLBACK: usize = 3;
const DEFAULT_MAX_OUTPUT_TOKENS: i32 = 16_384;
const ESCALATED_MAX_OUTPUT_TOKENS: i32 = 65_536;
const MAX_OUTPUT_RECOVERY_ATTEMPTS: usize = 3;
```

---

## 16. Cross-System Interactions

### Model Selection → Prompt Assembly

The selected model and provider influence prompt construction:

```rust
let dctx = prompt::DynamicContext {
    provider_name: selected_provider_id,   // Affects provider-specific hints
    model_name: selected_model_name,       // Displayed in system prompt
    // ... other fields
};
```

Model aliases are injected into the system prompt so the LLM knows what models
are available for user-requested switching.

### Model Selection → Steering

The provider ID is passed to the steering pipeline, which uses it to skip
provider-specific steering rules (e.g., Janus-only quota warnings).

### Model Selection → Tool Filtering

Some tools are filtered based on provider capabilities:

```rust
fn supports_tool_result_images(&self) -> bool
// Anthropic + Gemini: true → pass screenshots directly
// Others: false → convert to text via sidecar vision model
```

### Provider → Rate Limit → Budget UI

```
  Provider stream response headers
       │
       ▼
  RateLimitMeta (parsed in OpenAI/Anthropic handlers)
       │
       ▼
  StreamEvent::RateLimit → run_loop quota tracking
       │
       ├── state.quota_warning (>=80% usage)
       │   └── Injected into steering directives
       │
       └── WS broadcast → Frontend budget display
```

### Selector → Runner Fallback

```
  ModelSelector.mark_failed("provider/model")
       │
       ▼
  CooldownState { failure_count++, cooldown_until = now + backoff }
       │
       ▼
  Next iteration: select_for_task() skips cooled-down models
       │
       ▼
  provider_idx++ → round-robin to next Provider in list
       │
       └── Guard: never silently fall from CLI → Janus
```
