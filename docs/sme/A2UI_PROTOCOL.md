# A2UI Protocol v0.9 -- Comprehensive Technical Reference

> Source: [a2ui.org](https://a2ui.org) | [GitHub: google/A2UI](https://github.com/google/A2UI)
> Specification status: **Draft** (v0.9), Stable predecessor: v0.8
> Last verified: 2026-04-11

---

## 1. Protocol Overview

A2UI (Agent-to-UI) is a declarative, transport-agnostic protocol for agents (LLMs) to construct and update rich user interfaces. The protocol operates on three pillars:

1. **Streaming JSON messages** from agent to client (server-driven UI)
2. **Declarative component-based UI** described via a flat adjacency list
3. **Data binding** separating UI structure from application state

Design philosophy shift in v0.9: from "Structured Output First" to **"Prompt First"** -- schemas are designed to be human-readable and embeddable in LLM system prompts, not just machine-validated.

### Key Terminology

| Term | Definition |
|------|-----------|
| **Surface** | A single, independently renderable UI region with its own component tree and data model |
| **Catalog** | A JSON Schema file defining available components, functions, and themes |
| **Data Model** | A JSON object holding reactive application state, accessed via JSON Pointer paths |
| **Renderer** | The client-side runtime that parses A2UI messages and renders native UI |
| **Agent** | The server-side (typically LLM) that generates A2UI messages |

---

## 2. Schema Files

The v0.9 specification is split across modular JSON Schema files (Draft 2020-12):

| File | Purpose |
|------|---------|
| `common_types.json` | Reusable primitives: DynamicString, DynamicNumber, DynamicBoolean, DynamicStringList, ChildList, ComponentId, FunctionCall, CheckRule, Action, DataBinding |
| `server_to_client.json` | Top-level message envelope (4 message types). References `catalog.json` as a placeholder for the active catalog |
| `client_to_server.json` | Client-originated messages: `action` and `error` |
| `basic_catalog.json` | The standard catalog with 18 components, 14 functions, and a theme schema. `catalogId: "https://a2ui.org/specification/v0_9/basic_catalog.json"` |
| `basic_catalog_rules.txt` | Natural-language constraints supplementing the schema (e.g., "You MUST include ALL required properties for every component") |
| `client_capabilities.json` | Schema for `a2uiClientCapabilities` metadata object |
| `server_capabilities.json` | Schema for server capability advertisement |
| `client_data_model.json` | Schema for `a2uiClientDataModel` metadata attached to outgoing messages |
| `sample.json` | Schema for A2UI demonstration samples |

All schemas use `$id` namespace `https://a2ui.org/specification/v0_9/`.

---

## 3. Message Types

### 3.1 Server-to-Client Messages

All v0.9 messages include `"version": "v0.9"`. The top-level envelope is a `oneOf` over four message types. Messages use **JSONL** (JSON Lines) format for streaming.

#### 3.1.1 createSurface

Initializes a new surface. Must be sent before any `updateComponents` or `updateDataModel` for that surfaceId.

```json
{
  "version": "v0.9",
  "createSurface": {
    "surfaceId": "string (required)",
    "catalogId": "string (required) -- URI identifying the catalog",
    "theme": "object (optional) -- must validate against catalog theme schema",
    "sendDataModel": "boolean (optional, default false) -- if true, client attaches data model to every outgoing message"
  }
}
```

**Required fields:** `version`, `createSurface`, `createSurface.surfaceId`, `createSurface.catalogId`

#### 3.1.2 updateComponents

Adds or updates components in an existing surface. Can be sent multiple times. One component across all `updateComponents` messages for a surface MUST have `id: "root"`.

```json
{
  "version": "v0.9",
  "updateComponents": {
    "surfaceId": "string (required)",
    "components": [
      {
        "id": "string (required) -- ComponentId",
        "component": "string (required) -- discriminator, e.g. 'Text', 'Button'",
        "...": "component-specific properties"
      }
    ]
  }
}
```

**Required fields:** `version`, `updateComponents`, `surfaceId`, `components` (minItems: 1)

The `components` array items must conform to `catalog.json#/$defs/anyComponent`, which uses a `discriminator` on the `component` property.

#### 3.1.3 updateDataModel

Updates the data model for an existing surface at a specific JSON Pointer path.

```json
{
  "version": "v0.9",
  "updateDataModel": {
    "surfaceId": "string (required)",
    "path": "string (optional, defaults to '/') -- JSON Pointer",
    "value": "any (optional) -- if present, replaces value at path; if omitted, deletes key at path"
  }
}
```

**Required fields:** `version`, `updateDataModel`, `surfaceId`

#### 3.1.4 deleteSurface

Removes a surface and all its associated data.

```json
{
  "version": "v0.9",
  "deleteSurface": {
    "surfaceId": "string (required)"
  }
}
```

### 3.2 Client-to-Server Messages

The client sends exactly one of `action` or `error` per message. Format: `minProperties: 2, maxProperties: 2` (one of `action`/`error` plus `version`).

#### 3.2.1 action

Reports a user-initiated action from a component.

```json
{
  "version": "v0.9",
  "action": {
    "name": "string (required) -- from component's action.event.name",
    "surfaceId": "string (required)",
    "sourceComponentId": "string (required) -- triggering component's id",
    "timestamp": "string (required) -- ISO 8601 date-time",
    "context": "object (required) -- resolved key-value pairs from action.event.context"
  }
}
```

All `context` data bindings are resolved against the local data model before sending.

#### 3.2.2 error

Reports a client-side error. Two variants:

**VALIDATION_FAILED:**
```json
{
  "version": "v0.9",
  "error": {
    "code": "VALIDATION_FAILED",
    "surfaceId": "string (required)",
    "path": "string (required) -- JSON Pointer to failing field, e.g. '/components/0/text'",
    "message": "string (required) -- 1-2 sentence description"
  }
}
```

**Generic Error:**
```json
{
  "version": "v0.9",
  "error": {
    "code": "string (required, NOT 'VALIDATION_FAILED')",
    "surfaceId": "string (required)",
    "message": "string (required)"
  }
}
```

`VALIDATION_FAILED` errors enable the **Prompt-Generate-Validate loop**: agent generates UI JSON, client validates against catalog schema, errors feed back to the agent for self-correction.

---

## 4. Component Model -- Adjacency List

### 4.1 Architecture

A2UI uses a **flat adjacency list** rather than nested JSON trees. This design:

- Eliminates the need for perfect nesting in a single LLM generation pass
- Enables incremental streaming of components
- Supports targeted updates to specific components by ID
- Avoids deep-tree mutation complexity

### 4.2 Component Structure

Every component has:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | `ComponentId` (string) | Yes | Unique within the surface. One must be `"root"` |
| `component` | string | Yes | Discriminator: `"Text"`, `"Button"`, etc. |
| `accessibility` | `AccessibilityAttributes` | No | `label` (DynamicString) + `description` (DynamicString) |
| `weight` | number | No | flex-grow value; only valid as direct child of Row/Column |
| ...properties | varies | varies | Component-specific properties |

### 4.3 ChildList Type

`ChildList` is a `oneOf` with two forms:

**Static array** -- fixed set of child component IDs:
```json
"children": ["header-text", "body-row", "footer-btn"]
```

**Dynamic template** -- generates children from a data model list:
```json
"children": {
  "componentId": "item-template",
  "path": "/products"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `componentId` | ComponentId | Yes | Template component to repeat |
| `path` | string | Yes | JSON Pointer to an array in the data model |

When using a template, relative paths inside the template component resolve within each array item's scope.

### 4.4 Operations

Three fundamental operations on the flat list:

1. **Add**: Send new component definitions with novel IDs
2. **Update**: Send component definitions with existing IDs (properties are replaced)
3. **Remove**: Exclude IDs from parent `children` lists

---

## 5. Data Binding System

### 5.1 Data Model

The data model is a plain JSON object per surface. Paths follow **JSON Pointer (RFC 6901)** syntax extended with relative path support.

Examples:
- `/user/name` -- absolute path to a string
- `/cart/items/0` -- array index
- `/cart/items/0/price` -- nested within array item
- `name` -- relative path (no leading `/`), resolves within template scope

### 5.2 Dynamic Value Types

These are the core binding types in `common_types.json`. Each accepts a literal, a data binding, or a function call:

#### DynamicString
```
oneOf:
  - string (literal)
  - DataBinding { path: string }
  - FunctionCall with returnType: "string"
```

#### DynamicNumber
```
oneOf:
  - number (literal)
  - DataBinding { path: string }
  - FunctionCall with returnType: "number"
```

#### DynamicBoolean
```
oneOf:
  - boolean (literal)
  - DataBinding { path: string }
  - FunctionCall with returnType: "boolean"
```

#### DynamicStringList
```
oneOf:
  - array of strings (literal)
  - DataBinding { path: string }
  - FunctionCall with returnType: "array"
```

#### DynamicValue (universal)
```
oneOf:
  - string
  - number
  - boolean
  - array
  - DataBinding { path: string }
  - FunctionCall
```

#### DataBinding
```json
{
  "path": "string (required) -- JSON Pointer to data model value"
}
```

### 5.3 Path Resolution

| Path Type | Syntax | Resolution |
|-----------|--------|------------|
| Absolute | Starts with `/` (e.g., `/user/profile/name`) | Resolves from root of surface data model |
| Relative | No leading `/` (e.g., `name`) | Resolves within current template iteration scope |

**Type conversion rules:**
- Non-string values convert to string when needed
- `null`/`undefined` become `""`
- Objects/arrays stringify as JSON

### 5.4 Two-Way Binding (Input Components)

Interactive components (TextField, CheckBox, Slider, etc.) implement a Read/Write contract:

**Read contract:** Component reads value from bound `path`; re-renders when `updateDataModel` changes that path.

**Write contract:** User interaction **immediately** updates the local data model at the bound path. This is synchronous -- the data model is fully updated before any event action resolves.

**Reactivity:** The local data model is the single source of truth. Other components binding the same path update reactively when an input modifies it.

**Server sync:** Data model changes are sent to the server only when explicitly triggered by a user action (button click), NOT on passive input changes. This protects the network from UI noise.

### 5.5 sendDataModel

When `createSurface` sets `sendDataModel: true`:
- The renderer attaches the **entire surface data model** as metadata in every outgoing message
- In A2A transport: placed in `metadata.a2uiClientDataModel.surfaces[surfaceId]`
- Enables stateless agents that don't need to track client-side state
- Enables voice shortcuts and other non-button triggers

**Client Data Model schema:**
```json
{
  "version": "v0.9",
  "surfaces": {
    "<surfaceId>": { ... data model object ... }
  }
}
```

---

## 6. Actions

### 6.1 Action Type

`Action` is a `oneOf` between server events and local function calls:

```
oneOf:
  - { event: { name, context? } }      -- server-side
  - { functionCall: FunctionCall }       -- client-side
```

### 6.2 Server Actions (Events)

Dispatch data to the agent:

```json
{
  "event": {
    "name": "string (required) -- stable identifier for agent routing",
    "context": {
      "key1": "literal or { path: '/...' }",
      "key2": "..."
    }
  }
}
```

- `context` values can be literal values or `DynamicValue` data bindings
- All path references are resolved against the local data model before sending
- Context is a hand-picked subset/view of the data model state
- Use literal values for static IDs; use paths only for dynamically bound values

### 6.3 Local Actions (FunctionCall)

Execute on the renderer without network communication. The agent is unaware of local function calls.

```json
{
  "functionCall": {
    "call": "openUrl",
    "args": { "url": "https://example.com" }
  }
}
```

### 6.4 FunctionCall Object

```json
{
  "call": "string (required) -- function name from catalog",
  "args": "object (optional) -- key-value arguments, values can be DynamicValue or literal objects",
  "returnType": "string (optional) -- enum: string|number|boolean|array|object|any|void, default: boolean"
}
```

The `call` and `args` must conform to one of the `anyFunction` variants in the active catalog.

### 6.5 CheckRule and Validation

#### CheckRule Object
```json
{
  "condition": "DynamicBoolean (required) -- typically a FunctionCall returning boolean",
  "message": "string (required) -- error message to display on failure"
}
```

#### Checkable Mixin
Interactive components (Button, TextField, CheckBox, ChoicePicker, Slider, DateTimeInput) include the `Checkable` mixin:

```json
{
  "checks": [
    {
      "condition": {
        "call": "required",
        "args": { "value": { "path": "/partySize" } },
        "returnType": "boolean"
      },
      "message": "Party size is required"
    },
    {
      "condition": {
        "call": "length",
        "args": { "value": { "path": "/name" }, "min": 2 },
        "returnType": "boolean"
      },
      "message": "Name must be at least 2 characters"
    }
  ]
}
```

**Button behavior:** If any check fails, the button is automatically **disabled** on the renderer.

**Input behavior:** Failed checks display their error `message` adjacent to the input.

**Critical distinction:** Renderer checks manage UX state only. Server-side validation remains mandatory for data integrity.

---

## 7. Catalog System

### 7.1 Catalog Structure

A catalog is a JSON Schema defining what an agent can use to build UIs:

```json
{
  "catalogId": "string (required) -- unique URI, e.g. 'https://a2ui.org/specification/v0_9/basic_catalog.json'",
  "components": {
    "ComponentName": { "...JSON Schema..." }
  },
  "functions": {
    "functionName": { "...JSON Schema..." }
  },
  "$defs": {
    "theme": { "...JSON Schema..." },
    "anyComponent": { "oneOf": [...all components...], "discriminator": { "propertyName": "component" } },
    "anyFunction": { "oneOf": [...all functions...] },
    "CatalogComponentCommon": { "properties": { "weight": { "type": "number" } } }
  }
}
```

The `catalogId` is a **logical identifier** used for negotiation, not a runtime-fetched URL.

### 7.2 Catalog Negotiation Protocol

Three-step handshake:

1. **Server advertisement** (optional): Agent declares supported `catalogId` values via A2A AgentCard or server capabilities
2. **Client declaration** (required): Client sends ordered list of `supportedCatalogIds` in message metadata
3. **Agent selection**: Agent picks the best match; this choice persists for the surface's lifetime

### 7.3 Server Capabilities

```json
{
  "v0.9": {
    "supportedCatalogIds": ["https://a2ui.org/specification/v0_9/basic_catalog.json"],
    "acceptsInlineCatalogs": false
  }
}
```

- `supportedCatalogIds`: Catalogs the server can generate
- `acceptsInlineCatalogs`: Whether server accepts inline catalog definitions from clients (default: false)

### 7.4 Client Capabilities

```json
{
  "v0.9": {
    "supportedCatalogIds": [
      "https://a2ui.org/specification/v0_9/basic_catalog.json",
      "https://example.com/catalogs/v1/catalog.json"
    ],
    "inlineCatalogs": [
      {
        "catalogId": "https://example.com/catalogs/custom/v1",
        "components": { ... },
        "functions": [ ... ],
        "theme": { ... }
      }
    ]
  }
}
```

- `supportedCatalogIds` (required): URIs of catalogs the client can render
- `inlineCatalogs` (optional): Full catalog definitions sent inline; only if server's `acceptsInlineCatalogs` is true

#### Inline Catalog Structure
```json
{
  "catalogId": "string (required)",
  "components": "object (optional) -- additionalProperties are JSON Schemas",
  "functions": "array (optional) -- items are FunctionDefinition objects",
  "theme": "object (optional) -- additionalProperties are JSON Schemas for theme properties"
}
```

#### FunctionDefinition (in inline catalogs)
```json
{
  "name": "string (required)",
  "description": "string (optional)",
  "parameters": "JSON Schema (required)",
  "returnType": "string (required) -- enum: string|number|boolean|array|object|any|void"
}
```

### 7.5 Validation Strategy

**Two-phase validation:**
1. **Pre-send (agent side):** Validate generated JSON against catalog schema before transmission. Catches hallucinated properties.
2. **Post-receive (client side):** Validate against catalog on receipt. Strict contract compliance.

**Graceful degradation:**
- Unknown components: safe fallback (generic placeholder or text)
- Unknown properties: ignored
- Runtime errors: optional `VALIDATION_FAILED` error sent back to agent

### 7.6 Catalog Versioning

| Change Type | Version Impact | Examples |
|-------------|---------------|----------|
| Non-breaking | Same version | Adding leaf components, optional properties, metadata |
| Breaking | Major bump | Adding/removing containers, changing field types, requiring new properties |

Catalog identifiers should include version (e.g., `.../v1/`, `.../v2/`) for parallel support during migrations.

---

## 8. Basic Catalog -- Component Reference

`catalogId: "https://a2ui.org/specification/v0_9/basic_catalog.json"`

All components extend `ComponentCommon` (id + accessibility) and `CatalogComponentCommon` (weight).

### 8.1 Layout Components

#### Row
Horizontal layout. For grids, nest Columns within Rows.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `component` | `"Row"` | Yes | | Discriminator |
| `children` | ChildList | Yes | | Static array or template |
| `justify` | enum | No | `"start"` | `start`, `center`, `end`, `spaceBetween`, `spaceAround`, `spaceEvenly`, `stretch` |
| `align` | enum | No | `"stretch"` | `start`, `center`, `end`, `stretch` |

#### Column
Vertical layout. For grids, nest Rows within Columns.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `component` | `"Column"` | Yes | | Discriminator |
| `children` | ChildList | Yes | | Static array or template |
| `justify` | enum | No | `"start"` | `start`, `center`, `end`, `spaceBetween`, `spaceAround`, `spaceEvenly`, `stretch` |
| `align` | enum | No | `"stretch"` | `start`, `center`, `end`, `stretch` |

#### List
Scrollable list of items.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `component` | `"List"` | Yes | | Discriminator |
| `children` | ChildList | Yes | | Static array or template |
| `direction` | enum | No | `"vertical"` | `vertical`, `horizontal` |
| `align` | enum | No | `"stretch"` | `start`, `center`, `end`, `stretch` |

### 8.2 Display Components

#### Text
Supports simple Markdown (no HTML, images, or links).

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `component` | `"Text"` | Yes | | |
| `text` | DynamicString | Yes | | Text content |
| `variant` | enum | No | `"body"` | `h1`, `h2`, `h3`, `h4`, `h5`, `caption`, `body` |

#### Image

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `component` | `"Image"` | Yes | | |
| `url` | DynamicString | Yes | | Image URL |
| `description` | DynamicString | No | | Accessibility alt text |
| `fit` | enum | No | `"fill"` | `contain`, `cover`, `fill`, `none`, `scaleDown` |
| `variant` | enum | No | `"mediumFeature"` | `icon`, `avatar`, `smallFeature`, `mediumFeature`, `largeFeature`, `header` |

#### Icon

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `component` | `"Icon"` | Yes | |
| `name` | string enum OR DataBinding | Yes | See icon set below |

**Standard icon set (57 icons):** `accountCircle`, `add`, `arrowBack`, `arrowForward`, `attachFile`, `calendarToday`, `call`, `camera`, `check`, `close`, `delete`, `download`, `edit`, `event`, `error`, `fastForward`, `favorite`, `favoriteOff`, `folder`, `help`, `home`, `info`, `locationOn`, `lock`, `lockOpen`, `mail`, `menu`, `moreVert`, `moreHoriz`, `notificationsOff`, `notifications`, `pause`, `payment`, `person`, `phone`, `photo`, `play`, `print`, `refresh`, `rewind`, `search`, `send`, `settings`, `share`, `shoppingCart`, `skipNext`, `skipPrevious`, `star`, `starHalf`, `starOff`, `stop`, `upload`, `visibility`, `visibilityOff`, `volumeDown`, `volumeMute`, `volumeOff`, `volumeUp`, `warning`

#### Video

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `component` | `"Video"` | Yes | |
| `url` | DynamicString | Yes | Video URL |

#### AudioPlayer

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `component` | `"AudioPlayer"` | Yes | |
| `url` | DynamicString | Yes | Audio URL |
| `description` | DynamicString | No | Title or summary |

#### Divider

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `component` | `"Divider"` | Yes | | |
| `axis` | enum | No | `"horizontal"` | `horizontal`, `vertical` |

### 8.3 Container Components

#### Card
Container with elevation/border and padding.

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `component` | `"Card"` | Yes | |
| `child` | ComponentId | Yes | Single child. Wrap multiple elements in a layout (Column/Row) |

#### Tabs

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `component` | `"Tabs"` | Yes | |
| `tabs` | array (minItems: 1) | Yes | Each item: `{ title: DynamicString, child: ComponentId }` |

#### Modal
Overlay dialog.

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `component` | `"Modal"` | Yes | |
| `trigger` | ComponentId | Yes | Component that opens the modal |
| `content` | ComponentId | Yes | Component displayed inside the modal |

### 8.4 Interactive Components

All interactive components include the `Checkable` mixin (`checks` array).

#### Button

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `component` | `"Button"` | Yes | | |
| `child` | ComponentId | Yes | | Use Text for label, Icon only if explicitly needed |
| `variant` | enum | No | `"default"` | `default`, `primary`, `borderless` |
| `action` | Action | Yes | | Event or functionCall |
| `checks` | CheckRule[] | No | | Auto-disables if any fail |

#### TextField

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `component` | `"TextField"` | Yes | | |
| `label` | DynamicString | Yes | | Field label |
| `value` | DynamicString | No | | Current value (typically data-bound) |
| `variant` | enum | No | `"shortText"` | `shortText`, `longText`, `number`, `obscured` |
| `validationRegexp` | string | No | | Regex for client-side validation |
| `checks` | CheckRule[] | No | | Validation rules |

#### CheckBox

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `component` | `"CheckBox"` | Yes | |
| `label` | DynamicString | Yes | Text next to checkbox |
| `value` | DynamicBoolean | Yes | Checked state (typically data-bound) |
| `checks` | CheckRule[] | No | |

#### ChoicePicker
Replaces v0.8 "MultipleChoice".

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `component` | `"ChoicePicker"` | Yes | | |
| `label` | DynamicString | No | | Group label |
| `variant` | enum | No | `"mutuallyExclusive"` | `multipleSelection`, `mutuallyExclusive` |
| `options` | array | Yes | | Each: `{ label: DynamicString, value: string }` |
| `value` | DynamicStringList | Yes | | Selected values (data-bound to string array) |
| `displayStyle` | enum | No | `"checkbox"` | `checkbox`, `chips` |
| `filterable` | boolean | No | `false` | Show search input for filtering |
| `checks` | CheckRule[] | No | | |

#### Slider

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `component` | `"Slider"` | Yes | | |
| `label` | DynamicString | No | | |
| `value` | DynamicNumber | Yes | | Current value (data-bound) |
| `min` | number | No | `0` | Minimum value |
| `max` | number | Yes | | Maximum value |
| `checks` | CheckRule[] | No | | |

#### DateTimeInput

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `component` | `"DateTimeInput"` | Yes | | |
| `value` | DynamicString | Yes | | ISO 8601 format; init with `""` if unset |
| `enableDate` | boolean | No | `false` | Allow date selection |
| `enableTime` | boolean | No | `false` | Allow time selection |
| `min` | DynamicString | No | | Min date/time (ISO 8601) |
| `max` | DynamicString | No | | Max date/time (ISO 8601) |
| `label` | DynamicString | No | | |
| `checks` | CheckRule[] | No | | |

---

## 9. Basic Catalog -- Function Reference

14 functions in the basic catalog, organized by category:

### 9.1 Validation Functions (return boolean)

#### required
Checks value is not null, undefined, or empty.
```json
{ "call": "required", "args": { "value": "<DynamicValue>" }, "returnType": "boolean" }
```

#### regex
Checks value matches a regular expression.
```json
{ "call": "regex", "args": { "value": "<DynamicString>", "pattern": "string" }, "returnType": "boolean" }
```

#### length
Checks string length constraints. At least one of `min` or `max` required.
```json
{ "call": "length", "args": { "value": "<DynamicString>", "min?": "integer >= 0", "max?": "integer >= 0" }, "returnType": "boolean" }
```

#### numeric
Checks numeric range constraints. At least one of `min` or `max` required.
```json
{ "call": "numeric", "args": { "value": "<DynamicNumber>", "min?": "number", "max?": "number" }, "returnType": "boolean" }
```

#### email
Checks value is a valid email address.
```json
{ "call": "email", "args": { "value": "<DynamicString>" }, "returnType": "boolean" }
```

### 9.2 Formatting Functions (return string)

#### formatString
String interpolation with `${expression}` syntax. Supports JSON Pointer paths and nested function calls.
```json
{ "call": "formatString", "args": { "value": "<DynamicString>" }, "returnType": "string" }
```
Expression syntax:
- `${/absolute/path}` -- data model lookup
- `${relative/path}` -- template-scoped lookup
- `${functionName(arg1:val1, arg2:val2)}` -- function call
- `\\${` -- literal `${` escape

#### formatNumber
Formats a number with grouping and decimal precision.
```json
{ "call": "formatNumber", "args": { "value": "<DynamicNumber>", "decimals?": "<DynamicNumber>", "grouping?": "<DynamicBoolean>" }, "returnType": "string" }
```
- `grouping` defaults to `true` (locale-specific separators)

#### formatCurrency
Formats a number as currency.
```json
{ "call": "formatCurrency", "args": { "value": "<DynamicNumber>", "currency": "<DynamicString> (ISO 4217)", "decimals?": "<DynamicNumber>", "grouping?": "<DynamicBoolean>" }, "returnType": "string" }
```

#### formatDate
Formats a timestamp using Unicode TR35 date patterns.
```json
{ "call": "formatDate", "args": { "value": "<DynamicValue>", "format": "<DynamicString>" }, "returnType": "string" }
```
Pattern tokens:
- Year: `yy` (26), `yyyy` (2026)
- Month: `M` (1), `MM` (01), `MMM` (Jan), `MMMM` (January)
- Day: `d` (1), `dd` (01), `E` (Tue), `EEEE` (Tuesday)
- Hour (12h): `h`, `hh` -- requires `a` for AM/PM
- Hour (24h): `H`, `HH`
- Minute: `mm`
- Second: `ss`
- Period: `a` (AM/PM)

#### pluralize
Returns a localized string based on CLDR plural categories.
```json
{ "call": "pluralize", "args": { "value": "<DynamicNumber>", "other": "<DynamicString>", "zero?": "<DynamicString>", "one?": "<DynamicString>", "two?": "<DynamicString>", "few?": "<DynamicString>", "many?": "<DynamicString>" }, "returnType": "string" }
```
For English: use `one` and `other`.

### 9.3 Logic Functions (return boolean)

#### and
Logical AND over a list (minItems: 2).
```json
{ "call": "and", "args": { "values": ["<DynamicBoolean>", "<DynamicBoolean>", ...] }, "returnType": "boolean" }
```

#### or
Logical OR over a list (minItems: 2).
```json
{ "call": "or", "args": { "values": ["<DynamicBoolean>", "<DynamicBoolean>", ...] }, "returnType": "boolean" }
```

#### not
Logical NOT.
```json
{ "call": "not", "args": { "value": "<DynamicBoolean>" }, "returnType": "boolean" }
```

### 9.4 Action Functions (return void)

#### openUrl
Opens URL in browser/handler. No return value.
```json
{ "call": "openUrl", "args": { "url": "string (URI)" }, "returnType": "void" }
```

---

## 10. Theme System

### 10.1 Philosophy

A2UI follows **renderer-controlled styling**. Agents describe _what_ to show; renderers decide _how_ it looks.

### 10.2 Three-Layer Styling Architecture

| Layer | Owner | Purpose |
|-------|-------|---------|
| **Semantic hints** | Agent | `variant` / `usageHint` properties (e.g., `"h1"`, `"primary"`) |
| **Theme configuration** | Agent (via `createSurface.theme`) | Global design system parameters |
| **Component overrides** | Renderer | Platform-specific customization (CSS, ThemeData, etc.) |

### 10.3 Basic Catalog Theme Schema

```json
{
  "primaryColor": {
    "type": "string",
    "pattern": "^#[0-9a-fA-F]{6}$",
    "description": "Primary brand color for highlights (buttons, borders). Renderers may generate variants."
  },
  "iconUrl": {
    "type": "string",
    "format": "uri",
    "description": "Image URL identifying the agent/tool"
  },
  "agentDisplayName": {
    "type": "string",
    "description": "Attribution text for the agent/tool"
  }
}
```

The theme schema is extensible (`additionalProperties: true`), allowing custom catalogs to define additional theme properties.

### 10.4 Key Constraint

Agents provide semantic hints only. Direct visual styles (`fontSize`, `color`, `padding`) are NOT supported in agent payloads. The renderer maps semantic hints to platform-appropriate styling.

---

## 11. Transport Bindings

A2UI is **transport-agnostic**. Requirements for any transport:

1. Reliable in-order delivery
2. Message framing (JSONL, WebSocket frames, SSE events)
3. Metadata support for data model and capabilities
4. Optional bidirectional channel for actions

### 11.1 A2A (Agent-to-Agent) Binding

A2UI envelopes map to A2A message parts. Metadata fields:
- `metadata.a2uiClientCapabilities` -- client capabilities
- `metadata.a2uiClientDataModel` -- client data model (when `sendDataModel: true`)

### 11.2 MCP (Model Context Protocol) Binding

A2UI content travels as **Embedded Resources** in tool responses:

```json
{
  "type": "resource",
  "resource": {
    "uri": "a2ui://surface-name",
    "mimeType": "application/json+a2ui",
    "text": "... JSON-serialized A2UI message sequence ..."
  }
}
```

**URI scheme:** `a2ui://` prefix
**MIME type:** `application/json+a2ui`

**Capabilities exchange over MCP:**
- **Option A (Stateful):** During `initialize`, client declares `capabilities.a2ui.clientCapabilities`
- **Option B (Stateless):** Per-message `_meta` field carries capabilities with each `tools/call`

**Tool patterns:**
1. Custom tools returning A2UI (e.g., `get_recipe_a2ui`)
2. `action` tool receiving button clicks
3. `error` tool for rendering failures

**Annotations:** `audience=["user"]` hides JSON from LLM; empty audience shows to both.

### 11.3 Other Transports

- **AG-UI:** Low-latency agent-UI protocol
- **SSE + JSON-RPC:** Streaming events
- **WebSocket:** Persistent bidirectional
- **REST:** Standard HTTP
- **gRPC, message queues:** Custom

---

## 12. Renderer Architecture

### 12.1 Three-Layer Pattern

```
Protocol Handling → State Management → UI Rendering
```

### 12.2 Message Processing Pipeline

1. Ingest JSONL stream
2. Identify message type (`createSurface`, `updateComponents`, `updateDataModel`, `deleteSurface`)
3. Route to appropriate handler
4. Buffer components and data model updates
5. Render/re-render on relevant changes

### 12.3 Surface Registry

Each renderer maintains `Map<surfaceId, SurfaceData>`:
- **Component buffer:** Flat map of `id -> component definition`
- **Data model store:** JSON object with reactive bindings
- **Root component reference:** Component with `id: "root"`

### 12.4 web_core Library

`@a2ui/web-lib` provides reusable modules for web renderers:

| Module | Function |
|--------|----------|
| `MessageProcessor` | JSONL parsing, surface lifecycle dispatch |
| `SurfaceModel` | Component tree state management |
| `DataModel` | Path-based binding resolution, template evaluation |
| `ComponentModel` | Adjacency-to-tree conversion |

Web renderers (React, Angular, Lit) delegate protocol handling to web_core and only map A2UI component types to framework widgets.

### 12.5 Performance Optimizations

- **Batching:** Buffer updates for 16ms intervals
- **Diffing:** Only changed component properties trigger re-renders
- **Granular updates:** Target specific data paths
- **Progressive rendering:** Stream chunks display immediately

### 12.6 Error Handling

Renderers should:
- Skip malformed messages with continuation
- Report binding errors and unknown components back to server via `error` messages
- Display error states on network interruptions with reconnection support

---

## 13. Security Considerations

### 13.1 Sandboxed Execution

Agents cannot inject arbitrary code. Only pre-registered `functionCall` operations from the active catalog are permitted.

### 13.2 Data Isolation (Multi-Agent)

In multi-agent orchestration, the orchestrator **MUST** strip `a2uiClientDataModel` metadata before forwarding to sub-agents. Without stripping, a malicious sub-agent could scrape the state of other active surfaces.

**This is described as a mandatory security requirement.**

### 13.3 Component Allowlisting

Custom catalogs should only register trusted components. Property values from agents must be validated against type constraints. Agent-provided text must be sanitized before rendering.

---

## 14. v0.8 to v0.9 Migration

### 14.1 Message Type Renames

| v0.8 | v0.9 |
|------|------|
| `beginRendering` | `createSurface` |
| `surfaceUpdate` | `updateComponents` |
| `dataModelUpdate` | `updateDataModel` |
| `userAction` | `action` |

### 14.2 Component Structure Change

**v0.8:** Dynamic key wrapper
```json
{ "id": "t1", "component": { "Text": { "text": { "literalString": "Hello" } } } }
```

**v0.9:** Flat with discriminator
```json
{ "id": "t1", "component": "Text", "text": "Hello" }
```

### 14.3 Data Model Format Change

**v0.8:** Array of typed pairs
```json
[{ "key": "name", "valueString": "Alice" }, { "key": "age", "valueNumber": 30 }]
```

**v0.9:** Standard JSON object
```json
{ "name": "Alice", "age": 30 }
```

### 14.4 Property Renames

| Component | v0.8 | v0.9 |
|-----------|------|------|
| Row/Column | `distribution` | `justify` |
| Row/Column | `alignment` | `align` |
| Modal | `entryPointChild` | `trigger` |
| Modal | `contentChild` | `content` |
| Tabs | `tabItems` | `tabs` |
| TextField | `text` | `value` |
| Button | `primary: boolean` | `variant: enum` |
| MultipleChoice | (component name) | `ChoicePicker` |
| Slider | `minValue` / `maxValue` | `min` / `max` |
| Text/Image | `usageHint` | `variant` |

### 14.5 Binding Simplification

**v0.8:** `{ "literalString": "foo" }` or `{ "path": "/foo" }`
**v0.9:** `"foo"` (literal) or `{ "path": "/foo" }` (binding)

### 14.6 New in v0.9

- `createSurface` requires explicit `catalogId`
- `sendDataModel` boolean for automatic client state sync
- `theme` property (replaces `styles`)
- `formatString` function with `${expression}` interpolation
- `checks` arrays replace `validationRegexp` for TextField
- Root component must have `id: "root"` (implicit, not declared in `beginRendering`)
- Schema modularization (common_types, catalog separation)
- `VALIDATION_FAILED` error for Prompt-Generate-Validate loop
- `basic_catalog_rules.txt` natural-language constraints
- `version: "v0.9"` required on all messages

### 14.7 Breaking Changes

1. Component wrapping structure fundamentally different
2. Data model format changed from adjacency lists to objects
3. Button styling from boolean to enum
4. ChoicePicker replaces MultipleChoice
5. TextField validation via `checks` instead of `validationRegexp`
6. Root component implicit (`id: "root"`) vs explicit declaration

---

## 15. Complete Example

A restaurant reservation form demonstrating major features:

```jsonl
{"version":"v0.9","createSurface":{"surfaceId":"reservation","catalogId":"https://a2ui.org/specification/v0_9/basic_catalog.json","theme":{"primaryColor":"#FF6B35","agentDisplayName":"Restaurant Bot"},"sendDataModel":true}}
{"version":"v0.9","updateDataModel":{"surfaceId":"reservation","value":{"partySize":2,"date":"","specialRequests":"","agreedToTerms":false}}}
{"version":"v0.9","updateComponents":{"surfaceId":"reservation","components":[{"id":"root","component":"Column","children":["title","party-slider","date-input","requests-field","terms-check","submit-btn"],"align":"stretch"},{"id":"title","component":"Text","text":"Make a Reservation","variant":"h2"},{"id":"party-slider","component":"Slider","label":"Party Size","value":{"path":"/partySize"},"min":1,"max":20},{"id":"date-input","component":"DateTimeInput","label":"Date & Time","value":{"path":"/date"},"enableDate":true,"enableTime":true,"checks":[{"condition":{"call":"required","args":{"value":{"path":"/date"}},"returnType":"boolean"},"message":"Please select a date and time"}]},{"id":"requests-field","component":"TextField","label":"Special Requests","value":{"path":"/specialRequests"},"variant":"longText"},{"id":"terms-check","component":"CheckBox","label":"I agree to the cancellation policy","value":{"path":"/agreedToTerms"}},{"id":"submit-btn","component":"Button","child":"submit-text","variant":"primary","action":{"event":{"name":"submit_reservation","context":{"partySize":{"path":"/partySize"},"date":{"path":"/date"},"requests":{"path":"/specialRequests"}}}},"checks":[{"condition":{"call":"required","args":{"value":{"path":"/date"}},"returnType":"boolean"},"message":"Date is required"},{"condition":{"call":"and","args":{"values":[{"path":"/agreedToTerms"}]},"returnType":"boolean"},"message":"You must agree to the terms"}]},{"id":"submit-text","component":"Text","text":"Reserve Table"}]}}
```

---

## 16. Key Design Patterns

### Prompt-Generate-Validate Loop
1. Construct prompt embedding catalog schema + rules
2. LLM generates A2UI JSON
3. Validate against catalog schema
4. On failure, send `VALIDATION_FAILED` error back to LLM for self-correction
5. Repeat until valid

### Progressive Rendering
- Render as messages stream in
- Handle undefined values gracefully for paths not yet populated
- Buffer 16ms between UI updates

### Stateless Agent via sendDataModel
- Set `sendDataModel: true` in `createSurface`
- Client attaches full data model to every outgoing action
- Agent needs no server-side session state

### Catalog Indirection
- `catalog.json` placeholder in `server_to_client.json` allows swapping catalogs
- Same envelope schema works with any catalog
- Custom catalogs must preserve `ComponentId` and `ChildList` type references

---

## Appendix A: Schema File Locations (GitHub)

```
google/A2UI/specification/v0_9/json/
  basic_catalog.json          (43.7 KB)
  basic_catalog_rules.txt     (402 B)
  client_capabilities.json    (3.5 KB)
  client_data_model.json      (790 B)
  client_to_server.json       (3.1 KB)
  common_types.json           (9.2 KB)
  sample.json                 (709 B)
  server_capabilities.json    (1.2 KB)
  server_to_client.json       (5.6 KB)
  server_to_client_list.json  (323 B)
  client_to_server_list.json  (323 B)
  server_to_client_list_wrapper.json (548 B)
  client_to_server_list_wrapper.json (548 B)
  catalogs/                   (directory for additional catalogs)
```
