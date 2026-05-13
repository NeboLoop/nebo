# Data Models — Mock to Backend Mapping

Every data shape used by the frontend, mapped to the real Rust backend tables and API responses.

## 1. User & Profile

### Frontend Mock: `USER`
```ts
{
  name: string;           // 'Alex'
  email: string;          // 'alex@acme.co'
  displayName: string;    // 'Alex Tucker'
  occupation: string;     // 'Product Lead'
  location: string;       // 'San Francisco, CA'
  timezone: string;       // 'America/Los_Angeles'
  interests: string[];    // ['AI agents', ...]
  goals: string;          // free text
  context: string;        // free text
  commStyle: string;      // 'adaptive' | 'concise' | 'detailed'
  theme: string;          // 'nebo' | 'dark' | etc.
  language: string;       // 'en'
  plan: string;           // 'free' | 'pro' | 'team' | 'enterprise'
}
```

### Backend Mapping
| Field | Backend Endpoint | Table |
|-------|-----------------|-------|
| name, email | `GET /api/v1/user/me` | `users` |
| displayName, occupation, location, timezone, interests, goals, context | `GET /api/v1/user/me/profile` | `user_profiles` |
| commStyle, theme, language | `GET /api/v1/user/me/preferences` | `user_preferences` |
| plan | `GET /api/v1/neboloop/billing/subscription` | NeboLoop API |

### Migration
```ts
// BEFORE
import { USER } from '$lib/mockData.ts';

// AFTER
const user = await fetch('/api/v1/user/me').then(r => r.json());
const profile = await fetch('/api/v1/user/me/profile').then(r => r.json());
const prefs = await fetch('/api/v1/user/me/preferences').then(r => r.json());
```

---

## 2. Agents

### Frontend Mock: `MOCK_AGENTS`
```ts
{
  id: string;         // 'researcher', 'coder', etc.
  name: string;       // 'Researcher'
  initial: string;    // 'R'
  color: string;      // 'green'
  role: string;       // 'Web + market data'
  status: string;     // 'online' | 'idle' | 'running'
  editable: boolean;  // true
}
```

### Backend Mapping
- **Endpoint:** `GET /api/v1/agents`
- **Table:** `agents`
- **Notes:** Backend agents have `agent_md` (AGENT.md) and `agent_json` (agent.json) fields. The frontend `MOCK_AGENTS` is a simplified view. Backend returns `kind`, `is_active`, `manifest`, `views`, plus workflow counts.

### Frontend Mock: `AGENT_CONFIGS`
```ts
{
  [agentId: string]: {
    persona: string;          // system prompt
    model: string;            // 'claude-opus-4-6'
    inputs: InputField[];     // configurable fields
    workflows: Record<string, Workflow>;
  }
}
```

### Backend Mapping
| Field | Backend Endpoint | Table |
|-------|-----------------|-------|
| persona | `GET /api/v1/agents/{id}` → `agent_md` | `agents` |
| model | `GET /api/v1/agents/{id}` → `agent_json.model` | `agents` |
| inputs | `GET /api/v1/agents/{id}` → `agent_json.inputs` | `agents` |
| workflows | `GET /api/v1/agents/{id}/workflows` | `agent_workflows` |

### Frontend Mock: `AGENT_SKILLS`
```ts
{ [agentId: string]: string[] }  // skill names per agent
```

### Backend Mapping
- Skills are embedded in agent manifests and workflow tool bindings
- **Endpoint:** `GET /api/v1/agents/{id}` includes skill references
- **Table:** `workflow_tool_bindings`

### Frontend Mock: `AGENT_AUTOMATIONS`
```ts
{
  [agentId: string]: {
    id: string;
    name: string;
    trigger: string;      // 'schedule' | 'event'
    schedule?: string;    // '8:00 AM daily'
    event?: string;       // 'GitHub PR opened'
    enabled: boolean;
  }[]
}
```

### Backend Mapping
- Automations are agent workflows with triggers
- **Endpoint:** `GET /api/v1/agents/{id}/workflows`
- **Table:** `agent_workflows` (has `trigger_type`, `is_enabled`, `schedule_cron`)

---

## 3. Chat & Threads

### Frontend Mock: `MOCK_CHATS`
```ts
{
  id: string;
  title: string;
  lastMessage: string;
  agent: string;       // agent ID
  agentColor: string;
  updatedAt: string;   // '25m ago'
}
```

### Backend Mapping
- **Endpoint:** `GET /api/v1/chats` or `GET /api/v1/agents/{id}/chats`
- **Table:** `chats`

### Frontend Mock: `MOCK_THREADS`
```ts
{
  [agentId: string]: {
    id: string;
    name: string;
    preview: string;
    updatedAt: string;
    messages: number;
  }[]
}
```

### Backend Mapping
- Threads are chats scoped to an agent session
- **Endpoint:** `GET /api/v1/agents/{id}/chats`
- **Table:** `chats` + `sessions` (linked via `session.active_chat_id`)

### Frontend Mock: `CHAT_MESSAGES` / `THREAD_MESSAGES`
```ts
// Simple chat messages
{ role: 'user' | 'assistant'; text: string }

// Enhanced thread messages (with thinking + tools)
{
  type: 'user' | 'assistant' | 'thinking' | 'tool';
  content: string;
  time?: string;
  // Tool-specific:
  name?: string;
  status?: string;
  duration?: string;
  request?: object;
  response?: string;
}
```

### Backend Mapping
- **Endpoint:** `GET /api/v1/chats/{id}/messages` or `GET /api/v1/agent/sessions/{id}/messages`
- **Table:** `chat_messages`
- **Streaming:** WebSocket at `/ws` provides real-time events: `chat_stream`, `thinking`, `tool_start`, `tool_result`, `chat_complete`

---

## 4. Runs & Workflow Runs

### Frontend Mock: `MOCK_RUNS`
```ts
{
  [agentId: string]: {
    id: string;
    label: string;
    time: string;
    date: string;
    status: 'success' | 'failed' | 'skipped';
    duration: string;     // '2m 14s'
    steps: number;
    triggerType: string;  // 'schedule' | 'event'
    workflowRunId: string;
  }[]
}
```

### Backend Mapping
- **Endpoint:** `GET /api/v1/agents/{id}/runs`
- **Table:** `workflow_runs`

### Frontend Mock: `MOCK_WORKFLOW_RUNS`
```ts
{
  [agentId:workflowId: string]: {
    id: string;
    workflowId: string;
    status: string;
    startedAt: string;
    completedAt: string;
    duration: string;
    steps: number;
    triggerType: string;
    error?: string;
    tokens: { input: number; output: number };
    activities: {
      id: string;
      status: string;
      duration: string;
      output?: string;
      error?: string;
    }[];
  }[]
}
```

### Backend Mapping
- **Endpoint:** `GET /api/v1/workflows/{id}/runs` and `GET /api/v1/workflows/{id}/runs/{runId}`
- **Tables:** `workflow_runs` + `workflow_activity_results`

### Frontend Mock: `MOCK_WORKFLOW_STATS`
```ts
{
  [agentId: string]: {
    totalRuns: number;
    completed: number;
    failed: number;
    running: number;
    avgDuration: string;
    lastRunAt: string;
  }
}
```

### Backend Mapping
- **Endpoint:** `GET /api/v1/agents/{id}/stats`
- **Tables:** Aggregated from `workflow_runs` + `agent_workflows`

---

## 5. Marketplace

### Frontend Mock: `MARKETPLACE_SKILLS` / `MARKETPLACE_AGENTS_LIST` / `MARKETPLACE_PLUGINS` / `MARKETPLACE_CONNECTORS`
```ts
{
  id: string;
  name: string;
  desc: string;
  category: string;
  rating: number;
  installs: number;
  featured: boolean;
  price: string;        // 'Get' or '$X.XX/mo'
  code: string;         // install code like 'SKIL-W8R4-X1P2'
  authType?: string;    // connectors only: 'none' | 'oauth' | 'api_key'
}
```

### Backend Mapping
- **Endpoint:** `GET /api/v1/store/products` (unified) or `GET /api/v1/store/featured`, `GET /api/v1/store/categories`
- **Source:** NeboLoop marketplace API (proxied through backend)

### Frontend Mock: Detail records (`MARKETPLACE_AGENT_DETAILS`, `MARKETPLACE_SKILL_DETAILS`, etc.)
```ts
{
  // ...base fields...
  author: string;
  authorVerified: boolean;
  longDesc: string;
  features: string[];
  tools?: string[];
  screenshots: { title: string; desc: string }[];
  pricing?: { name: string; price: string; features: string[] }[];
  ratingDistribution: Record<number, number>;
  developer: { name: string; website: string; support: string; launched: string };
  reviews: { user: string; rating: number; text: string; date: string }[];
  requiredSkills?: { id: string; name: string }[];
  requiredPlugins?: { id: string; name: string }[];
  worksWith?: string[];
}
```

### Backend Mapping
- **Endpoint:** `GET /api/v1/store/products/{id}`, `GET /api/v1/store/products/{id}/reviews`, `GET /api/v1/store/products/{id}/similar`
- **Install:** `POST /api/v1/store/products/{id}/install` with `{installCode}`
- **Uninstall:** `DELETE /api/v1/store/products/{id}/install`

### Frontend Mock: `MARKETPLACE_PRIVATE_ITEMS`, `PRIVATE_ORGS`, `MARKETPLACE_COLLECTIONS`
- Private/org-scoped marketplace items
- **Backend:** Likely part of the store API with org filtering

---

## 6. Settings Data

### Frontend Mock: `PROVIDERS`
```ts
{
  id: string;
  name: string;
  status: 'connected' | 'not_connected';
  models: string[];
  keySet: boolean;
}
```

### Backend Mapping
- **Endpoint:** `GET /api/v1/providers`
- **Table:** `auth_profiles`

### Frontend Mock: `ROUTING_TASKS`
```ts
{ id: string; label: string; primary: string; backup: string }
```

### Backend Mapping
- **Endpoint:** `GET /api/v1/models` + `PUT /api/v1/models/task-routing`
- **Table:** `provider_models`

### Frontend Mock: `PERMISSIONS`
```ts
{ id: string; label: string; desc: string; enabled: boolean; locked: boolean }
```

### Backend Mapping
- **Endpoint:** `GET /api/v1/user/me/permissions` and `PUT /api/v1/user/me/permissions`
- **Table:** `user_profiles` (permissions field)

### Frontend Mock: `ADVISORS`
```ts
{ id: string; name: string; role: string; enabled: boolean; priority: string; desc: string }
```

### Backend Mapping
- **Endpoint:** `GET /api/v1/agent/advisors`, `POST/PUT/DELETE /api/v1/agent/advisors/{name}`
- **Table:** `advisors`

### Frontend Mock: `RULES`
```ts
{ section: string; rules: { id: string; text: string; enabled: boolean }[] }[]
```

### Backend Mapping
- Part of agent profile/config
- **Endpoint:** `GET /api/v1/agent/profile` or `GET /api/v1/agent/settings`
- **Table:** `agent_profile` or `settings`

### Frontend Mock: `MEMORIES`
```ts
{
  id: string;
  layer: 'tacit' | 'daily' | 'entity';
  namespace: string;
  value: string;
  tags: string[];
  accessCount: number;
  created: string;
  updated: string;
}
```

### Backend Mapping
- **Endpoint:** `GET /api/v1/memories`, `GET /api/v1/memories/search?q=`, `GET /api/v1/memories/stats`
- **Table:** `memories` + `memory_chunks` + `memory_embeddings`

### Frontend Mock: `PERSONALITY_PRESETS`
```ts
{ id: string; name: string; desc: string }
```

### Backend Mapping
- **Endpoint:** `GET /api/v1/agent/personality-presets`
- Hardcoded on backend

---

## 7. System Data

### Frontend Mock: `EVENTS`
```ts
{ id: string; type: 'agent'|'workflow'|'tool'|'error'; source: string; payload: string; time: string }
```

### Backend Mapping
- **Endpoint:** Real-time via WebSocket events
- **Table:** `error_logs` for errors; other events are ephemeral via `ClientHub` broadcast

### Frontend Mock: `SKILLS_INSTALLED`
```ts
{ id: string; name: string; enabled: boolean; tools: string[]; tags: string[]; source: string; bundled: boolean }
```

### Backend Mapping
- **Endpoint:** `GET /api/v1/extensions` (all skills) or `GET /api/v1/skills/{name}`
- **Source:** Filesystem-based (`~/.nebo/skills/`)

### Frontend Mock: `SESSIONS`
```ts
{ id: string; agent: string; messages: number; duration: string; time: string }
```

### Backend Mapping
- **Endpoint:** `GET /api/v1/agent/sessions`
- **Table:** `sessions`

### Frontend Mock: `AUTOMATIONS`
```ts
{ id: string; name: string; trigger: string; schedule?: string; event?: string; agent: string; enabled: boolean }
```

### Backend Mapping
- These map to `agent_workflows` — see Workflow section
- **Endpoint:** `GET /api/v1/agents/{id}/workflows`

---

## 8. Billing & Plans

### Frontend Mock: `PLANS`
```ts
{
  id: string;
  name: string;
  price: number | null;
  priceYearly: number | null;
  features: string[];
  current: boolean;
  popular?: boolean;
  description: string;
}
```

### Backend Mapping
- **Endpoint:** `GET /api/v1/neboloop/billing/prices`
- **Source:** NeboLoop billing API (Stripe-backed)

### Frontend Mock: `BILLING`
```ts
{
  plan: string;
  interval: 'monthly' | 'yearly';
  autoRenews: boolean;
  paymentMethod: { brand: string; lastFour: string; expiresAt: string };
  invoices: { id: string; date: string; amount: number; currency: string; status: string; description: string }[];
}
```

### Backend Mapping
- **Subscription:** `GET /api/v1/neboloop/billing/subscription`
- **Invoices:** `GET /api/v1/neboloop/billing/invoices`
- **Payment methods:** `GET /api/v1/neboloop/billing/payment-methods`
- **Portal:** `POST /api/v1/neboloop/billing/portal` → Stripe portal URL
- **Cancel:** `POST /api/v1/neboloop/billing/cancel`

---

## 9. MCP Integrations

### Frontend Mock: `MCP_INTEGRATIONS`
```ts
{
  id: string;
  name: string;
  serverUrl: string;
  authType: 'oauth' | 'api_key' | 'none';
  isEnabled: boolean;
  connectionStatus: 'connected' | 'disconnected';
  toolCount: number;
  lastConnectedAt: string;
  lastError: string | null;
}
```

### Backend Mapping
- **Endpoint:** `GET /api/v1/integrations`
- **Table:** `mcp_integrations`
- **Auth flow:** `GET /api/v1/integrations/{id}/oauth-url`, `POST /api/v1/integrations/{id}/connect`

### Frontend Mock: `MCP_REGISTRY`
```ts
{ id: string; name: string; description: string; authType: string; isBuiltin: boolean }
```

### Backend Mapping
- **Endpoint:** `GET /api/v1/integrations/registry` or `GET /api/v1/mcp/servers`

---

## 10. Notifications

### Frontend Mock: (in `stores/notifications.ts`)
```ts
{
  id: string;
  type: 'agent' | 'system' | 'warning' | 'error';
  title: string;
  message: string;
  time: string;
  read: boolean;
  link?: string;
}
```

### Backend Mapping
- **Endpoint:** `GET /api/v1/notifications`, `PUT /api/v1/notifications/{id}/read`, `PUT /api/v1/notifications/read-all`, `DELETE /api/v1/notifications/{id}`, `GET /api/v1/notifications/unread-count`
- **Table:** `notifications`

---

## 11. Workflow Builder Data

### Frontend Mock: `NODE_CATALOG_ITEMS`
```ts
{
  category: string;
  items: {
    type: string;        // 'trigger-schedule', 'activity-custom', 'flow-condition', etc.
    label: string;
    desc: string;
    icon: string;
    serverId?: string;   // for connector nodes
    agentId?: string;    // for agent nodes
  }[];
}[]
```

### Backend Mapping
- Catalog is partially static (trigger/activity/flow types) and partially dynamic (connected MCP servers, available agents)
- **Dynamic parts:** `GET /api/v1/integrations` (connected MCP servers), `GET /api/v1/agents` (available agents)

### Frontend Mock: `ARCHITECT_INTRO_MESSAGE`
- Static intro text for the AI Architect chat — no backend needed

---

## 12. Commander (Org Chart)

Not in current mock data but the backend has:
- **Endpoint:** `GET /api/v1/commander/graph`
- **Tables:** `commander_teams`, `commander_edges`
- Used for multi-agent orchestration canvas

---

## 13. ID Mapping

The frontend uses two ID schemes that must be reconciled:

| Short ID (calendar/tokens) | Full ID (mockData/AGENT_CONFIGS) | Backend ID |
|---|---|---|
| `res` | `researcher` | TBD — likely full name |
| `cod` | `coder` | TBD |
| `mkt` | `marketer` | TBD |
| `soc` | `social` | TBD |
| `tst` | `tester` | TBD |
| `ops` | `ops` | TBD |
| `ast` | `assistant` | TBD |
| `wrt` | `writer` | TBD |

The mapping lives in `src/lib/data.ts` as `AGENT_ID_MAP` / `AGENT_ID_REVERSE`. When connecting to the backend, agent IDs from the API response will determine which scheme to use. The mapping may need updating based on actual backend agent IDs.
