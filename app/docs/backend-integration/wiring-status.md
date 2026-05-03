# Frontend API Wiring Status

Generated 2026-05-01. Tracks which pages load from API vs show empty state.

## Legend
- **WIRED** — Has onMount API call, loads real data from backend
- **EMPTY** — Mock data removed, shows empty state, needs API wiring
- **STATIC** — Uses UI constants only (colors, catalog), no data loading needed
- **CONTEXT** — Gets data from parent layout's context provider

---

## Settings Pages (all WIRED)

| Page | API Call | Status |
|------|----------|--------|
| settings/account | `getUserProfile()`, `neboLoopAccountStatus()` | WIRED |
| settings/profile | `getUserProfile()`, `updateUserProfile()` | WIRED |
| settings/providers | `listAuthProfiles()` | WIRED |
| settings/agents | `listAgents()` | WIRED |
| settings/advisors | `listAdvisors()` | WIRED |
| settings/mcp | `listMCPIntegrations()`, `listMCPServerRegistry()` | WIRED |
| settings/skills | `listTools()` | WIRED |
| settings/sessions | `listAgentSessions()` | WIRED |
| settings/plugins | `listPlugins()` | WIRED |
| settings/memories | `listMemories()` | WIRED |
| settings/billing | `neboLoopBillingSubscription()`, `neboLoopBillingInvoices()` | WIRED |
| settings/permissions | `getToolPermissions()` | WIRED |
| settings/personality | `listPersonalityPresets()`, `getPersonality()` | WIRED |
| settings/rules | `getAgentSettings()` | WIRED |
| settings/routing | `getLanes()` | WIRED |

## Top-Level Pages

| Page | API Call | Status |
|------|----------|--------|
| activity | `listAgentSessions()` | WIRED |
| automate | `listTasks()` | WIRED |
| events | `listEventSources()` | WIRED |
| skills | `listTools()` | WIRED |
| workspaces | `listAgents()` | WIRED |
| upgrade | `neboLoopBillingPrices()`, `neboLoopBillingCheckout()` | WIRED |
| chat | `getCompanionChat()` | WIRED |
| schedule | `loadScheduleFromAPI()` → `listAgents()`, `getAgentWorkflows()`, `listAllRuns()` | WIRED |
| onboarding | `getToolPermissions()` | WIRED |

## Agent Pages

| Page | API Call | Status |
|------|----------|--------|
| [agentId]/+layout | `listAgents()`, `listAgentChats()`, `listAgentRuns()`, `getAgentStats()` | WIRED (partial) |
| [agentId]/threads/[threadId] | `getChat()` + WebSocket | WIRED |
| [agentId]/runs/[runId] | `getAgentWorkflows()` | WIRED |
| [agentId]/settings/[section] | via parent context | CONTEXT |
| workspace/[agentId] | `listAgents()` | WIRED |

**Agent layout gaps:** `AGENT_SKILLS`, `AGENT_CONFIGS`, `MOCK_WORKFLOW_RUNS` now empty arrays/objects.
Needs: `getAgentWorkflows()` and `getAgent()` for skills/config — verify backend returns expected shape.

## Marketplace Pages

| Page | API Call | Status |
|------|----------|--------|
| marketplace/+layout | `listStoreProducts()` | WIRED |
| marketplace/+page (featured) | `listStoreProducts()`, `loadInstalledItems()` | WIRED |
| marketplace/agents | `listStoreProducts({type:'agent'})` | WIRED |
| marketplace/agents/[id] | `getStoreProduct()`, `getStoreProductReviews()` | WIRED |
| marketplace/skills | `listStoreProducts({type:'skill'})` | WIRED |
| marketplace/skills/[id] | `getStoreProduct()`, `getStoreProductReviews()` | WIRED |
| marketplace/plugins | `listStoreProducts({type:'plugin'})` | WIRED |
| marketplace/plugins/[id] | `getStoreProduct()`, `getStoreProductReviews()` | WIRED |
| marketplace/connectors | `listStoreProducts({type:'connector'})` | WIRED |
| marketplace/connectors/[id] | `getStoreProduct()`, `getStoreProductReviews()` | WIRED |
| marketplace/categories | `listStoreProducts()` (derives categories) | WIRED |
| marketplace/collections | none | **EMPTY — needs collections endpoint** |
| marketplace/collections/[id] | none | **EMPTY — needs collections endpoint** |
| marketplace/installed | `loadInstalledItems()` | WIRED |

## Components

| Component | API Call | Status |
|-----------|----------|--------|
| Sidebar | `listAgents()`, `listChats()`, `listChatDays()` | WIRED |
| UserMenu | `getUserProfile()`, `neboLoopBillingSubscription()` | WIRED |
| ChatComposer | `listAgents()` | WIRED |
| DayDetailPane | none (uses schedule store) | STATIC (store has API) |
| NodeCatalog | `listMCPIntegrations()`, `listAgents()` | WIRED |
| BuilderChat | none | STATIC (intro message only) |

## Stores

| Store | API Call | Status |
|-------|----------|--------|
| schedule | `listWorkflows()`, `listAllRuns()` | WIRED |
| marketplace | `listPlugins()`, `listTools()` | WIRED |
| collections | none | **EMPTY — needs collections endpoint** |

---

## Remaining Work (3 items)

### Needs backend endpoints first (2 items):
1. marketplace/collections/+page.svelte → needs collection CRUD endpoints
2. marketplace/collections/[id]/+page.svelte → needs collection CRUD endpoints

### Needs agent config API verification (1 item):
3. [agentId]/+layout.svelte → needs `getAgent()` for skills/config, `getAgentWorkflows()` for workflow defs
