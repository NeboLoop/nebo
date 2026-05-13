# Workspace Views (`views.json`)

> Part of the [Agent](agents.md) package. Views are optional — agents without `views.json` are chat-only (or can create UIs dynamically via the `a2ui` tool during conversation).

The `views.json` file declares workspace UIs that render immediately when a user opens the agent's workspace — no LLM call required. This is for agents that have a known interface: dashboards, control panels, status displays, input forms.

```json
{
  "default": {
    "surface_type": "panel",
    "components": [
      { "id": "root", "component": "Column", "children": ["title", "metrics", "scan-btn"] },
      { "id": "title", "component": "Text", "text": "Morning Briefing", "variant": "h2" },
      { "id": "metrics", "component": "Text", "text": { "path": "/summary" } },
      { "id": "scan-btn", "component": "Button", "child": "scan-label", "variant": "primary",
        "action": { "event": { "name": "refresh", "context": {} } } },
      { "id": "scan-label", "component": "Text", "text": "Refresh" }
    ],
    "data": {
      "summary": "Loading..."
    },
    "data_bindings": [
      {
        "path": "/summary",
        "source": { "server": "slack", "tool": "get_daily_summary" },
        "interval_secs": 60
      }
    ],
    "actions": {
      "refresh": {
        "type": "mcp_call",
        "server": "slack",
        "tool": "get_daily_summary",
        "update_path": "/summary"
      }
    }
  },
  "_nav": [
    { "viewId": "default", "label": "Dashboard", "icon": "dashboard" },
    { "viewId": "settings", "label": "Settings", "icon": "settings" }
  ]
}
```

---

## Structure

The file is a map of **view ID → view definition**. The `default` view renders automatically when the user opens the agent's workspace. The special `_nav` key defines workspace navigation tabs.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `surface_type` | string | no | `"panel"` (default). Where the workspace renders. |
| `components` | array | yes | Flat component list (adjacency list — not nested). Each entry is a component object. |
| `data` | object | no | Initial data model for the view. Values referenced by data bindings in components. |
| `data_bindings` | array | no | Polling definitions — automatically fetch data from MCP tools at intervals. |
| `actions` | map | no | Deterministic action handlers — map action names to handlers that execute without LLM involvement. |

---

## Component Model

Components use a **flat adjacency list** — not nested trees. Each component has an `id` and references children by ID. This enables incremental updates and targeted changes.

### Component Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | yes | Unique component ID within the view |
| `component` | string | yes | Component type (PascalCase) from the A2UI basic catalog |
| `children` | string[] | no | IDs of child components (for layout components) |
| `child` | string | no | Single child ID (for Card, Button — shorthand for `children: [id]`) |
| `action` | object | no | Action to fire on interaction |
| `props` | object | no | Additional props (merged with top-level props) |

Plus any component-specific props at the top level (see tables below).

### Recognized Props

These fields are extracted as component props when placed at the top level of a component object:

`text`, `variant`, `justify`, `align`, `gap`, `weight`, `fit`, `url`, `label`, `size`, `name`, `wrap`, `class`, `description`, `value`, `change`, `content`, `placeholder`, `level`, `direction`, `surface_type`, `tabs`

---

## Available Components

All 18 components from the A2UI v0.9 basic catalog are supported.

### Layout

| Component | Description | Key Props |
|-----------|-------------|-----------|
| `Column` | Vertical layout | `children`, `justify`, `align`, `gap` |
| `Row` | Horizontal layout | `children`, `justify`, `align`, `gap`, `wrap` |
| `Card` | Container with border/elevation | `child` (single child ID) |
| `List` | Dynamic list (supports templates) | `children` (static IDs or template ref) |
| `Tabs` | Tabbed content sections | `tabs` (array of `{title, child}`) |
| `Modal` | Dialog overlay | `trigger` (component ID), `content` (component ID) |
| `Divider` | Visual separator | `axis` (`horizontal` or `vertical`) |

### Content

| Component | Description | Key Props |
|-----------|-------------|-----------|
| `Text` | Text display (Markdown supported) | `text`, `variant` (`h1`–`h5`, `body`, `caption`, `overline`) |
| `Icon` | Material Design icon | `name` (e.g. `check`, `settings`, `search`), `size` |
| `Image` | Image display | `url`, `description`, `fit`, `variant` |
| `AudioPlayer` | Audio playback | `url`, `description` |
| `Video` | Video playback | `url` |

### Inputs

| Component | Description | Key Props |
|-----------|-------------|-----------|
| `Button` | Clickable action trigger | `child` (Text or Icon ID), `action`, `variant` (`default`, `primary`, `borderless`) |
| `TextField` | Text input | `label`, `value`, `placeholder`, `variant` (`shortText`, `longText`, `number`, `obscured`) |
| `Slider` | Numeric range input | `value`, `max`, `min`, `label` |
| `CheckBox` | Boolean toggle | `label`, `value` |
| `ChoicePicker` | Single/multiple selection | `options` (array of `{label, value}`), `value`, `variant` (`mutuallyExclusive`, `multipleSelection`) |
| `DateTimeInput` | Date/time picker | `value` (ISO 8601), `enableDate`, `enableTime`, `label` |

### Common Props

All components accept these optional props:

| Prop | Type | Description |
|------|------|-------------|
| `weight` | number | Flex-grow weight when inside a Row or Column |
| `class` | string | Additional CSS class names |
| `accessibility` | object | Accessibility attributes (aria labels, roles) |

---

## Data Binding

Props can be either literal values or reactive bindings into the data model.

### Literal vs Bound Values

```json
// Literal — static
{ "text": "Hello world" }

// Data binding — reactive, updates when data model changes
{ "text": { "path": "/data/greeting" } }
```

### Path Resolution

Paths use **JSON Pointer** (RFC 6901) notation:

| Path | Type | Resolves from |
|------|------|---------------|
| `/user/name` | Absolute | Root data model |
| `name` | Relative | Current template scope (falls back to root) |

Relative paths are useful inside template children where each item has its own scope.

### Template Children (Dynamic Lists)

For rendering a component once per item in an array, use a template child reference instead of static IDs:

```json
{
  "default": {
    "components": [
      { "id": "root", "component": "Column", "children": { "componentId": "task-item", "path": "/tasks" } },
      { "id": "task-item", "component": "Row", "children": ["task-name", "task-status"] },
      { "id": "task-name", "component": "Text", "text": { "path": "name" } },
      { "id": "task-status", "component": "Text", "text": { "path": "status" }, "variant": "caption" }
    ],
    "data": {
      "tasks": [
        { "name": "Review proposal", "status": "pending" },
        { "name": "Send invoice", "status": "done" }
      ]
    }
  }
}
```

The `children` field accepts either:
- **Static:** `["id1", "id2"]` — fixed list of child IDs
- **Template:** `{ "componentId": "template-id", "path": "/array/path" }` — repeats component for each array element

Inside template children, data bindings use **relative paths** (e.g., `"name"`) that resolve against each array item.

---

## Data Bindings (Polling)

Data bindings poll MCP tools at regular intervals and inject results into the data model. This enables live dashboards without LLM involvement.

```json
"data_bindings": [
  {
    "path": "/metrics",
    "source": { "server": "analytics", "tool": "get_metrics" },
    "params": { "range": "7d" },
    "interval_secs": 120
  }
]
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `path` | string | yes | JSON Pointer into the data model where results are injected |
| `source.server` | string | yes | MCP server slug (e.g., `"slack"`, `"gws"`) |
| `source.tool` | string | yes | Tool name on the MCP server |
| `params` | object | no | Parameters passed to the tool call |
| `interval_secs` | number | no | Poll interval in seconds (default: 30) |

**Behavior:**
- Polling starts when the surface is created
- On repeated failures, backs off exponentially (up to 60s)
- Results are merged at the specified JSON Pointer path
- Stops when the surface is deactivated

---

## Actions

Actions define what happens when the user interacts with a component (e.g., clicks a button).

### Action Format (on components)

```json
{
  "id": "refresh-btn",
  "component": "Button",
  "child": "refresh-label",
  "action": { "event": { "name": "refresh", "context": { "scope": "all" } } }
}
```

The `action.event.name` is matched against keys in the view's `actions` map.

### Deterministic Action Handlers

Actions declared in the `actions` map execute without LLM involvement:

```json
"actions": {
  "refresh": {
    "type": "mcp_call",
    "server": "analytics",
    "tool": "get_metrics",
    "args": { "range": "7d" },
    "update_path": "/metrics"
  },
  "go_settings": {
    "type": "navigate",
    "view": "settings",
    "params": { "tab": "general" }
  },
  "toggle_mode": {
    "type": "update_data",
    "path": "/ui/dark_mode",
    "value": true
  }
}
```

| Type | Fields | Description |
|------|--------|-------------|
| `mcp_call` | `server`, `tool`, `args`, `update_path` | Calls an MCP tool and injects the result into the data model at `update_path` |
| `navigate` | `view`, `params` | Switches to a different view, optionally passing parameters as initial data |
| `update_data` | `path`, `value` | Directly updates the data model at a JSON Pointer path |

### LLM Fallthrough

Actions with `type: "agent"` (or action names not found in the `actions` map) fall through to the LLM, which processes the action using the full agent context. This lets you mix deterministic and agentic behavior in a single view.

### Action Deduplication

The system prevents double-click issues: while an action is processing, duplicate dispatches of the same action are ignored. The frontend shows a loading state on the triggering button until the action completes.

---

## Navigation (`_nav`)

The `_nav` key at the top level of `views.json` defines tabs in the workspace navigation bar.

```json
"_nav": [
  { "viewId": "default", "label": "Dashboard", "icon": "dashboard" },
  { "viewId": "settings", "label": "Settings", "icon": "settings" }
]
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `viewId` | string | yes | Matches a key in the views map |
| `label` | string | yes | Display label for the tab |
| `icon` | string | no | Material Design icon name |

**Auto-generation:** If `_nav` is omitted, it is auto-generated from the view keys (excluding underscore-prefixed keys like `_nav` itself).

---

## Multiple Views

An agent can declare multiple views. Only `default` renders automatically. Other views are activated via `navigate` actions or by the agent during conversation:

```json
{
  "default": {
    "components": [...],
    "data": { ... },
    "actions": {
      "open_settings": { "type": "navigate", "view": "settings" }
    }
  },
  "settings": {
    "components": [...],
    "data": { ... }
  },
  "results": {
    "components": [...],
    "data": { ... }
  }
}
```

When navigating, the target view's `data` model is initialized fresh. If the `navigate` action includes `params`, those are merged into the initial data model.

---

## theme.css — Agent Styling (Optional)

The `theme.css` file provides agent-specific CSS that applies inside the workspace UI. It is loaded dynamically when the user opens the agent's workspace and unloaded when they switch away.

```css
/* theme.css */
.btn-primary {
  background-color: #FF6B35;
}
.a2ui-surface-container {
  font-family: 'Inter', sans-serif;
}
```

**How it works:**
- Served via `GET /agents/{id}/theme.css`
- Scoped to the workspace surface (no global leakage)
- Supports hot-reload during development

---

## Hot-Reload

During development, changes to `views.json` and `theme.css` are picked up by the file watcher with a 1-second debounce. No restart needed.

---

## Complete Example: Task Dashboard

```json
{
  "default": {
    "surface_type": "panel",
    "components": [
      { "id": "root", "component": "Column", "children": ["header", "task-list", "add-btn"], "gap": "4" },
      { "id": "header", "component": "Text", "text": "Today's Tasks", "variant": "h2" },
      { "id": "task-list", "component": "Column",
        "children": { "componentId": "task-row", "path": "/tasks" }, "gap": "2" },
      { "id": "task-row", "component": "Row", "children": ["task-check", "task-name"], "align": "center", "gap": "2" },
      { "id": "task-check", "component": "CheckBox", "value": { "path": "done" } },
      { "id": "task-name", "component": "Text", "text": { "path": "title" } },
      { "id": "add-btn", "component": "Button", "child": "add-label", "variant": "primary",
        "action": { "event": { "name": "add_task", "context": {} } } },
      { "id": "add-label", "component": "Text", "text": "Add Task" }
    ],
    "data": {
      "tasks": [
        { "title": "Review proposal", "done": false },
        { "title": "Send follow-up", "done": true }
      ]
    },
    "data_bindings": [
      {
        "path": "/tasks",
        "source": { "server": "todoist", "tool": "get_today" },
        "interval_secs": 60
      }
    ],
    "actions": {
      "add_task": {
        "type": "agent"
      }
    }
  },
  "_nav": [
    { "viewId": "default", "label": "Tasks", "icon": "check_circle" }
  ]
}
```

This example shows:
- Template children for dynamic task list
- Relative data binding paths (`"done"`, `"title"`) within each task
- MCP polling for live data
- Agent fallthrough for the "add task" action (LLM handles the conversation)
