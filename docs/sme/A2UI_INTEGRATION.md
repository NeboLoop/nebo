# A2UI Integration Architecture for Nebo

> Last updated: 2026-05-18
> Status: **Superseded.** Apps build their own frontends using `@neboai/app-sdk`. The backend A2UI surface system (`a2ui.rs`, `a2ui_actions.rs`) still exists for agent-pushed dynamic UIs. The declarative `views.json` path, `a2ui_bindings.rs`, and `app/src/lib/a2ui/` frontend renderer have been removed.
>
> For the current app approach, see `docs/publishers-guide/apps.md`.
>
> Protocol target: A2UI v0.9
> Dependencies: `a2ui-rs` (MIT)

---

## 1. Executive Summary

Nebo becomes a first-class A2UI host — a desktop AI operating system where **agents are apps**. Each agent in the sidebar can declare A2UI views, turning it from a chat-only persona into a full interactive application with its own interface. Two rendering paths: **deterministic** (agent config declares views, MCP tools provide data, no LLM) and **LLM-generated** (agent creates novel surfaces on the fly via tools). The Lit renderer runs in Tauri WebViews.

### Core Design Decision: Agents Own Interfaces

Everything with a UI is an agent. The sidebar is the app launcher.

```
┌─────────────────────────────────────────┐
│ Agent = App                             │
│                                         │
│  AGENT.md (personality, instructions)   │
│  + skills (data bindings via MCP tools) │
│  + a2ui views (declared in agent.json)  │
│  + session + memory (scoped per agent)  │
│  + automations (proactive UI updates)   │
└─────────────────────────────────────────┘
```

- Agents WITHOUT views → chat-only (current behavior, equally first-class)
- Agents WITH views → full apps (interface + optional collapsible chat sidebar)
- Integrations/skills are data pipes — they don't own UIs
- Users see "their team" in the sidebar, not "tools" or "plugins"

### Surface Placement

```
Panel mode (surface: "panel") — inline in main window:
┌──────────┬──────────────────┬─────────────────────────┐
│ Agents   │ Chat             │ A2UI Panel              │
│ sidebar  │                  │ (tabbed for multiple)   │
│          │  messages         │                         │
│          │  tool results     │  [Surface content]      │
│          │                  │                         │
│          │     [Reply...]   │                    [×]  │
└──────────┴──────────────────┴─────────────────────────┘

Window mode (surface: "window") — own Tauri window:
┌───────────────────────────────────────────────────┐
│  CRM Dashboard                          [_][□][×] │
├──────────────┬────────────────────────────────────┤
│ 💬 Chat      │                                    │
│ (optional,   │        A2UI Surface                │
│  collapsible)│        (full app view)             │
│              │                                    │
│  [Reply...]  │                                    │
└──────────────┴────────────────────────────────────┘
```

Each app window gets a collapsible per-agent chat sidebar. This chat uses the
agent's own session key (`agent:{id}:{view}`) so context stays scoped. Users can
ask questions about what they're seeing; the agent responds in chat AND can
update the surface simultaneously.

### Key Differentiators Over OpenClaw

- **v0.9 from day one** (they're locked to v0.8 with explicit rejection of v0.9)
- **Agents are apps** (they have tools with no identity or persistence)
- **Deterministic rendering path** (they require LLM for every render)
- **Per-app chat sidebar** (they have separate floating panels, no co-rendering)
- **Multiple apps simultaneously** (they allow one canvas at a time)
- **Structured tool calls for actions** (they use plaintext chat messages)
- **No style monkey-patching** (they mutate Lit component styles at runtime)
- **Surface state persistence** (they have no saved layouts)

---

## 2. Message Flow Diagram

### 2.1 LLM Path (Novel UI Generation)

```
User types request in chat
  → WebSocket /ws → ws.rs handler
    → chat_dispatch.rs → run_chat()
      → Runner.run() → run_loop()
        → Provider streams response with tool_calls
          → Tool: a2ui(resource: surface, action: create, ...)
            → A2UIManager.create_surface()
              → validate via a2ui-validation
              → serialize via a2ui-core
              → ClientHub.broadcast({ type: "a2ui_message", ... })
                → Frontend WS receives
                  → A2UISurface.svelte
                    → processor.processMessage(msg)
                      → Lit renderer updates DOM

User clicks button in A2UI surface
  → Lit fires a2uiaction CustomEvent
    → A2UISurface.svelte captures event
      → Builds v0.9 ActionMessage
        → WS send({ type: "a2ui_action", ... })
          → ws.rs routes to A2UIManager
            → Injects action as tool_result into runner context
              → Agent processes and may respond with more A2UI messages
```

### 2.2 Deterministic Path (Pre-Composed Views)

```
User clicks agent icon / navigation item
  → Frontend sends: WS { type: "a2ui_open", agent_id, view_id }
    → ws.rs routes to A2UIManager.open_view()
      → Load agent views.json → find ViewDeclaration
        → For each data_binding in view:
          → MCP Bridge.call_tool(integration_id, tool_name, params)
            → Direct JSON-RPC call (NO LLM)
              → Response JSON stored at data model path
        → Build createSurface message (a2ui-core builder)
        → Build updateComponents message (from manifest layout)
        → Build updateDataModel message (from MCP responses)
        → ClientHub.broadcast() all three messages
          → Frontend renders immediately

User interaction:
  → Action event → ws.rs
    → A2UIManager resolves action binding from manifest
      → If bound to MCP tool: call_tool() directly, push updateDataModel
      → If bound to agent: inject into runner for LLM processing
```

### 2.3 Hybrid Path (Deterministic Layout + LLM Fallback)

```
Agent views.json declares views with layouts and data bindings
  → Deterministic path handles known views instantly
  → User makes ad hoc request within the agent context
    → LLM receives the current surface state as context
      → Agent can push updateComponents/updateDataModel
        → Renderer applies incremental updates
```

---

## 3. Crate Integration Map

### 3.1 Workspace Changes

```
Cargo.toml (workspace)
  [workspace.dependencies]
+ a2ui-types = { path = "crates/a2ui/a2ui-types" }
+ a2ui-core = { path = "crates/a2ui/a2ui-core" }
+ a2ui-validation = { path = "crates/a2ui/a2ui-validation" }

  [workspace.members]
+ "crates/a2ui/a2ui-types",
+ "crates/a2ui/a2ui-core",
+ "crates/a2ui/a2ui-validation",
```

### 3.2 Crate Dependency Graph

```
crates/a2ui/          # git subtree or submodule of applegrew/a2ui-rs
  a2ui-types/         # Pure serde models (no async, no logic)
  a2ui-core/          # Traits, builders, validation, prompt generation
  a2ui-validation/    # JSON Schema validation

crates/server/        # MODIFIED — new module: handlers/a2ui.rs
  Cargo.toml += a2ui-core, a2ui-types

crates/tools/         # MODIFIED — new A2UI domain tool
  Cargo.toml += a2ui-core, a2ui-types

crates/agent/         # MODIFIED — agent config a2ui views
  Cargo.toml += a2ui-types (for ViewDeclaration types)

crates/db/            # MODIFIED — new migration + queries
  (no new deps, just SQL)
```

### 3.3 New Modules Within Existing Crates

```
crates/server/src/
  handlers/
    a2ui.rs           # NEW: A2UI REST + WS handlers
  a2ui/
    mod.rs            # NEW: pub use manager, transport, catalog
    manager.rs        # NEW: A2UIManager — surface lifecycle, view rendering
    transport.rs      # NEW: impl ClientTransport for WebSocket sender
    catalog.rs        # NEW: impl CatalogProvider (basic + nebo catalogs)
    prompt.rs         # NEW: A2UI prompt injection for agent system prompt

crates/tools/src/
  a2ui_tool.rs        # NEW: STRAP domain tool for LLM path

crates/agent/src/
  a2ui_views.rs       # NEW: ViewDeclaration, DataBinding, ActionBinding types
                      # A2UI config parsed from agent.json

crates/db/
  migrations/
    0078_a2ui_surfaces.sql  # NEW
  src/queries/
    a2ui.rs           # NEW: surface state CRUD
```

### 3.4 Integration with a2ui-rs: Subtree (Recommended)

**Decision: `git subtree` into `crates/a2ui/`**

Rationale:
- Not a git submodule (those are fragile with Tauri builds and CI)
- Not a crates.io dependency (crate is too new, may need patches)
- Not a fork (we want to contribute upstream, not diverge)
- Subtree keeps the code in our repo, allows local patches, and supports clean upstream pulls

```bash
git subtree add --prefix crates/a2ui https://github.com/applegrew/a2ui-rs.git main --squash
# Later, to pull upstream:
git subtree pull --prefix crates/a2ui https://github.com/applegrew/a2ui-rs.git main --squash
```

---

## 4. Agent A2UI Configuration

### 4.1 views.json — Separate from agent.json

A2UI configuration lives in its own file, separate from agent identity.

```
agents/chief-of-staff/
  AGENT.md              # personality, instructions (who the agent IS)
  agent.json            # model, skills, automations (how the agent BEHAVES)
  views.json            # A2UI config, screens, bindings (what the agent LOOKS LIKE)
  ui/
    dashboard.a2ui.json # component layouts
    inbox.a2ui.json
    calendar.a2ui.json
```

**Rationale:**
- `agent.json` = identity. `views.json` = interface. Neither pollutes the other.
- Chat-only agent: no `views.json` — clean.
- Agent with 5 screens: rich `views.json` — also clean.
- The agent loader checks for `views.json` — if present, the agent has a workspace.
  If absent, chat-only. One file existence check determines the sidebar indicator.
- Skills can ship their own `views.json` that agents inherit by referencing the skill,
  without the skill author needing to understand agent configuration.

```json
{
  "catalog": "https://a2ui.org/specification/v0_9/basic_catalog.json",
  "theme": {
    "primaryColor": "#14b8a6",
    "agentDisplayName": "CRM Agent"
  },
  "views": [
      {
        "id": "dashboard",
        "title": "CRM Dashboard",
        "surface": "window",
        "size": { "width": 1200, "height": 800 },
        "persist_layout": true,
        "layout": "ui/dashboard.a2ui.json",
        "data_bindings": [
          {
            "path": "/contacts",
            "mcp_tool": "crm__list_contacts",
            "params": { "limit": 50 },
            "refresh": "on_open"
          },
          {
            "path": "/deals",
            "mcp_tool": "crm__list_deals",
            "params": { "status": "open" },
            "refresh": "30s"
          }
        ],
        "action_bindings": [
          {
            "action": "refresh_contacts",
            "mcp_tool": "crm__list_contacts",
            "params": { "limit": 50 },
            "update_path": "/contacts"
          },
          {
            "action": "open_deal",
            "type": "navigate",
            "view": "deal_detail",
            "params_from_context": { "deal_id": "id" }
          },
          {
            "action": "ask_agent",
            "type": "agent",
            "prompt_template": "The user wants help with: {{context.query}}"
          }
        ]
      },
      {
        "id": "deal_detail",
        "title": "Deal: {{/deal/name}}",
        "surface": "panel",
        "layout": "ui/deal_detail.a2ui.json",
        "data_bindings": [
          {
            "path": "/deal",
            "mcp_tool": "crm__get_deal",
            "params_from_nav": { "id": "deal_id" },
            "refresh": "on_open"
          }
        ]
      }
    ],
    "variants": {
      "contacts": {
        "default": "table",
        "options": ["table", "kanban", "list"],
        "layout_files": {
          "table": "ui/contacts_table.a2ui.json",
          "kanban": "ui/contacts_kanban.a2ui.json",
          "list": "ui/contacts_list.a2ui.json"
        }
      }
    }
  }
}
```

A2UI layout files (`.a2ui.json`) live in the `ui/` subdirectory alongside the
agent's AGENT.md and views.json. When agents are installed from the NeboLoop
marketplace, these files are included in the agent's archive (the `ui/`
directory is allowed in extraction with a 5MB per-file limit).

Skills can also ship a `views.json` — when an agent references that skill, the
agent loader merges the skill's views into the agent's view set. This lets skill
authors provide default UIs that any agent can adopt.

### 4.2 Sidebar Behavior

```
Agents                    Clicking an agent:
┌──────────────────┐
│ 🟣 Assistant     │  → Opens chat (no a2ui field)
│ 🔵 Chief of Staff│  → Opens app window with dashboard + chat sidebar
│ 🟢 email-categ.  │  → Opens app window with inbox view + chat sidebar
│ 🟡 math-tutor    │  → Opens chat (no a2ui field)
│ 🔴 Researcher    │  → Opens chat (no a2ui field)
└──────────────────┘
```

Agents with `a2ui.views` get a visual indicator (icon badge, different click
behavior, or a secondary "Open App" button). The agent list does NOT split into
"chat agents" vs "app agents" — they are all agents, some just have interfaces.

### 4.4 Rust Types (crates/agent/src/a2ui_views.rs)

```rust
/// Parsed from views.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2UIConfig {
    pub catalog: String,
    pub theme: Option<serde_json::Value>,
    pub views: Vec<ViewDeclaration>,
    #[serde(default)]
    pub variants: HashMap<String, ViewVariant>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewDeclaration {
    pub id: String,
    pub title: String,
    pub surface: SurfaceType,  // window, panel, overlay
    pub size: Option<ViewSize>,
    #[serde(default)]
    pub persist_layout: bool,
    pub layout: String,  // path to .a2ui.json file in ui/ dir
    #[serde(default)]
    pub data_bindings: Vec<DataBinding>,
    #[serde(default)]
    pub action_bindings: Vec<ActionBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceType {
    Window,
    Panel,
    Overlay,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewSize {
    pub width: f64,
    pub height: f64,
    pub min_width: Option<f64>,
    pub min_height: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataBinding {
    pub path: String,           // JSON Pointer into data model
    pub mcp_tool: String,       // tool name (e.g., "crm__list_contacts")
    #[serde(default)]
    pub params: serde_json::Value,
    #[serde(default)]
    pub params_from_nav: HashMap<String, String>,
    pub refresh: RefreshPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshPolicy {
    OnOpen,
    Manual,
    /// Interval-based refresh. Value is a duration string like "30s", "5m".
    #[serde(untagged)]
    Interval(String),
}
// NOTE: The untagged variant inside a tagged enum is tricky with serde.
// If it causes issues, flatten to: OnOpen, Manual, Interval30s, Interval1m, Interval5m
// as concrete variants — simpler and avoids serde edge cases entirely.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionBinding {
    pub action: String,         // A2UI action name
    #[serde(rename = "type")]
    pub action_type: ActionType,
    // Fields depend on action_type
    pub mcp_tool: Option<String>,
    pub params: Option<serde_json::Value>,
    pub update_path: Option<String>,
    pub view: Option<String>,
    pub params_from_context: Option<HashMap<String, String>>,
    pub prompt_template: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionType {
    McpCall,
    Navigate,
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewVariant {
    pub default: String,
    pub options: Vec<String>,
    pub layout_files: HashMap<String, String>,
}
```

---

## 5. Multi-Window Architecture

### 5.1 Window Lifecycle

```
Agent installed/configured with a2ui.views
  → ViewDeclarations stored in agent.json
  → Frontend shows agent in sidebar with app indicator

User clicks agent with views
  → Frontend: ws.send({ type: "a2ui_open", agent_id, view_id })
  → Backend: A2UIManager.open_view()
    → Load agent config, resolve data bindings, build A2UI messages
    → Store surface state in DB (a2ui_surfaces table)
    → hub.broadcast({ type: "a2ui_window_open", agent_id, view_id, url, size, title })
  → Frontend receives a2ui_window_open event
    → If Tauri: window.__TAURI__.window.WebviewWindow.create({ label, url, ... })
    → If browser: window.open(url) or inline panel
  → New window loads SvelteKit route: /agent/{agentId}/{viewId}
    → Connects WS, subscribes to a2ui_message filtered by surface_id
    → Initializes Lit renderer + per-agent chat sidebar
    → Receives initial A2UI messages → renders

User closes window
  → Window beforeunload → ws.send({ type: "a2ui_close", agent_id, view_id })
  → Backend: A2UIManager.close_surface()
    → Save window position/size to DB
    → Clean up surface state
```

### 5.2 WebSocket Multiplexing Strategy

**Same WS connection, surface_id-based filtering.**

Each Tauri window connects to the same `ws://localhost:27895/ws` endpoint. Messages include `surface_id` (format: `agent:{agent_id}:{view_id}`). Each window's A2UI component subscribes to messages matching its surface_id.

```typescript
// In A2UISurface.svelte
const surfaceId = `${agentId}:${viewId}`;
const unsub = wsClient.on('a2ui_message', (data) => {
    if (data.surface_id === surfaceId) {
        processor.processMessage(data.message);
    }
});
```

This avoids multiple WS connections while keeping routing efficient. The ClientHub broadcast is acceptable because:
- A2UI messages are small (< 10KB typically)
- Number of concurrent surfaces is low (< 10)
- Client-side filtering is O(1) string comparison

### 5.3 Tauri Window Creation (src-tauri/src/main.rs)

```rust
fn open_agent_window(
    app: &tauri::AppHandle,
    agent_id: &str,
    view_id: &str,
    title: &str,
    width: f64,
    height: f64,
) -> Result<(), tauri::Error> {
    let label = format!("agent_{}_{}", agent_id, view_id);
    let url = format!("http://localhost:27895/agent/{}/{}", agent_id, view_id);

    tauri::WebviewWindowBuilder::new(
        app,
        &label,
        tauri::WebviewUrl::External(url.parse().unwrap()),
    )
    .title(title)
    .inner_size(width, height)
    .min_inner_size(400.0, 300.0)
    .on_navigation(|url| {
        let host = url.host_str().unwrap_or("");
        host == "localhost" || host == "127.0.0.1"
    })
    .build()?;

    Ok(())
}
```

---

## 6. Surface State Persistence

### 6.1 SQLite Migration (0078_a2ui_surfaces.sql)

```sql
-- +goose Up
CREATE TABLE IF NOT EXISTS a2ui_surfaces (
    id TEXT PRIMARY KEY,                    -- "agent:{agent_id}:{view_id}"
    agent_id TEXT NOT NULL,
    view_id TEXT NOT NULL,
    surface_type TEXT NOT NULL DEFAULT 'window',  -- window, panel, overlay
    title TEXT NOT NULL DEFAULT '',
    is_open INTEGER NOT NULL DEFAULT 0,
    -- Window geometry (persisted on close)
    window_x REAL,
    window_y REAL,
    window_width REAL,
    window_height REAL,
    -- A2UI state
    data_model TEXT,                        -- JSON: last known data model
    active_variant TEXT,                    -- Current view variant selection
    -- Metadata
    last_opened_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(agent_id, view_id)
);

CREATE INDEX IF NOT EXISTS idx_a2ui_surfaces_agent ON a2ui_surfaces(agent_id);
CREATE INDEX IF NOT EXISTS idx_a2ui_surfaces_open ON a2ui_surfaces(is_open) WHERE is_open = 1;
```

### 6.2 Persistence Behavior

| Event | What's Saved | When Restored |
|-------|-------------|---------------|
| Window moved/resized | x, y, width, height | Next window open |
| View variant changed | active_variant | Next window open |
| Data model updated | data_model (JSON) | Next open if persist_layout=true |
| Window closed | is_open=0, geometry | — |
| App restart | All surfaces with is_open=1 | Surfaces NOT auto-reopened (user triggers) |

---

## 7. Custom Nebo Catalog

### 7.1 Catalog Definition

Catalog ID: `https://neboloop.com/a2ui/nebo_catalog/v1`

Extends the basic catalog with 6 Nebo-specific components:

| Component | Purpose | Props |
|-----------|---------|-------|
| `MetricTile` | KPI display (number + label + trend) | `value: DynamicNumber`, `label: DynamicString`, `trend: DynamicString`, `icon: DynamicString` |
| `KanbanBoard` | Drag-and-drop columns | `columns: ChildList`, `onMove: Action` |
| `KanbanColumn` | Single kanban column | `title: DynamicString`, `cards: ChildList`, `count: DynamicNumber` |
| `PipelineView` | Sales/deal pipeline stages | `stages: ChildList`, `onStageChange: Action` |
| `ActivityFeed` | Time-ordered event stream | `items: ChildList`, `onLoadMore: Action` |
| `SparklineChart` | Inline mini chart | `data: DynamicStringList`, `color: DynamicString`, `height: DynamicNumber` |

### 7.2 Implementation Strategy

Custom components are Lit web components built alongside the SvelteKit app.

**Current file structure (Phase 1 — basic catalog implemented):**

```
app/src/lib/components/a2ui/
  a2ui-markdown-provider.ts     # Markdown rendering for A2UI Text components
  nebo-action-context.ts        # Lit context for button pending state
  nebo-surface.ts               # NeboSurfaceElement (shadow DOM + style injection + ContextProvider)
  A2UISurfacePanel.svelte       # Svelte wrapper: surface model → web component + action routing
  A2UIWorkspaceNav.svelte       # Tab navigation from views.json _nav
  nebo-catalog/
    index.ts                    # neboCatalog registration (18 components)
    NeboButton.ts               # <nebo-a2ui-button> (pending state via ContextConsumer)
    NeboText.ts                 # <nebo-a2ui-text>
    NeboColumn.ts               # <nebo-a2ui-column>
    NeboRow.ts                  # <nebo-a2ui-row>
    NeboList.ts                 # <nebo-a2ui-list>
    NeboTabs.ts                 # <nebo-a2ui-tabs>
    NeboCard.ts                 # <nebo-a2ui-card>
    NeboModal.ts                # <nebo-a2ui-modal>
    NeboTextField.ts            # <nebo-a2ui-textfield>
    NeboChoicePicker.ts         # <nebo-a2ui-choicepicker>
    NeboCheckBox.ts             # <nebo-a2ui-checkbox>
    NeboSlider.ts               # <nebo-a2ui-slider>
    NeboDivider.ts              # <nebo-a2ui-divider>
    NeboImage.ts                # <nebo-a2ui-image>
    NeboIcon.ts                 # <nebo-a2ui-icon>
    NeboDateTimeInput.ts        # <nebo-a2ui-datetimeinput>
    NeboAudioPlayer.ts          # <nebo-a2ui-audioplayer>
    NeboVideo.ts                # <nebo-a2ui-video>
```

**Future (Phase 5 — custom Nebo catalog components, not yet built):**

```
app/src/lib/components/a2ui/nebo-catalog/
    MetricTile.ts         # <nebo-metric-tile>
    KanbanBoard.ts        # <nebo-kanban-board>
    KanbanColumn.ts       # <nebo-kanban-column>
    PipelineView.ts       # <nebo-pipeline-view>
    ActivityFeed.ts       # <nebo-activity-feed>
    SparklineChart.ts     # <nebo-sparkline-chart>
```

Each follows the A2UI Lit component pattern:
```typescript
@customElement("nebo-metric-tile")
export class NeboMetricTile extends A2uiLitElement<typeof MetricTileApi> {
    protected createController() {
        return new A2uiController(this, MetricTileApi);
    }
    render() { /* ... */ }
}
```

Registered as a custom catalog alongside basic:
```typescript
import { Catalog } from "@a2ui/web_core/v0_9";
import { basicCatalog } from "@a2ui/lit/v0_9";

export const neboCatalog = new Catalog(
    "https://neboloop.com/a2ui/nebo_catalog/v1",
    [NeboMetricTile, NeboKanbanBoard, ...],
    [],
);
```

### 7.3 Rust-Side CatalogProvider

```rust
pub struct NeboCatalogProvider {
    basic_catalog: Catalog,
    basic_schema: serde_json::Value,
    nebo_catalog: Catalog,
    nebo_schema: serde_json::Value,
}

impl CatalogProvider for NeboCatalogProvider {
    fn available_catalogs(&self) -> Vec<CatalogInfo> {
        vec![
            CatalogInfo {
                catalog_id: CatalogId::new("https://a2ui.org/specification/v0_9/basic_catalog.json"),
                description: Some("A2UI Basic Catalog v0.9".into()),
            },
            CatalogInfo {
                catalog_id: CatalogId::new("https://neboloop.com/a2ui/nebo_catalog/v1"),
                description: Some("Nebo Extended Catalog".into()),
            },
        ]
    }

    fn get_catalog(&self, id: &CatalogId) -> Option<Catalog> {
        match id.as_str() {
            "https://a2ui.org/specification/v0_9/basic_catalog.json" => Some(self.basic_catalog.clone()),
            "https://neboloop.com/a2ui/nebo_catalog/v1" => Some(self.nebo_catalog.clone()),
            _ => None,
        }
    }

    fn get_catalog_schema(&self, id: &CatalogId) -> Option<serde_json::Value> {
        match id.as_str() {
            "https://a2ui.org/specification/v0_9/basic_catalog.json" => Some(self.basic_schema.clone()),
            "https://neboloop.com/a2ui/nebo_catalog/v1" => Some(self.nebo_schema.clone()),
            _ => None,
        }
    }
}
```

---

## 8. Security Model

### 8.1 How A2UI Fits Nebo's Existing Security Layers

| Layer | A2UI Impact |
|-------|-------------|
| **Safeguard** | No change. A2UI messages are declarative JSON — no shell, no file system, no sudo. |
| **Policy** | New `a2ui` tool requires no approval (pure rendering, no side effects). Actions that trigger MCP calls go through normal MCP approval. |
| **Origin** | New `Origin::AgentUI` for events from agent views. Restricted to: a2ui tools, declared MCP tools only. |
| **Entity permissions** | Agent views inherit the agent's permission scope. An agent with `permissions: ["ui:window"]` can open windows. |
| **Resource grants** | No new physical resources needed (A2UI is virtual). |
| **Approval flow** | Deterministic path: no approval (pre-declared in views.json). LLM path: standard tool approval for a2ui.create_surface. |
| **Sandbox** | Agent UI content runs in Tauri WebView with navigation guard (localhost only). |

### 8.2 Specific Security Controls

**Surface creation permissions:**
- Agents can only create surfaces declared in their views.json
- LLM can create ad hoc surfaces (subject to tool approval)
- Surface IDs are namespaced: `agent:{agent_id}:{view_id}` — an agent cannot create a surface in another agent's namespace

**Catalog restrictions:**
- Agents declare which catalogs they use in views.json
- The CatalogProvider only serves catalogs that the active agent has declared
- Prevents agent A from using agent B's custom catalog components

**Action routing:**
- Actions from an agent's surface can only trigger:
  - MCP tools declared in that agent's `action_bindings`
  - Navigation within that agent's declared views
  - Agent processing (goes through normal chat pipeline with that agent's session)
- Actions CANNOT trigger shell commands, file operations, or cross-agent tools

**No arbitrary code execution:**
- A2UI is declarative JSON — never executed as code
- `FunctionCall` in A2UI is for client-side built-in functions only (formatString, required, email validators)
- No `eval()`, no `<script>` injection, no dynamic imports
- The Lit renderer sanitizes all text content

---

## 9. A2UI STRAP Tool Definition

### 9.1 Tool Schema

```json
{
  "name": "a2ui",
  "description": "Manage A2UI surfaces for rendering interactive user interfaces. Use this tool to create visual applications, dashboards, and interactive views.",
  "parameters": {
    "type": "object",
    "required": ["resource", "action"],
    "properties": {
      "resource": {
        "type": "string",
        "enum": ["surface"]
      },
      "action": {
        "type": "string",
        "enum": ["create", "update_components", "update_data", "delete"]
      },
      "surface_id": {
        "type": "string",
        "description": "Unique surface identifier"
      },
      "catalog_id": {
        "type": "string",
        "description": "Catalog URI (default: basic catalog)"
      },
      "components": {
        "type": "array",
        "description": "A2UI v0.9 component adjacency list"
      },
      "path": {
        "type": "string",
        "description": "JSON Pointer path for data model updates"
      },
      "value": {
        "description": "Value for data model update"
      },
      "theme": {
        "type": "object",
        "description": "Theme configuration"
      }
    }
  }
}
```

### 9.2 Tool Execution

```rust
pub struct A2UITool {
    manager: Arc<A2UIManager>,
}

impl DynTool for A2UITool {
    fn name(&self) -> &str { "a2ui" }

    fn requires_approval(&self) -> bool { false }  // Rendering is side-effect-free

    async fn execute_dyn(&self, ctx: &ToolContext, input: serde_json::Value) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("");
        let surface_id = input["surface_id"].as_str().unwrap_or("main");

        match action {
            "create" => {
                let catalog_id = input["catalog_id"].as_str()
                    .unwrap_or("https://a2ui.org/specification/v0_9/basic_catalog.json");
                let theme = input.get("theme").cloned();

                // Build message using a2ui-core builder
                let msg = CreateSurfaceBuilder::new(surface_id, catalog_id)
                    .theme(theme.unwrap_or_default())
                    .build();

                // Validate against catalog schema
                let schema = self.manager.catalog_provider.get_catalog_schema(&CatalogId::new(catalog_id));
                if let Some(schema) = schema {
                    validate_message(&serde_json::to_value(&msg)?, &schema, surface_id)?;
                }

                // Broadcast to frontend
                self.manager.broadcast_a2ui_message(surface_id, &msg).await?;

                ToolResult::text(format!("Created A2UI surface '{}'", surface_id))
            }
            "update_components" => {
                let components = input["components"].as_array()
                    .ok_or("components array required")?;

                let msg = UpdateComponentsBuilder::new(surface_id)
                    .components(components.clone())
                    .build();

                self.manager.broadcast_a2ui_message(surface_id, &msg).await?;

                ToolResult::text(format!("Updated {} components on '{}'", components.len(), surface_id))
            }
            "update_data" => {
                let path = input["path"].as_str();
                let value = input.get("value").cloned();

                let mut builder = UpdateDataModelBuilder::new(surface_id);
                if let Some(p) = path { builder = builder.path(p); }
                if let Some(v) = value { builder = builder.value(v); }
                let msg = builder.build();

                self.manager.broadcast_a2ui_message(surface_id, &msg).await?;

                ToolResult::text("Data model updated")
            }
            "delete" => {
                let msg = delete_surface_v09(surface_id);
                self.manager.broadcast_a2ui_message(surface_id, &msg).await?;

                ToolResult::text(format!("Deleted surface '{}'", surface_id))
            }
            _ => ToolResult::error(format!("Unknown action: {}", action)),
        }
    }
}
```

---

## 10. Frontend Architecture

### 10.1 SvelteKit Routes (Actual)

```
app/src/routes/
  (app)/+layout.svelte              # Main layout — subscribes to a2ui_message + a2ui_action_status WS events
  (workspace)/workspace/[agentId]/
    +page.svelte                    # Pop-out workspace window — same WS subscriptions
```

### 10.2 Architecture (Actual Implementation)

The frontend uses three layers:

1. **Svelte store** (`app/src/lib/stores/a2ui.ts`) — wraps `MessageProcessor`, tracks surfaces + pending actions
2. **Svelte component** (`A2UISurfacePanel.svelte`) — bridges store to web component, routes actions
3. **Lit web components** (`NeboSurfaceElement` + 18 catalog components) — renders A2UI component tree

```
a2ui store (MessageProcessor) ←→ A2UISurfacePanel.svelte ←→ <nebo-a2ui-surface>
                                        ↕                          ↕ (shadow DOM)
                                  WS client                   Lit context
                                  (a2ui_action)            (NeboActionState)
                                        ↕                          ↕
                                    Backend                  <nebo-a2ui-button>
                                  (ws.rs handler)          (pending spinner)
```

**Key components:**

- `a2ui store` — `MessageProcessor` + `pendingActions: Set<string>` + `handleActionStatus()`
- `A2UISurfacePanel.svelte` — subscribes to surface creation, routes actions to WS, bridges pending state to Lit
- `NeboSurfaceElement` — `A2uiSurface` subclass with shadow DOM style injection + `ContextProvider` for action state
- `NeboButtonElement` — `ContextConsumer` for pending state, shows DaisyUI spinner while action processes
- `nebo-action-context.ts` — Lit context definition (`NeboActionState` with `onComplete` callback registration)

**Action flow:**

1. User clicks button → `NeboButton` sets `_pending = true`, shows spinner, calls `props.action()`
2. `A2UISurfacePanel` receives action event → checks `isActionPending()` → sends `a2ui_action` via WS
3. Backend `ws.rs` → tries `a2ui_actions::dispatch()` for deterministic handling
4. If not deterministic → `try_begin_action()` (dedup) → `run_chat()` → `end_action()`
5. Backend broadcasts `a2ui_action_status: processing` / `complete`
6. Frontend store updates `pendingActions` → `A2UISurfacePanel` calls `notifyActionComplete()` on surface element
7. `NeboSurfaceElement` fires completion listeners → `NeboButton` clears `_pending`, hides spinner

### 10.3 Main Window Integration

In `(app)/+layout.svelte`, add listeners for opening agent app windows:

```typescript
// In the onMount block:
const unsubA2UIOpen = wsClient.on('a2ui_window_open', async (data) => {
    if (window.__TAURI__) {
        const { WebviewWindow } = await import('@tauri-apps/api/window');
        new WebviewWindow(`agent_${data.agent_id}_${data.view_id}`, {
            url: `/agent/${data.agent_id}/${data.view_id}`,
            title: data.title,
            width: data.size?.width ?? 1000,
            height: data.size?.height ?? 700,
        });
    } else {
        // Browser fallback: open in new tab
        window.open(`/agent/${data.agent_id}/${data.view_id}`, '_blank');
    }
});
```

---

## 11. Answers to Key Questions

### Q1: How should a2ui-rs be integrated?

**git subtree.** Not a submodule (fragile), not a fork (want upstream contribution), not crates.io (too new). Subtree allows local patches while maintaining a clean pull path from upstream.

### Q2: WS message format for A2UI messages?

Multiplexed on the existing `/ws` connection with new event types:
```json
{"type": "a2ui_message", "data": {"surface_id": "crm:dashboard", "message": {<A2UI v0.9 message>}}}
{"type": "a2ui_action", "data": {"surface_id": "crm:dashboard", "action": {<A2UI v0.9 action>}}}
{"type": "a2ui_window_open", "data": {"agent_id": "crm", "view_id": "dashboard", "title": "...", "size": {...}}}
{"type": "a2ui_window_close", "data": {"surface_id": "crm:dashboard"}}
```

No separate endpoint needed. Surface IDs serve as routing keys.

### Q3: How does the Lit renderer's API work?

- `MessageProcessor.processMessage(msg)` accepts a SINGLE v0.9 message object
- `MessageProcessor.processMessages(msgs[])` accepts an ARRAY
- No built-in JSONL streaming — caller parses individual JSON objects and feeds them one at a time
- The `<a2ui-surface>` custom element is the render target
- Actions bubble up as `a2uiaction` CustomEvents (v0.8) or via `ActionListener` callback (v0.9)

### Q4: MCP response to data model mapping?

**1:1 mapping declared in the manifest.** Each `data_binding` in the view declaration specifies:
- `mcp_tool`: which MCP tool to call
- `params`: static parameters
- `path`: where to put the response in the A2UI data model

The MCP tool response (JSON) is placed directly at the specified path via `updateDataModel`. No transformation layer needed for v1 — the MCP tool's response schema should match what the A2UI view expects. If transformation is needed later, a `transform` field can be added to the binding.

### Q5: One WS connection or per-window?

**One per window, but to the same backend.** Each Tauri window loads a separate SvelteKit page which creates its own WS connection to `ws://localhost:27895/ws`. This is simpler than multiplexing because:
- Each window has its own lifecycle (connect on open, disconnect on close)
- The backend's ClientHub already broadcasts to all subscribers
- Client-side filtering by surface_id keeps messages scoped

The cost is minimal — each WS connection is a single TCP socket on localhost.

### Q6: Offline deterministic rendering?

**Yes, with caveats.** If the agent's MCP server is local (running on the same machine), deterministic rendering works fully offline. The A2UI messages are generated by Nebo's Rust code, not by an LLM. However:
- Data bindings that call remote MCP servers (e.g., Gmail API) will fail offline
- The surface can render with cached data from the last successful fetch (stored in `a2ui_surfaces.data_model`)
- LLM fallback is obviously unavailable offline

### Q7: Catalog versioning strategy?

**Nebo bundles specific catalog versions.** The `@a2ui/lit` npm package pins the basic catalog. When A2UI moves to v1.0:
- Nebo ships both v0.9 and v1.0 catalogs
- Agents declare which version they target in their views.json
- The CatalogProvider serves the requested version
- Old agents continue working with the old catalog until updated

### Q8: Custom catalog bundling strategy?

**Bundled with Nebo's frontend build, not dynamically loaded.** Custom Nebo catalog components are Lit web components compiled into the SvelteKit app bundle. This ensures:
- No runtime dynamic imports from untrusted sources
- Type safety at build time
- Consistent versions across all windows
- No CORS or loading failures

Third-party custom catalogs (from marketplace agents) are a Phase 5+ concern and would require a secure dynamic loading mechanism.

---

## 12. Implementation Phases

### Phase 1: Core Infrastructure (Foundation) — COMPLETE

**Shipped in commit `fcb850a` (2026-04-12)**

All items implemented:

1. ✅ `git subtree add` a2ui-rs into `crates/a2ui/`
2. ✅ Add workspace members to `Cargo.toml`
3. ✅ `NeboCatalogProvider` (basic catalog, 18 components)
4. ✅ `A2UIManager` with `broadcast_a2ui_message()`, surface CRUD, action dedup (`pending_actions`)
5. ✅ `A2UITool` registered (STRAP: `a2ui(surface, create|update_components|update_data|navigate|delete|list)`)
6. ✅ `a2ui_message` + `a2ui_action` + `a2ui_action_status` WS event types
7. ✅ `pnpm add @a2ui/lit @a2ui/web_core` + 18 Nebo catalog web components
8. ✅ `A2UISurfacePanel.svelte` + `NeboSurfaceElement` (shadow DOM with style injection)
9. ✅ WS subscription for `a2ui_message` events in both main layout and workspace routes
10. ✅ DB migration `0078_a2ui_surfaces.sql`
11. ✅ Verified: agent calls a2ui tool → surface renders in workspace panel

**Additional Phase 1 deliverables (beyond original plan):**

- ✅ **Action dedup** — `A2UIManager.pending_actions: RwLock<HashSet<String>>` prevents double-click LLM dispatch. `try_begin_action()` / `end_action()` broadcast `a2ui_action_status` events.
- ✅ **Button loading state** — Lit context (`@lit/context`) with `ContextProvider` on `NeboSurfaceElement` and `ContextConsumer` on `NeboButtonElement`. Button shows DaisyUI spinner while action is pending.
- ✅ **Deterministic action dispatch** — `a2ui_actions.rs` routes `mcp_call`, `navigate`, `update_data` without LLM. Unmatched actions fall through to LLM.
- ✅ **Data binding manager** — `DataBindingManager` polls MCP tools at configured intervals, injects results into surface data model.
- ✅ **Agent theme CSS** — `theme.css` loaded per-agent, injected into shadow DOM via `MutationObserver`. Uses `media="not all"` to prevent global leakage.
- ✅ **Workspace navigation** — `A2UIWorkspaceNav.svelte` renders tabs from `views.json._nav`
- ✅ **Surface restore on reconnect** — `restore_surfaces()` replays persisted state from DB
- ✅ **Markdown in A2UI** — `a2ui-markdown-provider.ts` wraps text rendering

### Phase 2: Deterministic Rendering — COMPLETE (merged into Phase 1)

Originally planned separately, but delivered as part of Phase 1:

1. ✅ `views.json` loader in `agent_loader.rs` (parsed into `LoadedAgent.views`)
2. ✅ `DataBinding`, `ActionBinding` types in `a2ui_bindings.rs` and `a2ui_actions.rs`
3. ✅ Deterministic render: views.json → create_surface → MCP poll → updateDataModel
4. ✅ `a2ui_action` WS handler dispatches to deterministic handlers
5. ✅ Action bindings: mcp_call → bridge.call_tool() → update data model
6. ✅ Navigation between views via `A2UIManager::navigate_view()`
7. ✅ Workspace indicator in sidebar for agents with views

### Phase 3: Multi-Window + Persistence

**Goal:** Agent apps open in their own windows, state survives restarts.

1. Implement `open_agent_window()` in `src-tauri/src/main.rs`
2. Wire `a2ui_window_open` event → Tauri window creation in frontend
3. Implement surface state CRUD in `crates/db/src/queries/a2ui.rs`
4. Save window geometry on close, restore on open
5. Save data model for `persist_layout: true` views
6. Implement view variants (table/kanban/list switching)
7. **Verify:** Open agent app in window → resize → close → reopen → same size → same data

### Phase 4: First Killer Apps

**Goal:** Real-world agents demonstrating the platform.

1. Google Workspace agent (Gmail + Calendar in A2UI surfaces)
2. GitHub Dashboard agent (repos, PRs, issues)
3. Shopify Store Manager agent
4. Each: MCP server + agent manifest + views.json + A2UI layouts + deterministic rendering

### Phase 5: Custom Nebo Catalog

**Goal:** Nebo-specific components available to all agents.

1. Design and implement 6 custom Lit web components
2. Create `neboCatalog` with Zod schemas
3. Update `NeboCatalogProvider` to serve nebo catalog
4. Update `build_prompt_context_v09()` to include nebo catalog in LLM prompts
5. Build example agents using MetricTile, KanbanBoard, etc.

### Phase 6: Ecosystem

**Goal:** Nebo recognized as first-class A2UI host.

1. Submit PR to google/A2UI adding Nebo to ecosystem renderers table
2. Post in A2UI GitHub Discussions
3. Publish blog post
4. Add A2UI agent examples to NeboLoop marketplace

---

## 13. OpenClaw Comparison Table

| Aspect | OpenClaw | Nebo (Implemented) |
|--------|----------|-------------------|
| Protocol version | v0.8 (hard-locked, rejects v0.9) | v0.9 from day one |
| Rendering path | LLM-only (every render needs agent) | **Deterministic + LLM fallback** (both working) |
| Action bridge | Proprietary plaintext (`CANVAS_A2UI action=...`) | Spec-compliant v0.9 ActionMessage + deterministic dispatch |
| Action dedup | `chatSending` boolean (state-driven) | `pending_actions` HashSet + Lit context (state-driven) |
| Style customization | Runtime monkey-patching of Lit styles | Agent `theme.css` + shadow DOM isolation + MutationObserver |
| Window model | Single canvas per session | Multi-window, per-agent (panel + pop-out) |
| State persistence | None | SQLite (geometry, data model, components) |
| Catalog support | Basic only | Basic + custom Nebo catalog (18 components) |
| Renderer integration | Vendored snapshot, no version pin | npm package, semver pinned |
| Offline capability | No (requires LLM) | Yes (deterministic path + MCP polling) |
| Build system | Rolldown + manual vendor copy | pnpm + git subtree |
| Transport | Gateway RPC → WebView evaluateJS | WebSocket (same as chat) |
| Platform | iOS/macOS/Android (WebView per platform) | macOS/Windows/Linux (Tauri WebView) |

---

## 14. Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| a2ui-rs crate is immature/buggy | Medium | High | Subtree allows local patches; contribute fixes upstream |
| @a2ui/lit v0.9 API changes before stable | Medium | Medium | Pin exact npm version; Lit web components are stable |
| LLM generates invalid A2UI JSON | High | Low | Validate all messages via a2ui-validation; feed errors back for self-correction |
| Tauri WebView inconsistencies across platforms | Low | Medium | A2UI/Lit uses standard web components; test on all 3 platforms early |
| Performance with many concurrent surfaces | Low | Medium | Surface count limited by design; Lit is efficient |
| MCP tool latency on deterministic path | Medium | Medium | Cache last-known data model; show loading state |
| Custom catalog components diverge from spec | Low | High | Follow A2UI component authoring guide exactly; use Zod schemas |

---

## Appendix A: File Reference

### Research Documents
- `docs/sme/A2UI_PROTOCOL.md` — Full v0.9 protocol reference (1174 lines)
- `docs/sme/A2UI_INTEGRATION.md` — This document

### Key Nebo Files to Modify
- `Cargo.toml` — workspace members
- `crates/server/src/handlers/ws.rs` — WS message routing
- `crates/server/src/state.rs` — AppState fields
- `crates/server/src/lib.rs` — initialization
- `crates/tools/src/registry.rs` — tool registration
- `crates/agent/src/manifest.rs` — manifest schema
- `src-tauri/src/main.rs` — window management
- `app/src/routes/(app)/+layout.svelte` — WS event listeners
- `app/package.json` — @a2ui/lit dependency

### Key a2ui-rs Files
- `a2ui-core/src/traits.rs` — CatalogProvider, ClientTransport traits
- `a2ui-core/src/message.rs` — CreateSurfaceBuilder, UpdateComponentsBuilder
- `a2ui-core/src/validation.rs` — validate_message(), parse_client_message_v09()
- `a2ui-core/src/prompt.rs` — build_prompt_context_v09()
- `a2ui-types/src/v09/` — All v0.9 type definitions
