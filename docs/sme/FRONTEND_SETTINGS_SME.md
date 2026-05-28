# Frontend Settings System & Onboarding Flow — SME Document

Deep-dive reference for the Nebo frontend settings panel and first-run onboarding wizard.
Covers architecture, navigation, every settings section, data flow, stores, API integration,
and the multi-step onboarding process.

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Settings Shell & Navigation](#2-settings-shell--navigation)
3. [Settings Sections — Detail](#3-settings-sections--detail)
   - 3.1 Account
   - 3.2 Profile
   - 3.3 Billing
   - 3.4 Usage
   - 3.5 Identity
   - 3.6 Personality
   - 3.7 Rules
   - 3.8 Advisors
   - 3.9 Agents
   - 3.10 Skills
   - 3.11 Plugins
   - 3.12 MCP
   - 3.13 Providers (dev-only)
   - 3.14 Routing (dev-only)
   - 3.15 Secrets (dev-only)
   - 3.16 Permissions
   - 3.17 Sessions
   - 3.18 Memories
   - 3.19 Status
   - 3.20 Developer
   - 3.21 About
4. [Data Flow & State Management](#4-data-flow--state-management)
5. [API Endpoints Summary](#5-api-endpoints-summary)
6. [Validation & Save Patterns](#6-validation--save-patterns)
7. [Onboarding Flow](#7-onboarding-flow)
8. [Onboarding State Machine](#8-onboarding-state-machine)
9. [Onboarding Store & Backend Sync](#9-onboarding-store--backend-sync)
10. [Cross-Cutting Concerns](#10-cross-cutting-concerns)

---

## 1. Architecture Overview

```
+layout.svelte (root)
 |
 |  $effect: if (!onboardingComplete) goto('/onboarding')
 |
 +-- /onboarding/+layout.svelte   (full-screen overlay, z-60)
 |     +-- /onboarding/+page.svelte  (5-step wizard)
 |
 +-- /settings/+layout.svelte      (wraps SettingsShell)
       +-- SettingsShell.svelte     (modal overlay, z-60, sidebar + content)
             |
             +-- /settings/account/+page.svelte
             +-- /settings/profile/+page.svelte
             +-- /settings/billing/+page.svelte
             +-- /settings/usage/+page.svelte
             +-- /settings/identity/+page.svelte
             +-- /settings/personality/+page.svelte
             +-- /settings/rules/+page.svelte
             +-- /settings/advisors/+page.svelte
             +-- /settings/agents/+page.svelte
             +-- /settings/skills/+page.svelte
             +-- /settings/plugins/+page.svelte
             +-- /settings/mcp/+page.svelte
             +-- /settings/providers/+page.svelte    (dev-only)
             +-- /settings/routing/+page.svelte      (dev-only)
             +-- /settings/secrets/+page.svelte      (dev-only)
             +-- /settings/permissions/+page.svelte
             +-- /settings/sessions/+page.svelte
             +-- /settings/memories/+page.svelte
             +-- /settings/status/+page.svelte
             +-- /settings/developer/+page.svelte
             +-- /settings/about/+page.svelte
```

**Key architectural decisions:**

- Settings is a **modal overlay** (`fixed inset-0 z-60`), not a full page. The underlying
  app remains mounted behind the backdrop. Pressing Escape or clicking the X closes
  settings and navigates back to `/`.
- Each settings section is a standalone `+page.svelte` under `/settings/<section>/`.
  SvelteKit file-based routing drives navigation; the SettingsShell sidebar highlights
  the active tab via `$page.url.pathname`.
- Onboarding is a completely separate full-screen overlay (`fixed inset-0 z-60`) with
  its own layout. It blocks all interaction until completed.
- Three stores manage cross-cutting state: `onboarding.ts`, `devmode.ts`, `theme.ts`.
- All settings pages use dynamic imports (`await import('$lib/api/nebo')`) for API calls
  inside `onMount`, which keeps the initial bundle lean and prevents SSR issues.

---

## 2. Settings Shell & Navigation

**File:** `app/src/lib/components/SettingsShell.svelte`

```
+-----------------------------------------------------------------------+
|  Settings  v2.4.1                                           [X close] |
+----------+------------------------------------------------------------+
| Account  |                                                            |
| Profile  |          (Content area — max-w-2xl)                        |
| Billing  |                                                            |
| Usage    |          Rendered by child +page.svelte                    |
|----------|                                                            |
| Identity |                                                            |
| Personal.|                                                            |
| Rules    |                                                            |
| Advisors |                                                            |
|----------|                                                            |
| Agents   |                                                            |
| Skills   |                                                            |
| Plugins  |                                                            |
| MCP      |                                                            |
|----------|                                                            |
| Providers|  <-- dev-only (hidden unless devMode store is true)        |
| Routing  |  <-- dev-only                                              |
| Secrets  |  <-- dev-only                                              |
|----------|                                                            |
| Permiss. |                                                            |
|----------|                                                            |
| Sessions |                                                            |
| Memories |                                                            |
| Status   |                                                            |
|----------|                                                            |
| Developer|                                                            |
|----------|                                                            |
| About    |                                                            |
+----------+------------------------------------------------------------+
```

### Navigation groups (separated by `null` spacers in `allItems`)

| Group         | Sections                           | Visibility |
|---------------|------------------------------------|------------|
| Account       | Account, Profile, Billing, Usage   | Always     |
| Agent Config  | Identity, Personality, Rules, Advisors | Always |
| Extensions    | Agents, Skills, Plugins, MCP       | Always     |
| Dev Infra     | Providers, Routing, Secrets        | `devMode` store only |
| Security      | Permissions                        | Always     |
| Data          | Sessions, Memories, Status         | Always     |
| Meta          | Developer, About                   | Always     |

### Dev-only gating

The `allItems` array annotates items with `devOnly: true`. The `$derived` computed
property filters them out when `$devMode` is false. Adjacent null spacers are collapsed
to prevent double gaps.

```typescript
// Store: app/src/lib/stores/devmode.ts
const stored = localStorage.getItem('nebo-devmode');
export const devMode = writable(stored === 'true');
// Persists to localStorage on every change
```

### Active tab detection

```typescript
const activeTab = $derived(
  allTabs.find(t => $page.url.pathname.startsWith(t.path))?.id || 'account'
);
```

The active tab gets `bg-primary/10 text-primary ring-1 ring-primary/20` styling.
Non-active tabs use `text-base-content/90 hover:bg-base-200`.

### Close behavior

- Escape key handler on `<svelte:window>` calls `goto('/')`.
- X button in header calls `goto('/')`.

---

## 3. Settings Sections — Detail

### 3.1 Account

**File:** `app/src/routes/settings/account/+page.svelte`

**Purpose:** Manage the NeboAI cloud connection — the link between the local Nebo
desktop app and the NeboAI backend (marketplace, billing, Janus AI gateway).

**State:**
```typescript
let user = $state({ name: '', email: '', displayName: '' });
let connected = $state(true);
let reconnecting = $state(false);
let reconnectError = $state('');
```

**Data loaded on mount:**
1. `api.getProfile()` — populates display name
2. `api.userGetCurrentUser()` — populates email and name
3. `api.neboAIAccountStatus()` — determines connection status and email

**Actions:**
- **Reconnect** — calls `neboAIOAuthStartWithJanus(false)` from `$lib/api/index`,
  opens browser for OAuth, then polls `neboAIOAuthStatus(state)` every 2 seconds
  for up to 3 minutes. On success: updates connected state and user info.
- **Disconnect** — calls `api.neboAIAccountDisconnect()`, sets `connected = false`.
- **Delete Account** — confirmation prompt, then `api.userDeleteAccount()`.

**OAuth polling pattern** (shared with Onboarding step 2):
```
neboAIOAuthStartWithJanus() --> returns { state }
  |
  +-- setInterval(2000ms): neboAIOAuthStatus(state)
  |     status === 'complete' --> stop polling, update state
  |     status === 'error'    --> stop polling, show error
  |     status === 'expired'  --> stop polling, show expired message
  |     status === 'pending'  --> keep polling
  |
  +-- setTimeout(180_000ms): timeout, stop polling, show error
```

**UI sections:**
- Connection status card (avatar initial, display name, email, connected badge)
- View Usage link (`/settings/usage`)
- Reconnect / Disconnect buttons
- Danger zone: Delete Account

---

### 3.2 Profile

**File:** `app/src/routes/settings/profile/+page.svelte`

**Purpose:** User personal information, preferences, and theme selection.

**State:**
```typescript
let user = $state({
  displayName: '', occupation: '', location: '',
  timezone: '', interests: [] as string[],
  goals: '', commStyle: 'adaptive'
});
let snapshot = $state({ ... });  // for revert
let saved = $state(false);
```

**Data loaded on mount:** `api.getProfile()` — maps `profile.displayName`, `occupation`,
`location`, `timezone`, `interests`, `goals`, `communicationStyle`.

**Save pattern:** Debounced (800ms) auto-save. Every field change triggers `debounceSave()`,
which delays 800ms then calls `persistProfile()`. The snapshot is updated on successful
save, enabling revert.

**Fields:**
| Field | Type | Notes |
|-------|------|-------|
| Theme | Grid of 11 theme buttons | Updates `$theme` store immediately |
| Display Name | text input | Debounced save |
| Occupation | text input | Debounced save |
| Location | text input | Debounced save |
| Timezone | text input + Auto-detect button | Uses `Intl.DateTimeFormat().resolvedOptions().timeZone` |
| Interests | tag list + text input | Add on Enter, remove with X button |
| Goals | textarea | Debounced save |
| Communication Style | 3-button selector: casual/professional/adaptive | Immediate save |

**Theme store:**
```typescript
// app/src/lib/stores/theme.ts
export const theme = writable(localStorage.getItem('nebo-theme') || 'nebo');
theme.subscribe(value => {
  document.documentElement.setAttribute('data-theme', value);
  localStorage.setItem('nebo-theme', value);
});
```

Available themes: nebo, light, dark, cupcake, nord, sunset, autumn, lemonade, night, coffee, winter.

**API calls:**
- `api.getProfile()` — load
- `api.updateProfile({ displayName, occupation, location, timezone, interests, goals, communicationStyle })` — save

---

### 3.3 Billing

**File:** `app/src/routes/settings/billing/+page.svelte`

**Purpose:** Subscription management, payment methods, invoices, Stripe integration.

**State:**
```typescript
let status = $state<AccountStatusResponse | null>(null);
let subscription = $state<NeboAIBillingSubscriptionResponse | null>(null);
let paymentMethods = $state<PaymentMethodInfo[]>([]);
let invoices = $state<InvoiceInfo[]>([]);
```

**Data loaded on mount:**
1. `api.neboAIAccountStatus()` — checks if NeboAI is connected
2. If connected, parallel loads:
   - `api.neboAIBillingSubscription()` — current plan and subscription details
   - `api.neboAIBillingPaymentMethods()` — credit cards on file
   - `api.neboAIBillingInvoices()` — receipt history

**Actions:**
- **Adjust Plan** — navigates to `/upgrade`
- **Update Payment** — opens modal with Stripe Elements (Payment Element)
  - Calls `api.neboAIBillingSetupIntent()` to get `clientSecret` + `publishableKey`
  - Loads Stripe.js dynamically, creates Elements, mounts Payment Element
  - `confirmSetup()` on save, then refreshes payment methods
- **View Invoices** — expands inline invoice list
- **Open Billing Portal** — `api.neboAIBillingPortal()` opens Stripe portal
- **Cancel Subscription** — two-step confirm, calls `api.neboAIBillingCancel({ subscriptionId })`

**Conditional rendering:**
- If not connected to NeboAI: shows "Connect your NeboAI account" with link to Account settings
- Listens to `nebo:plan_changed` window event to update plan display in real-time

---

### 3.4 Usage

**File:** `app/src/routes/settings/usage/+page.svelte`

**Purpose:** Monitor Janus AI gateway usage pools, plan limits, and balance.

**State:**
```typescript
let usage = $state<TypedJanusUsage | null>(null);
// TypedJanusUsage has: session, weekly, budget pools
let accountStatus = $state<AccountStatusResponse | null>(null);
let subscription = $state<...>(null);
```

**Data loaded on mount:**
1. `api.neboAIAccountStatus()` — connection check
2. If connected: `api.neboAIJanusUsage()` and `api.neboAIBillingSubscription()`

**UI sections:**
- **Plan card** — current plan name + price, link to `/upgrade`
- **Plan Limits** — session and weekly usage bars with percentage, reset countdown
- **Balance** — free pool, gift pool, and credits (in microdollars and cents)
- **Refresh** button — calls `api.neboAIJanusUsageRefresh()`

---

### 3.5 Identity

**File:** `app/src/routes/settings/identity/+page.svelte`

**Purpose:** Configure the primary agent's name, avatar, and persona.

**State:**
```typescript
let agentName = $state('');
let emoji = $state('');
let role = $state('');
let creature = $state('');  // archetype: owl/fox/bear/dolphin
let vibe = $state('');
let snap = $state({ ... }); // revert snapshot
```

**Data loaded on mount:** `api.getProfile()` — maps `profile.name`, `emoji`, `role`,
`creature`, `vibe`.

**Save pattern:** Debounced (800ms) auto-save, same as Profile. Select/dropdown changes
trigger immediate save via `saveNow()`.

**Fields:**
| Field | Type | Notes |
|-------|------|-------|
| Avatar | Display only (emoji or initial) + Upload button (placeholder) | Shows emoji or first letter |
| Agent Name | text input | Debounced save |
| Emoji | text input (small, centered) | Debounced save |
| Role | text input | Debounced save |
| Creature Archetype | select dropdown | Owl/Fox/Bear/Dolphin, immediate save |
| Vibe | textarea | Debounced save |

**API calls:**
- `api.getProfile()` — load
- `api.updateProfile({ name, emoji, role, creature, vibe })` — save

---

### 3.6 Personality

**File:** `app/src/routes/settings/personality/+page.svelte`

**Purpose:** Tune the agent's communication style through presets and dimensional sliders.

**Data loaded on mount:**
1. `api.listPersonalityPresets()` — available presets (e.g., Professional, Casual)
2. `api.getPersonality()` — current `personalityPreset` value

**UI sections:**
- **Preset selector** — button group, highlights active preset
- **Custom System Prompt** — textarea with placeholder
- **Tuning dimensions** — 5 slider-style button groups:
  - Voice: neutral / warm / professional / enthusiastic
  - Response Length: concise / adaptive / detailed
  - Emoji Usage: none / minimal / moderate / frequent
  - Formality: casual / adaptive / formal
  - Proactivity: reactive / moderate / proactive

**Save:** Manual "Save Changes" button calls `api.updatePersonality({ personalityPreset })`.

---

### 3.7 Rules

**File:** `app/src/routes/settings/rules/+page.svelte`

**Purpose:** Define behavior constraints and guidelines for the agent.

**Data loaded on mount:** `api.getProfile()` — parses `profile.agentRules` as a
newline-separated text block. Each non-empty line becomes a toggleable rule.

**UI:** List of rules with toggle switches, "Add Section" and "Reset to Defaults" buttons.

---

### 3.8 Advisors

**File:** `app/src/routes/settings/advisors/+page.svelte`

**Purpose:** Manage advisor personas that provide different perspectives during conversations.

**Data loaded on mount:** `api.listAdvisors()` — returns advisor objects with `name`,
`role`, `description`, `priority`, `enabled`.

**UI:** Card list with color-coded role bars (using `ADVISOR_ROLE_COLORS` tokens),
priority badges, enable/disable toggles, and "Add Advisor" button.

---

### 3.9 Agents

**File:** `app/src/routes/settings/agents/+page.svelte`

**Purpose:** View and manage installed agents.

**Data loaded on mount:** `api.listAgents()` — maps agents to display objects with
`name`, `description`, `isEnabled` status, and color.

**UI:** Card list showing agent initial in colored circle, name, role description,
status badge (online/paused), and "Configure" link to `/{agentId}/settings`.

---

### 3.10 Skills

**File:** `app/src/routes/settings/skills/+page.svelte`

**Purpose:** Manage installed skills (bundled vs marketplace) and their capabilities.

**Data loaded on mount:** `api.listExtensions()` — maps skills with `name`, `bundled`,
`enabled`, `tools` (capability list).

**Actions:**
- **Toggle skill** — calls `api.toggleSkill(skill.name)`, updates local state.

**UI:** Card list with skill name, bundled/marketplace badge, tool tags, enable toggle.
"Browse more skills" link to `/marketplace/skills`.

---

### 3.11 Plugins

**File:** `app/src/routes/settings/plugins/+page.svelte`

**Purpose:** Manage installed plugins, their authentication, and dependencies.

**State:**
```typescript
let plugins = $state<Plugin[]>([]);
let authStatuses = $state<Record<string, 'connected' | 'disconnected' | 'connecting'>>({});
let selectedPlugin = $state<Plugin | null>(null);
let modalDependents = $state<Dependent[]>([]);
```

**Data loaded on mount:**
1. `api.listPlugins()` — maps all installed plugins with auth info
2. For each plugin with `hasAuth`: `api.authStatus(plugin.id)` — checks actual auth state

**Plugin interface:**
```typescript
interface Plugin {
  id: string; name: string; desc: string; author: string; version: string;
  hasAuth: boolean; authType: string; authEnvVars: string[];
  authKeysSet: boolean; hasEvents: boolean; eventCount: number;
  enabled: boolean; updateAvailable: string | null;
}
```

**Auth flow types:**
1. **OAuth** (`authType !== 'env'`) — Connect/Disconnect buttons, calls `api.authLogin(id)` / `api.authLogout(id)`
2. **API Key/Env** (`authType === 'env'`) — Set API Keys via password inputs, calls `setPluginConfig(id, payload)`
3. **None** — No auth UI shown

**Plugin detail modal:**
- Opens on plugin name click
- Shows description, status, API key inputs (if applicable), dependent skills/agents
- Calls `api.listDependents(plugin.id)` for dependency check
- **Uninstall** — only allowed if no dependents, calls `api.removePlugin(plugin.id)`
- **Upgrade** — link to marketplace if `updateAvailable`

**Window events listened:**
- `nebo:plugin_auth_complete` — marks plugin as connected
- `nebo:plugin_auth_error` — marks plugin as disconnected
- `nebo:plugin_auth_url` — opens auth URL in new tab

**Search:** Client-side filtering by name, description, and author.

---

### 3.12 MCP (Model Context Protocol)

**File:** `app/src/routes/settings/mcp/+page.svelte`

**Purpose:** Manage remote MCP server connections (OAuth, API Key, or unauthenticated).

**State:**
```typescript
let integrations = $state<MCPIntegration[]>([]);
let registry = $state<MCPRegistryEntry[]>([]);
```

**Data loaded on mount:**
1. `api.listIntegrations()` — existing server connections
2. `api.listRegistry()` — available pre-configured servers

**Summary cards:** Servers count, Connected count, Total Tools count.

**Server list:** Each integration card shows:
- Status dot (green=connected, red=error, gray=disconnected)
- Name, auth type badge, tool count, server URL
- Action buttons: reauthenticate (OAuth error), toggle enable, test, remove

**Add Server modal (3-step wizard):**
```
Step 1: Pick server            Step 2: Auth (custom only)    Step 3: Configure
+------------------------+     +------------------------+    +------------------------+
| [Search servers...]    |     | How does this server   |    | Server Name (custom)   |
| GitHub        [OAuth]  |     | authenticate?          |    | Server URL             |
| Slack         [OAuth]  |     |                        |    | API Key (if api_key)   |
| Jira         [API Key] |     | (o) OAuth 2.1          |    | OAuth notice (if oauth)|
| ...                    |     | ( ) API Key / Bearer   |    | No auth notice (none)  |
| ---- or ----           |     | ( ) None               |    |                        |
| [+ Custom Server]      |     |                        |    | [Back] [Connect/Add]   |
+------------------------+     +------------------------+    +------------------------+
```

**OAuth flow for MCP servers:**
```
createIntegration() --> returns { integration: { id } }
  |
  +-- If OAuth: startOAuthFlow(id)
  |     getOauthUrl(id) --> { authUrl }
  |     window.open(authUrl)
  |     setInterval(3000ms): connectIntegration(id)
  |       success --> stop polling, update state
  |     setTimeout(180_000ms): timeout
  |
  +-- If API Key/None: connectIntegration(id) directly
```

**Actions:**
- **Toggle enabled** — `api.updateIntegration(id, { isEnabled })` or connect/disconnect
- **Test connection** — `api.testIntegration(id)`
- **Reauthenticate** — `api.reauthenticateIntegration(id)`, starts new OAuth flow
- **Remove** — `api.deleteIntegration(id)`
- **Browse Connectors** — link to `/marketplace/connectors`

---

### 3.13 Providers (dev-only)

**File:** `app/src/routes/settings/providers/+page.svelte`

**Purpose:** Configure LLM providers and API keys. Only visible when Developer Mode is enabled.

**Data loaded on mount:** `api.listProviders()` — returns `AuthProfile[]` with `id`, `name`,
`provider`, `model`, `isActive`.

**UI:** Card list with provider icon, name, model, status badge (Connected/Not connected).
Inline edit mode for API key entry (password input, Enter to save, Esc to cancel).

**Save:** `api.updateProvider(id, { apiKey })` — updates local state to show "Connected".

---

### 3.14 Routing (dev-only)

**File:** `app/src/routes/settings/routing/+page.svelte`

**Purpose:** View task-to-model routing configuration and lane status. Dev-only.

**Data loaded on mount:**
1. `api.listModels()` — `taskRouting` map (task type -> primary + backup model)
2. `api.getLanes()` — lane status (name, concurrency, active, queued)

**UI:**
- **Task Routing table** — Task name | Primary model | Backup model
- **Lane Status** — card list with name, active/concurrency count, queued count

---

### 3.15 Secrets (dev-only)

**File:** `app/src/routes/settings/secrets/+page.svelte`

**Purpose:** Manage API keys and credentials used by skills and plugins. Dev-only.

**Data loaded on mount:**
1. `api.listPlugins()` — gets all plugins
2. For each plugin: `api.getPluginConfig(slug)` — loads config fields
3. Filters to only show `secret: true` fields

**UI:** Grouped by plugin name. Each secret field shows key name (monospace), set/not-set
status badge, and password input. Enter or blur with content triggers save via
`setPluginConfig(slug, { [key]: value })`.

---

### 3.16 Permissions

**File:** `app/src/routes/settings/permissions/+page.svelte`

**Purpose:** Control what the agent can access. Includes autonomous mode with
multi-step safety confirmation.

**State:**
```typescript
let permissions = $state<{ key: string; label: string; desc: string; enabled: boolean }[]>([]);
let autonomous = $state(false);
```

**Data loaded on mount:**
1. `api.userGetPermissions()` — returns `ToolPermission[]` with `tool` and `allowed`
2. `api.getSettings()` — reads `autonomousMode` flag

**Capability labels** (8 capabilities):
| Key | Label | Description |
|-----|-------|-------------|
| chat | Chat | Respond to messages and conversations |
| file | File Access | Read and write files on your system |
| shell | Shell Commands | Execute terminal commands |
| web | Web Access | Make HTTP requests and browse the web |
| contacts | Contacts | Access your contacts and address book |
| desktop | Desktop | Control mouse, keyboard, and windows |
| media | Media | Access camera, microphone, and screen |
| system | System | Access system information and settings |

**Autonomous Mode activation flow:**
```
Toggle ON clicked
  |
  +-- Show confirmation modal
        |
        +-- Warning: "execute all tools without permission"
        +-- Risk list (file mods, shell, network, liability)
        +-- Disclaimer text (Nebo Labs liability waiver)
        +-- Checkbox: "I understand the risks"
        +-- Text input: must type "ENABLE"
        +-- [Cancel] [Enable Autonomous Mode]
              |
              +-- Both conditions met --> autonomous = true
              +-- api.updateSettings({ autonomousMode: true })
```

**When autonomous is active:**
- All capability toggles are forced ON and disabled
- Warning banner shown
- Auto-approval section hidden

**Auto-approval section** (non-autonomous only):
- File reads, File writes, Bash commands, Web requests (toggle each)

**Approval dialog preview:** Opens `ApprovalModal` component with mock data.

---

### 3.17 Sessions

**File:** `app/src/routes/settings/sessions/+page.svelte`

**Purpose:** View conversation history and manage session cleanup.

**Data loaded on mount:** `api.listSessions()` — maps to `{ id, agent, messages, duration, time }`.

**UI:**
- Stats bar: total sessions, total messages
- Session list: agent initial, name, message count, token count, date
- Cleanup section: delete sessions older than 30/90/180 days

---

### 3.18 Memories

**File:** `app/src/routes/settings/memories/+page.svelte`

**Purpose:** View, search, and filter the agent's stored knowledge across memory layers.

**Data loaded on mount:** `api.listMemories()` — maps to `{ id, layer, value, tags, accessCount }`.

**UI:**
- Stats bar: total, tacit, daily, entity counts
- Search input + layer filter buttons (all/tacit/daily/entity)
- Memory list: layer badge (color-coded), value text, tag chips, access count

**Client-side filtering:** `$derived` computed from `layerFilter` and `searchText`.

---

### 3.19 Status

**File:** `app/src/routes/settings/status/+page.svelte`

**Purpose:** System health monitoring — service status overview.

**Data loaded on mount:**
1. `api.getStatus()` — agent runtime status + tool count
2. `api.getLanes()` — each lane becomes a service entry
3. `api.neboAIAccountStatus()` — NeboAI connection status

**UI:**
- Overall status banner (all operational / partial degradation)
- Service list: status dot, name, latency/tool count, status badge

---

### 3.20 Developer

**File:** `app/src/routes/settings/developer/+page.svelte`

**Purpose:** Toggle developer mode and manage sideloaded apps.

**Controls:**
- **Developer Mode toggle** — updates `$devMode` store. When enabled, reveals Providers,
  Routing, and Secrets in the settings sidebar.
- **Explanation text:** "By default, Nebo routes all requests through the Nebo-1 / Janus
  gateway. Enable developer mode to configure your own providers and routing rules."

**Dev-only sections (shown when devMode is true):**
- **Sideload App** — text input for path + Load button
- **Sideloaded Apps** — list with name, path, status, Relaunch/Unload buttons
- **How it works** — explanation of sideloading in sandboxed environment

---

### 3.21 About

**File:** `app/src/routes/settings/about/+page.svelte`

**Purpose:** Application info, version, platform, and resource links.

**Data loaded on mount:**
- Version: `fetch('/health')` — reads `data.version`
- Platform: `navigator.userAgent` detection (macOS/Windows/Linux)

**Resource links:**
| Label | URL |
|-------|-----|
| Documentation | https://docs.neboai.com |
| Release Notes | https://neboai.com/changelog |
| Community Discord | https://discord.gg/neboai |
| Report an Issue | https://github.com/NeboAI/nebo/issues |
| Privacy Policy | https://neboai.com/privacy |
| Terms of Service | https://neboai.com/terms |

---

## 4. Data Flow & State Management

### Load pattern (used by all settings pages)

```
+page.svelte
  |
  onMount(async () => {
    const api = await import('$lib/api/nebo');  // dynamic import
    const resp = await api.<endpoint>();
    // Map response to local $state variables
  });
```

Every settings page uses the same dynamic import pattern: `await import('$lib/api/nebo')`.
This defers loading the full API module until the page is actually mounted.

### Save patterns

```
Pattern 1: Debounced auto-save (Profile, Identity)
  field change --> debounceSave() --> setTimeout(800ms) --> persist()

Pattern 2: Immediate save (Profile comm style, Identity creature)
  field change --> saveNow() --> persist()

Pattern 3: Toggle save (Permissions, Skills)
  toggle change --> toggleCapability(key) --> api.userUpdatePermissions()

Pattern 4: Modal/form save (Providers, Plugins API keys)
  Enter key or blur --> saveKey(id) --> api.updateProvider(id, { apiKey })

Pattern 5: Manual save button (Personality)
  [Save Changes] click --> savePersonality() --> api.updatePersonality()
```

### Snapshot/revert pattern (Profile, Identity)

Both Profile and Identity pages maintain a `snapshot` copy of the last-saved state.
On successful save, the snapshot is updated. The revert button restores from snapshot.

```typescript
// Save
snapshot = { ...user, interests: [...user.interests] };

// Revert
user = { ...snapshot, interests: [...snapshot.interests] };
```

### Stores

| Store | File | Type | Persistence | Purpose |
|-------|------|------|-------------|---------|
| `onboardingComplete` | `stores/onboarding.ts` | writable<boolean> | localStorage + backend | Tracks setup completion |
| `onboardingChecked` | `stores/onboarding.ts` | writable<boolean> | Memory only | Whether backend check is done |
| `backendReady` | `stores/onboarding.ts` | writable<boolean> | Memory only | Backend health status |
| `backendChecking` | `stores/onboarding.ts` | writable<boolean> | Memory only | Polling in progress flag |
| `devMode` | `stores/devmode.ts` | writable<boolean> | localStorage | Gates dev-only settings |
| `theme` | `stores/theme.ts` | writable<string> | localStorage | DaisyUI theme name |

---

## 5. API Endpoints Summary

All endpoints are called via functions in `app/src/lib/api/nebo.ts` unless otherwise
noted. Functions in `app/src/lib/api/index.ts` are marked with `(index)`.

### Account & User

| Function | Method | Endpoint | Used By |
|----------|--------|----------|---------|
| `getProfile()` | GET | `/api/v1/profile` | Account, Profile, Identity, Rules |
| `updateProfile(req)` | PUT | `/api/v1/profile` | Profile, Identity |
| `userGetCurrentUser()` | GET | `/api/v1/user` | Account |
| `userDeleteAccount()` | DELETE | `/api/v1/user` | Account |
| `userAcceptTerms()` | POST | `/api/v1/user/terms` | Onboarding |
| `userUpdatePreferences(req)` | PUT | `/api/v1/user/preferences` | Onboarding |
| `userGetPermissions()` | GET | `/api/v1/user/permissions` | Permissions, Onboarding |
| `userUpdatePermissions(req)` | PUT | `/api/v1/user/permissions` | Permissions, Onboarding |
| `getSettings()` | GET | `/api/v1/settings` | Permissions |
| `updateSettings(req)` | PUT | `/api/v1/settings` | Permissions |

### NeboAI

| Function | Method | Endpoint | Used By |
|----------|--------|----------|---------|
| `neboAIAccountStatus()` | GET | `/api/v1/neboai/status` | Account, Billing, Usage, Status |
| `neboAIAccountDisconnect()` | POST | `/api/v1/neboai/disconnect` | Account |
| `neboAIOAuthStartWithJanus(janus)` (index) | GET | `/api/v1/neboai/oauth/start` | Account, Onboarding |
| `neboAIOAuthStatus(state)` (index) | GET | `/api/v1/neboai/oauth/status` | Account, Onboarding |

### Billing

| Function | Method | Endpoint | Used By |
|----------|--------|----------|---------|
| `neboAIBillingSubscription()` | GET | `/api/v1/neboai/billing/subscription` | Billing, Usage |
| `neboAIBillingPaymentMethods()` | GET | `/api/v1/neboai/billing/payment-methods` | Billing |
| `neboAIBillingInvoices()` | GET | `/api/v1/neboai/billing/invoices` | Billing |
| `neboAIBillingSetupIntent()` | POST | `/api/v1/neboai/billing/setup-intent` | Billing |
| `neboAIBillingPortal()` | POST | `/api/v1/neboai/billing/portal` | Billing |
| `neboAIBillingCancel(req)` | POST | `/api/v1/neboai/billing/cancel` | Billing |
| `neboAIJanusUsage()` | GET | `/api/v1/neboai/janus/usage` | Usage |
| `neboAIJanusUsageRefresh()` | POST | `/api/v1/neboai/janus/usage/refresh` | Usage |

### Agents & Extensions

| Function | Method | Endpoint | Used By |
|----------|--------|----------|---------|
| `listAgents()` | GET | `/api/v1/agents` | Agents |
| `listExtensions()` | GET | `/api/v1/extensions` | Skills |
| `toggleSkill(name)` | POST | `/api/v1/skills/{name}/toggle` | Skills |
| `listAdvisors()` | GET | `/api/v1/advisors` | Advisors |

### Plugins

| Function | Method | Endpoint | Used By |
|----------|--------|----------|---------|
| `listPlugins()` | GET | `/api/v1/plugins` | Plugins, Secrets |
| `removePlugin(slug)` | DELETE | `/api/v1/plugins/{slug}` | Plugins |
| `authLogin(slug)` | POST | `/api/v1/plugins/{slug}/auth/login` | Plugins |
| `authLogout(slug)` | POST | `/api/v1/plugins/{slug}/auth/logout` | Plugins |
| `authStatus(slug)` | GET | `/api/v1/plugins/{slug}/auth/status` | Plugins |
| `getPluginConfig(slug)` | GET | `/api/v1/plugins/{slug}/config` | Secrets |
| `setPluginConfig(slug, config)` (index) | PUT | `/api/v1/plugins/{slug}/config` | Plugins, Secrets |
| `listDependents(slug)` | GET | `/api/v1/plugins/{slug}/dependents` | Plugins |

### MCP Integrations

| Function | Method | Endpoint | Used By |
|----------|--------|----------|---------|
| `listIntegrations()` | GET | `/api/v1/integrations` | MCP |
| `listRegistry()` | GET | `/api/v1/integrations/registry` | MCP |
| `createIntegration(req)` | POST | `/api/v1/integrations` | MCP |
| `connectIntegration(id)` | POST | `/api/v1/integrations/{id}/connect` | MCP |
| `updateIntegration(id, req)` | PUT | `/api/v1/integrations/{id}` | MCP |
| `testIntegration(id)` | POST | `/api/v1/integrations/{id}/test` | MCP |
| `reauthenticateIntegration(id)` | POST | `/api/v1/integrations/{id}/reauth` | MCP |
| `deleteIntegration(id)` | DELETE | `/api/v1/integrations/{id}` | MCP |
| `getOauthUrl(id)` | GET | `/api/v1/integrations/{id}/oauth-url` | MCP |

### Providers & Routing (dev-only)

| Function | Method | Endpoint | Used By |
|----------|--------|----------|---------|
| `listProviders()` | GET | `/api/v1/providers` | Providers |
| `updateProvider(id, req)` | PUT | `/api/v1/providers/{id}` | Providers |
| `listModels()` | GET | `/api/v1/models` | Routing |
| `getLanes()` | GET | `/api/v1/lanes` | Routing, Status |

### Personality & Misc

| Function | Method | Endpoint | Used By |
|----------|--------|----------|---------|
| `getPersonality()` | GET | `/api/v1/personality` | Personality |
| `updatePersonality(req)` | PUT | `/api/v1/personality` | Personality |
| `listPersonalityPresets()` | GET | `/api/v1/personality/presets` | Personality |
| `listSessions()` | GET | `/api/v1/sessions` | Sessions |
| `listMemories()` | GET | `/api/v1/memories` | Memories |
| `getStatus()` | GET | `/api/v1/status` | Status |
| `status()` | GET | `/api/v1/status` | Onboarding store |
| `complete()` | POST | `/api/v1/setup/complete` | Onboarding store |

---

## 6. Validation & Save Patterns

### Input validation

Most settings pages use minimal client-side validation:

- **Account reconnect:** disables button during `reconnecting` state
- **Profile:** no field validation — empty strings are acceptable
- **Providers:** `editApiKey.trim()` check before save
- **Plugins API keys:** `Object.keys(payload).length` check before save
- **MCP add server:** `configureDisabled` derived checks for required fields:
  - Server URL must be non-empty
  - Server name required for custom servers
  - API key required when auth type is `api_key`
- **Permissions autonomous mode:** requires both checkbox AND typing "ENABLE"
- **Secrets:** `value.trim()` check before save

### Error handling

Every API call is wrapped in try/catch with silent failure (empty catch blocks or
`/* keep mock data */` comments). The pattern is:

```typescript
try {
  const api = await import('$lib/api/nebo');
  const resp = await api.someEndpoint();
  // Update state
} catch { /* silent */ }
```

Only Account and MCP show user-visible error messages (OAuth errors, connection errors).
Billing shows dismissible error banners for action failures.

### Optimistic updates

Several pages update local state before the API call completes:
- **Plugins:** `authStatuses[id] = 'connecting'` before API call
- **MCP:** integration list updated with temporary ID before `createIntegration` resolves
- **Permissions:** toggle state updated before `userUpdatePermissions` call
- **Skills:** `skill.enabled` toggled before `toggleSkill` API call

---

## 7. Onboarding Flow

**Files:**
- `app/src/routes/onboarding/+layout.svelte` — full-screen centered overlay
- `app/src/routes/onboarding/+page.svelte` — 5-step wizard
- `app/src/lib/stores/onboarding.ts` — state management

### Step-by-step flow

```
+===========================================================================+
|                                                                           |
|    (1)------(2)------(3)------(4)------(5)                               |
|   Welcome  Language  Connect  Perms    Done                               |
|    + T&C                                                                  |
|                                                                           |
+===========================================================================+

Step 0: WELCOME + TERMS & CONDITIONS
+---------------------------------------------------+
|              [N]  Nebo logo                        |
|         Welcome to Nebo                            |
|   Your AI agent team, running locally              |
|                                                    |
|   +-- Terms & Privacy box --+                      |
|   | Shield icon             |                      |
|   | Nebo runs AI agents...  |                      |
|   | [x] I accept the T&C   |                      |
|   +-------------------------+                      |
|                                                    |
|         [Get Started ->]                           |
|         (disabled until checkbox)                  |
+---------------------------------------------------+
    |  acceptTerms() --> api.userAcceptTerms()
    v

Step 1: LANGUAGE SELECTION
+---------------------------------------------------+
|              [Globe icon]                          |
|        Choose your language                        |
|     You can change this later in Settings.         |
|                                                    |
|   [ English ] [ Deutsch ] [ Espanol ]              |
|   [ Francais] [Italiano ] [Portugues]              |
|   ... 25 languages in 5-column grid ...            |
|                                                    |
|         [<- Back]  [Continue ->]                   |
+---------------------------------------------------+
    |  saveLocale() --> localStorage + api.userUpdatePreferences({ language })
    v

Step 2: CONNECT NEBOAI
+---------------------------------------------------+
|              [Link icon]                           |
|         Connect NeboAI                           |
|   Access marketplace, billing, Janus gateway       |
|                                                    |
|   State A: [Connect with NeboAI] button          |
|   State B: [Spinner] Waiting for authorization...  |
|   State C: [Check] Connected as user@email.com     |
|                                                    |
|         [<- Back]  [Continue ->] or [Skip for now] |
+---------------------------------------------------+
    |  connectNeboAI() --> neboAIOAuthStartWithJanus(true)
    |  Poll neboAIOAuthStatus() every 2s for up to 3min
    v

Step 3: PERMISSIONS / CAPABILITIES
+---------------------------------------------------+
|              [Shield icon]                         |
|            Permissions                             |
|   Choose what your agents can access.              |
|                                                    |
|   +-- Autonomous Mode toggle --+                   |
|   | [warning] Execute all tools|                   |
|   +----------------------------+                   |
|                                                    |
|   Chat .................. [ON]                     |
|   File Access ........... [ON]                     |
|   Shell Commands ........ [ON]                     |
|   Web Access ............ [ON]                     |
|   Contacts .............. [ON]                     |
|   Desktop ............... [ON]                     |
|   Media ................. [ON]                     |
|   System ................ [ON]                     |
|                                                    |
|   [Eye] Preview approval dialog                    |
|                                                    |
|         [<- Back]  [Continue ->]                   |
+---------------------------------------------------+
    |  savePermissions() --> api.userUpdatePermissions({ permissions })
    v

Step 4: DONE
+---------------------------------------------------+
|              [Check icon]                          |
|         You're all set!                            |
|   Nebo is ready. Your agent team is standing by.   |
|                                                    |
|         [Open Nebo ->]                             |
+---------------------------------------------------+
    |  finish() --> completeOnboarding() --> goto('/')
    v
    Main app
```

### Step indicator

A horizontal step indicator shows all 5 steps as numbered circles connected by lines.
Completed steps show a checkmark with green background. The current step has primary
color. Future steps are gray.

### Navigation

- Every step (except Welcome) has a Back button
- Step 2 (Connect) has a "Skip for now" option
- Step 3 continues regardless of permission changes
- Step 4 has only "Open Nebo"

---

## 8. Onboarding State Machine

```
                   checkOnboardingStatus()
                          |
              +-----------+-----------+
              |                       |
         backend OK              backend down
              |                       |
       api.status()           startPolling()
              |                 (2s interval)
     +--------+--------+            |
     |                  |      tryBackendHealth()
 setupComplete     not complete     |
     |                  |      +----+----+
 set complete      set false   |         |
 skip onboarding   show wizard OK     fail
     |                        |      (retry)
     v                   stop polling
  Main app                   |
                    checkOnboardingStatus()
```

### State transitions

| Event | From | To | Side Effect |
|-------|------|----|-------------|
| Page load | — | Checking | `checkOnboardingStatus()` |
| Backend reachable, setupComplete=true | Checking | Complete | `onboardingComplete.set(true)` |
| Backend reachable, setupComplete=false | Checking | Onboarding | `goto('/onboarding')` |
| Backend unreachable | Checking | Polling | `startPolling()` every 2s |
| Health check OK during poll | Polling | Checking | `checkOnboardingStatus()` |
| User completes step 4 | Onboarding | Complete | `completeOnboarding()` |

---

## 9. Onboarding Store & Backend Sync

**File:** `app/src/lib/stores/onboarding.ts`

### Dual-layer persistence

The onboarding state is stored in **two places** for resilience:

1. **localStorage** (`nebo-onboarding-complete`) — instant cache, no network needed
2. **Backend** (`api.status()` → `setupComplete`, `api.complete()`) — authoritative

On app load:
1. Read localStorage cache for immediate UI decision
2. Call `checkOnboardingStatus()` to verify with backend
3. Backend response overwrites localStorage

### `completeOnboarding()`

Called when user clicks "Open Nebo" on step 4:

```typescript
export async function completeOnboarding(): Promise<void> {
  onboardingComplete.set(true);  // Immediate local update
  await Promise.all([
    userUpdatePreferences({ onboardingCompleted: true }),
    complete()  // POST /api/v1/setup/complete
  ]);
}
```

### Root layout redirect

```typescript
// app/src/routes/+layout.svelte
$effect(() => {
  if ($onboardingChecked && !$onboardingComplete && !$page.url.pathname.startsWith('/onboarding')) {
    goto('/onboarding');
  }
});
```

This reactive effect fires whenever the stores change. It guards all routes — if
onboarding is not complete and the user is not already on `/onboarding`, they are
redirected there.

### Backend health polling

If the backend is unreachable during the initial check, the store starts polling
`/health` every 2 seconds. Once the backend responds OK, it runs
`checkOnboardingStatus()` to get the real setup status. This handles the case where
the Tauri backend takes a few seconds to start.

---

## 10. Cross-Cutting Concerns

### Shared patterns between Settings and Onboarding

The **Permissions** settings page and **Onboarding step 3** share:
- The same `CAPABILITY_LABELS` definition (duplicated, not shared)
- The same `ApprovalModal` component for preview
- The same autonomous mode activation modal (duplicated)
- The same capability toggle UI pattern

The **Account** settings page and **Onboarding step 2** share:
- The same OAuth polling pattern (`neboAIOAuthStartWithJanus` + `neboAIOAuthStatus`)
- The same 3-minute timeout
- The same status handling (complete/error/expired/pending)

### Dynamic API imports

Every settings page uses `await import('$lib/api/nebo')` inside `onMount`. This is
deliberate — it prevents the full API module from being bundled into the initial
JavaScript payload and avoids SSR issues (the API module references `window`/`fetch`
that are not available server-side).

### Error handling philosophy

Settings pages universally use silent error handling:
```typescript
catch { /* keep mock data */ }
catch { /* silent */ }
catch { /* keep empty */ }
```

The reasoning: settings should always render even if the backend is partially unavailable.
Fields show their default/empty state rather than error screens. Only critical flows
(Account OAuth, MCP connection) surface error messages to the user.

### CSS architecture

All settings pages follow the project rules:
- No `<style>` blocks in Svelte files
- No inline styles (except theme preview swatches in Profile which use dynamic colors)
- DaisyUI semantic tokens for all colors
- Tailwind utilities for layout

### Accessibility

- Modal dialogs use `role="dialog"` and `aria-modal="true"`
- Escape key closes settings shell and modals
- Toggle switches use DaisyUI's `toggle` component
- Form inputs use `<label>` wrappers
- Backdrop overlays are keyboard-dismissible

---

## 7. Install Code System (Frontend)

### Code Entry Points

1. **Chat composer** — user pastes code as a message, detected by regex in `controller.svelte.ts`
2. **New thread page** — same detection in `threads/+page.svelte` `handleSend()`
3. **Marketplace sidebar** — dedicated "Install code" input with Go button in `marketplace/+layout.svelte`

All three dispatch `nebo:code_processing` for instant modal feedback, then send via WebSocket.

### CodeInstallModal (`app/src/lib/components/chat/CodeInstallModal.svelte`)

Self-contained modal driven entirely by DOM CustomEvents (no props for event data).
Subscribes to WS-dispatched events in `onMount`, unsubscribes in `onDestroy`.

**Props:**
- `show: boolean` (bindable) — controls visibility
- `onclose?: () => void` — called when modal dismisses
- `onAgentSetup?: (agentId, agentName) => void` — called when agent needs auth setup

**Phases:** `installing` → `auth` → `done` | `error` | `payment`

**Key behaviors:**
- 30-second timeout fallback if `code_result` never arrives
- Multi-plugin OAuth queue (step N of M) with Connect/Skip per plugin
- Dependency list with live status indicators (pending → installing → installed/failed)
- Dispatches `nebo:agent_installed` on completion to refresh sidebar roster
- Only hands off to AgentSetupModal when `needsAuth === true` (not for every agent)

**Mounted in:** `[agentId]/+layout.svelte`, `app/[agentId]/+page.svelte`, `marketplace/+layout.svelte`

### WS Event Listeners (`app/src/lib/websocket/listeners.ts`)

Code install events dispatched as DOM CustomEvents:
```
code_processing, code_result, plugin_installing, plugin_installed,
dep_pending, dep_installed, dep_failed, dep_cascade_complete
```

Auth events (already existed):
```
plugin_auth_started, plugin_auth_url, plugin_auth_complete,
plugin_auth_error, agent_auth_required
```

Agent lifecycle events (trigger sidebar refresh):
```
agent_activated, agent_deactivated, agent_installed,
agent_uninstalled, agent_updated
```

### Delete Confirmation Modal (`[agentId]/+layout.svelte`)

Triggered from sidebar context menu → Delete action. Shows:
- Warning header with agent name
- "All threads, runs, and memory will be permanently removed"
- Cancel / Delete Agent buttons (Delete button shows spinner while deleting)
- On confirm: `deleteAgent(id)` → `loadAgentRoster()` → navigate away if viewing deleted agent
