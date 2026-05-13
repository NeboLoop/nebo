# WORKSPACES_SYSTEM.md — Subject Matter Expert Document

## Overview

Workspaces are Nebo's mechanism for turning **agents into apps**. Instead of chat-only interactions, agents with declared `views.json` configurations render as full interactive UIs using the **A2UI (Agent-to-UI) Protocol v0.9**.

Core design principle: *Everything with a UI is an agent. The sidebar is the app launcher.*

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                        FRONTEND (SvelteKit)                          │
│                                                                     │
│  /workspaces (multi-agent)     /workspace/[agentId] (single-agent) │
│  ┌──────┬────────────┬──────┐  ┌────────────────────┬──────┐       │
│  │Sidebar│ A2Surface  │ Chat │  │    A2Surface       │ Chat │       │
│  │(agents)│(components)│(panel)│  │   (components)    │(panel)│       │
│  └──────┴────────────┴──────┘  └────────────────────┴──────┘       │
│         ↕ WebSocket                    ↕ WebSocket                  │
├─────────────────────────────────────────────────────────────────────┤
│                        BACKEND (Rust/Axum)                           │
│                                                                     │
│  ws.rs ─→ a2ui_actions.rs ─→ A2UIManager (a2ui.rs)                 │
│              │                     │           │                     │
│              ├─ mcp_call           ├─ persist  ├─ broadcast          │
│              ├─ navigate           │  (SQLite) │  (ClientHub)        │
│              ├─ update_data        │           │                     │
│              └─ agent (→ LLM)      │           │                     │
│                                    ▼           ▼                     │
│                             a2ui_surfaces    WebSocket               │
│                               (table)        clients                 │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Database Schema

**Table: `a2ui_surfaces`** (migration `0078_a2ui_surfaces.sql`)

| Column | Type | Description |
|--------|------|-------------|
| `id` | TEXT PK | Format: `agent:{agent_id}:{view_id}` |
| `agent_id` | TEXT FK | References `agents(id)` |
| `view_id` | TEXT | From views.json (e.g., "default", "dashboard") |
| `surface_type` | TEXT | `panel` / `window` / `overlay` |
| `components` | TEXT | Last-known component tree (JSON) |
| `data_model` | TEXT | Last-known data model (JSON) |
| `window_geometry` | TEXT | Serialized x,y,w,h for window restore |
| `is_active` | INTEGER | 1 = active, 0 = deactivated |

Indexed on `(agent_id)` and unique on `(agent_id, view_id)`.

---

## A2UI Protocol (v0.9)

### Component Model — Flat Adjacency List

Components are NOT nested trees. They use flat lists with ID references:

```json
[
  { "id": "root", "component": "Column", "children": ["header", "body"] },
  { "id": "header", "component": "Text", "props": { "variant": "h1", "content": "Dashboard" } },
  { "id": "body", "component": "Row", "children": ["stat1", "stat2"] },
  { "id": "stat1", "component": "Stat", "props": { "label": "Users", "value": { "path": "/metrics/users" } } }
]
```

Why: eliminates perfect nesting requirements, enables streaming/incremental updates, supports targeted updates by ID.

### Data Binding

- **Literal:** `"Hello"` → renders as-is
- **Path binding:** `{ "path": "/metrics/users" }` → resolved from data model via JSON Pointer (RFC 6901)
- **Template children:** `{ "componentId": "item-template", "path": "/items" }` → repeats component for each array element

### Message Types

| Direction | Event | Purpose |
|-----------|-------|---------|
| Server→Client | `a2ui_message` (CreateSurface) | Initialize surface with catalog + theme |
| Server→Client | `a2ui_message` (UpdateComponents) | Push component tree |
| Server→Client | `a2ui_message` (UpdateDataModel) | Merge data at JSON Pointer path |
| Server→Client | `a2ui_action_status` | Action processing/complete status |
| Client→Server | `a2ui_action` | User triggered action with context |
| Client→Server | `a2ui_init` | Request surface replay on reconnect |

### Catalog (18 Components)

**Layout:** Row, Column, List, Tabs, Card, Modal
**Display:** Text, Image, Icon, Video, AudioPlayer, Divider
**Interactive:** Button, TextField, CheckBox, ChoicePicker, Slider, DateTimeInput
**Extended (Nebo):** Badge, Stat, Dot

---

## Backend Flow

### A2UIManager (`crates/server/src/a2ui.rs`)

In-memory state + SQLite persistence + WebSocket broadcast:

```rust
pub struct A2UIManager {
    hub: Arc<ClientHub>,                    // WebSocket broadcast
    store: Arc<Store>,                      // SQLite persistence
    catalog_provider: Arc<NeboCatalogProvider>,
    surfaces: RwLock<HashMap<String, SurfaceState>>,
    pending_actions: RwLock<HashSet<String>>,  // deduplication
}
```

Key operations:
- `create_surface()` → persist + broadcast CreateSurface
- `update_components()` → normalize + persist + broadcast UpdateComponents
- `update_data_model(surface_id, path, value)` → merge at JSON Pointer + persist + broadcast
- `get_agent_replay_messages()` → build replay chain for reconnect hydration
- `try_begin_action()` / `end_action()` → deduplication (prevents double-click)

### Action Dispatcher (`crates/server/src/a2ui_actions.rs`)

Routes actions by `action_type`:

| Type | Behavior |
|------|----------|
| `mcp_call` | Call MCP tool → inject result into data model at `update_path` |
| `navigate` | Switch active view → create new surface if needed |
| `update_data` | Direct data model update (no LLM, no MCP) |
| `agent` (default) | Fall through to LLM chat dispatch |

### API Endpoints

| Endpoint | Purpose |
|----------|---------|
| `GET /agents/{id}/surfaces` | Replay messages (auto-creates default surface if needed) |
| `GET /agents/{id}/nav` | Navigation config from views.json |
| `GET /agents/{id}/theme.css` | Agent-specific CSS theme |

---

## Frontend Flow

### Routes

- **`/workspaces`** — Multi-agent dashboard with left sidebar, center A2Surface, right chat panel
- **`/workspace/[agentId]`** — Single-agent mode (pop-out window target), no sidebar

### Data Loading

```
onMount() → api.listAgents()
  → Promise.all(agents.map(a => api.getAgent(a.id)))
    → JSON.parse(views) → transformViewsConfig()
      → A2UIViewsConfig { _nav, [viewId]: A2UIView }
```

### Reactive Data Updates

```
WebSocket 'a2ui_update_data_model' → CustomEvent 'nebo:a2ui_data'
  → merge at JSON Pointer path into viewData (immutable)
    → $derived reactiveView = { ...currentView, data: viewData }
      → A2Surface re-renders
```

### Action Dispatch

```
Button click → handleAction(name, payload)
  → ws.send('a2ui_action', { surfaceId: "agent:{id}:{viewId}", name, context })
```

### Chat Integration

Parallel to A2UI — chat sidebar uses session key `agent:{agentId}:web`:
- `nebo:chat_stream` → streaming content
- `nebo:chat_message` → finalized message
- `nebo:chat_complete` → done signal

---

## End-to-End Lifecycle

### 1. Surface Creation
1. Frontend fetches agent → gets `views.json`
2. Backend auto-creates default surface on first `GET /agents/{id}/surfaces`
3. Surface persisted to DB, replay messages broadcast

### 2. User Interaction
1. User clicks button → `a2ui_action` sent via WebSocket
2. Backend resolves action binding from views.json or inline context
3. Deterministic actions (mcp_call, navigate, update_data) execute immediately
4. LLM actions fall through to chat runner → stream response

### 3. Reconnect/Refresh
1. Client sends `a2ui_init`
2. Backend replays CreateSurface + UpdateComponents + UpdateDataModel for all active surfaces
3. Client re-hydrates without flash

### 4. Persistence
- Component trees and data models persisted on every update
- Window geometry stored for restore
- Surfaces deactivated (not deleted) on close

---

## Key Files

| File | Role |
|------|------|
| `crates/db/migrations/0078_a2ui_surfaces.sql` | Schema |
| `crates/db/src/queries/a2ui_surfaces.rs` | DB queries |
| `crates/server/src/a2ui.rs` | A2UIManager + NeboCatalogProvider |
| `crates/server/src/a2ui_actions.rs` | Action routing |
| `crates/server/src/handlers/ws.rs:463-595` | WebSocket handlers |
| `crates/server/src/handlers/agents.rs:1992` | Surfaces API endpoint |
| `app/src/routes/workspaces/+page.svelte` | Multi-agent workspace |
| `app/src/routes/workspace/[agentId]/+page.svelte` | Single-agent workspace |
| `app/src/lib/a2ui/A2Surface.svelte` | Component renderer |
| `app/src/lib/a2ui/A2Node.svelte` | Recursive node renderer |
| `app/src/lib/a2ui/transform.ts` | views.json → A2UIViewsConfig |
| `app/src/lib/a2ui/resolve.ts` | Data binding resolution |
| `app/src/lib/a2ui/types.ts` | TypeScript type definitions |
| `docs/sme/A2UI_PROTOCOL.md` | Protocol specification |
| `docs/sme/A2UI_INTEGRATION.md` | Integration design doc |

---

## Design Decisions

1. **Flat adjacency list** over nested trees — enables streaming, incremental updates, simpler LLM generation
2. **Deterministic first** — actions resolve without LLM when possible (mcp_call, navigate, update_data)
3. **Replay-based hydration** — no separate "load state" API; reconnects replay the creation sequence
4. **Action deduplication** — pending set prevents double-click double-dispatch
5. **Two rendering paths** — deterministic (config-declared views) and LLM-generated (future: agents create UI on-the-fly)
6. **Chat is parallel, not embedded** — workspace has collapsible chat sidebar, separate from A2UI surfaces
