# Janus Gateway — Product Requirements Document

> Janus is the NeboAI AI gateway. It sits between Nebo desktop clients and
> upstream AI providers, handling authentication, usage metering, rate limiting,
> model routing, and credit billing. Named after the Roman god of gates and
> transitions.
>
> **Current state:** Janus proxies LLM chat completions (OpenAI-compatible
> `/v1/chat/completions`) and tracks token usage. This PRD defines the full
> vision including voice, compute, and marketplace capabilities.

---

## Table of Contents

1. [Problem Statement](#1-problem-statement)
2. [Users & Stakeholders](#2-users--stakeholders)
3. [Architecture Overview](#3-architecture-overview)
4. [Current Capabilities (Shipped)](#4-current-capabilities-shipped)
5. [Voice Pipeline](#5-voice-pipeline)
6. [Compute Sandbox](#6-compute-sandbox)
7. [Image Generation](#7-image-generation)
8. [Billing & Credits](#8-billing--credits)
9. [Rate Limiting & Fair Use](#9-rate-limiting--fair-use)
10. [Provider Routing](#10-provider-routing)
11. [Observability](#11-observability)
12. [Security](#12-security)
13. [API Reference](#13-api-reference)
14. [Client Integration](#14-client-integration)
15. [Rollout Plan](#15-rollout-plan)
16. [Open Questions](#16-open-questions)

---

## 1. Problem Statement

Nebo users interact with multiple AI providers (Anthropic, OpenAI, Google,
local models). Each provider has its own API format, auth mechanism, billing
model, and rate limits. Users shouldn't need API keys from every provider or
worry about per-provider billing.

**Janus solves this by:**
- Providing a single gateway that normalizes all providers behind an
  OpenAI-compatible interface
- Managing API keys server-side so users never handle raw provider credentials
- Tracking and metering usage across all providers in a unified credit system
- Enforcing rate limits and fair-use policies per plan tier
- Enabling new capabilities (voice, compute, image gen) without client changes

---

## 2. Users & Stakeholders

| Persona | Needs |
|---------|-------|
| **Nebo Free user** | Limited credits, access to basic models, voice via local only |
| **Nebo Pro user** | Generous credits, all models, cloud voice (TTS + STT), compute sandbox |
| **Nebo Team admin** | Team-wide usage dashboards, per-seat budgets, audit logs |
| **Nebo agent developer** | Predictable API, model fallback, tool stickiness routing |
| **NeboAI platform** | Revenue from credit consumption, provider cost optimization |

---

## 3. Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                    Nebo Desktop Client                       │
│  OpenAIProvider(base_url: janus_url)                        │
│  Headers: Authorization, X-Bot-ID, X-Lane                   │
├─────────────────────────────────────────────────────────────┤
                           │ HTTPS
                           ▼
┌─────────────────────────────────────────────────────────────┐
│                      Janus Gateway                           │
│                   janus.neboai.com                          │
│                                                              │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌────────────┐  │
│  │ Auth &   │  │ Router   │  │ Metering │  │ Rate       │  │
│  │ Identity │  │          │  │ & Billing│  │ Limiter    │  │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────┬───────┘  │
│       │              │              │              │          │
│  ┌────▼──────────────▼──────────────▼──────────────▼───────┐ │
│  │                   Provider Adapters                      │ │
│  │  ┌─────────┐ ┌────────┐ ┌────────┐ ┌────────┐          │ │
│  │  │Anthropic│ │ OpenAI │ │ Google │ │ Future │          │ │
│  │  └─────────┘ └────────┘ └────────┘ └────────┘          │ │
│  └─────────────────────────────────────────────────────────┘ │
│                                                              │
│  ┌─────────────────────────────────────────────────────────┐ │
│  │              Capability Endpoints                        │ │
│  │  /v1/chat/completions  (LLM — shipped)                   │ │
│  │  /v1/audio/speech      (TTS — Phase 1)                   │ │
│  │  /v1/audio/transcriptions (STT — Phase 1)                │ │
│  │  /v1/images/generations (Image — Phase 2)                │ │
│  │  /v1/execute           (Compute sandbox — Phase 3)       │ │
│  └─────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
                           │
              ┌────────────┼────────────┐
              ▼            ▼            ▼
         Anthropic      OpenAI       Google
           API           API          API
```

### Design Principles

1. **OpenAI-compatible.** Janus speaks the OpenAI API format. Clients use
   the standard `OpenAIProvider` with `base_url` pointed at Janus.
2. **Credits, not API keys.** Users buy Nebo credits. Janus converts credits
   to provider-specific API calls at cost + margin.
3. **Provider-agnostic.** Adding a new upstream provider requires only a new
   adapter — no client changes.
4. **Streaming-first.** All endpoints support SSE streaming. No buffering
   of full responses before forwarding.
5. **Fail-open for local.** When Janus is unreachable, the client falls back
   to local models (Ollama, GGUF) and local voice (Piper, whisper.cpp).

---

## 4. Current Capabilities (Shipped)

### 4.1 LLM Chat Completions

**Endpoint:** `POST /v1/chat/completions`

Proxies to Anthropic, OpenAI, or Google based on the `model` field.
OpenAI-compatible request/response format with SSE streaming.

**Headers (client → Janus):**
| Header | Purpose |
|--------|---------|
| `Authorization: Bearer <jwt>` | Bot identity + plan tier |
| `X-Bot-ID: <bot_id>` | Per-bot billing isolation |
| `X-Lane: <lane>` | Routing hint for tool stickiness |

**Headers (Janus → client, in SSE stream):**
| Header | Purpose |
|--------|---------|
| `X-Session-Limit-Tokens` | Session token budget |
| `X-Session-Remaining-Tokens` | Session tokens remaining |
| `X-Session-Reset-At` | Session reset timestamp (ISO8601) |
| `X-Weekly-Limit-Tokens` | Weekly token budget |
| `X-Weekly-Remaining-Tokens` | Weekly tokens remaining |
| `X-Weekly-Reset-At` | Weekly reset timestamp |

**Model routing:**
- `model: "claude-sonnet-4-6"` → Anthropic API
- `model: "gpt-4o"` → OpenAI API
- `model: "gemini-2.5-pro"` → Google API
- `model: "auto"` → Janus picks best available model for the task

### 4.2 Usage Tracking

**Endpoint:** `GET /v1/usage`

Returns current credit balance, session/weekly consumption, and per-provider
breakdown.

```json
{
  "plan": "pro",
  "credits_remaining": 42500,
  "session": {
    "input_tokens": 12340,
    "output_tokens": 5670,
    "limit_tokens": 500000,
    "reset_at": "2026-05-03T12:00:00Z"
  },
  "weekly": {
    "input_tokens": 145000,
    "output_tokens": 67000,
    "limit_tokens": 5000000,
    "reset_at": "2026-05-05T00:00:00Z"
  }
}
```

---

## 5. Voice Pipeline

### 5.1 Overview

Janus adds TTS and STT as first-class gateway capabilities. The client
calls Janus voice endpoints using the same auth and billing. Janus routes
to the cheapest/fastest upstream provider and bills in credits.

### 5.2 Text-to-Speech

**Endpoint:** `POST /v1/audio/speech`

**Request:**
```json
{
  "model": "tts-1",
  "input": "Hello, I'm your AI assistant.",
  "voice": "nova",
  "response_format": "wav",
  "speed": 1.0
}
```

**Response:** Binary audio stream (`Content-Type: audio/wav` or `audio/mp3`).

**Streaming:** For real-time playback, the client can set
`Accept: text/event-stream` to receive chunked audio as base64-encoded
SSE events:

```
data: {"chunk": "<base64 audio>", "sequence": 0}
data: {"chunk": "<base64 audio>", "sequence": 1}
data: [DONE]
```

**Provider routing:**
| Model | Provider | Quality | Latency | Cost |
|-------|----------|---------|---------|------|
| `tts-1` | OpenAI | Good | ~200ms | $15/1M chars |
| `tts-1-hd` | OpenAI | Excellent | ~400ms | $30/1M chars |
| `eleven-turbo` | ElevenLabs | Best | ~300ms | Variable |
| `google-standard` | Google Cloud TTS | Good | ~150ms | $4/1M chars |
| `google-wavenet` | Google Cloud TTS | Excellent | ~200ms | $16/1M chars |

**Voices:** Normalized voice IDs across providers. Janus maps
`voice: "nova"` to the appropriate provider-specific voice.

| Voice ID | Style | OpenAI | ElevenLabs | Google |
|----------|-------|--------|------------|--------|
| `nova` | Warm, conversational | nova | Rachel | en-US-Neural2-F |
| `echo` | Clear, professional | echo | Adam | en-US-Neural2-D |
| `alloy` | Balanced, neutral | alloy | Bella | en-US-Neural2-C |
| `onyx` | Deep, authoritative | onyx | Antoni | en-US-Neural2-A |
| `shimmer` | Bright, energetic | shimmer | Elli | en-US-Neural2-E |

**Credit cost:** Billed per character of input text. 1 credit = ~100
characters. Exact rate varies by model/provider.

### 5.3 Speech-to-Text

**Endpoint:** `POST /v1/audio/transcriptions`

**Request:** Multipart form data:
- `file` — Audio file (wav, mp3, webm, ogg, flac)
- `model` — `whisper-1` (OpenAI), `gemini-transcribe` (Google)
- `language` — ISO 639-1 code (optional, improves accuracy)
- `response_format` — `json`, `text`, `verbose_json`

**Response:**
```json
{
  "text": "Hello, I'm your AI assistant.",
  "language": "en",
  "duration": 2.4
}
```

**Provider routing:**
| Model | Provider | Languages | Max Duration | Cost |
|-------|----------|-----------|-------------|------|
| `whisper-1` | OpenAI | 98 | 25 min | $0.006/min |
| `gemini-transcribe` | Google | 125 | 480 min | $0.004/min |

**Credit cost:** Billed per second of audio. 1 credit = ~10 seconds.

### 5.4 Client Integration

The Nebo desktop client uses a tiered voice strategy:

```
Voice request
  │
  ├─ Local engines available? (piper / whisper.cpp)
  │   ├─ Yes → Use local (free, private, fast)
  │   └─ No ──┐
  │            │
  ├─ User has API keys? (OpenAI / Google)
  │   ├─ Yes → Direct provider call (user's own billing)
  │   └─ No ──┐
  │            │
  └─ Janus gateway (Nebo credits)
      └─ POST janus.neboai.com/v1/audio/speech
```

**Fallback order matches the existing provider priority:**
1. Local (free, always available)
2. Direct API keys (user's subscription)
3. Janus (Nebo credits, always last)

---

## 6. Compute Sandbox

### 6.1 Overview

Cloud-side code execution for skills that need runtimes (Python, Node.js)
not installed locally. Currently a `TODO` in `execute_tool.rs:486`.

**Endpoint:** `POST /v1/execute`

**Request:**
```json
{
  "runtime": "python3",
  "script": "print('hello')",
  "files": {
    "data.csv": "<base64>"
  },
  "timeout_secs": 30,
  "memory_mb": 512
}
```

**Response (streaming):**
```
data: {"type": "stdout", "text": "hello\n"}
data: {"type": "exit", "code": 0, "duration_ms": 234}
data: [DONE]
```

### 6.2 Sandboxing

- Firecracker microVMs or gVisor containers per execution
- No network access by default (opt-in via `"network": true`)
- 30-second default timeout (configurable up to 300s for Pro)
- Ephemeral — VM destroyed after response completes
- Max 512MB memory (Free), 2GB (Pro), 4GB (Team)

### 6.3 Credit cost

Billed per second of compute time + memory tier.
- Base: 1 credit/second (256MB)
- Pro: 2 credits/second (2GB)
- GPU: 10 credits/second (when available)

### 6.4 Tier Limits

| Tier | Timeout | Memory | Concurrent | Network |
|------|---------|--------|------------|---------|
| Free | 30s | 512MB | 1 | No |
| Pro | 300s | 2GB | 3 | Opt-in |
| Team | 600s | 4GB | 10 | Opt-in |

---

## 7. Image Generation

### 7.1 Overview

**Endpoint:** `POST /v1/images/generations`

OpenAI-compatible image generation, routed to the best available provider.

**Request:**
```json
{
  "model": "dall-e-3",
  "prompt": "A serene mountain landscape at sunset",
  "n": 1,
  "size": "1024x1024",
  "quality": "standard",
  "response_format": "url"
}
```

**Provider routing:**
| Model | Provider | Quality | Cost |
|-------|----------|---------|------|
| `dall-e-3` | OpenAI | Excellent | $0.04/image |
| `imagen-3` | Google | Excellent | $0.04/image |
| `flux-1.1` | Replicate | Good | $0.03/image |

**Credit cost:** Fixed per image based on model + size + quality.

---

## 8. Billing & Credits

### 8.1 Credit Model

Credits are the universal currency across all Janus capabilities.
Users purchase credit packs or receive monthly allocations with their plan.

| Plan | Monthly Credits | Overage Rate |
|------|----------------|--------------|
| Free | 5,000 | N/A (hard cap) |
| Pro ($20/mo) | 100,000 | $10/10K credits |
| Team ($40/seat/mo) | 200,000/seat | $8/10K credits |

### 8.2 Credit Conversion Rates

| Capability | Unit | Credits |
|------------|------|---------|
| LLM input tokens | 1K tokens | 1 credit |
| LLM output tokens | 1K tokens | 3 credits |
| LLM cache read tokens | 1K tokens | 0.1 credit |
| TTS characters | 1K chars | 1 credit |
| STT audio | 1 minute | 6 credits |
| Image generation | 1 image | 40 credits |
| Compute time | 1 second | 1 credit |

### 8.3 Billing Events

Every API call produces a billing event stored in the metering database:

```json
{
  "event_id": "evt_...",
  "bot_id": "bot_...",
  "timestamp": "2026-05-03T10:30:00Z",
  "capability": "chat",
  "model": "claude-sonnet-4-6",
  "provider": "anthropic",
  "input_tokens": 1234,
  "output_tokens": 567,
  "credits_consumed": 2.934,
  "latency_ms": 1230,
  "status": "success"
}
```

### 8.4 Credit Pools

Credits are consumed in priority order:

```
1. Plan allocation (monthly reset)
2. Bonus credits (gifted, promotional, one-time)
3. Pay-as-you-go balance (auto-refill optional)
```

When all pools are exhausted, requests return `429 Too Many Requests` with
a `Retry-After` header indicating when plan credits reset.

---

## 9. Rate Limiting & Fair Use

### 9.1 Rate Limit Tiers

| Limit | Free | Pro | Team |
|-------|------|-----|------|
| Requests/minute | 10 | 60 | 120 |
| Tokens/session | 50K | 500K | 1M |
| Tokens/week | 250K | 5M | 20M |
| Concurrent streams | 1 | 3 | 10 |
| TTS chars/minute | 500 | 5,000 | 10,000 |
| STT minutes/day | 5 | 60 | 240 |

### 9.2 Rate Limit Headers

Returned on every response:

```
X-RateLimit-Limit: 60
X-RateLimit-Remaining: 45
X-RateLimit-Reset: 1714732800
X-Session-Limit-Tokens: 500000
X-Session-Remaining-Tokens: 487660
```

### 9.3 Backpressure

When rate limited, Janus returns:
```json
{
  "error": {
    "message": "Rate limit exceeded",
    "type": "rate_limit_error",
    "retry_after": 12
  }
}
```

The Nebo client handles this via the existing retry loop in the agent runner
(10 transient retries with exponential backoff).

---

## 10. Provider Routing

### 10.1 Routing Strategy

Janus selects the upstream provider based on:

1. **Explicit model** — `model: "claude-sonnet-4-6"` → Anthropic
2. **Cost optimization** — For equivalent models, route to cheapest provider
3. **Availability** — If primary provider is down, failover to secondary
4. **Affinity** — `X-Lane` header keeps tool-use conversations on the same
   provider to avoid context loss
5. **Geographic** — Route to nearest provider region for latency

### 10.2 Model Aliases

Janus supports aliases for convenience:

| Alias | Resolves to | Provider |
|-------|-------------|----------|
| `auto` | Best available for task | Dynamic |
| `fast` | Cheapest fast model | Dynamic |
| `smart` | Most capable model | Dynamic |
| `sonnet` | `claude-sonnet-4-6` | Anthropic |
| `opus` | `claude-opus-4-6` | Anthropic |
| `gpt4o` | `gpt-4o` | OpenAI |

### 10.3 Tool Stickiness

When a conversation involves tool use, Janus tracks the provider used for
the initial tool call via `provider_metadata`. Subsequent messages in the
same conversation are routed to the same provider to maintain tool call
ID continuity.

The client sends `metadata` in the `ChatRequest`, and Janus echoes it back
on `Done` events with updated routing info.

---

## 11. Observability

### 11.1 Metrics (Prometheus)

| Metric | Type | Labels |
|--------|------|--------|
| `janus_requests_total` | Counter | capability, provider, model, status |
| `janus_latency_seconds` | Histogram | capability, provider |
| `janus_tokens_total` | Counter | provider, model, direction (input/output) |
| `janus_credits_consumed` | Counter | plan, capability |
| `janus_active_streams` | Gauge | provider |
| `janus_provider_errors` | Counter | provider, error_type |
| `janus_tts_characters` | Counter | provider, voice |
| `janus_stt_seconds` | Counter | provider, language |

### 11.2 Logging

Structured JSON logs with:
- Request ID (propagated via `X-Request-ID`)
- Bot ID
- Model + provider
- Token counts
- Latency breakdown (queue → upstream TTFB → stream duration)
- Error details

### 11.3 Client-Side Metrics

The Nebo client reports usage via `GET /v1/usage` (cached in
`AppState.janus_usage`) and exposes it in the settings UI. The
`janus_usage_refresh` endpoint forces a fresh fetch.

### 11.4 Admin Dashboard

NeboAI admin panel at `admin.neboai.com/janus`:
- Real-time request throughput
- Per-provider error rates and latency
- Credit consumption heatmaps
- Per-bot usage breakdown
- Anomaly detection (abuse, runaway agents)

---

## 12. Security

### 12.1 Authentication

- **JWT tokens** issued during NeboAI OAuth flow
- Tokens contain: `bot_id`, `plan_tier`, `org_id`, `exp`
- Token rotation on every AUTH_OK (comms connection)
- Cached at `<data_dir>/neboai_token.cache` for resilience

### 12.2 API Key Management

- Provider API keys stored server-side in Janus (never sent to clients)
- Keys rotated automatically via provider key management APIs
- Per-provider key pools to distribute rate limits
- Key usage tracked for anomaly detection

### 12.3 Request Validation

- Max request body: 10MB (LLM), 25MB (audio), 50MB (compute)
- Input sanitization for prompt injection markers
- Tool call validation against registered tool schemas
- Script sandboxing for compute endpoint

### 12.4 Encryption

- TLS 1.3 for all client ↔ Janus communication
- Provider API calls over TLS
- Sensitive fields (API keys, tokens) encrypted at rest (AES-256-GCM)

---

## 13. API Reference

### 13.1 Base URL

```
Production: https://janus.neboai.com
Staging:    https://janus-staging.neboai.com
```

### 13.2 Endpoints Summary

| Method | Path | Phase | Description |
|--------|------|-------|-------------|
| `POST` | `/v1/chat/completions` | Shipped | LLM chat (streaming SSE) |
| `GET` | `/v1/models` | Shipped | List available models |
| `GET` | `/v1/usage` | Shipped | Credit balance + consumption |
| `POST` | `/v1/audio/speech` | Phase 1 | Text-to-speech |
| `POST` | `/v1/audio/transcriptions` | Phase 1 | Speech-to-text |
| `POST` | `/v1/images/generations` | Phase 2 | Image generation |
| `POST` | `/v1/execute` | Phase 3 | Compute sandbox |
| `GET` | `/v1/providers` | Phase 3 | List available providers |
| `GET` | `/v1/limits` | Phase 3 | Current rate limit status |

### 13.3 Common Headers

**Request:**
```
Authorization: Bearer <jwt>
X-Bot-ID: <bot_id>
X-Lane: <lane_key>
X-Request-ID: <uuid>
Content-Type: application/json
```

**Response:**
```
X-Request-ID: <uuid>
X-RateLimit-Limit: 60
X-RateLimit-Remaining: 45
X-Credits-Consumed: 3.2
X-Credits-Remaining: 96800
```

### 13.4 Error Format

All errors follow the OpenAI error format:

```json
{
  "error": {
    "message": "Insufficient credits",
    "type": "insufficient_quota",
    "code": "credits_exhausted",
    "param": null
  }
}
```

| HTTP Status | Error Type | When |
|-------------|-----------|------|
| 400 | `invalid_request_error` | Bad request format |
| 401 | `authentication_error` | Invalid/expired JWT |
| 403 | `permission_error` | Plan doesn't include capability |
| 429 | `rate_limit_error` | Rate or credit limit exceeded |
| 500 | `server_error` | Internal Janus error |
| 502 | `upstream_error` | Provider returned error |
| 503 | `overloaded_error` | All providers unavailable |

---

## 14. Client Integration

### 14.1 Provider Priority (Nebo Desktop)

```
1. Direct API keys (user-provided, free for Nebo)
2. CLI providers (claude, gemini binaries — user's subscription)
3. Janus gateway (Nebo credits — always LAST)
4. Local models (GGUF — always-available fallback)
```

### 14.2 Voice Fallback Chain

```
1. Local Piper TTS / whisper.cpp STT (free, offline)
2. Direct OpenAI TTS/Whisper API (user's API key)
3. Janus voice endpoints (Nebo credits)
```

### 14.3 Janus Provider in Rust

The existing `OpenAIProvider` is reused with:
- `base_url` → `config.neboai.janus_url + "/v1"`
- `provider_id` → `"janus"`
- `bot_id` → set via `set_bot_id()`
- `lane` → set dynamically per request

No new provider implementation needed. Voice endpoints use the same
auth and base URL pattern.

### 14.4 Voice Client Integration

```rust
// In crates/voice/src/lib.rs — future JanusVoice backend

pub async fn synthesize_janus(
    janus_url: &str,
    token: &str,
    bot_id: &str,
    req: TtsRequest,
) -> Result<Vec<u8>, VoiceError> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{janus_url}/v1/audio/speech"))
        .bearer_auth(token)
        .header("X-Bot-ID", bot_id)
        .json(&serde_json::json!({
            "model": "tts-1",
            "input": req.text,
            "voice": req.voice,
            "response_format": "wav",
            "speed": req.speed,
        }))
        .send()
        .await
        .map_err(|e| VoiceError::SubprocessFailed(e.to_string()))?;

    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| VoiceError::Io(std::io::Error::other(e)))
}
```

---

## 15. Rollout Plan

### Phase 0 — Current (Shipped)
- [x] LLM chat completions proxy
- [x] Token usage tracking
- [x] Session/weekly rate limits
- [x] JWT authentication
- [x] X-Bot-ID billing isolation
- [x] Tool stickiness routing

### Phase 1 — Voice (Next)
- [ ] `POST /v1/audio/speech` — TTS proxy (OpenAI, Google)
- [ ] `POST /v1/audio/transcriptions` — STT proxy (OpenAI Whisper)
- [ ] Voice credit metering (per-character TTS, per-second STT)
- [ ] Voice rate limits per plan tier
- [ ] Client: Janus TTS as third fallback after local + direct API
- [ ] Voice usage in `GET /v1/usage` response

### Phase 2 — Image & Models
- [ ] `POST /v1/images/generations` — Image gen proxy
- [ ] `GET /v1/models` — Dynamic model catalog with capabilities
- [ ] Model aliases (`auto`, `fast`, `smart`)
- [ ] Provider health monitoring and automatic failover
- [ ] Cost optimization routing (cheapest equivalent model)

### Phase 3 — Compute & Advanced
- [ ] `POST /v1/execute` — Sandboxed code execution
- [ ] Firecracker microVM infrastructure
- [ ] `GET /v1/providers` — Provider status endpoint
- [ ] `GET /v1/limits` — Real-time limit status
- [ ] Admin dashboard at admin.neboai.com/janus
- [ ] Plugin provider type `"speech"` wiring (marketplace TTS plugins)

### Phase 4 — Enterprise
- [ ] Team-wide usage dashboards
- [ ] Per-seat credit budgets
- [ ] Audit logs
- [ ] Custom model deployments (fine-tuned models)
- [ ] On-premise Janus deployment option
- [ ] SLA guarantees with provider failover

---

## 16. Open Questions

1. **Voice model selection.** Should Janus auto-select the TTS provider
   based on language (Google for non-English, OpenAI for English)? Or let
   the client specify?

2. **Streaming TTS format.** SSE with base64 chunks vs. raw binary
   chunked transfer encoding. Base64 is 33% larger but works over
   standard HTTP proxies. Raw binary is more efficient but requires
   WebSocket or chunked transfer.

3. **Compute pricing.** Should GPU compute be a separate tier or fold
   into the credit system? GPU costs 10-50x CPU.

4. **Provider key pooling.** How many API keys per provider? Current
   single-key approach hits rate limits under load. Need pool of
   10-50 keys with round-robin distribution.

5. **Offline credit tracking.** When the client is offline (local models
   only), should we track "shadow credits" for analytics even though
   no real cost is incurred?

6. **Voice privacy.** STT sends audio to cloud providers. Should there
   be a client-side setting to force local-only STT even for Janus users?
   (Default: yes — local Whisper preferred, Janus STT opt-in.)

7. **Plugin voice providers.** The plugin system supports
   `PluginProviderDef.provider_type: "speech"`. When should marketplace
   plugins be able to register as voice providers in Janus?

---

*Last updated: 2026-05-03*
*Author: Nebo Engineering*
