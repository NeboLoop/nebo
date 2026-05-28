# API Endpoint Mapping — Frontend Mock → Backend Routes

Complete mapping from every mock data import in the frontend to the real Rust backend API endpoint.

## Backend Base URL
- **Dev:** `http://localhost:27895/api/v1`
- **Prod/embedded:** `http://localhost:27895/api/v1`
- **Frontend proxy:** SvelteKit proxies `/api` and `/ws` to `:27895` in dev mode

## WebSocket
- **Client:** `/ws` (chat streaming, real-time events, approvals)
- **Agent:** `/agent/ws` (agent-to-agent communication)
- **Extension:** `/ws/extension` (Chrome bridge)

---

## Master Mapping Table

### User & Profile

| Frontend Mock | Backend Endpoint | Method | Auth |
|---------------|-----------------|--------|------|
| `USER.name`, `USER.email` | `/user/me` | GET | JWT |
| `USER.displayName`, `.occupation`, `.location`, `.timezone`, `.interests`, `.goals`, `.context` | `/user/me/profile` | GET/PUT | Public |
| `USER.commStyle`, `.theme`, `.language` | `/user/me/preferences` | GET/PUT | Public |
| `USER.plan` | `/neboai/billing/subscription` | GET | JWT |
| Accept T&C | `/user/me/accept-terms` | POST | Public |
| Change password | `/user/me/change-password` | POST | JWT |
| Delete account | `/user/me` | DELETE | JWT |

### Agents

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| `MOCK_AGENTS` | `/agents` | GET |
| `MOCK_AGENTS[i].status` | `/agents/{id}` | GET |
| Toggle agent | `/agents/{id}/toggle` | POST |
| `AGENT_CONFIGS[id].persona` | `/agents/{id}` → `agent_md` | GET |
| `AGENT_CONFIGS[id].model` | `/agents/{id}` → `agent_json.model` | GET |
| `AGENT_CONFIGS[id].inputs` | `/agents/{id}` → `agent_json.inputs` | GET |
| Update agent inputs | `/agents/{id}/inputs` | PUT |
| `AGENT_CONFIGS[id].workflows` | `/agents/{id}/workflows` | GET |
| Create workflow | `/agents/{id}/workflows` | POST |
| Update workflow | `/agents/{id}/workflows/{binding_name}` | PUT |
| Delete workflow | `/agents/{id}/workflows/{binding_name}` | DELETE |
| Toggle workflow | `/agents/{id}/workflows/{binding_name}/toggle` | POST |
| `AGENT_SKILLS[id]` | `/agents/{id}` (manifest includes skills) | GET |
| `AGENT_AUTOMATIONS[id]` | `/agents/{id}/workflows` (trigger-based) | GET |
| Agent stats | `/agents/{id}/stats` | GET |
| Agent runs | `/agents/{id}/runs` | GET |
| Agent surfaces (A2UI) | `/agents/{id}/surfaces` | GET |
| Agent nav | `/agents/{id}/nav` | GET |
| Create agent | `/agents` | POST |
| Delete agent | `/agents/{id}` | DELETE |
| Duplicate agent | `/agents/{id}/duplicate` | POST |
| Update full agent | `/agents/{id}` | PUT |

### Chat & Threads

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| `MOCK_CHATS` | `/chats` | GET |
| `CHAT_GROUPS` | `/chats/days` | GET |
| `MOCK_THREADS[agentId]` | `/agents/{id}/chats` | GET |
| Create thread | `/agents/{id}/chats` | POST |
| `CHAT_MESSAGES` / `THREAD_MESSAGES` | `/chats/{id}/messages` | GET |
| Send message | `/chats/message` or WebSocket `/ws` | POST |
| Edit message | `/chats/messages/{id}/edit` | POST |
| Tool output | `/chats/{chatId}/tool-output/{toolCallId}` | GET |
| Search messages | `/chats/search?q=` | GET |
| Chat history by day | `/chats/history/{day}` | GET |
| Companion chat | `/chats/companion` | GET |
| `STARTER_PROMPTS` | Static (keep client-side) or `/agent/profile` | — |

### Runs & Workflows

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| `MOCK_RUNS[agentId]` | `/agents/{id}/runs` | GET |
| `MOCK_WORKFLOW_RUNS[key]` | `/workflows/{id}/runs` | GET |
| Run detail | `/workflows/{id}/runs/{runId}` | GET |
| `MOCK_WORKFLOW_STATS[agentId]` | `/agents/{id}/stats` | GET |
| Trigger run | `/workflows/{id}/run` | POST |
| Cancel run | `/workflows/{id}/runs/{runId}/cancel` | POST |
| Active runs | `/runs/active` | GET |

### Standalone Workflows

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| List workflows | `/workflows` | GET |
| Create workflow | `/workflows` | POST |
| Get workflow | `/workflows/{id}` | GET |
| Update workflow | `/workflows/{id}` | PUT |
| Delete workflow | `/workflows/{id}` | DELETE |
| Toggle workflow | `/workflows/{id}/toggle` | POST |
| Workflow bindings | `/workflows/{id}/bindings` | GET/PUT |

### Marketplace

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| `MARKETPLACE_CATEGORIES` | `/store/categories` | GET |
| `MARKETPLACE_SKILLS` | `/store/products?type=skill` | GET |
| `MARKETPLACE_AGENTS_LIST` | `/store/products?type=agent` | GET |
| `MARKETPLACE_PLUGINS` | `/store/products?type=plugin` | GET |
| `MARKETPLACE_CONNECTORS` | `/store/products?type=connector` | GET |
| Featured items | `/store/featured` | GET |
| Top items | `/store/products/top` | GET |
| `MARKETPLACE_SKILL_DETAILS[id]` | `/store/products/{id}` | GET |
| `MARKETPLACE_AGENT_DETAILS[id]` | `/store/products/{id}` | GET |
| `MARKETPLACE_PLUGIN_DETAILS[id]` | `/store/products/{id}` | GET |
| `MARKETPLACE_CONNECTOR_DETAILS[id]` | `/store/products/{id}` | GET |
| Reviews | `/store/products/{id}/reviews` | GET |
| Submit review | `/store/products/{id}/reviews` | POST |
| Similar items | `/store/products/{id}/similar` | GET |
| Install item | `/store/products/{id}/install` | POST |
| Uninstall item | `/store/products/{id}/install` | DELETE |
| Redeem code | `/codes` | POST |
| `PRIVATE_ORGS` | TBD (NeboAI API) | GET |
| `MARKETPLACE_PRIVATE_ITEMS` | TBD (org-scoped store products) | GET |
| `MARKETPLACE_COLLECTIONS` | TBD (org-scoped) | GET |

### Settings — Providers & Models

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| `PROVIDERS` | `/providers` | GET |
| Add provider | `/providers` | POST |
| Update provider | `/providers/{id}` | PUT |
| Delete provider | `/providers/{id}` | DELETE |
| Test provider | `/providers/{id}/test` | POST |
| `ROUTING_TASKS` | `/models` | GET |
| Update task routing | `/models/task-routing` | PUT |
| Update model | `/models/{provider}/{modelId}` | PUT |
| Local models | `/local-models/status` | GET |

### Settings — Agent Profile

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| Agent profile | `/agent/profile` | GET/PUT |
| `PERSONALITY_PRESETS` | `/agent/personality-presets` | GET |
| Agent settings | `/agent/settings` | GET/PUT |
| Agent status | `/agent/status` | GET |
| System info | `/agent/system-info` | GET |
| Heartbeat config | `/agent/heartbeat` | GET/PUT |
| Lanes | `/agent/lanes` | GET |

### Settings — Advisors

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| `ADVISORS` | `/agent/advisors` | GET |
| Create advisor | `/agent/advisors` | POST |
| Update advisor | `/agent/advisors/{name}` | PUT |
| Delete advisor | `/agent/advisors/{name}` | DELETE |

### Settings — Skills

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| `SKILLS_INSTALLED` | `/extensions` | GET |
| Skill content | `/skills/{name}/content` | GET |
| Toggle skill | `/skills/{name}/toggle` | POST |
| Skill secrets | `/skills/{name}/secrets` | GET |
| Set skill secret | `/skills/{name}/secrets` | PUT |
| Delete secret | `/skills/{name}/secrets/{key}` | DELETE |

### Settings — MCP / Integrations

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| `MCP_INTEGRATIONS` | `/integrations` | GET |
| `MCP_REGISTRY` | `/integrations/registry` or `/mcp/servers` | GET |
| Add integration | `/integrations` | POST |
| Update integration | `/integrations/{id}` | PUT |
| Delete integration | `/integrations/{id}` | DELETE |
| Test integration | `/integrations/{id}/test` | POST |
| Connect (API key) | `/integrations/{id}/connect` | POST |
| OAuth URL | `/integrations/{id}/oauth-url` | GET |
| Re-authenticate | `/integrations/{id}/reauthenticate` | POST |
| All tools | `/integrations/tools` | GET |

### Settings — Permissions

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| `PERMISSIONS` | `/user/me/permissions` | GET |
| Update permissions | `/user/me/permissions` | PUT |

### Settings — Memory

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| `MEMORIES` | `/memories` | GET |
| Search memories | `/memories/search?q=` | GET |
| Memory stats | `/memories/stats` | GET |
| Update memory | `/memories/{id}` | PUT |
| Delete memory | `/memories/{id}` | DELETE |

### Settings — Sessions

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| `SESSIONS` | `/agent/sessions` | GET |
| Session messages | `/agent/sessions/{id}/messages` | GET |
| Delete session | `/agent/sessions/{id}` | DELETE |

### Billing & Plans

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| `PLANS` | `/neboai/billing/prices` | GET |
| `BILLING` subscription | `/neboai/billing/subscription` | GET |
| `BILLING.invoices` | `/neboai/billing/invoices` | GET |
| `BILLING.paymentMethod` | `/neboai/billing/payment-methods` | GET |
| Subscribe | `/neboai/billing/subscribe` | POST |
| Checkout session | `/neboai/billing/checkout` | POST |
| Stripe portal | `/neboai/billing/portal` | POST |
| Cancel subscription | `/neboai/billing/cancel` | POST |
| Setup intent | `/neboai/billing/setup-intent` | POST |

### Notifications

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| Notification list | `/notifications` | GET |
| Unread count | `/notifications/unread-count` | GET |
| Mark as read | `/notifications/{id}/read` | PUT |
| Mark all read | `/notifications/read-all` | PUT |
| Delete | `/notifications/{id}` | DELETE |

### System Events

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| `EVENTS` | WebSocket `/ws` events | Stream |
| Error logs | (in `error_logs` table) | — |

### NeboAI Account

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| Start OAuth | `/neboai/oauth/start` | GET |
| OAuth status | `/neboai/oauth/status` | GET |
| Account info | `/neboai/account` | GET |
| Disconnect | `/neboai/account` | DELETE |
| Usage/Janus | `/neboai/janus/usage` | GET |
| Refresh usage | `/neboai/janus/usage/refresh` | POST |
| Referral code | `/neboai/referral-code` | GET |

### Scheduled Tasks

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| User-created schedule items | `/tasks` | GET |
| Create scheduled task | `/tasks` | POST |
| Update task | `/tasks/{name}` | PUT |
| Delete task | `/tasks/{name}` | DELETE |
| Toggle task | `/tasks/{name}/toggle` | POST |
| Run task now | `/tasks/{name}/run` | POST |
| Task history | `/tasks/{name}/history` | GET |

### Commander

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| (not in mock data) | `/commander/graph` | GET |
| Save layout | `/commander/layout` | PUT |
| Create team | `/commander/teams` | POST |
| Update team | `/commander/teams/{id}` | PUT |
| Delete team | `/commander/teams/{id}` | DELETE |
| Create edge | `/commander/edges` | POST |
| Delete edge | `/commander/edges/{id}` | DELETE |

### Setup / Onboarding

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| Setup status | `/setup/status` | GET |
| Create admin | `/setup/admin` | POST |
| Complete setup | `/setup/complete` | POST |
| Setup personality | `/setup/personality` | GET/PUT |

### Auth

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| (not in mock — no auth yet) | `/auth/login` | POST |
| | `/auth/register` | POST |
| | `/auth/refresh` | POST |
| | `/auth/config` | GET |
| | `/auth/forgot` | POST |
| | `/auth/reset` | POST |

### Files

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| (not in mock) | `/files/browse` | POST |
| | `/files/pick` | POST |
| | `/files/pick-folder` | POST |
| | `/files/{*path}` | GET |

### Entity Config

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| (not in mock) | `/entity-config/{type}/{id}` | GET/PUT/DELETE |

### Plugins

| Frontend Mock | Backend Endpoint | Method |
|---------------|-----------------|--------|
| (partial in mock) | `/plugins` | GET |
| | `/plugins/{slug}/auth/login` | POST |
| | `/plugins/{slug}/auth/logout` | POST |
| | `/plugins/{slug}/auth/status` | GET |
| | `/plugins/{slug}` | DELETE |

---

## WebSocket Events (from ClientHub)

Events streamed to the frontend via `/ws`:

| Event | Payload | Used By |
|-------|---------|---------|
| `chat_stream` | Text content chunks | ChatPane |
| `thinking` | Extended thinking blocks | ChatPane |
| `tool_start` | Tool invocation initiated | ChatPane |
| `tool_result` | Tool output received | ChatPane |
| `chat_error` | Error during chat | ChatPane |
| `usage` | Token counts | ChatPane |
| `approval_request` | Waiting for tool approval | ApprovalModal |
| `ask_request` | Interactive question prompt | ChatPane |
| `chat_complete` | Chat finished | ChatPane |
| `chat_cancelled` | User cancelled | ChatPane |
| `connected` | WS connection established | — |
| `chat_ack` | Message accepted | ChatComposer |
| `chat_created` | Run started | — |
| `quota_warning` | Usage >80% | NotificationBell |
| `stream_status` | Idle/running probe | StatusDot |
| `session_reset` | Session cleared | — |
| `session_compact` | Compaction result | — |
| `code_processing` | Marketplace code installing | — |
| `code_result` | Code install outcome | Marketplace |
| `dep_installed` | Dependency cascade step | — |
| `dep_cascade_complete` | All deps installed | — |
| `tool_quarantined` | Tool disabled at runtime | — |
| `tool_error` | Tool registration error | — |

---

## Data That Stays Client-Side (No Backend Needed)

| Data | File | Reason |
|------|------|--------|
| `AGENT_COLORS_MAP` | mockData.ts | CSS class mapping |
| `AGENT_COLORS` | tokens.ts | CSS class mapping |
| `N` (design tokens) | tokens.ts | Color palette |
| `ADVISOR_ROLE_COLORS` | mockData.ts | CSS class mapping |
| `EVENT_COLORS` | mockData.ts | CSS class mapping |
| `CAL_DAYS` | data.ts | Static labels |
| `AGENT_ID_MAP` / `AGENT_ID_REVERSE` | data.ts | ID translation (may change) |
| `ARCHITECT_INTRO_MESSAGE` | mockData.ts | Static intro text |
| `STARTER_PROMPTS` | mockData.ts | Static suggestions |
| `NODE_CATALOG_ITEMS` (static parts) | mockData.ts | Trigger/activity/flow types |
| Theme store | stores/theme.ts | localStorage |
| Sidebar store | stores/sidebar.ts | UI state |
| Toast store | stores/toast.ts | Ephemeral UI |
| DevMode store | stores/devmode.ts | localStorage |
