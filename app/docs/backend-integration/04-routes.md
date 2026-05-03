# Routes — Data Consumption & Mutations

Every route in the app, what data it consumes, what user actions it supports, and the backend API calls needed.

## Route Map

```
/                               → Redirect to /assistant/threads
/[agentId]/*                    → Agent 3-column layout
  /threads                      → New thread empty state
  /threads/[threadId]           → Thread detail + chat
  /runs                         → Runs overview + stats
  /runs/[runId]                 → Run detail + activity timeline
  /settings                     → Redirect to /settings/general
  /settings/[section]           → Settings section
/chat                           → Standalone chat
/activity                       → Session history feed
/automate                       → Automations list
/schedule                       → Calendar (day/week/month)
/workspaces                     → Agent workspace views
/workspace/[agentId]            → Single agent workspace
/events                         → System events feed
/skills                         → Installed skills
/marketplace/*                  → Marketplace (own layout)
  /                             → Featured
  /agents, /agents/[id]         → Agent listings + detail
  /skills, /skills/[id]         → Skill listings + detail
  /plugins, /plugins/[id]       → Plugin listings + detail
  /connectors, /connectors/[id] → Connector listings + detail
  /categories                   → Category grid
  /collections, /collections/[id] → Collections + org detail
  /installed                    → Installed items
/settings/*                     → Settings modal overlay
  /account                      → NeboLoop connection
  /profile                      → Profile + theme picker
  /billing                      → Plan, payment, invoices
  /usage                        → Usage stats + balance
  /identity                     → Agent avatar/name
  /personality                  → Presets + tuning
  /rules                        → Behavior rules
  /advisors                     → Advisor personas
  /agents                       → Agent list
  /skills                       → Installed skills
  /plugins                      → Plugin auth
  /mcp                          → MCP servers
  /providers                    → LLM providers (dev)
  /routing                      → Task routing (dev)
  /secrets                      → API keys (dev)
  /permissions                  → Capabilities
  /sessions                     → Session history
  /memories                     → Memory search
  /status                       → System health
  /developer                    → Dev mode toggle
  /about                        → App info
/onboarding                     → 5-step setup wizard
/upgrade                        → Plan selection
```

---

## Agent Routes

### `/[agentId]/+layout.svelte` — Agent Container

**The hub for all agent data.** ~700 lines. Sets context for all child routes.

| Mock Import | Backend Endpoint |
|-------------|-----------------|
| `MOCK_AGENTS` | `GET /api/v1/agents` |
| `AGENT_COLORS_MAP` | Static (keep client-side) |
| `MOCK_THREADS[agentId]` | `GET /api/v1/agents/{id}/chats` |
| `MOCK_RUNS[agentId]` | `GET /api/v1/agents/{id}/runs` |
| `AGENT_SKILLS[agentId]` | `GET /api/v1/agents/{id}` (manifest) |
| `AGENT_CONFIGS[agentId]` | `GET /api/v1/agents/{id}` + `GET /api/v1/agents/{id}/workflows` |
| `MOCK_WORKFLOW_RUNS` | `GET /api/v1/workflows/{id}/runs` |
| `MOCK_WORKFLOW_STATS[agentId]` | `GET /api/v1/agents/{id}/stats` |

**User Actions:**
- Toggle agent status (pause/online)
- Open workflow canvas editor (full-screen modal)
- Create/delete workflows
- Edit workflow activities (add, remove, duplicate, reorder)
- Edit activity types and parameters
- Manage connections between activities
- Save workflow changes

**Mutations → Backend:**
| Action | Endpoint |
|--------|----------|
| Toggle agent status | `POST /api/v1/agents/{id}/toggle` |
| Save workflow | `PUT /api/v1/agents/{id}/workflows/{binding_name}` |
| Create workflow | `POST /api/v1/agents/{id}/workflows` |
| Delete workflow | `DELETE /api/v1/agents/{id}/workflows/{binding_name}` |
| Toggle workflow active | `POST /api/v1/agents/{id}/workflows/{binding_name}/toggle` |

### `/[agentId]/threads/+page.svelte` — New Thread

**Reads:** Agent context (from layout)
**Writes:** Creates new thread + sends first message

| Action | Endpoint |
|--------|----------|
| Send message | `POST /api/v1/agents/{id}/chats` then `POST /api/v1/agents/{id}/chat` |

### `/[agentId]/threads/[threadId]/+page.svelte` — Thread Detail

**Reads:** `THREAD_MESSAGES` → `GET /api/v1/chats/{chatId}/messages`
**Writes:** Send message, edit message

| Action | Endpoint |
|--------|----------|
| Send message | `POST /api/v1/chats/message` or WebSocket `/ws` |
| Edit message | `POST /api/v1/chats/messages/{id}/edit` |
| Get tool output | `GET /api/v1/chats/{chatId}/tool-output/{toolCallId}` |

### `/[agentId]/runs/+page.svelte` — Runs Overview

**Reads:** `ctx.runs`, `ctx.workflowStats` from layout context
**Backend:** `GET /api/v1/agents/{id}/runs` + `GET /api/v1/agents/{id}/stats`
**Writes:** None (read-only)

### `/[agentId]/runs/[runId]/+page.svelte` — Run Detail

**Reads:** Run detail with activity timeline
**Backend:** `GET /api/v1/workflows/{id}/runs/{runId}`

| Action | Endpoint |
|--------|----------|
| Retry failed run | `POST /api/v1/workflows/{id}/run` |
| Cancel running | `POST /api/v1/workflows/{id}/runs/{runId}/cancel` |

### `/[agentId]/settings/[section]/+page.svelte` — Agent Settings

Settings sections with their data needs:

| Section | Mock Data | Backend Endpoints |
|---------|-----------|-------------------|
| `general` | Agent status, model | `GET /api/v1/agents/{id}`, `POST /api/v1/agents/{id}/toggle` |
| `identity` | Agent name, role, color | `PUT /api/v1/agents/{id}` |
| `persona` | System prompt, temperature | `PUT /api/v1/agents/{id}` (agent_md) |
| `configure` | Dynamic input fields | `PUT /api/v1/agents/{id}/inputs` |
| `workflows` | Workflow list + stats | `GET /api/v1/agents/{id}/workflows`, `GET /api/v1/agents/{id}/stats` |
| `skills` | Skill list | `GET /api/v1/agents/{id}` (manifest skills) |
| `memory` | Agent memory | `GET /api/v1/memories?namespace=agent:{id}` |
| `permissions` | Capability toggles | `GET/PUT /api/v1/user/me/permissions` |

---

## Top-Level Routes

### `/chat/+page.svelte`
**Reads:** `CHAT_MESSAGES` → `GET /api/v1/chats/companion` + `GET /api/v1/chats/{id}/messages`
**Writes:** `POST /api/v1/chats/message` or WebSocket

### `/activity/+page.svelte`
**Reads:** `SESSIONS` → `GET /api/v1/agent/sessions`

### `/automate/+page.svelte`
**Reads:** `AUTOMATIONS` → Derived from `GET /api/v1/agents` + their workflows
**Writes:** Toggle → `POST /api/v1/agents/{id}/workflows/{name}/toggle`

### `/schedule/+page.svelte`
**Reads:** Schedule store (derived from `AGENT_CONFIGS` workflows + `MOCK_WORKFLOW_RUNS`)
**Backend:** `GET /api/v1/agents` + `GET /api/v1/agents/{id}/workflows` for each agent
**Writes:** User-created schedule items → `POST /api/v1/tasks` (cron jobs)

### `/events/+page.svelte`
**Reads:** `EVENTS` → Real-time via WebSocket events from `ClientHub`
**Backend:** WebSocket `/ws` stream events

### `/workspaces/+page.svelte`
**Reads:** `MOCK_AGENTS`, agent views (A2UI surfaces)
**Backend:** `GET /api/v1/agents` + `GET /api/v1/agents/{id}/surfaces`

### `/skills/+page.svelte`
**Reads:** `SKILLS_INSTALLED` → `GET /api/v1/extensions`

---

## Marketplace Routes

### `/marketplace/+layout.svelte` — Marketplace Container

Loads all marketplace data for child routes.

| Mock Import | Backend Endpoint |
|-------------|-----------------|
| `MARKETPLACE_SKILLS` | `GET /api/v1/store/products?type=skill` |
| `MARKETPLACE_AGENTS_LIST` | `GET /api/v1/store/products?type=agent` |
| `MARKETPLACE_PLUGINS` | `GET /api/v1/store/products?type=plugin` |
| `MARKETPLACE_CONNECTORS` | `GET /api/v1/store/products?type=connector` |
| `MARKETPLACE_CATEGORIES` | `GET /api/v1/store/categories` |
| `MARKETPLACE_PRIVATE_ITEMS` | `GET /api/v1/store/products?scope=org` |
| `PRIVATE_ORGS` | Part of store API |

**User Actions:**
- Search marketplace (debounced)
- Redeem install code → `POST /api/v1/codes`

### `/marketplace/+page.svelte` — Featured
**Reads:** `GET /api/v1/store/featured`

### `/marketplace/[type]/+page.svelte` — Type Listings
**Reads:** `GET /api/v1/store/products?type={type}`

### `/marketplace/[type]/[id]/+page.svelte` — Item Detail
**Reads:** `GET /api/v1/store/products/{id}` + `GET /api/v1/store/products/{id}/reviews`

| Action | Endpoint |
|--------|----------|
| Install | `POST /api/v1/store/products/{id}/install` |
| Uninstall | `DELETE /api/v1/store/products/{id}/install` |
| Submit review | `POST /api/v1/store/products/{id}/reviews` |

### `/marketplace/installed/+page.svelte`
**Reads:** Installed items store → Backend tracks what's installed
**Writes:** Uninstall → `DELETE /api/v1/store/products/{id}/install`

### `/marketplace/collections/+page.svelte`
**Reads:** Org list + collections

### `/marketplace/collections/[id]/+page.svelte`
**Reads:** Collection detail with items

---

## Settings Routes

All render inside `SettingsShell` modal overlay.

| Route | Mock Data | Backend Read | Backend Write |
|-------|-----------|-------------|---------------|
| `/settings/account` | `USER` | `GET /api/v1/neboloop/account` | `DELETE /api/v1/neboloop/account` |
| `/settings/profile` | `USER` | `GET /api/v1/user/me/profile` + `GET /api/v1/user/me/preferences` | `PUT /api/v1/user/me/profile`, `PUT /api/v1/user/me/preferences` |
| `/settings/billing` | `BILLING` | `GET /api/v1/neboloop/billing/subscription` + invoices + payment methods | `POST /api/v1/neboloop/billing/cancel`, `POST /api/v1/neboloop/billing/portal` |
| `/settings/usage` | Usage stats | `GET /api/v1/neboloop/janus/usage` | `POST /api/v1/neboloop/janus/usage/refresh` |
| `/settings/identity` | None | `GET /api/v1/agent/profile` | `PUT /api/v1/agent/profile` |
| `/settings/personality` | `PERSONALITY_PRESETS` | `GET /api/v1/agent/profile` + `GET /api/v1/agent/personality-presets` | `PUT /api/v1/agent/profile` |
| `/settings/rules` | `RULES` | `GET /api/v1/agent/settings` | `PUT /api/v1/agent/settings` |
| `/settings/advisors` | `ADVISORS` | `GET /api/v1/agent/advisors` | `POST/PUT/DELETE /api/v1/agent/advisors/{name}` |
| `/settings/agents` | `MOCK_AGENTS` | `GET /api/v1/agents` | — |
| `/settings/skills` | `SKILLS_INSTALLED` | `GET /api/v1/extensions` | `POST /api/v1/skills/{name}/toggle` |
| `/settings/plugins` | Plugin status | `GET /api/v1/plugins` | `POST /api/v1/plugins/{slug}/auth/login` |
| `/settings/mcp` | `MCP_INTEGRATIONS`, `MCP_REGISTRY` | `GET /api/v1/integrations` + `GET /api/v1/integrations/registry` | `POST/PUT/DELETE /api/v1/integrations/{id}`, `POST /api/v1/integrations/{id}/connect` |
| `/settings/providers` | `PROVIDERS` | `GET /api/v1/providers` + `GET /api/v1/models` | `POST/PUT/DELETE /api/v1/providers/{id}`, `POST /api/v1/providers/{id}/test` |
| `/settings/routing` | `ROUTING_TASKS` | `GET /api/v1/models` | `PUT /api/v1/models/task-routing` |
| `/settings/secrets` | Skill secrets | `GET /api/v1/skills/{name}/secrets` | `PUT /api/v1/skills/{name}/secrets` |
| `/settings/permissions` | `PERMISSIONS` | `GET /api/v1/user/me/permissions` | `PUT /api/v1/user/me/permissions` |
| `/settings/sessions` | `SESSIONS` | `GET /api/v1/agent/sessions` | `DELETE /api/v1/agent/sessions/{id}` |
| `/settings/memories` | `MEMORIES` | `GET /api/v1/memories` + `GET /api/v1/memories/search` + `GET /api/v1/memories/stats` | `PUT/DELETE /api/v1/memories/{id}` |
| `/settings/status` | System health | `GET /api/v1/agent/status` + `GET /api/v1/agent/system-info` | — |
| `/settings/developer` | Dev mode | Local (`localStorage`) | — |
| `/settings/about` | App info | Static | — |

---

## Other Routes

### `/onboarding/+page.svelte`
5-step wizard. Uses `PERMISSIONS` for step 3.

| Step | Backend Calls |
|------|--------------|
| 0 — Welcome + T&C | `POST /api/v1/user/me/accept-terms` |
| 1 — Language | `PUT /api/v1/user/me/preferences` |
| 2 — Connect NeboLoop | `GET /api/v1/neboloop/oauth/start` |
| 3 — Permissions | `PUT /api/v1/user/me/permissions` |
| 4 — Done | `POST /api/v1/setup/complete` |

### `/upgrade/+page.svelte`
**Reads:** `PLANS` → `GET /api/v1/neboloop/billing/prices`
**Writes:** Subscribe → `POST /api/v1/neboloop/billing/checkout` or `POST /api/v1/neboloop/billing/subscribe`
