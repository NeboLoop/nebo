# Backend Integration Overview

This document maps 100% of the Nebo V2 frontend to enable a rapid transition from mock data to a live production backend.

## Architecture

```
Frontend (SvelteKit)
  ├── src/lib/mockData.ts        ← ~45 exports, ALL mock data lives here
  ├── src/lib/data.ts            ← Agent ID mappings, calendar day constants
  ├── src/lib/tokens.ts          ← Design tokens (no backend data)
  ├── src/lib/utils.ts           ← Pure utility functions (no backend data)
  ├── src/lib/stores/            ← 10 reactive stores (some wrap mockData)
  │   ├── schedule.ts            ← Parses AGENT_CONFIGS + MOCK_WORKFLOW_RUNS → calendar items
  │   ├── marketplace.ts         ← Installed items store (wraps MARKETPLACE_AGENT_DETAILS)
  │   ├── collections.ts         ← Private collections store (wraps MARKETPLACE_COLLECTIONS)
  │   ├── notifications.ts       ← Notification list (hardcoded initial data)
  │   ├── permissions.ts         ← Auto-approved actions (localStorage)
  │   ├── devmode.ts             ← Dev mode toggle (localStorage)
  │   ├── onboarding.ts          ← Onboarding complete flag (localStorage)
  │   ├── theme.ts               ← Theme selection (localStorage)
  │   ├── sidebar.ts             ← Sidebar collapse state (in-memory)
  │   └── toast.ts               ← Ephemeral toast notifications (in-memory, no backend)
  └── src/routes/                ← ~55 route files consuming the above
```

## Data Flow Pattern

Every page currently:
1. `import { SOME_EXPORT } from '$lib/mockData.ts'` at the top
2. Uses the data directly in the template or passes to components
3. Mutations are in-memory only (lost on refresh)

### Migration Strategy

Replace each mock import with an API fetch:

```
BEFORE:  import { MOCK_AGENTS } from '$lib/mockData.ts';
AFTER:   const agents = await fetch('/api/agents').then(r => r.json());
```

For stores that wrap mock data (schedule, marketplace, collections, notifications), the store initialization should call the API instead of importing constants.

## Mock Data Inventory — `src/lib/mockData.ts`

### User & Identity
| Export | Type | Description |
|--------|------|-------------|
| `USER` | Object | Current user profile (name, email, plan, theme, interests, goals) |

### Agents
| Export | Type | Description |
|--------|------|-------------|
| `MOCK_AGENTS` | Array[8] | Agent roster (id, name, initial, color, role, status, editable) |
| `AGENT_COLORS_MAP` | Record | Agent color → CSS class mapping (bg, ink, border) |
| `AGENT_CONFIGS` | Record[8] | Full agent configs (persona, model, inputs, workflows) |
| `AGENT_SKILLS` | Record[8] | Per-agent skill name arrays |
| `AGENT_AUTOMATIONS` | Record[8] | Per-agent automation list (triggers, schedules) |

### Chat
| Export | Type | Description |
|--------|------|-------------|
| `MOCK_CHATS` | Array[8] | Chat list (id, title, lastMessage, agent, updatedAt) |
| `CHAT_GROUPS` | Array[4] | Chat grouping (Starred, Today, Yesterday, Previous 7 days) |
| `STARTER_PROMPTS` | Array[4] | Starter prompt suggestions |
| `CHAT_MESSAGES` | Array[4] | Chat message history (role, text) |

### Threads
| Export | Type | Description |
|--------|------|-------------|
| `MOCK_THREADS` | Record[8] | Per-agent thread lists (id, name, preview, updatedAt, messages) |
| `THREAD_MESSAGES` | Array[12] | Enhanced thread messages with thinking blocks and tool invocations |

### Runs & Workflows
| Export | Type | Description |
|--------|------|-------------|
| `MOCK_RUNS` | Record[8] | Per-agent run lists (id, label, time, status, duration, steps) |
| `MOCK_WORKFLOW_RUNS` | Record[12] | Per-workflow run history with full activity breakdown |
| `MOCK_WORKFLOW_STATS` | Record[8] | Per-agent aggregate stats (totalRuns, completed, failed, avgDuration) |

### Marketplace — Public
| Export | Type | Description |
|--------|------|-------------|
| `MARKETPLACE_CATEGORIES` | Array[14] | Category list (slug, name, emoji, count) |
| `MARKETPLACE_SKILLS` | Array[12] | Skill listings (id, name, desc, rating, installs, price) |
| `MARKETPLACE_AGENTS_LIST` | Array[6] | Agent listings |
| `MARKETPLACE_PLUGINS` | Array[8] | Plugin listings |
| `MARKETPLACE_CONNECTORS` | Array[8] | MCP connector listings |
| `MARKETPLACE_SKILL_DETAILS` | Record[2] | Expanded skill detail (reviews, features, tools, screenshots) |
| `MARKETPLACE_AGENT_DETAILS` | Record[2] | Expanded agent detail (reviews, pricing, dependencies) |
| `MARKETPLACE_PLUGIN_DETAILS` | Record[2] | Expanded plugin detail (reviews, features, platforms) |
| `MARKETPLACE_CONNECTOR_DETAILS` | Record[8] | Expanded connector detail (tools, features, reviews) |

### Marketplace — Private (Org-scoped)
| Export | Type | Description |
|--------|------|-------------|
| `PRIVATE_ORGS` | Array[2] | Organizations the user belongs to |
| `MARKETPLACE_PRIVATE_ITEMS` | Array[6] | Org-scoped marketplace items |
| `MARKETPLACE_COLLECTIONS` | Array[3] | Curated bundles of items within orgs |

### Settings
| Export | Type | Description |
|--------|------|-------------|
| `PROVIDERS` | Array[5] | LLM provider configs (id, name, status, models, keySet) |
| `ROUTING_TASKS` | Array[5] | Task-to-model routing rules |
| `PERMISSIONS` | Array[8] | Capability permission list (id, label, enabled, locked) |
| `ADVISORS` | Array[4] | Advisor personas (role, enabled, priority) |
| `ADVISOR_ROLE_COLORS` | Record[8] | Role → DaisyUI color class mapping |
| `RULES` | Array[4 sections] | Behavior rules grouped by section |
| `PERSONALITY_PRESETS` | Array[4] | Personality preset options |
| `MEMORIES` | Array[8] | Memory entries (layer, namespace, value, tags) |

### System
| Export | Type | Description |
|--------|------|-------------|
| `EVENTS` | Array[10] | System event feed (type, source, payload, time) |
| `EVENT_COLORS` | Record[4] | Event type → DaisyUI color class mapping |
| `SKILLS_INSTALLED` | Array[6] | Installed skills with tools and tags |
| `SESSIONS` | Array[5] | Session history (agent, messages, duration) |
| `AUTOMATIONS` | Array[5] | Automation list (trigger, schedule, agent) |

### Billing & Plans
| Export | Type | Description |
|--------|------|-------------|
| `PLANS` | Array[4] | Available plans (free, pro, team, enterprise) |
| `BILLING` | Object | Current billing state (plan, payment method, invoices, usage) |

### MCP
| Export | Type | Description |
|--------|------|-------------|
| `MCP_INTEGRATIONS` | Array[5] | User's configured MCP servers |
| `MCP_REGISTRY` | Array[10] | Available MCP server registry |

### Workflow Builder
| Export | Type | Description |
|--------|------|-------------|
| `NODE_CATALOG_ITEMS` | Array[6 groups] | Node catalog for visual workflow builder |
| `ARCHITECT_INTRO_MESSAGE` | Object | AI architect chat intro message |

## Supplementary Data — `src/lib/data.ts`

| Export | Type | Description |
|--------|------|-------------|
| `AGENT_ID_MAP` | Record | Full agent ID → short ID mapping (researcher→res) |
| `AGENT_ID_REVERSE` | Record | Short → full ID mapping (res→researcher) |
| `AGENTS` | Array[6] | Calendar-specific agent list (short IDs) |
| `CAL_DAYS` | Array[7] | Calendar day labels |

## Design Tokens — `src/lib/tokens.ts`

| Export | Type | Description |
|--------|------|-------------|
| `N` | Object | Color palette (bg, surface, ink levels, status colors) |
| `AGENT_COLORS` | Record[6] | Per-agent CSS class sets for calendar (fillClass, dotClass, etc.) |

These are rendering constants, not backend data.

## Related Documentation

- [02-data-models.md](./02-data-models.md) — TypeScript interfaces for every data shape
- [03-workflows.md](./03-workflows.md) — Complete workflow system documentation
- [04-routes.md](./04-routes.md) — Route-by-route data consumption and mutations
- [05-components.md](./05-components.md) — Component props, data needs, and callbacks
- [06-stores.md](./06-stores.md) — Reactive store architecture and backend endpoints
- [07-api-endpoints.md](./07-api-endpoints.md) — Complete API endpoint mapping
