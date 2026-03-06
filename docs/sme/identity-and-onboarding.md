# Identity, Onboarding, and Introduction: Complete Migration Reference

**Source:** `cmd/nebo/agent.go`, `internal/handler/`, `internal/agent/`, `extensions/skills/introduction/SKILL.md` | **Target:** `crates/server/`, `crates/agent/`, `crates/config/`, `crates/db/` | **Status:** Draft

---

## Table of Contents

1. [Bot Identity](#1-bot-identity)
2. [Onboarding Wizard](#2-onboarding-wizard)
3. [NeboLoop OAuth](#3-neboloop-oauth)
4. [Introduction Skill](#4-introduction-skill)
5. [Ask Widget Integration](#5-ask-widget-integration)
6. [Skill Install via Store Tool](#6-skill-install-via-store-tool)
7. [Onboarding Detection and Deduplication](#7-onboarding-detection-and-deduplication)
8. [Database Schema](#8-database-schema)
9. [Frontend Reference](#9-frontend-reference)
10. [Rust Implementation Status](#10-rust-implementation-status)

---

## 1. Bot Identity

**File(s):** `cmd/nebo/agent.go:406-431`, `internal/apps/settings/store.go`, `internal/agent/comm/neboloop/plugin.go`

Every Nebo installation has a single immutable UUID -- the **bot_id** -- that uniquely identifies it across NeboLoop comms, Janus billing, loop channels, and A2A communication. The identifier is generated locally on first startup, persisted to stable storage, and NEVER changes.

### 1.1 Generation

The Go implementation uses `ensureBotID()`, called early in `runAgent()` before any comms or NeboLoop work begins:

```go
func ensureBotID(ctx context.Context, pluginStore *settings.Store) string {
    // 1. Check if already exists
    settings, err := pluginStore.GetSettingsByName(ctx, "neboloop")
    if err == nil && settings["bot_id"] != "" {
        return settings["bot_id"]  // Immutable -- return existing
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

- **Algorithm:** UUID v4 (random) via RFC 4122
- **Format:** 36-char string with hyphens (e.g., `550e8400-e29b-41d4-a716-446655440000`)
- **Timing:** Called early in agent startup, before any comms
- **Idempotent:** Subsequent calls return the stored value -- never regenerates
- **Fallback:** If persistence fails, the UUID exists in-memory for the current session (logged as warning)

The Rust rewrite stores bot_id as a flat file instead of in `plugin_settings`:

```rust
// crates/config/src/defaults.rs

/// Reads the bot_id from `<data_dir>/bot_id`.
/// Returns `None` if the file doesn't exist or the value isn't a valid 36-char UUID.
pub fn read_bot_id() -> Option<String> {
    let dir = data_dir().ok()?;
    let data = fs::read_to_string(dir.join(files::BOT_ID)).ok()?;
    let id = data.trim().to_string();
    if id.len() == 36 { Some(id) } else { None }
}

/// Persists the bot_id to `<data_dir>/bot_id` with read-only permissions.
pub fn write_bot_id(id: &str) -> Result<(), NeboError> {
    let dir = data_dir()?;
    let path = dir.join(files::BOT_ID);
    let _ = fs::remove_file(&path); // May be read-only
    fs::write(&path, id)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o400);
        fs::set_permissions(&path, perms)?;
    }
    Ok(())
}
```

**Key difference from Go:** Rust persists to `<data_dir>/bot_id` as a file with 0o400 permissions. Go persists to the `plugin_settings` table under plugin_id `builtin-neboloop`, key `bot_id`. Both approaches ensure immutability.

### 1.2 Persistence and Durability

Go stores bot_id in `plugin_settings`:

```
plugin_registry
+-- id: "builtin-neboloop"
+-- name: "neboloop"
+-- plugin_type: "comm"

plugin_settings
+-- plugin_id: "builtin-neboloop"
+-- setting_key: "bot_id"
+-- setting_value: "<36-char-uuid>"
+-- is_secret: 0
+-- UNIQUE(plugin_id, setting_key)
```

Rust stores bot_id as a file:

```
<data_dir>/bot_id          -- plain text, 36 chars, 0o400 permissions
```

| Scenario | Survives? |
|----------|-----------|
| App restart | Y |
| App update | Y -- updates replace binary, not data dir |
| NeboLoop reconnect | Y -- read from storage, not regenerated |
| OAuth re-auth | Y -- only JWT refreshed |
| Database backup/restore | Y (Go); N/A (Rust uses file) |
| Uninstall + reinstall | Only if data dir persists |
| Fresh install (new machine) | N -- new UUID generated |

### 1.3 Five Injection Points

bot_id flows through a single injection function (`injectNeboLoopAuth()` in Go) that merges it with the owner OAuth JWT:

```go
func injectNeboLoopAuth(ctx, sqlDB, botID, base) map[string]string {
    out := copy(base)
    out["bot_id"] = botID
    if out["token"] == "" {
        out["token"] = getNeboLoopJWT(ctx, sqlDB)
    }
    return out
}
```

Called at 5 EXACT injection points:

| # | Point | Go Line | Context |
|---|-------|---------|---------|
| 1 | Agent startup (fresh connect) | 607 | First connection after code redemption |
| 2 | Loop code handling | 719 | Joining a loop via connection code |
| 3 | Skill code handling | 812 | Installing a skill via connection code |
| 4 | Store tool factory | 1170 | Building NeboLoop client for store operations |
| 5 | Comm plugin (re)connect | 2342, 3537 | Startup reconnect or mid-session reconnect |

### 1.4 Usage Across Subsystems

```
                         bot_id (UUID)
+----------------+--------------+-------------+----------+
| NeboLoop       | Janus        | REST API    | Comms    |
| CONNECT frame  | X-Bot-ID     | /bots/{id}  | Routing  |
|                | header       | endpoints   | (DM/A2A) |
+----------------+--------------+-------------+----------+
```

**1. NeboLoop WebSocket Comms:**

bot_id is sent in the CONNECT frame to identify this bot instance to the gateway.

```
ensureBotID() -> agentState.botID
  -> injectNeboLoopAuth() merges into config map
    -> commPlugin.Connect(config)
      -> neboloopsdk.Connect(Config{BotID: p.botID, Token: jwt})
        -> CONNECT frame: {"token": "<jwt>", "bot_id": "<uuid>"}
```

Gateway auto-registers unknown bot_ids under the JWT subject. A new bot_id with a valid JWT creates a new bot registration.

**2. Janus Gateway (AI Provider Billing):**

bot_id is sent as `X-Bot-ID` header on every LLM request through Janus. In Rust, this is implemented in the OpenAI provider:

```rust
// crates/ai/src/providers/openai.rs
pub fn set_bot_id(&mut self, id: impl Into<String>) {
    self.bot_id = Some(id.into());
}

// In request building:
if let Some(ref bot_id) = self.bot_id {
    req_builder = req_builder.header(
        "X-Bot-ID",
        bot_id.parse().expect("valid X-Bot-ID header"),
    );
}
```

**3. NeboLoop REST API:**

bot_id is a path parameter or body field in all bot-scoped REST endpoints:

| Endpoint | Method | bot_id Usage |
|----------|--------|-------------|
| `/api/v1/bots/{bot_id}` | PUT | Update bot identity |
| `/api/v1/bots/{bot_id}/channels` | GET | List bot channels |
| `/api/v1/bots/{bot_id}/loops` | GET | List bot loops |
| `/api/v1/skills/{id}/install` | POST | Body: `{"bot_id": "..."}` |
| `/api/v1/skills/redeem` | POST | Body: `{"bot_id": "..."}` |
| `/api/v1/loops/join` | POST | Body: `{"bot_id": "..."}` |
| `/api/v1/bots/connect/redeem` | POST | Body: `{"bot_id": "..."}` |

**4. DM and A2A Routing:**

bot_id identifies the sender/recipient in direct messages and agent-to-agent tasks:

- Owner DMs: routed to main lane (same session as web UI)
- External DMs: routed to comm lane, session key = `dm-{ConversationID}`
- A2A tasks: `TaskSubmission.FromBotID` identifies the requesting bot
- Loop channels: member lists include each bot's bot_id and online status

**5. Connection Code Redemption:**

```
User enters connection code
  -> ensureBotID() generates/retrieves UUID
    -> RedeemCode(apiServer, code, name, purpose, botID)
      -> Server: POST /api/v1/bots/connect/redeem
        -> Returns: connection token, bot name, bot slug
```

Two paths to code redemption exist: agent-side (CLI) and HTTP-side (web UI). Both use the same `ensureBotID()` logic to guarantee the same UUID regardless of entry point.

### 1.5 Relationship to Other Identifiers

| Identifier | Scope | Persistence | Purpose |
|------------|-------|-------------|---------|
| **bot_id** (UUID) | Per-installation | SQLite / file (immutable) | External identity -- NeboLoop, Janus, comms |
| **ownerID** | Per-NeboLoop-account | JWT `sub` claim | Account-level identity (multiple bots per owner) |
| **agentID** | Per-session | In-memory only | Internal session tracking within comm plugin |
| **Session ID** | Per-conversation | SQLite `sessions` table | Conversation isolation |
| **Message ID** | Per-message | SQLite `session_messages` | Message deduplication |

There is NO device ID, machine ID, or install ID -- bot_id is the single per-installation identifier.

### 1.6 Security

| Aspect | Detail |
|--------|--------|
| Sensitivity | NOT secret -- just an identifier (like a username) |
| Storage | Plaintext in SQLite / file -- appropriate |
| Transmission | CONNECT frames and HTTP headers -- over TLS |
| Immutability | Read-or-generate, never overwrite |
| Binding | Gateway binds bot_id to JWT subject -- prevents impersonation |
| Enumeration | UUIDv4 has 122 bits of randomness -- NOT guessable |

**Trust model:** bot_id proves "I am this bot instance" but NOT "I am authorized." Authorization comes from the accompanying JWT. A bot_id without a valid JWT is rejected at the gateway.

---

## 2. Onboarding Wizard

**File(s):** `app/src/lib/components/onboarding/OnboardingFlow.svelte`, `internal/handler/user/profilehandler.go`, `internal/handler/user/permissionshandler.go`, `internal/handler/provider/`, `internal/handler/neboloop/oauth.go`

Nebo's onboarding is a single Svelte component wizard (`OnboardingFlow.svelte`) that renders as a full-screen overlay over the app layout. It gates ALL app access until `onboarding_completed = 1` in the `user_profiles` table.

### 2.1 Design Principles

- Single-user mode (hardcoded `user_id = "default-user"`)
- State managed entirely in Svelte 5 runes (component-scoped, no shared store)
- Progress persists to SQLite via API calls at each step transition
- Full page reload after completion triggers re-check in layout
- Janus (NeboLoop-managed AI) is the recommended/default provider path

### 2.2 Seven-Step Flow

```
+---------+    +-------+    +-----------------+    +---------+
| welcome |--->| terms |--->| provider-choice |--->| api-key |--+
+---------+    +-------+    +--------+--------+    +---------+  |
                                     |                          |
                              +------+------+                   |
                              |   (janus)   |                   |
                              v             v                   |
                        +----------+  +---------+              |
                        | neboloop |  | CLI     |              |
                        | (OAuth)  |  | setup   |              |
                        +----+-----+  +----+----+              |
                             |             |                    |
                             v             v                    v
                        +------------------------------------------+
                        |            capabilities                   |
                        +--------------------+---------------------+
                                             |
                              +--------------+---------------+
                              | (if !cameFromJanus)          |
                              v                              v
                        +----------+                  +----------+
                        | neboloop |                  | complete  |
                        | (opt-in) |--(or skip)------>|          |
                        +----------+                  +----------+
```

**Progress dots:** 6 user-visible dots: `welcome`, `terms`, `provider-choice`, `capabilities`, `neboloop`, `complete`. The `api-key` step is hidden from the progress indicator -- it maps to `provider-choice`'s dot position.

### 2.3 Step Details

**Step 1 -- Welcome:**
UI shows sparkles icon, "Welcome to Nebo" heading, "Get Started" button. Client-side only, no API call.

**Step 2 -- Terms:**
UI shows shield icon, 5 privacy/data disclosure sections in scrollable box, checkbox acceptance. Calls `POST /api/v1/user/me/accept-terms`. Sets `terms_accepted_at = unixepoch()` in `user_profiles`. Gate: checkbox must be checked before button enables.

Five disclosures:
1. Your Data Stays Local (SQLite, no server sync)
2. AI Provider Communication (messages sent to chosen provider)
3. API Keys (stored locally, encrypted)
4. System Access (user-controlled capabilities)
5. No Analytics or Telemetry

**Step 3 -- Provider Choice:**
Radio-style card selection. Janus pre-selected with "Recommended" badge. On mount, calls `GET /api/v1/models` to detect CLI tools and load models.yaml config.

| Option | Badge | Condition | Next Step |
|--------|-------|-----------|-----------|
| Janus | Recommended | Always shown | `neboloop` (sets `cameFromJanus = true`) |
| Claude Code CLI | Ready | Installed + authenticated | `capabilities` (via `setupCLI()`) |
| Codex CLI | Ready | Installed + authenticated | `capabilities` (via `setupCLI()`) |
| Gemini CLI | Ready | Installed + authenticated | `capabilities` (via `setupCLI()`) |
| [CLI] | Needs Login | Installed, not authenticated | Disabled |
| Add API Key | -- | Always shown | `api-key` |

CLIs and API key are hidden behind an expandable "Use your own API key or CLI instead" section.

**Step 4 -- API Key (hidden from progress dots):**
Key icon, provider dropdown (Anthropic/OpenAI/Google), password input.

Flow:
1. `POST /api/v1/providers` -- create auth profile with encrypted API key
2. `POST /api/v1/providers/{id}/test` -- validate key against provider
3. If valid: show success alert, auto-advance to `capabilities` after 500ms
4. If invalid: show error, `DELETE /api/v1/providers/{id}` to clean up

**Step 5 -- Capabilities:**
Shield/info icon, 8 toggle cards in scrollable grid:

| Key | Label | Default | Notes |
|-----|-------|---------|-------|
| `chat` | Chat & Memory | ON | Locked (`alwaysOn: true`) |
| `file` | File System | ON | |
| `shell` | Shell & Terminal | OFF | Security-sensitive |
| `web` | Web Browsing | ON | |
| `contacts` | Contacts & Calendar | OFF | Privacy-sensitive |
| `desktop` | Desktop Control | ON | |
| `media` | Media & Capture | OFF | Security-sensitive |
| `system` | System | ON | |

Submit calls `PUT /api/v1/user/me/permissions`. If `cameFromJanus`, calls `completeOnboarding()` directly. Otherwise advances to `neboloop` step.

**Step 6 -- NeboLoop:**
Store icon. Two modes: OAuth prompt or "Already Connected" display. If `cameFromJanus`, OAuth happened BEFORE capabilities. For non-Janus paths, NeboLoop is optional and skippable via "Skip for now."

**Step 7 -- Complete:**
Green checkmark, "You're all set!" heading, "Start Chatting" button. `completeOnboarding()` already called `PUT /api/v1/user/me/profile { onboardingCompleted: true }`. Button triggers `window.location.href = '/agent'` -- full page reload.

### 2.4 Decision Tree (Janus vs Non-Janus)

```
Provider Choice:
+-- Janus selected:
|   provider-choice -> neboloop (OAuth) -> capabilities -> complete
|   (cameFromJanus = true throughout)
|
+-- CLI selected:
|   provider-choice -> setupCLI() -> capabilities -> neboloop (optional) -> complete
|
+-- API Key selected:
    provider-choice -> api-key -> capabilities -> neboloop (optional) -> complete
```

**CRITICAL insight:** The Janus path does NeboLoop OAuth BEFORE capabilities (because Janus requires NeboLoop account). Non-Janus paths do capabilities first, NeboLoop is optional.

### 2.5 CLI Setup

When a CLI provider is selected:

1. Gets provider info from `cliProviderInfo[cliKey]`
2. Calls `PUT /api/v1/providers/model-config` with `{ primary: "{id}/{defaultModel}" }`
3. Advances to `capabilities`

CLI detection happens on mount of the provider-choice step via `GET /api/v1/models`, which returns `cliStatuses` (installed/authenticated status per CLI tool) and `cliProviderInfo` (provider config from models.yaml).

---

## 3. NeboLoop OAuth

**File(s):** `internal/handler/neboloop/oauth.go` (Go), `crates/server/src/handlers/neboloop.rs` (Rust)

### 3.1 PKCE Flow

```
Frontend                    Backend                     NeboLoop
   |                           |                           |
   | startNeboLoopOAuth()      |                           |
   +--GET /oauth/start-------->|                           |
   |                           | Generate PKCE:            |
   |                           |  state, codeVerifier,     |
   |                           |  codeChallenge            |
   |                           | Store in pendingFlows     |
   |                           | Open browser ------------->
   |<--{state, authorizeURL}---|                           |
   |                           |                           |
   | Poll every 2s:            |                           |
   +--GET /oauth/status------->|                           |
   |<--{status:"pending"}------|                           |
   |                           |                           |
   |                           |    <--callback?code=X-----|
   |                           | exchangeOAuthCode()       |
   |                           | fetchUserInfo()           |
   |                           | storeNeboLoopProfile()    |
   |                           | activateNeboLoopComm()    |
   |                           | Mark flow complete        |
   |                           | Return close-window HTML  |
   |                           |                           |
   +--GET /oauth/status------->|                           |
   |<--{status:"complete",     |                           |
   |    email:"user@..."}------|                           |
   |                           |                           |
   | neboLoopConnected = true  |                           |
```

- **Timeout:** 3 minutes (Go) / 10 minutes (Rust -- see `OAUTH_FLOW_TIMEOUT`)
- **Poll interval:** 2 seconds (frontend-controlled)
- **State storage:** In-memory (`sync.Map` in Go, `LazyLock<Mutex<HashMap>>` in Rust)
- **Client ID:** `nbl_nebo_desktop`

### 3.2 PKCE Implementation

Go generates PKCE parameters and stores them in a `sync.Map`. Rust mirrors this with a `LazyLock<Mutex<HashMap<String, OAuthFlowState>>>`:

```rust
// crates/server/src/handlers/neboloop.rs

struct OAuthFlowState {
    code_verifier: String,
    created_at: Instant,
    completed: bool,
    error: String,
    email: String,
    display_name: String,
    janus_provider: bool,
}

static PENDING_FLOWS: LazyLock<Mutex<HashMap<String, OAuthFlowState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
```

PKCE helpers generate a 32-byte random verifier (base64url-encoded) and compute the S256 challenge:

```rust
fn generate_code_verifier() -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

fn compute_code_challenge(verifier: &str) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let hash = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hash)
}
```

### 3.3 OAuth Start Handler

The `/oauth/start` handler (Rust implementation):

1. Validates NeboLoop integration is enabled
2. Generates PKCE state, verifier, and challenge
3. Constructs authorize URL with query parameters: `response_type`, `client_id`, `redirect_uri`, `scope`, `state`, `code_challenge`, `code_challenge_method`
4. Stores flow state in `PENDING_FLOWS` (cleans up expired flows first)
5. Opens browser via `open::that()`
6. Returns `{ authorizeUrl, state }` to frontend

```rust
// Redirect URI format:
let redirect_uri = format!("http://localhost:{}/auth/neboloop/callback", state.config.port);

// Authorize URL format:
let authorize_url = format!("{}/oauth/authorize?{}", frontend_url, query_string);
```

### 3.4 Token Exchange and Storage

On successful callback, the backend:

1. Exchanges authorization code for tokens via `POST {api_url}/oauth/token`
2. Fetches user info via `GET {api_url}/oauth/userinfo` with Bearer token
3. Stores NeboLoop profile in `auth_profiles` table

Token storage fields:

| Field | Value |
|-------|-------|
| `provider` | `"neboloop"` |
| `api_key` | OAuth access token |
| `auth_type` | `"oauth"` |
| `base_url` | NeboLoop API URL |
| `metadata` (JSON) | `{"owner_id":"uuid", "email":"user@...", "refresh_token":"...", "janus_provider":"true"}` |

The Rust implementation handles both creation of new profiles and updating existing ones, plus cleans up duplicate profiles:

```rust
fn store_neboloop_profile(
    app_state: &AppState,
    api_url: &str,
    owner_id: &str,
    email: &str,
    token: &str,
    refresh_token: &str,
    janus_provider: bool,
) -> Result<(), String> {
    let profiles = app_state
        .store
        .list_active_auth_profiles_by_provider("neboloop")
        .unwrap_or_default();

    // Carry forward janus_provider from existing profile if not explicitly set
    let janus = if janus_provider {
        true
    } else {
        profiles.iter().any(|p| /* check existing janus_provider metadata */)
    };

    // Create or update auth_profiles entry
    // Delete any extra duplicate profiles
}
```

### 3.5 OAuth Status Polling

The `/oauth/status` handler returns one of four states:

| Status | Condition | Response Fields |
|--------|-----------|-----------------|
| `pending` | Flow exists, not completed | -- |
| `complete` | Flow completed, no error | `email`, `displayName` |
| `error` | Flow completed with error | `error` message |
| `expired` | Flow not found (expired or never existed) | -- |

Completed flows are removed from the map on first status read (single-consume pattern).

### 3.6 State Lifecycle

- Flows are created on `/oauth/start` and stored in-memory
- Expired flows are cleaned up lazily on each new `/oauth/start` call
- Completed flows are removed on the first `/oauth/status` read that returns complete
- If the server restarts mid-flow, the callback WILL fail (state lost)

### 3.7 Account Management

**Account status** (`GET /api/v1/neboloop/account`): Reads `auth_profiles` where `provider = "neboloop"`. Returns `connected`, `janusProvider`, `profileId`, `ownerId`, `email`.

**Account disconnect** (`DELETE /api/v1/neboloop/disconnect`): Deletes all NeboLoop auth profiles.

**Bot status** (`GET /api/v1/neboloop/status`): Returns bot connection status. In Rust, returns connected=true if any NeboLoop auth profile exists (no persistent comm connection).

**Janus usage** (`GET /api/v1/neboloop/janus/usage`): In Rust, returns zeroed-out usage (no Janus proxy in local mode).

---

## 4. Introduction Skill

**File(s):** `extensions/skills/introduction/SKILL.md`, `internal/agent/skills/skill.go`, `internal/agent/skills/loader.go`, `internal/agent/tools/skill_tool.go`

The introduction is a skill-driven conversational flow that runs AFTER the onboarding wizard completes. Onboarding handles technical setup (provider, permissions, OAuth). Introduction handles the human first-touch -- building rapport, orienting the user, and installing personalized skills.

### 4.1 Skill Definition

```yaml
name: introduction
description: First meeting -- make them feel seen, set them up for success
version: "0.1.4"
priority: 100          # Highest -- runs before other skills
max_turns: 8           # Auto-expires after 8 turns of inactivity
triggers:
  - hello, hi, hey, start, help me get started
  - who are you, what can you do, introduce yourself
tools:
  - agent              # ask widgets + memory storage
  - store              # skill installs from NeboLoop catalog
metadata:
  nebo:
    emoji: wave
```

### 4.2 Four-Part Conversational Flow (Mandatory Order)

| Part | Purpose | Exchanges | Key Rule |
|------|---------|-----------|----------|
| 1 -- Connection | Build rapport: name, location, work | 3 | One question per message, 1-2 sentences max |
| 2 -- Orientation | Explain how Nebo works | 1 message | Apple-like writing. MANDATORY -- never skip |
| 3 -- Skill Picker | Recommend and install 3-4 personalized skills | Interactive | Always 3-4 options + "Skip for now" in single widget |
| 4 -- Handoff | Warm close, let user come to agent | 1 message | No CTA, no pitch. Then STOP. |

**Part 1 -- The Connection:**

Three exchanges to build emotional attachment via "unexpected understanding":

1. **Name:** First message: `"Hi! I'm Nebo."` + ask widget (`text_input`). Greet by name, ask location (plain text).
2. **Location:** React genuinely (NOT "cool!"). Ask what they do (plain text).
3. **Work:** Reflect back something they did NOT say -- the emotional truth underneath the facts.

Transition: `"Before I get out of your way -- quick rundown on how things work, so nothing catches you off guard."`

**Part 2 -- Orientation:**

One message. Short declarative sentences. Fragments that breathe. Topics: lives on your computer (not cloud), windows may open/close (that is me working), approval prompts (you control), persistent memory (never repeat yourself). End: `"One more thing -- let me set you up."`

**Part 3 -- Skill Picker:**

Map user job/role to 3-4 skills from the catalog. Present via ask widget with buttons. Install silently via store tool.

**Part 4 -- Handoff:**

`"That's it. Put me to work whenever you're ready."` Then STOP. Let user come to agent.

### 4.3 Anti-Patterns (Encoded in Skill Template)

The skill template explicitly warns the LLM against these:

- No empty flattery reactions
- No recapping facts (parroting is NOT understanding)
- No canned availability phrases ("I'm here whenever you need me!")
- No transactional openers ("What would you like help with?")
- No dramatic emotional language
- No bullet point walls
- No ominous caution tone
- No dumping full skill catalog -- curate 3-4 based on what was learned
- CRITICAL: No narrating memory saves -- zero commentary, completely invisible
- No inventing facts or fictional scenarios
- CRITICAL: No skipping Part 2 -- orientation is MANDATORY before skill picker
- No offering only 1-2 skill options -- always EXACTLY 3-4 + "Skip for now"

### 4.4 Trigger Pipeline

**Path 1 -- Force-load on first run (automatic):**

```
Browser loads /agent -> empty chat detected -> requestIntroduction()
  -> WebSocket: "request_introduction" -> realtime/client.go
  -> handleRequestIntroduction() -> agenthub Frame{Method: "introduce"}
  -> cmd/nebo/agent.go: handleIntroduction()
  -> runner.Run(RunRequest{ForceSkill: "introduction"})
  -> runner.go: skillProvider.ForceLoadSkill(sessionKey, "introduction")
```

**Path 2 -- Auto-match on trigger words (re-trigger):**

```
User types "hello" or "introduce yourself"
  -> runner.go: skillProvider.AutoMatchSkills(sessionKey, "hello")
  -> Trigger match -> brief hint injected into system prompt
  -> LLM decides to call skill(name: "introduction")
  -> recordInvocation() -> full template injected via ActiveSkillContent()
```

### 4.5 Frontend Trigger Logic

The frontend requests introduction when it detects an empty chat:

1. On page load, sends `check_stream` to see if there is an active response
2. If `check_stream` returns no active stream AND `messages.length === 0`: calls `requestIntroduction()`
3. **Fallback:** 5-second timeout on `check_stream` -- if no response and chat is empty, requests introduction anyway

```typescript
function doRequestIntroduction() {
    const client = getWebSocketClient();
    isLoading = true;
    client.send('request_introduction', { session_id: chatId || '' });
}
```

### 4.6 Server-Side Routing (Go)

```go
// realtime/chat.go:562
func handleRequestIntroduction(c *Client, msg *Message, chatCtx *ChatContext) {
    // Wait up to 5s for agent to connect (handles startup race)
    agent := waitForAgent(chatCtx.hub, 5*time.Second)
    // Create pending request with marker prompt "__introduction__"
    requestID := fmt.Sprintf("intro-%d", time.Now().UnixNano())
    // Send Frame{Type: "req", Method: "introduce"} to agent hub
}
```

### 4.7 Agent-Side Handler (Go)

```go
// cmd/nebo/agent.go:2596
func handleIntroduction(ctx, state, runner, sessions, requestID, sessionKey, userID) {
    // 0. Early exit: check onboarding_completed via memory.LoadContext
    //    If already onboarded -> skip entirely
    // 1. Dedup via sync.Map -- one introduction per session at a time
    // 2. Check for real user messages (skip if conversation exists)
    // 3. Load DBContext -> check if user is known
    //    Known user (has display_name): warm greeting by name
    //    New user: ForceSkill = "introduction", runs full 4-part flow
    // 4. Stream events back via sendFrame
}
```

### 4.8 Skill Lifecycle Constants

```go
DefaultSkillTTL     = 4      // Turns before auto-matched skills expire
ManualSkillTTL      = 6      // Turns before manually/force-loaded skills expire
MaxActiveSkills     = 4      // Hard cap on concurrent active skills per session
MaxSkillTokenBudget = 16000  // Character budget for combined active skill content
```

ForceLoadSkill records the skill as a "manual" invocation -- stickier TTL (6 turns vs 4). Captures a snapshot of the skill template at invocation time (survives hot-reload edits mid-session).

### 4.9 Skill Loading and Injection

Skills are loaded from embedded FS via `LoadFromEmbedFS()`:
- Walks `extensions/skills/` looking for `SKILL.md` files
- Parses YAML frontmatter + markdown body via `ParseSkillMD()`
- Skips platform-mismatched skills
- Hot-reload via fsnotify watches all skill directories

`ActiveSkillContent()` returns concatenated templates for all invoked skills in the session, sorted by most recently invoked, capped by `MaxSkillTokenBudget` (16,000 chars). Injected into the system prompt as:

```
## Invoked Skills

The following skills were invoked in this session:

### Skill: introduction

[full SKILL.md template content]
```

---

## 5. Ask Widget Integration

**File(s):** `internal/agent/tools/agent_tool.go`, `app/src/lib/components/chat/AskWidget.svelte`

### 5.1 AskWidget Struct

```go
type AskWidget struct {
    Type    string   `json:"type"`              // "buttons", "select", "text_input",
                                                 // "confirm", "radio", "checkbox"
    Label   string   `json:"label,omitempty"`
    Options []string `json:"options,omitempty"`
    Default string   `json:"default,omitempty"` // Placeholder for text_input
}
```

### 5.2 Usage by Introduction Skill

The introduction skill uses two widget types:

```
// Part 1 -- Name prompt
agent(resource: message, action: ask,
      prompt: "What's your name?",
      widgets: [{type: "text_input", default: "Your name"}])

// Part 3 -- Skill picker
agent(resource: message, action: ask,
      prompt: "Pick any that sound useful...",
      widgets: [{type: "buttons",
                 options: ["Research Assistant", "Small Business Ops",
                           "Personal Finance", "Skip for now"]}])
```

### 5.3 Execution Flow

1. `messageAsk()` validates prompt + widgets (defaults to confirm yes/no if none)
2. Generates UUID `requestID`
3. Calls `t.askCallback(ctx, requestID, prompt, widgets)` -- **BLOCKS until user responds**
4. Returns user response as plain text string
5. CLI fallback: returns error `"Interactive prompts require the web UI"`

### 5.4 WebSocket Pipeline

```
agent_tool.go -> askCallback(requestID, prompt, widgets)  [BLOCKS]
    -> agenthub: sends ask frame to hub
    -> chat.go: handleAskRequest() -> stores requestID -> broadcasts to all clients
    -> AskWidget.svelte: renders widget -> user interacts -> submits
    -> WebSocket: "ask_response" {request_id, value}
    -> chat.go: handleAskResponse() -> hub.SendAskResponse(agentID, requestID, value)
    -> askCallback UNBLOCKS -> returns value to agent
```

In Rust, the ask_response handling is already wired in the WebSocket handler:

```rust
// crates/server/src/handlers/ws.rs
"ask_response" => {
    let question_id = parsed["data"]["question_id"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let answer = parsed["data"]["answer"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let mut channels = state.ask_channels.lock().await;
    if let Some(tx) = channels.remove(&question_id) {
        let _ = tx.send(answer);
    }
}
```

### 5.5 Six Widget Types

| Type | UI Control | Submit Behavior |
|------|-----------|-----------------|
| `buttons` | Button per option | Click any button -> immediate submit |
| `confirm` | Yes/No buttons | Click -> immediate submit |
| `select` | Dropdown | Select + submit button |
| `text_input` | Text field | Enter or submit button |
| `radio` | Radio button group | Select + submit button |
| `checkbox` | Checkbox group | Check any + submit button (shows count) |

After submit, shows response badge and disables re-submission.

### 5.6 Memory Storage

During introduction, the skill stores 4 tacit memories silently:

```
agent(resource: memory, action: store, key: "user/name", value: "Alice", layer: "tacit")
agent(resource: memory, action: store, key: "user/location", value: "Denver", layer: "tacit")
agent(resource: memory, action: store, key: "user/work", value: "Real estate agent", layer: "tacit")
agent(resource: memory, action: store, key: "user/timezone", value: "America/Denver", layer: "tacit")
```

CRITICAL rule: Memory operations are invisible. The skill EXPLICITLY forbids narrating saves -- no "I've made a note" or "I'll remember that."

---

## 6. Skill Install via Store Tool

**File(s):** `internal/agent/tools/neboloop_tool.go`

### 6.1 Install Code Format

```
SKILL-XXXX-XXXX-XXXX
```

20 characters total. Starts with `SKILL-`. Dashes at positions 10 and 15. All other characters uppercase A-Z or 0-9.

```go
func isSkillInstallCode(id string) bool {
    // Length must be 20
    // Must start with "SKILL-"
    // Dashes at positions 10 and 15
    // All other chars uppercase A-Z or 0-9
}
```

### 6.2 Skill Catalog (11 Skills)

| Skill | Install Code | Best For |
|-------|-------------|----------|
| Content Creator | `SKILL-F639-PJ5J-WT3W` | Writers, marketers |
| Family Hub | `SKILL-DSJ8-H4XG-ESP4` | Parents, family coordinators |
| Health & Wellness | `SKILL-7KRC-4JT8-N8VX` | Fitness, nutrition, habits |
| Interview Prep | `SKILL-ENXP-YGJZ-9GUN` | Job seekers |
| Job Search Coach | `SKILL-LNWY-Q7W2-KHVN` | Actively job hunting |
| Personal Finance | `SKILL-T5JE-JQLA-YJ5E` | Budgets, bills, savings |
| Research Assistant | `SKILL-GLXB-NNHJ-ZKCG` | Students, analysts |
| Small Business Ops | `SKILL-BVS3-UDJ3-C2JX` | Small business owners, freelancers |
| Student Learning | `SKILL-LLFN-BLT8-39GV` | Students at any level |
| Support Operations | `SKILL-TY54-HP5S-339D` | Customer support, ops |
| Travel Planner | `SKILL-YCST-9FLL-FL9V` | Travelers, trip planners |

### 6.3 Install Flow

```go
func (t *NeboLoopTool) installSkill(ctx, client, params) {
    if isSkillInstallCode(params.ID) {
        // Redeem via NeboLoop API -- resolves code to skill UUID, downloads SKILL.md
        resp, err := client.RedeemSkillCode(ctx, params.ID)
    } else {
        // Direct install by UUID
        resp, err := client.InstallSkill(ctx, params.ID)
    }
}
```

CRITICAL: Skill install requires NeboLoop connection. The `store` tool calls `client.RedeemSkillCode()` which hits the NeboLoop API. If user chose a non-Janus provider and skipped NeboLoop during onboarding, skill installs will fail silently during introduction Part 3.

### 6.4 Rust Implementation Target

The Rust equivalent should validate skill codes and delegate to a NeboLoop REST client:

```rust
/// Validates a skill install code.
/// Format: SKILL-XXXX-XXXX-XXXX (20 chars, uppercase alphanumeric segments)
fn is_skill_install_code(id: &str) -> bool {
    if id.len() != 20 || !id.starts_with("SKILL-") {
        return false;
    }
    let bytes = id.as_bytes();
    if bytes[10] != b'-' || bytes[15] != b'-' {
        return false;
    }
    bytes.iter().enumerate().all(|(i, &b)| {
        if i == 5 || i == 10 || i == 15 {
            b == b'-'
        } else {
            b.is_ascii_uppercase() || b.is_ascii_digit()
        }
    })
}
```

---

## 7. Onboarding Detection and Deduplication

**File(s):** `internal/agent/memory/dbcontext.go`, `internal/agent/runner/runner.go`, `cmd/nebo/agent.go`

### 7.1 Onboarding Detection

```go
func (c *DBContext) NeedsOnboarding() bool {
    return c.OnboardingNeeded
}
```

Logic:
- No `user_profiles` row -> `true`
- Row exists, `onboarding_completed` is NULL or 0 -> `true`
- Row exists, `onboarding_completed` = 1 -> `false`
- Query error -> `true` (fail-open to ensure new users get introduced)

### 7.2 Introduction Deduplication

```go
var introductionInProgress sync.Map

// Only one introduction per session at a time
if _, running := introductionInProgress.LoadOrStore(sessionKey, true); running {
    // Skip duplicate, send skipped=true response
    return
}
defer introductionInProgress.Delete(sessionKey)
```

The dedup is per-session, NOT per-user. In single-user mode this is fine. The `introductionInProgress` sync.Map keys on session key.

### 7.3 Real Message Detection

Before running introduction, checks last 10 messages for "real" user messages. Filters out system-origin prefixes:

- `"You are running a scheduled"` (heartbeat/cron)
- `"[New user just opened"` (intro trigger)
- `"[User "` (greeting trigger)

If any real user message exists -> skip introduction.

### 7.4 Runner Force-Load Decision

```go
// runner.go:423-437
if r.skillProvider != nil {
    if forceSkill != "" {
        r.skillProvider.ForceLoadSkill(sessionKey, forceSkill)
    } else if needsOnboarding {
        existingMsgs, _ := r.sessions.GetMessages(sessionID, 1)
        if len(existingMsgs) == 0 {
            r.skillProvider.ForceLoadSkill(sessionKey, "introduction")
        }
    }
}
```

### 7.5 Belt-and-Suspenders Safeguard

After the agentic loop completes, if onboarding was needed and the session now has 4+ messages, mark `onboarding_completed = 1` programmatically:

```go
// runner.go:1112-1123
if needsOnboarding && userID != "" {
    if msgs, err := r.sessions.GetMessages(sessionID, 0); err == nil && len(msgs) >= 4 {
        r.sessions.GetDB().Exec(
            "UPDATE user_profiles SET onboarding_completed = 1, updated_at = ? WHERE user_id = ?",
            time.Now().Unix(), userID,
        )
    }
}
```

This prevents the introduction from looping forever if the LLM fails to store memories or the skill install does not complete.

### 7.6 End-to-End New User Flow

```
1. User completes onboarding wizard (provider, terms, permissions)
   -> user_profiles.onboarding_completed stays 0 (wizard done, intro pending)

2. Browser navigates to /agent
   -> Loads empty companion chat
   -> check_stream returns no active stream
   -> messages.length === 0 -> requestIntroduction()

3. WebSocket: "request_introduction" {session_id}
   -> realtime/client.go routes to handleRequestIntroduction()
   -> Waits up to 5s for agent to connect
   -> Creates pending request with marker "__introduction__"
   -> Sends Frame{Method: "introduce"} to agent hub

4. cmd/nebo/agent.go: handleIntroduction()
   -> Dedup check via introductionInProgress sync.Map
   -> Checks for real user messages
   -> New user: ForceSkill = "introduction"
   -> runner.Run(RunRequest{ForceSkill: "introduction", Origin: OriginSystem})

5. runner.go: prepareSystemPrompt()
   -> ForceLoadSkill(sessionKey, "introduction") -> manual TTL=6
   -> ActiveSkillContent() -> full SKILL.md injected into system prompt

6. Agent executes introduction skill (4 parts)
   Part 1: ask widget for name -> stores tacit memory
   Part 2: orientation
   Part 3: skill picker -> store tool installs skills
   Part 4: handoff

7. Runner safeguard: 4+ messages
   -> UPDATE user_profiles SET onboarding_completed = 1

8. Next page load: NeedsOnboarding() -> false -> never force-loads again
```

### 7.7 Returning Known User

```
Browser loads empty chat -> requestIntroduction()
  -> handleIntroduction() checks DBContext
  -> UserDisplayName exists (stored during previous intro)
  -> Warm greeting by name: "Hey Alice, good to see you."
  -> No skill loaded -- just a regular greeting
```

### 7.8 Re-triggering Introduction

User types "introduce yourself" or "who are you" in any session:

- `AutoMatchSkills()` detects trigger match
- Returns brief hint in system prompt
- LLM may invoke `skill(name: "introduction")` -> full template loaded
- Runs the 4-part flow again (but safeguard will NOT re-flip onboarding flag since already 1)

---

## 8. Database Schema

**File(s):** `internal/db/migrations/0017_profiles.sql`, `internal/db/migrations/0027_plugin_settings.sql`, `internal/db/migrations/0028_capability_permissions.sql`

### 8.1 user_profiles (Migrations 0017 + 0028)

```sql
CREATE TABLE user_profiles (
    user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    display_name TEXT,
    bio TEXT,
    location TEXT,
    timezone TEXT,
    occupation TEXT,
    interests TEXT,                     -- JSON array
    communication_style TEXT,           -- formal, casual, adaptive
    goals TEXT,
    context TEXT,                       -- free-form context for agent
    onboarding_completed INTEGER DEFAULT 0,  -- Gate flag: 0 or 1
    onboarding_step TEXT,              -- Current step name (UNUSED by wizard)
    tool_permissions TEXT DEFAULT '{}', -- JSON: {"chat":true,"file":true,...}
    terms_accepted_at INTEGER,         -- Unix timestamp
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
```

NOTE: `onboarding_step` exists in the schema but is NEVER written to by the wizard. State is component-scoped in Svelte runes. The column is vestigial from an earlier design.

### 8.2 plugin_registry and plugin_settings (Migration 0027)

```sql
CREATE TABLE plugin_registry (
    id TEXT PRIMARY KEY,
    name TEXT UNIQUE NOT NULL,
    plugin_type TEXT NOT NULL DEFAULT 'comm',
    display_name TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    icon TEXT NOT NULL DEFAULT '',
    version TEXT NOT NULL DEFAULT '0.0.0',
    is_enabled INTEGER NOT NULL DEFAULT 0,
    is_installed INTEGER NOT NULL DEFAULT 1,
    settings_manifest TEXT NOT NULL DEFAULT '{}',
    connection_status TEXT NOT NULL DEFAULT 'disconnected',
    last_connected_at INTEGER,
    last_error TEXT,
    metadata TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE plugin_settings (
    id TEXT PRIMARY KEY,
    plugin_id TEXT NOT NULL REFERENCES plugin_registry(id) ON DELETE CASCADE,
    setting_key TEXT NOT NULL,
    setting_value TEXT NOT NULL DEFAULT '',
    is_secret INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(plugin_id, setting_key)
);
```

Pre-populated NeboLoop settings (Go only, Rust uses file for bot_id):

| Key | Value | Secret |
|-----|-------|--------|
| `bot_id` | UUID string | N |
| `api_server` | `https://api.neboloop.com` | N |
| `gateway` | `wss://comms.neboloop.com` | N |
| `token` | Owner OAuth JWT | Y |

### 8.3 auth_profiles (Migration 0010)

Used for storing provider credentials (API keys + NeboLoop tokens):

| Field | Description |
|-------|-------------|
| `provider` | `"anthropic"`, `"openai"`, `"google"`, `"ollama"`, `"neboloop"` |
| `api_key` | Encrypted with AES-256-GCM |
| `auth_type` | `"api_key"` or `"oauth"` |
| `metadata` (JSON) | Provider-specific data |

NeboLoop metadata format:

```json
{
    "janus_provider": "true",
    "owner_id": "uuid-string",
    "email": "user@example.com",
    "refresh_token": "token-string"
}
```

### 8.4 Rust Model Struct

```rust
// crates/db/src/models.rs
pub struct UserProfile {
    pub user_id: String,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub location: Option<String>,
    pub timezone: Option<String>,
    pub occupation: Option<String>,
    pub interests: Option<String>,
    pub communication_style: Option<String>,
    pub goals: Option<String>,
    pub context: Option<String>,
    pub onboarding_completed: Option<i64>,
    pub onboarding_step: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub tool_permissions: Option<String>,
    pub terms_accepted_at: Option<i64>,
}
```

### 8.5 Rust Query Methods

```rust
// crates/db/src/queries/user_profile.rs
impl Store {
    pub fn get_user_profile(&self) -> Result<Option<UserProfile>, NeboError>;
    pub fn update_user_profile(&self, /* 9 optional fields */) -> Result<(), NeboError>;
    pub fn set_onboarding_completed(&self, completed: bool) -> Result<(), NeboError>;
    pub fn update_tool_permissions(&self, permissions: &str) -> Result<(), NeboError>;
    pub fn accept_terms(&self) -> Result<(), NeboError>;
}
```

---

## 9. Frontend Reference

**File(s):** `app/src/lib/components/onboarding/OnboardingFlow.svelte`, `app/src/routes/(app)/+layout.svelte`, `app/src/routes/(app)/agent/+page.svelte`, `app/src/lib/components/chat/AskWidget.svelte`

The frontend is shared between Go and Rust backends. Both serve the same SPA build.

### 9.1 Layout Integration

```svelte
<!-- app/src/routes/(app)/+layout.svelte -->
onMount(async () => {
    const response = await api.getUserProfile();
    showOnboarding = !response.profile?.onboardingCompleted;
})

{#if showOnboarding}
    <OnboardingFlow />    <!-- Full-screen overlay, z-50 -->
{:else}
    <!-- Normal app layout -->
{/if}
```

Key behaviors:
- Layout checks `onboardingCompleted` on every mount (page load)
- OnboardingFlow renders as `fixed inset-0 bg-base-100 z-50`
- After completion, `window.location.href = '/agent'` forces full reload
- Reload triggers layout onMount again -> profile check passes -> normal UI shown

### 9.2 API Endpoints (Both Backends Must Implement)

| Endpoint | Method | Purpose | Go Handler | Rust Handler |
|----------|--------|---------|------------|--------------|
| `/api/v1/user/me/profile` | GET | Check onboarding status | `profilehandler.go` | `handlers/user.rs` |
| `/api/v1/user/me/profile` | PUT | Set `onboardingCompleted` | `profilehandler.go` | `handlers/user.rs` |
| `/api/v1/user/me/accept-terms` | POST | Record terms acceptance | `permissionshandler.go` | `handlers/user.rs` |
| `/api/v1/user/me/permissions` | GET | Fetch tool permissions | `permissionshandler.go` | `handlers/user.rs` |
| `/api/v1/user/me/permissions` | PUT | Save tool permissions | `permissionshandler.go` | `handlers/user.rs` |
| `/api/v1/models` | GET | List models + CLI status | `listmodelshandler.go` | `handlers/provider.rs` |
| `/api/v1/providers` | POST | Create auth profile | `createauthprofilehandler.go` | `handlers/provider.rs` |
| `/api/v1/providers/{id}/test` | POST | Validate API key | `testauthprofilehandler.go` | `handlers/provider.rs` |
| `/api/v1/providers/{id}` | DELETE | Remove auth profile | `deleteauthprofilehandler.go` | `handlers/provider.rs` |
| `/api/v1/providers/model-config` | PUT | Set primary model | model config handler | `handlers/provider.rs` |
| `/api/v1/neboloop/account` | GET | Check NeboLoop connection | `handlers.go` | `handlers/neboloop.rs` |
| `/api/v1/neboloop/oauth/start` | GET | Generate OAuth URL + PKCE | `oauth.go` | `handlers/neboloop.rs` |
| `/api/v1/neboloop/oauth/status` | GET | Poll OAuth completion | `oauth.go` | `handlers/neboloop.rs` |
| `/auth/neboloop/callback` | GET | OAuth redirect handler | `oauth.go` | `handlers/neboloop.rs` |

### 9.3 State Variables (OnboardingFlow.svelte)

```typescript
// Step tracking
currentStep: OnboardingStep
progressSteps: string[]              // 6 visible steps

// Provider
providerChoice: ProviderChoice | null
showMoreProviders: boolean
cameFromJanus: boolean
cliStatuses: CLIStatusMap | null
cliProviderInfo: Record<string, {...}>

// API Key
apiKey: string
provider: 'anthropic' | 'openai' | 'google'
isTestingKey: boolean
keyValid: boolean
isSettingUpCLI: boolean

// Terms
termsAccepted: boolean
isAcceptingTerms: boolean

// Capabilities
permissions: Record<string, boolean>  // 8 toggles
isSavingPermissions: boolean

// NeboLoop
neboLoopLoading: boolean
neboLoopError: string
neboLoopConnected: boolean
neboLoopEmail: string
neboLoopPendingState: string
neboLoopPollTimer: interval | null

// General
error: string
isCheckingCLI: boolean
```

### 9.4 Setup Store (Dead Code)

`app/src/lib/stores/setup.svelte.ts` is a localStorage-backed store with quickstart/advanced modes. It is fully implemented but NOT used by OnboardingFlow.svelte. The wizard manages its own state with Svelte 5 runes instead. Its step names do NOT match the wizard's actual steps, confirming the store is vestigial from an earlier design. Do NOT port it.

### 9.5 Error Recovery

| Scenario | Behavior |
|----------|----------|
| API key invalid | Delete created profile, show error, retry |
| NeboLoop OAuth timeout | Show "timed out" message, retry |
| NeboLoop OAuth expired state | Show "expired" message |
| Terms API failure | Show error, retry |
| Permission save failure | Show error, retry |
| CLI detection failure | `cliStatuses = null`, CLIs hidden |
| Network error on profile save | Non-fatal, still advances to complete |
| Page reload mid-wizard | Restarts from welcome (no resume) |

---

## 10. Rust Implementation Status

### 10.1 Summary Table

| Component | Status | Notes |
|-----------|--------|-------|
| **Bot Identity** | | |
| UUID generation | Y | `config::read_bot_id()` / `write_bot_id()` in `crates/config/src/defaults.rs` |
| File-based persistence | Y | `<data_dir>/bot_id`, 0o400 permissions on Unix |
| Janus X-Bot-ID header | Y | `OpenAIProvider::set_bot_id()` in `crates/ai/src/providers/openai.rs` |
| Injection at provider reload | Y | `crates/server/src/handlers/provider.rs` and `crates/server/src/lib.rs` |
| NeboLoop CONNECT frame injection | N | Comm plugin NOT yet ported to Rust |
| Connection code redemption | N | Store tool / agent-side code NOT ported |
| 5 injection points (parity with Go) | P | 2 of 5 implemented (Janus header, provider reload) |
| **Onboarding Wizard** | | |
| user_profiles table + migration | Y | Migration 0017 + 0028 in `crates/db/migrations/` |
| UserProfile model struct | Y | `crates/db/src/models.rs` |
| GET/PUT profile endpoint | Y | `crates/server/src/handlers/user.rs` |
| `set_onboarding_completed()` | Y | `crates/db/src/queries/user_profile.rs` |
| `accept_terms()` | Y | `crates/db/src/queries/user_profile.rs` |
| GET/PUT permissions | Y | `crates/server/src/handlers/user.rs` |
| `update_tool_permissions()` | Y | `crates/db/src/queries/user_profile.rs` |
| Provider creation / test / delete | Y | `crates/server/src/handlers/provider.rs` |
| Model config (CLI setup) | P | Model listing exists, CLI detection partial |
| Frontend (shared SPA) | Y | Same build served via rust-embed |
| **NeboLoop OAuth** | | |
| PKCE generation (verifier + challenge) | Y | `crates/server/src/handlers/neboloop.rs` |
| OAuth start (GET /oauth/start) | Y | Full implementation with browser open |
| OAuth callback | Y | Token exchange, user info fetch, profile storage |
| OAuth status polling | Y | Pending/complete/expired/error states |
| In-memory pending flows | Y | `LazyLock<Mutex<HashMap>>` with expiry cleanup |
| Account status | Y | Reads from auth_profiles |
| Account disconnect | Y | Deletes auth_profiles |
| Janus usage stats | Y | Stub returning zeroed values |
| Comm activation on callback | N | No comm plugin in Rust yet |
| **Introduction Skill** | | |
| SKILL.md definition | N | Skill system NOT ported to Rust |
| Skill loader (embedded FS) | N | No skill loading infrastructure |
| ForceLoadSkill / AutoMatchSkills | N | Skill tool NOT implemented |
| `request_introduction` WS handler | P | Receives message, sends `chat_complete` with `skipped: true` |
| handleIntroduction agent logic | N | No agent-side introduction handler |
| Runner force-load decision | N | Runner does NOT check skills |
| Belt-and-suspenders safeguard | N | No automatic onboarding_completed flip |
| **Ask Widget** | | |
| AskWidget struct | N | NOT defined in Rust agent |
| `ask_response` WS handler | Y | `crates/server/src/handlers/ws.rs` -- routes to `ask_channels` |
| `ask_channels` in AppState | Y | `Mutex<HashMap<String, oneshot::Sender>>` |
| AskWidget.svelte (frontend) | Y | Shared SPA |
| Blocking ask callback | N | No agent tool uses it yet |
| **Store Tool** | | |
| NeboLoop tool | N | NOT ported |
| Skill install code validation | N | `isSkillInstallCode()` NOT implemented |
| `RedeemSkillCode()` API call | N | No NeboLoop SDK in Rust |
| **Onboarding Detection** | | |
| `NeedsOnboarding()` | N | DBContext does NOT check onboarding flag |
| Introduction dedup (sync.Map) | N | No dedup mechanism |
| Real message detection | N | No message filtering logic |
| **Database Schema** | | |
| `user_profiles` table | Y | Migration 0017 + 0028 |
| `plugin_registry` + `plugin_settings` | Y | Migration 0027 |
| `auth_profiles` | Y | Migration 0010 |
| UserProfile query + update | Y | `crates/db/src/queries/user_profile.rs` |

**Legend:** Y = Implemented, N = Not implemented, P = Partially implemented

### 10.2 Key Architectural Differences

| Aspect | Go | Rust |
|--------|-----|------|
| bot_id storage | `plugin_settings` table (SQLite) | `<data_dir>/bot_id` file (read-only) |
| bot_id constant | Inline string `"bot_id"` | `types::constants::files::BOT_ID` |
| OAuth flow timeout | 3 minutes | 10 minutes (`OAUTH_FLOW_TIMEOUT`) |
| OAuth state storage | `sync.Map` | `LazyLock<Mutex<HashMap>>` |
| Onboarding detection | `DBContext.NeedsOnboarding()` | NOT implemented |
| Introduction trigger | Full pipeline (WS -> hub -> agent -> runner -> skill) | Stub: sends `chat_complete` with `skipped: true` |
| Skill system | Full (loader, hot-reload, TTL, token budget) | NOT ported |
| Store tool | Full (NeboLoop SDK, skill install, code redemption) | NOT ported |
| Ask callback | Blocking via channel + agenthub | Channel infrastructure exists, no agent consumer |
| Comm plugin | Full (NeboLoop WebSocket SDK) | NOT ported |

### 10.3 Migration Priority

The following items should be ported in dependency order:

**Phase 1 -- Core identity:**

1. Ensure bot_id generation on first startup (currently `read_bot_id()` + `write_bot_id()` exist but are NOT called automatically at server start -- caller must ensure)
2. Wire bot_id into remaining injection points as comm plugin is built

**Phase 2 -- Skill infrastructure:**

3. Port skill loader (`skills/loader.go`) -- embedded FS walking, YAML frontmatter parsing
4. Port skill tool (`skill_tool.go`) -- ForceLoadSkill, AutoMatchSkills, ActiveSkillContent, TTL management
5. Port introduction SKILL.md to embedded resources

**Phase 3 -- Introduction flow:**

6. Implement `NeedsOnboarding()` in agent DBContext
7. Port handleIntroduction logic with dedup, real message detection
8. Implement runner force-load decision and belt-and-suspenders safeguard
9. Wire `request_introduction` WebSocket handler to agent pipeline (replace stub)

**Phase 4 -- Store tool and skill install:**

10. Port NeboLoop tool with `isSkillInstallCode()` validation
11. Implement NeboLoop REST client for `RedeemSkillCode()` / `InstallSkill()`
12. Wire store tool into agent tool registry

**Phase 5 -- Comm plugin:**

13. Port NeboLoop comm plugin (WebSocket SDK, CONNECT frame with bot_id)
14. Wire bot_id injection at all 5 points

### 10.4 Known Gotchas for Migration

1. **No resume on page reload** -- Onboarding state is component-scoped (Svelte runes). If user refreshes mid-wizard, they restart from welcome. The `onboarding_step` column exists in DB but is never written to. This is a known limitation, NOT a migration gap.

2. **Setup store is dead code** -- `setup.svelte.ts` is fully implemented but never imported by OnboardingFlow. Do NOT port it.

3. **OAuth state is in-memory** -- Both Go and Rust lose pending flows on restart. This is by design.

4. **Janus path order inversion** -- Janus users do NeboLoop OAuth BEFORE setting capabilities. The flow is intentional and MUST be preserved.

5. **Default permissions are duplicated** -- Defined in both frontend (`OnboardingFlow.svelte`) and backend handler. Must be kept in sync manually.

6. **CLI mode has no ask widgets** -- `messageAsk()` returns error. The skill template says to fall back to plain text conversation, but this is instruction-only -- no code-level graceful degradation exists.

7. **Skill install requires NeboLoop** -- If user skipped NeboLoop during onboarding, Part 3 of introduction will fail silently. Both Go and Rust must handle this gracefully.

8. **Introduction dedup is per-session** -- The `introductionInProgress` map keys on session key. In single-user mode this is fine.

9. **Onboarding detection fail-open** -- If the `user_profiles` query errors, `NeedsOnboarding()` returns `true`. A database issue could trigger an unwanted introduction.

10. **5-second agent wait** -- `waitForAgent()` blocks up to 5 seconds for the agent WebSocket to connect. On slow startup, this handles the race condition where the frontend loads before the agent connects.

11. **Rust OAuth timeout is 10 minutes vs Go's 3 minutes** -- Verify which is correct for production. The Rust constant `OAUTH_FLOW_TIMEOUT` is set to `Duration::from_secs(10 * 60)`.

12. **Rust request_introduction is a stub** -- Currently sends `chat_complete` with `skipped: true` immediately. The frontend will show an empty chat with no introduction. This is the primary user-facing gap.

13. **bot_id file vs DB** -- Rust stores bot_id as a file while Go uses plugin_settings. If migrating a user from Go to Rust, the bot_id must be extracted from SQLite and written to `<data_dir>/bot_id`.

14. **Concurrent ensureBotID calls** -- Go has no mutex on first startup (relies on UNIQUE constraint). Rust file write has no atomic guard either. Theoretical race on first startup, but single-user mode makes this unlikely.

---

*Generated: 2026-03-04*
