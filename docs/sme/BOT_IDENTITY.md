# Bot Identity — Internal Reference

Every Nebo installation has a single immutable UUID — the **bot_id** — that uniquely identifies it across NeboLoop comms, Janus billing, loop channels, and A2A communication. It is generated locally on first startup, persisted to SQLite, and never changes.

**Key insight:** Nebo generates its own identity locally. The server doesn't assign one — it registers whatever bot_id Nebo presents at connection time.

---

## Architecture

```
First Startup
  └─ ensureBotID()
       ├─ Check plugin_settings for existing bot_id
       │   └─ Found? → return it (immutable)
       └─ Not found?
            ├─ uuid.New().String()           ← RFC 4122 v4 (random)
            ├─ Persist to plugin_settings    ← survives restarts
            └─ Return UUID

Usage (every session thereafter):
  ┌──────────────────────────────────────────────────────┐
  │                     bot_id (UUID)                     │
  ├──────────────┬──────────────┬─────────────┬──────────┤
  │ NeboLoop     │ Janus        │ REST API    │ Comms    │
  │ CONNECT      │ X-Bot-ID     │ /bots/{id}  │ Routing  │
  │ frame        │ header       │ endpoints   │ (DM/A2A) │
  └──────────────┴──────────────┴─────────────┴──────────┘
```

---

## File Map

| Layer | File | Purpose |
|-------|------|---------|
| Generation | `cmd/nebo/agent.go:406-431` | `ensureBotID()` — generate or retrieve |
| Agent state | `cmd/nebo/agent.go:96` | `agentState.botID` — in-memory for session |
| Auth injection | `cmd/nebo/agent.go:3478-3495` | `injectNeboLoopAuth()` — merge bot_id + JWT into config |
| DB migration | `internal/db/migrations/0027_plugin_settings.sql` | Schema + neboloop plugin pre-population |
| DB queries | `internal/db/queries/plugins.sql` | CRUD for plugin_settings |
| Plugin store | `internal/apps/settings/store.go` | `GetSettingsByName()`, `UpdateSettings()` |
| Comm plugin | `internal/agent/comm/neboloop/plugin.go:118,337,357` | Receives bot_id in config, passes to SDK |
| REST client | `internal/neboloop/client.go:23,39-41` | Requires bot_id, uses in all API paths |
| Janus billing | `internal/agent/ai/api_openai.go:39,119-122,193-194` | `SetBotID()`, `X-Bot-ID` header |
| HTTP handler | `internal/handler/plugins/handler.go:637-672` | Web UI code redemption (parallel path) |
| Code redemption | `internal/neboloop/client.go:346-353` | `RedeemCode()` — pass bot_id to server |

---

## Generation

**Function:** `ensureBotID(ctx, pluginStore)` in `cmd/nebo/agent.go:406-431`

```go
func ensureBotID(ctx context.Context, pluginStore *settings.Store) string {
    // 1. Check if already exists
    settings, err := pluginStore.GetSettingsByName(ctx, "neboloop")
    if err == nil && settings["bot_id"] != "" {
        return settings["bot_id"]  // Immutable — return existing
    }

    // 2. Generate new UUID v4
    botID := uuid.New().String()

    // 3. Persist to DB
    p, _ := pluginStore.GetPlugin(ctx, "neboloop")
    pluginStore.UpdateSettings(ctx, p.ID, map[string]string{"bot_id": botID}, nil)

    return botID
}
```

**Characteristics:**
- **Algorithm:** UUID v4 (random) via `github.com/google/uuid`
- **Format:** 36-char string with hyphens (e.g., `550e8400-e29b-41d4-a716-446655440000`)
- **Timing:** Called early in `runAgent()` (line 2298), before any comms or NeboLoop work
- **Idempotent:** Subsequent calls return the stored value — never regenerates
- **Fallback:** If DB write fails, the UUID still exists in-memory for the current session (logged as warning)

---

## Persistence

### Storage Schema

bot_id lives in the `plugin_settings` table, linked to the pre-populated `neboloop` entry in `plugin_registry`:

```
plugin_registry
├── id: "builtin-neboloop"
├── name: "neboloop"
├── plugin_type: "comm"
└── display_name: "NeboLoop"

plugin_settings
├── plugin_id: "builtin-neboloop"  (FK → plugin_registry.id)
├── setting_key: "bot_id"
├── setting_value: "<36-char-uuid>"
├── is_secret: 0                   (not sensitive — just an identifier)
└── UNIQUE(plugin_id, setting_key) (prevents duplicates)
```

### Other NeboLoop Settings (same table, same plugin_id)

| Key | Value | Secret |
|-----|-------|--------|
| `bot_id` | UUID string | No |
| `api_server` | `https://api.neboloop.com` | No |
| `gateway` | `wss://comms.neboloop.com` | No |
| `token` | Owner OAuth JWT | Yes |

### Durability

| Scenario | bot_id survives? |
|----------|-----------------|
| App restart | Yes — persisted in SQLite |
| App update | Yes — updates replace binary, not database |
| NeboLoop reconnect | Yes — read from DB, not regenerated |
| OAuth re-auth | Yes — only JWT refreshed, bot_id untouched |
| Database backup/restore | Yes — comes with the DB file |
| Uninstall + reinstall | **Only if database file persists** in data dir |
| Fresh install (new machine) | No — new UUID generated |

---

## Usage Across Subsystems

### 1. NeboLoop WebSocket Comms

bot_id is sent in the CONNECT frame to identify this bot instance to the gateway.

**Flow:**
```
ensureBotID() → agentState.botID
  → injectNeboLoopAuth() merges into config map
    → commPlugin.Connect(config)
      → p.botID = config["bot_id"]
        → neboloopsdk.Connect(Config{BotID: p.botID, Token: jwt})
          → CONNECT frame: {"token": "<jwt>", "bot_id": "<uuid>"}
            → Gateway registers bot under JWT.sub (owner account)
```

**Key behavior:** Gateway auto-registers unknown bot_ids under the JWT subject. If you connect with a new bot_id but valid JWT, the server creates a new bot registration.

### 2. Janus Gateway (AI Provider Billing)

bot_id is sent as `X-Bot-ID` header on every LLM request through Janus.

**Flow:**
```
providers.go: reads bot_id from plugin_settings
  → OpenAIProvider.SetBotID(botID)
    → Every request: option.WithHeader("X-Bot-ID", p.botID)
      → Janus uses for per-bot usage tracking and billing
```

**SQL used to load bot_id for Janus:**
```sql
SELECT ps.setting_value FROM plugin_settings ps
  JOIN plugin_registry pr ON pr.id = ps.plugin_id
  WHERE pr.name = 'neboloop' AND ps.setting_key = 'bot_id'
```

### 3. NeboLoop REST API

bot_id is a path parameter or body field in all bot-scoped REST endpoints:

| Endpoint | Method | bot_id Usage |
|----------|--------|-------------|
| `/api/v1/bots/{bot_id}` | PUT | Update bot identity (name, role) |
| `/api/v1/bots/{bot_id}/channels` | GET | List bot's channels |
| `/api/v1/bots/{bot_id}/loops` | GET | List bot's loops |
| `/api/v1/bots/{bot_id}/loops/{loopID}` | GET | Get specific loop |
| `/api/v1/bots/{bot_id}/loops/{loopID}/members` | GET | Loop members |
| `/api/v1/bots/{bot_id}/channels/{channelID}/members` | GET | Channel members |
| `/api/v1/bots/{bot_id}/channels/{channelID}/messages` | GET | Channel messages |
| `/api/v1/skills/{id}/install` | POST | Body: `{"bot_id": "..."}` |
| `/api/v1/skills/redeem` | POST | Body: `{"bot_id": "..."}` |
| `/api/v1/loops/join` | POST | Body: `{"bot_id": "..."}` |
| `/api/v1/bots/connect/redeem` | POST | Body: `{"bot_id": "..."}` (initial setup) |

**Validation:** `neboloop.NewClient()` fails immediately if bot_id is empty:
```go
if botID == "" {
    return nil, fmt.Errorf("bot_id not configured")
}
```

### 4. DM and A2A Routing

bot_id identifies the sender/recipient in direct messages and agent-to-agent tasks:

- **Owner DMs:** Routed to main lane (same session as web UI)
- **External DMs:** Routed to comm lane, session key = `dm-{ConversationID}`
- **A2A tasks:** `TaskSubmission.FromBotID` identifies the requesting bot
- **Loop channels:** Member lists include each bot's bot_id and online status

### 5. Connection Code Redemption

When connecting to NeboLoop for the first time (via onboarding wizard or settings):

```
User enters connection code
  → ensureBotID() generates/retrieves UUID
    → RedeemCode(apiServer, code, name, purpose, botID)
      → Server: POST /api/v1/bots/connect/redeem
        → Server registers this bot_id under the code's owner account
          → Returns: connection token, bot name, bot slug
            → Store settings: api_server, bot_id, token
```

**Two paths to code redemption:**
1. **Agent-side:** `cmd/nebo/agent.go:555-603` — called from CLI or agent tool
2. **HTTP-side:** `internal/handler/plugins/handler.go:637-672` — called from web UI

Both use the same `ensureBotID()` logic to guarantee the same UUID is used regardless of entry point.

---

## Injection Pipeline

bot_id flows through a single injection function that merges it with the owner's OAuth JWT:

```go
// cmd/nebo/agent.go:3478-3495
func injectNeboLoopAuth(ctx, sqlDB, botID, base) map[string]string {
    out := copy(base)
    out["bot_id"] = botID           // Always inject bot_id
    if out["token"] == "" {
        out["token"] = getNeboLoopJWT(ctx, sqlDB)  // Inject JWT if missing
    }
    return out
}
```

**Called at 5 injection points:**

| Point | Line | Context |
|-------|------|---------|
| Agent startup (fresh connect) | 607 | First connection after code redemption |
| Loop code handling | 719 | Joining a loop via connection code |
| Skill code handling | 812 | Installing a skill via connection code |
| Store tool factory | 1170 | Building NeboLoop client for store operations |
| Comm plugin (re)connect | 2342, 3537 | Startup reconnect or mid-session reconnect |

---

## Relationship to Other Identifiers

| Identifier | Scope | Persistence | Purpose |
|------------|-------|-------------|---------|
| **bot_id** (UUID) | Per-installation | SQLite (immutable) | External identity — NeboLoop, Janus, comms |
| **ownerID** | Per-NeboLoop-account | JWT `sub` claim | Account-level identity (multiple bots per owner) |
| **agentID** | Per-session | In-memory only | Internal session tracking within comm plugin |
| **Session ID** | Per-conversation | SQLite `sessions` table | Conversation isolation |
| **Message ID** | Per-message | SQLite `session_messages` | Message deduplication |

**No device ID, machine ID, or install ID exists** — bot_id is the single per-installation identifier.

---

## Security Considerations

| Aspect | Status |
|--------|--------|
| **Sensitivity** | Not secret — just an identifier (like a username, not a password) |
| **Storage** | Plaintext in SQLite (`is_secret: 0`) — appropriate |
| **Transmission** | Sent in WebSocket CONNECT frames and HTTP headers — over TLS |
| **Immutability** | Enforced by code (read-or-generate, never overwrite) — no admin endpoint to change |
| **Binding** | Gateway binds bot_id to JWT subject — prevents impersonation |
| **Enumeration** | UUIDv4 is 122 bits of randomness — not guessable |

**Trust model:** bot_id proves "I am this bot instance" but NOT "I am authorized." Authorization comes from the accompanying JWT. A bot_id without a valid JWT is rejected at the gateway.

---

## What's NOT Exposed

- **No HTTP API** to query bot_id directly (it's internal to the agent process)
- **No CLI command** to display bot_id (could be useful for debugging)
- **No ServiceContext field** — HTTP server doesn't need bot_id knowledge
- **No `nebo doctor` output** — diagnostics don't display bot_id yet

The NeboLoop status handler (`GET /api/v1/plugins/neboloop/status`) does return bot_id in its response, but only as part of connection status.

---

## Edge Cases

| Scenario | Behavior |
|----------|----------|
| `pluginStore` is nil | `ensureBotID()` returns empty string — no NeboLoop features work |
| neboloop plugin not in registry | UUID generated in-memory, persistence fails (logged as warning) |
| DB write fails during generation | UUID returned in-memory, lost on restart (new UUID next time) |
| Two code redemptions | Second uses same bot_id — server sees same bot reconnecting |
| Database deleted | New bot_id generated on next startup — server sees a new bot |
| Concurrent calls to `ensureBotID()` | No mutex — theoretical race on first startup, but `UNIQUE` constraint prevents duplicate rows |
