# Onboarding System — SME Deep-Dive

> **Scope:** First-run setup wizard — frontend component, backend handlers, database schema, OAuth flow, CLI command.

---

## Architecture Overview

Nebo's onboarding is a **single Svelte component wizard** (`OnboardingFlow.svelte`) that renders as a full-screen overlay over the app layout. It gates all app access until `onboarding_completed = 1` in the `user_profiles` table.

**Design principles:**
- Single-user mode (hardcoded `user_id = "default-user"`)
- State managed entirely in Svelte 5 runes (component-scoped, no shared store)
- Progress persists to SQLite via API calls at each step transition
- Full page reload after completion triggers re-check in layout
- Janus (NeboLoop-managed AI) is the recommended/default provider path

---

## File Map

| File | Purpose |
|------|---------|
| `app/src/lib/components/onboarding/OnboardingFlow.svelte` | The wizard UI (1047 lines, single file) |
| `app/src/lib/stores/setup.svelte.ts` | localStorage-based store (**defined but NOT used** by the wizard) |
| `app/src/routes/(app)/+layout.svelte` | Onboarding gate — checks `onboardingCompleted` flag |
| `internal/handler/user/profilehandler.go` | Get/Update user profile (includes onboarding flag) |
| `internal/handler/user/permissionshandler.go` | Accept terms + Get/Update tool permissions |
| `internal/handler/provider/listmodelshandler.go` | List models + CLI detection |
| `internal/handler/provider/createauthprofilehandler.go` | Create auth profile (API key) |
| `internal/handler/provider/testauthprofilehandler.go` | Test auth profile |
| `internal/handler/provider/deleteauthprofilehandler.go` | Delete auth profile |
| `internal/handler/neboloop/oauth.go` | NeboLoop OAuth start/callback/status |
| `internal/handler/neboloop/handlers.go` | NeboLoop account status/disconnect |
| `internal/db/migrations/0017_profiles.sql` | `user_profiles` table (includes onboarding columns) |
| `internal/db/migrations/0028_capability_permissions.sql` | Adds `tool_permissions` + `terms_accepted_at` |
| `cmd/nebo/onboard.go` | CLI `nebo onboard` command (interactive terminal wizard) |

---

## Step Flow (7 internal steps, 6 visible to user)

```
┌─────────┐    ┌───────┐    ┌─────────────────┐    ┌─────────┐
│ welcome │───►│ terms │───►│ provider-choice │───►│ api-key │──┐
└─────────┘    └───────┘    └────────┬────────┘    └─────────┘  │
                                     │                           │
                              ┌──────┴──────┐                    │
                              │   (janus)   │                    │
                              ▼             ▼                    │
                        ┌──────────┐  ┌─────────┐               │
                        │ neboloop │  │ CLI     │               │
                        │ (OAuth)  │  │ setup   │               │
                        └────┬─────┘  └────┬────┘               │
                             │             │                     │
                             ▼             ▼                     ▼
                        ┌──────────────────────────────────────────┐
                        │            capabilities                   │
                        └──────────────────┬───────────────────────┘
                                           │
                              ┌─────────────┴──────────────┐
                              │ (if !cameFromJanus)        │
                              ▼                            ▼
                        ┌──────────┐                ┌──────────┐
                        │ neboloop │                │ complete  │
                        │ (opt-in) │───────────────►│          │
                        └──────────┘   (or skip)    └──────────┘
```

### Progress Dots

6 user-visible dots: `welcome`, `terms`, `provider-choice`, `capabilities`, `neboloop`, `complete`

The `api-key` step is hidden from the progress indicator — it maps to `provider-choice`'s dot position.

---

## Step Details

### 1. Welcome

**UI:** Sparkles icon, "Welcome to Nebo" heading, "Get Started" button.
**Action:** `currentStep = 'terms'` (client-side only, no API call).

### 2. Terms

**UI:** Shield icon, 5 privacy/data disclosure sections in scrollable box, checkbox acceptance.
**API Call:** `POST /api/v1/user/me/accept-terms`
**DB Effect:** Sets `terms_accepted_at = unixepoch()` in `user_profiles`
**Gate:** Checkbox `termsAccepted` must be checked before button enables.
**Disclosures:**
1. Your Data Stays Local (SQLite, no server sync)
2. AI Provider Communication (messages sent to chosen provider)
3. API Keys (stored locally, encrypted)
4. System Access (user-controlled capabilities)
5. No Analytics or Telemetry

### 3. Provider Choice

**UI:** Radio-style card selection. Janus pre-selected with "Recommended" badge.
**On Mount:** Calls `GET /api/v1/models` to detect CLI tools + load models.yaml config.
**Options:**

| Option | Badge | Condition | Next Step |
|--------|-------|-----------|-----------|
| Janus | Recommended | Always shown | `neboloop` (sets `cameFromJanus = true`) |
| Claude Code CLI | Ready | Installed + authenticated | `capabilities` (via `setupCLI()`) |
| Codex CLI | Ready | Installed + authenticated | `capabilities` (via `setupCLI()`) |
| Gemini CLI | Ready | Installed + authenticated | `capabilities` (via `setupCLI()`) |
| [CLI] | Needs Login | Installed, not authenticated | Disabled (shows terminal instructions) |
| Add API Key | — | Always shown | `api-key` |

**Expandable Section:** "Use your own API key or CLI instead" — CLIs + API key are hidden by default behind a `ChevronDown` toggle.

**CLI Setup (`setupCLI()`):**
1. Gets provider info from `cliProviderInfo[cliKey]`
2. Calls `PUT /api/v1/providers/model-config` with `{ primary: "{id}/{defaultModel}" }`
3. Advances to `capabilities`

### 4. API Key (hidden from progress dots)

**UI:** Key icon, provider dropdown (Anthropic/OpenAI/Google), password input, "Get an API key" link.
**Flow (`testAndSaveApiKey()`):**
1. `POST /api/v1/providers` — creates auth profile with encrypted API key
2. `POST /api/v1/providers/{id}/test` — validates key against provider
3. **If valid:** Show success alert, auto-advance to `capabilities` after 500ms
4. **If invalid:** Show error, `DELETE /api/v1/providers/{id}` to clean up
**Back button:** Returns to `provider-choice`

### 5. Capabilities

**UI:** Shield/info icon, 8 toggle cards in scrollable grid.
**Capabilities:**

| Key | Label | Default | Notes |
|-----|-------|---------|-------|
| `chat` | Chat & Memory | ON | Locked (always on, `alwaysOn: true`) |
| `file` | File System | ON | |
| `shell` | Shell & Terminal | OFF | Security-sensitive |
| `web` | Web Browsing | ON | |
| `contacts` | Contacts & Calendar | OFF | Privacy-sensitive |
| `desktop` | Desktop Control | ON | |
| `media` | Media & Capture | OFF | Security-sensitive |
| `system` | System | ON | |

**Submit (`savePermissionsAndFinish()`):**
1. `PUT /api/v1/user/me/permissions` — saves `{ permissions: {...} }` as JSON
2. **If `cameFromJanus`:** Calls `completeOnboarding()` directly (NeboLoop already connected)
3. **Otherwise:** Advances to `neboloop` step + calls `checkNeboLoopStatus()`

### 6. NeboLoop

**UI:** Store icon. Two modes: OAuth prompt or "Already Connected" display.

**If already connected:** Shows email + "Continue" button → `completeOnboarding()` (non-Janus) or `capabilities` (Janus returning).

**OAuth Flow (`startNeboLoopOAuth()`):**
1. Calls `GET /api/v1/neboloop/oauth/start?janus={cameFromJanus}` — generates PKCE params
2. Backend opens browser to `https://neboloop.com/oauth/authorize?...`
3. Frontend starts polling `GET /api/v1/neboloop/oauth/status?state=...` every 2 seconds
4. 3-minute timeout auto-cancels
5. Browser redirects to `http://localhost:{PORT}/auth/neboloop/callback`
6. Callback exchanges code for tokens, stores profile, activates comms, renders close-window HTML
7. Poll detects `status: "complete"` → shows success, enables "Continue"

**Navigation:**
- **Janus path, connected:** "Continue" → `capabilities` step
- **Non-Janus path, connected:** "Continue" → `completeOnboarding()`
- **"Skip for now"** (non-Janus only): → `completeOnboarding()`
- **"← Back":**
  - Janus path: resets `cameFromJanus = false`, goes to `provider-choice`
  - Non-Janus: goes to `capabilities`

### 7. Complete

**UI:** Green checkmark, "You're all set!" heading, "Start Chatting" button.
**On arrive:** `completeOnboarding()` already called `PUT /api/v1/user/me/profile { onboardingCompleted: true }`
**Button (`finishOnboarding()`):** `window.location.href = '/agent'` — full page reload.

---

## Decision Tree (Janus vs Non-Janus)

```
Provider Choice:
├── Janus selected:
│   1. provider-choice → neboloop (OAuth) → capabilities → complete
│   (cameFromJanus = true throughout)
│
├── CLI selected:
│   1. provider-choice → setupCLI() → capabilities → neboloop (optional, skippable) → complete
│
└── API Key selected:
    1. provider-choice → api-key → capabilities → neboloop (optional, skippable) → complete
```

**Key insight:** The Janus path does NeboLoop OAuth BEFORE capabilities (because Janus requires NeboLoop account). Non-Janus paths do capabilities first, NeboLoop is optional.

---

## State Variables

```typescript
// Step tracking
currentStep: OnboardingStep          // Current wizard step
progressSteps: string[]              // 6 visible steps for dot indicators

// Provider
providerChoice: ProviderChoice | null  // Selected provider option
showMoreProviders: boolean             // Expanded CLI/API key section
cameFromJanus: boolean                 // User chose Janus path
cliStatuses: CLIStatusMap | null       // Detected CLI tools (from API)
cliProviderInfo: Record<string, {...}> // CLI config from models.yaml (from API)

// API Key
apiKey: string                         // User-entered key
provider: 'anthropic' | 'openai' | 'google'  // Selected provider
isTestingKey: boolean                  // Loading state during test
keyValid: boolean                      // Test result
isSettingUpCLI: boolean               // Loading state during CLI setup

// Terms
termsAccepted: boolean                 // Checkbox state
isAcceptingTerms: boolean             // Loading state

// Capabilities
permissions: Record<string, boolean>   // 8 permission toggles
isSavingPermissions: boolean          // Loading state

// NeboLoop
neboLoopLoading: boolean              // OAuth in progress
neboLoopError: string                 // Error message
neboLoopConnected: boolean            // OAuth completed
neboLoopEmail: string                 // User email from OAuth
neboLoopPendingState: string          // PKCE state parameter
neboLoopPollTimer: interval | null    // Polling timer reference

// General
error: string                         // Step-level error message
isCheckingCLI: boolean                // Loading CLI detection
```

---

## API Endpoints

| Endpoint | Method | Handler File | Purpose |
|----------|--------|-------------|---------|
| `/api/v1/user/me/profile` | GET | `profilehandler.go` | Check onboarding status |
| `/api/v1/user/me/profile` | PUT | `profilehandler.go` | Set `onboardingCompleted = true` |
| `/api/v1/user/me/accept-terms` | POST | `permissionshandler.go` | Record terms acceptance |
| `/api/v1/user/me/permissions` | GET | `permissionshandler.go` | Fetch tool permissions |
| `/api/v1/user/me/permissions` | PUT | `permissionshandler.go` | Save tool permissions |
| `/api/v1/models` | GET | `listmodelshandler.go` | List models + CLI status + CLI providers |
| `/api/v1/providers` | POST | `createauthprofilehandler.go` | Create auth profile (encrypted key) |
| `/api/v1/providers/{id}/test` | POST | `testauthprofilehandler.go` | Validate API key |
| `/api/v1/providers/{id}` | DELETE | `deleteauthprofilehandler.go` | Remove auth profile |
| `/api/v1/providers/model-config` | PUT | (model config handler) | Set primary model (CLI setup) |
| `/api/v1/neboloop/account` | GET | `handlers.go` | Check NeboLoop connection |
| `/api/v1/neboloop/oauth/start` | GET | `oauth.go` | Generate OAuth URL + PKCE |
| `/api/v1/neboloop/oauth/status` | GET | `oauth.go` | Poll OAuth completion |
| `/auth/neboloop/callback` | GET | `oauth.go` | OAuth redirect handler |

---

## Database Schema

### user_profiles (Migration 0017 + 0028)

```sql
CREATE TABLE user_profiles (
    user_id TEXT PRIMARY KEY,          -- "default-user"
    display_name TEXT,
    bio TEXT,
    location TEXT,
    timezone TEXT,
    occupation TEXT,
    interests TEXT,                     -- JSON array
    communication_style TEXT,
    goals TEXT,
    context TEXT,
    onboarding_completed INTEGER DEFAULT 0,  -- Gate flag: 0 or 1
    onboarding_step TEXT,              -- Current step name (unused by wizard)
    tool_permissions TEXT DEFAULT '{}', -- JSON: {"chat":true,"file":true,...}
    terms_accepted_at INTEGER,         -- Unix timestamp
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
```

### auth_profiles (Migration 0010)

Used for storing provider credentials (API keys + NeboLoop tokens):
- `provider` field: `"anthropic"`, `"openai"`, `"google"`, `"ollama"`, `"neboloop"`
- `api_key` encrypted with AES-256-GCM
- `metadata` JSON: `{"janus_provider":"true", "owner_id":"uuid", "email":"...", "refresh_token":"..."}`

---

## NeboLoop OAuth (PKCE) Flow

```
Frontend                    Backend                     NeboLoop
   │                           │                           │
   │ startNeboLoopOAuth()      │                           │
   ├──GET /oauth/start─────────►                           │
   │                           │ Generate PKCE:            │
   │                           │  state, codeVerifier,     │
   │                           │  codeChallenge            │
   │                           │ Store in pendingFlows     │
   │                           │ Open browser ─────────────►
   │◄──{state, authorizeURL}───┤                           │
   │                           │                           │
   │ Poll every 2s:            │                           │
   ├──GET /oauth/status────────►                           │
   │◄──{status:"pending"}──────┤                           │
   │                           │                           │
   │                           │    ◄──callback?code=X─────┤
   │                           │ exchangeOAuthCode()       │
   │                           │ fetchUserInfo()           │
   │                           │ storeNeboLoopProfile()    │
   │                           │ activateNeboLoopComm()    │
   │                           │ Mark flow complete        │
   │                           │ Return close-window HTML  │
   │                           │                           │
   ├──GET /oauth/status────────►                           │
   │◄──{status:"complete",     │                           │
   │    email:"user@..."}──────┤                           │
   │                           │                           │
   │ neboLoopConnected = true  │                           │
```

**Timeout:** 3 minutes. **Poll interval:** 2 seconds.
**State storage:** `pendingFlows` sync.Map (in-memory, lost on server restart).

---

## Layout Integration

**File:** `app/src/routes/(app)/+layout.svelte`

```svelte
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

**Key behaviors:**
- Layout checks `onboardingCompleted` on every mount (page load)
- OnboardingFlow renders as `fixed inset-0 bg-base-100 z-50` — covers entire viewport
- After completion, `window.location.href = '/agent'` forces full reload
- Reload triggers layout's `onMount` again → profile check passes → normal UI shown

---

## Setup Store (Unused)

**File:** `app/src/lib/stores/setup.svelte.ts`

A localStorage-backed store with quickstart/advanced modes, step tracking, and persistence. **Fully implemented but NOT used by OnboardingFlow.svelte.** The wizard manages its own state with Svelte 5 runes instead.

The store defines two mode tracks:
- Quickstart: `welcome → account → provider → neboloop → complete`
- Advanced: `welcome → account → provider → neboloop → models → permissions → personality → complete`

These step names don't match the wizard's actual steps, confirming the store is vestigial from an earlier design.

---

## CLI Onboard Command

**File:** `cmd/nebo/onboard.go`

`nebo onboard` — a terminal-based interactive setup wizard (separate from the web wizard):
1. Creates Nebo data directory
2. Interactive provider selection (Anthropic, OpenAI, Google, Ollama)
3. Optional channel setup (Telegram, Discord, Slack)
4. Generates `config.yaml` and channel configs

This is independent of the web UI onboarding flow.

---

## Error Recovery

| Scenario | Behavior |
|----------|----------|
| API key invalid | Delete created profile, show error, user can retry |
| NeboLoop OAuth timeout (3min) | Show "timed out" message, user can retry |
| NeboLoop OAuth expired state | Show "expired" message |
| Terms API failure | Show error, user can retry |
| Permission save failure | Show error, user can retry |
| CLI detection failure | `cliStatuses = null`, CLIs hidden |
| Network error on profile save | Non-fatal in `completeOnboarding()`, still advances to complete step |
| Page reload mid-wizard | Restarts from welcome (no resume, state is component-scoped) |

---

## Gotchas & Known Issues

1. **No resume on page reload** — State is component-scoped (Svelte runes), not persisted to localStorage or DB step tracking. If user refreshes mid-wizard, they restart from welcome. The `onboarding_step` column exists in DB but is never written to.

2. **Setup store is dead code** — `setup.svelte.ts` is fully implemented but never imported by OnboardingFlow. Its step names don't match the wizard's actual steps.

3. **OAuth state is in-memory** — `pendingFlows` sync.Map is lost on server restart. If Nebo restarts while user is completing OAuth in browser, the callback will fail.

4. **Janus path order inversion** — Janus users do NeboLoop OAuth BEFORE setting capabilities. This means if they cancel OAuth and go back, `cameFromJanus` is reset and they must re-choose a provider.

5. **`authenticatedCLIs` is a derived function** — It returns a closure (`$derived(() => {...})`), so it must be called as `authenticatedCLIs()` in templates, not accessed as a value.

6. **Default permissions are duplicated** — Defined in both `OnboardingFlow.svelte` (frontend defaults) and `permissionshandler.go` (backend defaults). Must be kept in sync manually.
