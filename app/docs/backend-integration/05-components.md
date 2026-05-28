# Components — Props, Data Needs & Callbacks

Every component in `src/lib/components/`, its props, mock data imports, callbacks, and backend API requirements.

## Component Inventory

### Layout & Shell Components (no backend data)

| Component | Props | Purpose |
|-----------|-------|---------|
| `NeboShell` | `tab`, `children` | Top-level app shell with header tabs |
| `MarketplaceShell` | `children` | Marketplace layout wrapper |
| `SettingsShell` | `children` | Settings modal with sidebar nav |
| `ColorCalendarShell` | `view`, `selectedDate`, `enabled`, `onopencanvas` | Calendar header + view switcher |

### UI Primitives (no backend data)

| Component | Props | Purpose |
|-----------|-------|---------|
| `Avatar` | `ch`, `size`, `tone` | Agent avatar circle |
| `StatusDot` | `kind`, `size` | Status indicator with pulse |
| `MiniMonth` | `selectedDate`, `onselect` | Compact month date picker |
| `Toast` | (global store) | Ephemeral notifications |

---

## Components With Backend Data Needs

### Sidebar.svelte
**Path:** `src/lib/components/Sidebar.svelte`

**Props:**
- `activePage` — current page identifier
- `activeChat` — current chat ID
- `enabled` — schedule agent toggle states
- `onToggleAgent` — callback when agent toggled
- `marketplaceTab` — current marketplace tab

**Mock Data Imports:**
| Import | Replace With |
|--------|-------------|
| `MOCK_AGENTS` | `GET /api/v1/agents` |
| `MOCK_CHATS` | `GET /api/v1/chats` |
| `CHAT_GROUPS` | `GET /api/v1/chats/days` (group by recency) |
| `AGENT_COLORS_MAP` | Keep (static CSS mapping) |

**Schedule Data:**
| Import | Replace With |
|--------|-------------|
| `AGENTS` from data.ts | Derive from backend agents |
| `getScheduleAgents()` | Derive from agent workflows |
| `runsPerWeek()` | `GET /api/v1/agents/{id}/stats` |
| `userScheduleItems` store | `GET /api/v1/tasks` |

---

### UserMenu.svelte
**Props:** `collapsed`

**Mock Data:**
| Import | Replace With |
|--------|-------------|
| `USER` | `GET /api/v1/user/me` + profile |
| `PLANS` | `GET /api/v1/neboai/billing/subscription` |

---

### NotificationBell.svelte
**Props:** None (uses global store)

**Store:** `notifications`, `unreadCount`, `markAsRead`, `markAllRead`, `removeNotification`

**Backend Endpoints:**
| Store Function | Endpoint |
|----------------|----------|
| Initial load | `GET /api/v1/notifications` |
| Unread count | `GET /api/v1/notifications/unread-count` |
| Mark as read | `PUT /api/v1/notifications/{id}/read` |
| Mark all read | `PUT /api/v1/notifications/read-all` |
| Remove | `DELETE /api/v1/notifications/{id}` |
| Real-time | WebSocket events |

---

### AgentTabBar.svelte
**Props:** `agentId`, `agentName`, `agentInitial`, `status`
**Backend:** Props provided by parent layout (no direct imports)

---

### CommandPalette.svelte
**Props:** `show`, `onclose`
**Backend:** None (hardcoded navigation items). Could be enhanced with `GET /api/v1/agents` for dynamic agent list.

---

### ApprovalModal.svelte
**Props:** `show`, `agent`, `actionType`, `actionDetail`, `actionKey`, `onApprove`, `onDeny`, `onclose`
**Store:** `approveAlways` from permissions store
**Backend:** Approval decisions come via WebSocket `approval_request` events. The `approveAlways` store should sync with `PUT /api/v1/user/me/permissions`.

---

### AgentSetupModal.svelte
**Props:** `show`, `agentName`, `onclose`
**Backend:** `POST /api/v1/agents/{id}/setup` on completion

---

### OAuthConnectModal.svelte
**Props:** `show`, `pluginName`, `onclose`
**Backend:** `GET /api/v1/integrations/{id}/oauth-url` → redirect flow → `GET /api/v1/integrations/oauth/callback`

---

## Calendar Components

### ColorDayView.svelte
**Props:** `enabled`, `selectedDate`, `onopencanvas`

**Data Flow:**
| Import | Replace With |
|--------|-------------|
| `AGENT_COLORS` | Keep (static) |
| `AGENTS` | Derive from backend |
| `userScheduleItems` store | Store backed by `GET /api/v1/tasks` + agent workflows |
| Schedule helper functions | Keep (compute from backend data) |

**User Actions:**
- Double-click time slot → open ScheduleEventModal
- Click event → show DayDetailPane
- Events show run status badges (green/red/yellow dots)

---

### ColorWeekView.svelte
**Props:** `enabled`, `selectedDate`, `onopencanvas`
**Data:** Same as ColorDayView but renders 7 columns
**Uses:** `packLanes()` from utils.ts for overlap layout

---

### ColorMonthView.svelte
**Props:** `enabled`, `selectedDate` (bindable)
**Data:** Same schedule store sources
**Shows:** Agent color badges with run status indicators per day

---

### DayDetailPane.svelte
**Props:** `item` (CalendarItem), `createData`, `onclose`, `onopencanvas`, `preview`

**Mock Data:**
| Import | Replace With |
|--------|-------------|
| `AGENT_CONFIGS` | `GET /api/v1/agents/{id}/workflows` |
| Schedule store functions | Keep |

**Shows:** Event details (when, recurrence, trigger, last run status, workflow activities, recent runs)
**Backend:** `GET /api/v1/workflows/{id}/runs` for recent runs

---

### ScheduleEventModal.svelte
**Props:** `open`, `hour`, `date`
**Data:** `AGENTS`, `AGENT_COLORS` (static), schedule store
**Writes:** `addUserItem()` → `POST /api/v1/tasks`

---

## Chat Components

### ChatPane.svelte
**Props:** `messages`, `agentName`, `agentId`, `headerTitle`, `headerRight`, `placeholder`, `emptyIcon`, `emptyTitle`, `emptyDesc`, `onsend`, `onedit`, `onredo`, `isLoading`

**Exported Methods:** `focusComposer()`, `showCreations(title)`, `hideCreations()`

**Mock Data:** Hardcoded artifact examples (documents, tables, code)
**Backend:** Messages come via props (parent fetches from `GET /api/v1/chats/{id}/messages`). Send via WebSocket.

---

### ChatComposer.svelte
**Props:** `agentName`, `agentId`, `placeholder`, `onsend`, `isLoading`

**Mock Data:**
| Import | Replace With |
|--------|-------------|
| `MOCK_AGENTS` | `GET /api/v1/agents` (for @mentions) |
| `AGENT_COLORS_MAP` | Keep (static) |

**Features:** Slash commands, @agent mentions, file attachments, multi-line input
**Backend:** File upload → `POST /api/v1/files` or WebSocket attachment

---

### SlashCommandMenu.svelte
**Props:** `query`, `onselect`, `onclose`
**Data:** `filterCommands()` from `slashCommands.ts` (static command definitions)
**Backend:** None

---

## Workflow Components

### WorkflowBuilder.svelte
**Props:** `workflows`, `agentId`, `agentName`, `onclose`, `onsave`

**Features:**
- Left: AI Architect chat
- Center: BuilderCanvas + NodeCatalog
- Right: NodeConfigPanel
- Toolbar: Undo/redo, validation, save/discard
- Workflow tabs

**Backend:** `onsave(workflows)` → `PUT /api/v1/agents/{id}/workflows/{binding_name}` per workflow

---

### BuilderCanvas.svelte
**Props:** `workflow`, `workflowName`, `agentId`, `mode`, `selectedNodeId`, + callbacks

**Callbacks:**
- `onselect(nodeId)` — node selected
- `onopenCatalog(afterNodeId, branchLabel)` — open node catalog
- `onremove(nodeId)` — delete node
- `onduplicate(nodeId)` — duplicate node
- `oncreateConnection(fromId, toId)` — create edge
- `onremoveConnection(fromId, toId)` — delete edge
- `ondropNode(item, afterNodeId)` — drop catalog item

**Backend:** None directly (parent manages state)

---

### NodeConfigPanel.svelte
**Props:** `workflowName`, `workflow`, `selectedNodeId`, `activity`, `mode`, + update callbacks

**Mock Data:** `ACTIVITY_TYPES` from `workflowTypes.ts` (static type definitions)

**Callbacks:**
- `onupdateActivity(idx, changes)` — update activity
- `onupdateTrigger(trigger)` — change trigger config
- `onupdateEmit(emit)` — change emit event
- `onupdateDescription(desc)` — change description
- `onremove(nodeId)` — delete node
- `onremoveWorkflow()` — delete workflow
- `onclose()` — close panel
- `onselectActivity(id)` — select activity

---

### NodeCatalog.svelte
**Props:** `onselect`, `onclose`

**Mock Data:**
| Import | Replace With |
|--------|-------------|
| `NODE_CATALOG_ITEMS` | Partially static + `GET /api/v1/integrations` (MCP) + `GET /api/v1/agents` (agents) |

**Features:** Search, drag-and-drop with `application/x-workflow-node` MIME type

---

### BuilderChat.svelte
**Props:** `agentId`, `workflows`, `selectedWorkflowName`, `onaction`

**Mock Data:** `ARCHITECT_INTRO_MESSAGE` (static)
**Backend:** Could use `POST /api/v1/agents/{id}/chat` for actual AI architect responses

---

### WorkflowCanvas.svelte
**Props:** `workflows`, `agentId`
**Data:** Workflow definitions from props (read-only multi-workflow view)
**Backend:** None directly

---

## Component → Backend Migration Priority

### P0 — Critical (app broken without these)
1. **Sidebar** — needs agent list + chat list
2. **ChatPane/ChatComposer** — needs real chat via WebSocket
3. **UserMenu** — needs user profile
4. **NotificationBell** — needs notifications API

### P1 — Core Features
5. **DayDetailPane** — needs workflow run history
6. **Calendar views** — needs schedule data from agent workflows
7. **WorkflowBuilder** — needs workflow persistence
8. **ApprovalModal** — needs WebSocket approval flow

### P2 — Settings & Marketplace
9. **SettingsShell routes** — each section needs its API
10. **Marketplace pages** — needs store API
11. **AgentSetupModal** — needs agent config save
12. **OAuthConnectModal** — needs OAuth flow
