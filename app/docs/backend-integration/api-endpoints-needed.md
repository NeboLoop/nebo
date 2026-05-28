# API Endpoints Needed by V2 Frontend

Generated 2026-05-01. This documents every API endpoint the frontend requires,
whether it exists, and what shape the response must have.

---

## 1. Marketplace / Store

### Existing Endpoints (need response shape verification)

#### `GET /api/v1/store/products`
- **Frontend calls:** `listStoreProducts(params?)`
- **Used by:** All marketplace listing pages, featured page, categories page
- **Required query params:**
  - `type` — filter by `agent | skill | plugin | connector`
  - `category` — filter by category slug
  - `featured` — boolean, return only featured items
  - `orgId` — filter by organization (for private/org-scoped items)
  - `q` — search query
  - `page`, `pageSize` — pagination
- **Required response shape:**
```json
{
  "apps": [
    {
      "id": "string",
      "name": "string",
      "slug": "string",
      "description": "string",
      "icon": "string",
      "category": "string",
      "version": "string",
      "author": { "id": "string", "name": "string", "verified": true },
      "installCount": 0,
      "rating": 4.8,
      "reviewCount": 0,
      "isInstalled": false,
      "status": "published",
      "price": "Get",
      "featured": true,
      "code": "SKIL-XXXX-XXXX",
      "type": "skill"
    }
  ],
  "totalCount": 0,
  "page": 1,
  "pageSize": 20
}
```
- **GAPS:** Frontend expects `price`, `featured`, `code`, `type` fields on each item.
  Current `AppItem` type in neboComponents.ts does not include these. Backend must add them.

#### `GET /api/v1/store/products/:id`
- **Frontend calls:** `getStoreProduct(id)`
- **Used by:** All marketplace detail pages (agents/[id], skills/[id], plugins/[id], connectors/[id])
- **Required response shape:**
```json
{
  "app": {
    "id": "string",
    "name": "string",
    "description": "string",
    "longDesc": "string",
    "category": "string",
    "author": { "id": "", "name": "NeboAI", "verified": true },
    "rating": 4.8,
    "installCount": 0,
    "price": "Get",
    "type": "skill",
    "features": ["Feature 1", "Feature 2"],
    "screenshots": [{ "title": "", "desc": "" }],
    "platforms": ["macOS", "Windows", "Linux"],
    "tools": ["tool_name_1", "tool_name_2"],
    "worksWith": ["GitHub Actions", "Slack"],
    "serverType": "stdio",
    "authType": "oauth",
    "hasAuth": true,
    "ratingDistribution": { "5": 100, "4": 50, "3": 20, "2": 5, "1": 2 },
    "developer": {
      "name": "NeboAI",
      "website": "neboai.com",
      "support": "support@neboai.com",
      "launched": "Jan 2026"
    },
    "pricing": [
      {
        "name": "Starter",
        "price": "$9.99/mo",
        "annual": "$99/yr",
        "trial": "14-day free trial",
        "features": ["Feature 1"],
        "popular": false
      }
    ],
    "requiredSkills": [{ "id": "s9", "name": "GitHub Actions" }],
    "requiredPlugins": [{ "id": "p4", "name": "GitHub" }],
    "usedBy": [{ "id": "a1", "name": "DevOps Pro" }]
  }
}
```
- **GAPS:** `longDesc`, `features`, `tools`, `worksWith`, `serverType`, `authType`,
  `hasAuth`, `ratingDistribution`, `developer`, `pricing`, `requiredSkills`,
  `requiredPlugins`, `usedBy` are NOT in the current `StoreAppDetail` type.
  Backend must add these fields to the product detail response.

#### `GET /api/v1/store/products/:id/reviews`
- **Frontend calls:** `getStoreProductReviews(id)`
- **Used by:** All detail pages
- **Required response shape:**
```json
{
  "reviews": [
    {
      "id": "string",
      "userName": "string",
      "rating": 5,
      "title": "string",
      "body": "string",
      "createdAt": "2026-04-01",
      "helpful": 0,
      "role": "Engineering Lead",
      "duration": "Using for 6 months"
    }
  ],
  "totalCount": 0,
  "average": 4.8,
  "distribution": [100, 50, 20, 5, 2]
}
```
- **GAPS:** Frontend expects `role` and `duration` on each review. Not in current `Review` type.

### New Endpoints Needed

#### `GET /api/v1/store/categories`
- **Used by:** marketplace categories page, layout sidebar
- **Response:**
```json
{
  "categories": [
    { "slug": "productivity", "name": "Productivity", "emoji": "⚡", "count": 24 }
  ]
}
```
- **Alternative:** Backend can include category counts in `listStoreProducts` meta,
  and frontend derives. But a dedicated endpoint is cleaner.

#### `GET /api/v1/store/orgs`
- **Used by:** marketplace collections page, collection detail
- **Response:**
```json
{
  "orgs": [
    { "id": "acme", "name": "Acme Corp", "initial": "A", "itemCount": 4 }
  ]
}
```
- **Purpose:** Lists organizations the user has access to for private marketplace items.

#### Collection CRUD — 7 endpoints

```
GET    /api/v1/store/collections                → { collections: Collection[] }
GET    /api/v1/store/collections/:id             → { collection: Collection }
POST   /api/v1/store/collections                 → { collection: Collection }
PUT    /api/v1/store/collections/:id             → { collection: Collection }
DELETE /api/v1/store/collections/:id             → { deleted: true }
POST   /api/v1/store/collections/:id/items       → { collection: Collection }
DELETE /api/v1/store/collections/:id/items/:itemId → { collection: Collection }
```

**Collection shape:**
```json
{
  "id": "col-1",
  "name": "Sales Enablement Kit",
  "desc": "Everything your sales team needs",
  "orgId": "acme",
  "items": ["prv-s1", "prv-s2", "prv-a1"],
  "itemCount": 3,
  "curator": "Jordan M.",
  "updated": "2026-04-29T00:00:00Z",
  "visibility": "public"
}
```

---

## 2. Agent Configuration

### Existing Endpoints (need response shape verification)

#### `GET /api/v1/agents/:id/workflows`
- **Frontend calls:** `getAgentWorkflows(agentId)`
- **Used by:** agent layout, runs detail, schedule, DayDetailPane
- **Required response must include per workflow:**
```json
{
  "workflows": {
    "morning-scan": {
      "trigger": {
        "type": "schedule",
        "schedule": "8:00 AM daily"
      },
      "description": "Scan configured topics for overnight developments",
      "isActive": true,
      "lastFired": "Today, 8:00 AM",
      "emit": "research.digest.ready",
      "activities": [
        {
          "id": "scan-sources",
          "type": "research",
          "intent": "Check all configured topic sources",
          "skills": ["@nebo/skills/web-scraper@^1.0.0"],
          "steps": ["Search news for each configured topic"],
          "params": {}
        }
      ],
      "connections": [
        { "from": "__trigger__", "to": "scan-sources" },
        { "from": "scan-sources", "to": "__emit__" }
      ]
    }
  }
}
```
- **GAPS:** Verify the backend returns `activities` with `type`, `intent`, `skills`,
  `steps`, `params` fields. Also verify `connections` array with `from`, `to`, `label`.
  Also verify `isActive`, `lastFired`, `emit` fields per workflow.

#### `GET /api/v1/agents/:id`
- **Frontend calls:** `getAgent(agentId)`
- **Required response must include:**
```json
{
  "agent": {
    "id": "researcher",
    "name": "Researcher",
    "persona": "You are the Research Analyst...",
    "model": "claude-sonnet-4-6",
    "inputs": [
      {
        "key": "research_topics",
        "label": "Topics to track",
        "type": "textarea",
        "required": false,
        "placeholder": "AI trends...",
        "description": "Topics for daily digests",
        "default": "",
        "options": []
      }
    ],
    "skills": ["Web Browser", "Web Scraper", "PDF Analyzer"]
  }
}
```
- **GAPS:** Frontend reads `persona`, `model`, `inputs` (with full field definitions),
  and per-agent `skills` list. Verify these are in the response.

#### `GET /api/v1/agents/:id/runs` (with stats)
- **Frontend calls:** `listAgentRuns(agentId, limit, offset)` and `getAgentStats(agentId)`
- **Run shape needed:**
```json
{
  "id": "run1",
  "label": "Morning market scan",
  "time": "8:00 AM",
  "date": "Today",
  "status": "success",
  "duration": "2m 14s",
  "steps": 4,
  "triggerType": "schedule",
  "workflowRunId": "wr6"
}
```
- **Stats shape needed:**
```json
{
  "totalRuns": 124,
  "completed": 119,
  "failed": 3,
  "running": 0,
  "avgDuration": "1m 48s",
  "lastRunAt": "Today, 8:02 AM"
}
```

#### `GET /api/v1/workflows/:id/runs/:runId`
- **Frontend calls:** `getWorkflowRun(workflowId, runId)`
- **Required response shape:**
```json
{
  "run": {
    "id": "wr6",
    "workflowId": "morning-scan",
    "status": "success",
    "startedAt": "Today, 8:00 AM",
    "completedAt": "Today, 8:02 AM",
    "duration": "2m 14s",
    "steps": 4,
    "triggerType": "schedule",
    "tokens": { "input": 3800, "output": 1650 },
    "error": null
  },
  "activities": [
    {
      "id": "scan-sources",
      "status": "success",
      "duration": "1m 32s",
      "output": "Found 14 articles across 6 sources",
      "error": null
    }
  ]
}
```

### New Endpoints Needed

#### `GET /api/v1/agents/:id/skills`
- **Used by:** agent layout (provides skills to settings/skills section)
- **Response:**
```json
{
  "skills": ["File System", "Web Browser", "Code Review", "Meeting Notes"]
}
```
- **Alternative:** If `getAgent()` already returns skills array, this is not needed separately.

---

## 3. Chat / Threads

### Existing Endpoints (verify shape)

#### `GET /api/v1/agents/:id/chats`
- **Frontend calls:** `listAgentChats(agentId)`
- **Used by:** agent layout (thread list in left panel)
- **Required response shape:**
```json
{
  "chats": [
    {
      "id": "t1",
      "name": "Q3 board deck summary",
      "preview": "Key takeaways from the board presentation...",
      "updatedAt": "25m ago",
      "messages": 5
    }
  ]
}
```
- **GAPS:** Frontend expects `name`, `preview`, `updatedAt`, `messages` count.
  Verify the backend returns these fields (not just `id` and `title`).

#### `GET /api/v1/chats/:id/days`
- **Frontend calls:** `listChatDays(params?)`
- **Used by:** Sidebar (groups chats by time period)
- **Required response shape:**
```json
{
  "groups": [
    { "label": "Today", "chats": ["c1", "c2"] },
    { "label": "Yesterday", "chats": ["c3", "c4"] },
    { "label": "Previous 7 days", "chats": ["c5", "c6"] }
  ]
}
```

---

## 4. Onboarding

### Existing Endpoints (verify)

#### `GET /api/v1/tools/permissions`
- **Frontend calls:** `getToolPermissions()`
- **Used by:** onboarding page (step 3: capability toggles), settings/permissions
- **Required response shape:**
```json
{
  "permissions": [
    {
      "id": "chat",
      "label": "Chat",
      "desc": "Send and receive messages",
      "enabled": true,
      "locked": true
    }
  ]
}
```
- **GAPS:** Frontend expects `locked` boolean (some permissions can't be toggled).
  Verify backend includes this.

---

## 5. Workflow Builder

### New Endpoints Needed

#### `GET /api/v1/workflows/catalog` (or include in agent config)
- **Used by:** NodeCatalog.svelte (workflow builder node picker)
- **Purpose:** Returns available workflow activity types, plus dynamic sections
  for connected MCP servers and available agents.
- **Response:**
```json
{
  "triggers": [
    { "type": "trigger-schedule", "label": "Schedule", "desc": "Run at set times" }
  ],
  "activities": [
    { "type": "activity-custom", "label": "Custom Activity", "desc": "Define steps and skills" },
    { "type": "activity-research", "label": "Research", "desc": "Web search and analysis" }
  ],
  "flowControl": [
    { "type": "flow-condition", "label": "Condition", "desc": "If/else branching" }
  ],
  "connectors": [
    { "type": "connector-int1", "label": "Google Workspace", "toolCount": 12, "serverId": "int1" }
  ],
  "agents": [
    { "type": "agent-researcher", "label": "Researcher", "agentId": "researcher", "role": "Web + market data" }
  ]
}
```
- **Alternative:** Frontend can derive connectors from `listMCPIntegrations()` and agents
  from `listAgents()`. The static trigger/activity/flow types are already hardcoded in
  `NODE_CATALOG_ITEMS`. So this endpoint is optional — frontend can compose the catalog
  from existing endpoints.

---

## Summary: What Backend Must Add

### New Endpoints (3 required + 7 collection CRUD)
1. `GET /api/v1/store/categories` — marketplace categories with counts
2. `GET /api/v1/store/orgs` — user's accessible organizations
3. `GET /api/v1/store/collections` — CRUD (7 endpoints total)

### Existing Endpoints — Fields to Add
4. `listStoreProducts` response: add `price`, `featured`, `code`, `type` to each item
5. `getStoreProduct` response: add `longDesc`, `features`, `tools`, `worksWith`,
   `serverType`, `authType`, `hasAuth`, `ratingDistribution`, `developer`, `pricing`,
   `requiredSkills`, `requiredPlugins`, `usedBy`
6. `getStoreProductReviews` response: add `role`, `duration` to each review
7. `getAgent` response: verify `persona`, `model`, `inputs`, `skills` fields
8. `getAgentWorkflows` response: verify `activities` with `type`, `intent`, `skills`,
   `steps`, `params`, and `connections` array
9. `listAgentChats` response: verify `name`, `preview`, `updatedAt`, `messages` fields
10. `getToolPermissions` response: verify `locked` boolean per permission
11. `listAgentRuns`/`getAgentStats` response: verify shapes match frontend expectations
