# Janus Gateway — Internal Reference

Janus is NeboLoop's managed AI gateway. An OpenAI-compatible proxy at `https://janus.neboloop.com` that routes requests to upstream LLM providers (Anthropic, OpenAI, Google, etc.) so non-technical users never touch an API key.

**Zero public documentation exists.** Everything below is derived from the Nebo codebase.

---

## Why It Exists

Nebo's ICP (realtors, lawyers, freelancers) will never get an Anthropic API key. Janus is the "one click, no API keys" onramp: user signs up for NeboLoop, opts in to Janus, done.

---

## Architecture

```
Nebo (desktop)
  └─ OpenAI SDK ─→ https://janus.neboloop.com/v1
                     │
                     ├─ Routes to Anthropic
                     ├─ Routes to OpenAI
                     ├─ Routes to Gemini
                     └─ (model selection is server-side — Nebo sends model="janus")
```

Nebo creates a standard `OpenAIProvider` with `providerID="janus"` and base URL `janus.neboloop.com/v1`. The `GatewayRequest` proto has **no model field** — Janus decides routing internally.

---

## File Map

| Layer | File (in nebo repo) | Purpose |
|-------|---------------------|---------|
| Config | `internal/config/config.go:104-105` | Default URL: `https://janus.neboloop.com` |
| URL bootstrap | `cmd/nebo/root.go:166` | `SetJanusURL()` at startup |
| Provider loading | `cmd/nebo/providers.go:262-304` | Creates `OpenAIProvider` with Janus base URL, `providerID="janus"`, `X-Bot-ID` |
| Wire protocol | `internal/agent/ai/api_openai.go` | OpenAI SDK with Janus-specific middleware |
| Rate limit capture | `api_openai.go:62-103` | HTTP middleware parses 6 `X-RateLimit-*` headers |
| In-memory store | `internal/svc/servicecontext.go:85` | `JanusUsage atomic.Pointer[ai.RateLimitInfo]` |
| Usage API | `internal/handler/neboloop/handlers.go:227-259` | `GET /api/v1/neboloop/janus/usage` |
| Account status | `handlers.go:175-185` | `GET /api/v1/neboloop/account/status` (includes `janusProvider` bool) |
| OAuth flow | `internal/handler/neboloop/oauth.go` | `?janus=true` query param triggers opt-in |
| Metadata persist | `handlers.go:264-314` | `storeNeboLoopProfile()` carries forward `janus_provider=true` on re-auth |
| Steering | `internal/agent/steering/generators.go:250-288` | Quota warning at >80% usage (once per session) |
| Steering template | `internal/agent/steering/templates.go:71-73` | Warning message text |
| Proto | `proto/apps/v0/gateway.proto` | `GatewayService` gRPC interface (generalized gateway for app platform) |
| Models config | `internal/defaults/dotnebo/models.yaml:168-183` | Janus model definitions |
| Frontend: providers | `app/src/routes/(app)/settings/ProvidersSection.svelte` | Toggle + usage bars |
| Frontend: neboloop | `app/src/routes/(app)/settings/neboloop/+page.svelte` | Account page with usage display |
| Runner callback | `internal/agent/runner/runner.go:211-215` | `SetRateLimitStore()` wires callback |
| Agent wiring | `cmd/nebo/agent.go:963-968` | Connects runner callback to `svcCtx.JanusUsage.Store()` |

---

## Activation Flow

1. User starts NeboLoop OAuth with `?janus=true` query param
2. OAuth completes -> `storeNeboLoopProfile()` writes `janus_provider: "true"` in auth profile metadata
3. On provider load, `loadProvidersFromDB()` finds neboloop profile with `janus_provider=true`
4. Creates `OpenAIProvider(apiKey=JWT, model="janus", baseURL="https://janus.neboloop.com/v1")`
5. Sets `providerID="janus"` and `botID` from `plugin_settings` table

**Carry-forward safety:** Re-auth preserves `janus_provider=true` from existing profile so opt-in is never lost.

---

## Request Headers (Nebo -> Janus)

| Header | Value | Purpose |
|--------|-------|---------|
| `Authorization` | `Bearer <NeboLoop JWT>` | Auth (via OpenAI SDK) |
| `X-Bot-ID` | UUID | Per-bot billing (immutable, from `plugin_settings.bot_id`) |
| `X-Lane` | `main`/`events`/`subagent`/`heartbeat`/`comm` | Request routing/analytics (only when `providerID=="janus"`) |

Bot ID is read from:
```sql
SELECT ps.setting_value FROM plugin_settings ps
  JOIN plugin_registry pr ON pr.id = ps.plugin_id
  WHERE pr.name = 'neboloop' AND ps.setting_key = 'bot_id'
```

---

## Response Headers (Janus -> Nebo)

| Header | Example | Purpose |
|--------|---------|---------|
| `X-RateLimit-Session-Limit-Tokens` | `500000` | Max tokens per session window |
| `X-RateLimit-Session-Remaining-Tokens` | `350000` | Tokens remaining in session |
| `X-RateLimit-Session-Reset` | RFC3339 timestamp | Session window reset time |
| `X-RateLimit-Weekly-Limit-Tokens` | `5000000` | Max tokens per billing week |
| `X-RateLimit-Weekly-Remaining-Tokens` | `4200000` | Tokens remaining this week |
| `X-RateLimit-Weekly-Reset` | RFC3339 timestamp | Weekly window reset time |

Captured by `captureRateLimitHeaders` middleware in `api_openai.go:62-103`.

---

## Rate Limit Storage

**Currently in-memory only** — stored as `atomic.Pointer[ai.RateLimitInfo]` on ServiceContext. Lost on restart; repopulated on next Janus API response.

```go
// internal/svc/servicecontext.go:85
JanusUsage atomic.Pointer[ai.RateLimitInfo]
```

**Callback chain:**
1. `captureRateLimitHeaders` middleware parses headers -> stores on `OpenAIProvider.rateLimit`
2. Runner reads via `provider.GetRateLimit()` after each turn (`runner.go:1868-1877`)
3. Runner calls `rateLimitStore` callback -> `svcCtx.JanusUsage.Store(rl)` (`agent.go:966`)
4. Frontend polls `GET /api/v1/neboloop/janus/usage` which reads from that pointer

**TODO:** Persist to `<data_dir>/janus_usage.json` so usage survives restarts.

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

---

## Steering Integration

`janusQuotaWarning` generator (`steering/generators.go:250-288`):
- Fires when either window is >80% consumed (`remaining/limit < 0.20`)
- Once per session (tracked in `janusQuotaWarnedSessions` sync.Map)
- Injects steering message telling agent to warn user and suggest upgrading

Template (`steering/templates.go:71-73`):
```
Your NeboLoop Janus token budget is %d%% used (%s window running low).
Warn the user that their AI usage quota is running low. Suggest shorter prompts or upgrading their plan.
You can open the billing page with: agent(resource: profile, action: open_billing)
```

---

## Streaming Quirks (4 Janus-specific workarounds)

All in `api_openai.go`:

1. **Tool name duplication** (lines 356-365) — Janus sends tool name in every chunk. SDK accumulator concatenates -> `"agentagent..."`. Fix: track seen names per tool index, clear duplicates.

2. **Complete JSON args in one chunk** (lines 366-374) — Unlike standard OpenAI (incremental), Janus sends full JSON arguments at once. Fix: detect valid JSON, mark as seen, clear subsequent.

3. **Missing SSE `[DONE]` sentinel** (lines 410-417) — Janus doesn't always send `[DONE]` after `finish_reason`. Without early break, `stream.Next()` blocks ~120s. Fix: break on any non-empty `finish_reason`.

4. **Non-null assistant content** (lines 288-293) — Routing to Gemini backends: null content + tool_calls = rejection. Fix: set content to `" "` when empty but tool_calls present.

---

## Models (from models.yaml)

| ID | Purpose | Context | Capabilities |
|----|---------|---------|-------------|
| `janus` | Chat completion (server-side routing) | 200k | vision, tools, streaming, code, reasoning |
| `text-embedding-small` | Embeddings for memory/search | 8,191 | embeddings |
| `text-embedding-large` | Higher-quality embeddings | 8,191 | embeddings |

All `active: true` by default. Other providers are `active: false` until user adds keys.

---

## GatewayService Proto (app platform interface)

`proto/apps/v0/gateway.proto` — generalized gRPC interface for gateway apps:

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
- Current Janus uses OpenAI REST directly; this proto is for app-platform gateway apps

---

## Embedding Provider Priority

From `cmd/nebo/agent.go`: **Janus (centralized) -> OpenAI (direct) -> Ollama (local)**

When Janus is the only configured provider and default task routing points to unavailable models, the runner falls back to Janus.

---

## Frontend Usage Display

Both `ProvidersSection.svelte` and `neboloop/+page.svelte` show:
- Progress bars for session usage (color-coded: warning at 80%+)
- Progress bars for weekly usage
- Reset time display
- Janus toggle (enable/disable as AI provider)

API calls:
- `GET /api/v1/neboloop/account/status` -> `NeboLoopAccountStatusResponse` (includes `janusProvider` bool)
- `GET /api/v1/neboloop/janus/usage` -> `NeboLoopJanusUsageResponse` (session + weekly windows)

---

## Types (internal/types/types.go)

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
