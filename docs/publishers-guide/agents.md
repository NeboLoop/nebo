# Agents (`@org/agents/name`)

An Agent is a job description with a schedule. It bundles workflows and skills into a complete job profile — and it defines *when* each workflow runs. The Agent is the only artifact type that owns event bindings.

Only `AGENT.md` (persona) is required. `agent.json` (operational wiring) and `manifest.json` (identity metadata) are both optional — the loader checks for `AGENT.md`'s existence and loads the other two only if present.

For packaging format and manifest.json, see [Packaging](packaging.md).

---

## agent.json — The Job Definition

The `agent.json` carries the operational structure: which workflows to run, when to run them, and what events to listen for. This is the file that makes an Agent more than a folder of workflows — it's what makes it an employee who already knows the job.

```json
{
  "workflows": {
    "morning-briefing": {
      "trigger": {
        "type": "schedule",
        "cron": "0 7 * * *"
      },
      "description": "Daily morning briefing before the user wakes up",
      "activities": [
        { "id": "gather", "prompt": "Gather today's calendar, unread emails, and open tasks" },
        { "id": "write", "prompt": "Write a concise morning briefing from the gathered data" }
      ]
    },
    "day-monitor": {
      "trigger": {
        "type": "heartbeat",
        "interval": "30m",
        "window": "08:00-18:00"
      },
      "description": "Monitors for changes and interrupts only when something matters"
    },
    "evening-wrap": {
      "trigger": {
        "type": "schedule",
        "cron": "0 18 * * *"
      },
      "description": "End of day summary — what happened, what's unresolved, what's tomorrow"
    },
    "interrupt": {
      "trigger": {
        "type": "event",
        "sources": ["calendar.changed", "email.urgent"]
      },
      "description": "Fires when something urgent surfaces that needs immediate attention"
    }
  },
  "requires": {
    "plugins": ["PLUG-PJ3Z-ECFV"]
  },
  "skills": [
    "@nebo/skills/briefing-writer@^1.0.0"
  ],
  "pricing": {
    "model": "monthly_fixed",
    "cost": 47.0
  },
  "inputs": [
    {
      "key": "timezone",
      "label": "Your Timezone",
      "type": "select",
      "required": true,
      "default": "US/Eastern",
      "options": [
        { "value": "US/Eastern", "label": "Eastern" },
        { "value": "US/Pacific", "label": "Pacific" }
      ]
    },
    {
      "key": "briefing_focus",
      "label": "What should briefings focus on?",
      "type": "textarea",
      "placeholder": "e.g., sales pipeline, client deadlines, market news"
    }
  ],
  "defaults": {
    "timezone": "user_local",
    "configurable": ["workflows.morning-briefing.trigger.cron", "workflows.evening-wrap.trigger.cron", "workflows.day-monitor.trigger.interval"]
  }
}
```

---

## agent.json Fields

### Top-Level

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `workflows` | map | no | `{}` | Workflow bindings with triggers (keyed by binding name) |
| `requires` | object | no | `{}` | Hard dependencies. `requires.plugins` is an array of plugin install codes (e.g., `["PLUG-PJ3Z-ECFV"]`) that are auto-installed before skills during agent install. |
| `skills` | string[] | no | `[]` | Additional skill qualified names (beyond what workflows declare) |
| `inputs` | array | no | `[]` | Input field definitions for the agent's Configure tab (see [Input Fields](#input-fields) below) |
| `pricing` | object | no | — | Pricing configuration (see below) |
| `defaults` | object | no | `{}` | Default settings and user-configurable fields (see below) |
| `memory` | object | no | `{}` | Memory scoping configuration (see [Memory](#memory) below) |
| `tools` | AgentToolDef[] | no | `[]` | Sidecar HTTP endpoints exposed as native LLM tools (see [Sidecar Tool Definitions](#sidecar-tool-definitions)) |
| `scopes` | map | no | `{}` | Named tool restriction sets (see [Tool Scoping](#tool-scoping)) |
| `soul` | string | no | — | Agent personality/voice (DB-only via `agents.soul` column; see [Agent Soul](#agent-soul)). Not parsed from agent.json — set via Settings UI or API. |

### Pricing

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `model` | string | yes | Pricing model: `monthly_fixed` or `per_run` |
| `cost` | float | yes | Price in USD. For `monthly_fixed`, the monthly subscription price. For `per_run`, the cost per workflow execution. |

### Defaults

| Field | Type | Description |
|-------|------|-------------|
| `timezone` | string | Timezone for schedule triggers. `user_local` resolves to the user's system timezone at install time. Also accepts IANA timezone names (e.g., `America/New_York`). |
| `configurable` | string[] | JSON paths within `agent.json` that the user can override after installation. |

### Input Fields

Input fields define a dynamic form rendered in the agent's Configure tab. Users fill in values after installation; the agent uses them at runtime.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `key` | string | yes | Unique reference key (used in `{{key}}` template substitution and system prompt injection) |
| `label` | string | yes | Display label shown to the user |
| `type` | string | yes | Field type: `text`, `textarea`, `number`, `select`, `checkbox`, `radio` |
| `description` | string | no | Help text displayed below the field |
| `required` | boolean | no | Whether the field must be filled before the agent can activate |
| `default` | any | no | Default value pre-filled in the form |
| `placeholder` | string | no | Placeholder text for text/textarea fields |
| `options` | array | no | For `select`/`radio` fields: `[{ "value": "...", "label": "..." }]` |

**How input values are used:**

1. **System prompt injection** — All filled input values are appended to the agent's system prompt as a "Configured Inputs" section. The LLM sees them and uses them without asking the user again.
2. **Watch trigger template substitution** — `{{key}}` placeholders in watch trigger commands are replaced with the corresponding input value at runtime. Example: `gmail +watch --project {{gcp_project}}`. **The placeholder name must exactly match an input `key`** — if the command uses `{{gcp_project}}`, there must be an input with `"key": "gcp_project"`. Unmatched placeholders are left as literal text (e.g., `--project {{gcp_project}}`), which will cause the watch command to fail or behave unexpectedly.
3. **Stored separately from schema** — The input field *schema* lives in `agent.json`. The user-supplied *values* are stored in the `input_values` DB column and updated via `PUT /agents/{id}/inputs`.

### Memory

Controls how the agent's memories are scoped and inherited. By default, each agent gets its own isolated memory pool (`user_id:agent:{agent_id}`). These fields extend that behavior.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `inherit_user` | boolean | `false` | When `true`, the agent can **read** the user's main Nebo preferences (timezone, language, communication style). Read-only — the agent never writes to the user's memory scope. |
| `context_isolated` | boolean | `false` | When `true`, memories are isolated per `contextId` from SDK embed sessions. Each document/project/record gets its own memory pool. |

**Example:**

```json
{
  "memory": {
    "inherit_user": true,
    "context_isolated": true
  }
}
```

**When to use `context_isolated`:**

Use this when your agent handles multiple independent contexts — legal clients, project documents, patient records — where facts from one context must never leak into another. The `contextId` comes from the SDK embed:

```typescript
nebo.chat.mount(container, { contextId: document.id });
```

Each context maintains its own memory pool. Agent-wide memories (stored without a `contextId`) are still visible to all contexts.

**When to use `inherit_user`:**

Use this when your agent needs user preferences without asking for them — timezone for scheduling, language for communication, name for personalization. The agent reads from the main Nebo companion's `tacit/preferences` memories (read-only).

**Three-tier user_id convention:**

Memory scoping follows a layered naming convention:

| Layer | user_id format | Description |
|-------|---------------|-------------|
| Layer 1 (User) | `"user123"` | Nebo companion preferences (timezone, language, style) |
| Layer 2 (Agent) | `"user123:agent:brief"` | Agent-wide memories |
| Layer 3 (Context) | `"user123:agent:brief:ctx:doc-123"` | Per-document/project memories |

How the config flags interact:
- **Default** (both `false`) — reads/writes Layer 2 only
- **`inherit_user: true`** — reads Layer 1 (read-only) + reads/writes Layer 2
- **`context_isolated: true`** — reads/writes Layer 3 instead of Layer 2
- **Both enabled** — reads all 3 layers, writes Layer 3

### Workflows Overview

The `workflows` map pairs triggers (when to run) with activities (what to do). Each binding connects a trigger to either inline activities or an external workflow reference.

For the full reference — trigger types, activities, event system, watch triggers, budget math, and examples — see **[Workflows & Automation](workflows.md)**.

### Trigger Types (Summary)

| Type | Description |
|------|-------------|
| `schedule` | Fires on a cron schedule |
| `heartbeat` | Fires at a recurring interval (with optional time window) |
| `event` | Fires when a matching event occurs |
| `watch` | Long-running plugin process emitting NDJSON |
| `folder` | Fires when files change in a watched directory |
| `manual` | Only fires by explicit user request or API call |

---

## AGENT.md — The Persona

The `AGENT.md` is the agent's job description in prose. It defines who the agent *is* when operating as this Agent — capabilities, communication style, priorities, judgment calls. Think of it as the job description.

```markdown
# Chief of Staff

You are a Chief of Staff. You have been up for two hours before the
principal opens their eyes. You already know what their day looks like,
what matters most, and what can wait.

Your job is to make sure the principal is never blindsided. You surface
what's important, suppress what isn't, and interrupt only when something
genuinely demands attention.

## Communication Style

- Lead with the one thing that matters most today
- Be direct. No preamble, no pleasantries in briefings
- When you interrupt during the day, say why in one sentence
- Evening wraps are reflective, not just recaps

## Judgment

- "Important" means: time-sensitive, high-stakes, or likely to be missed
- If two things compete for attention, pick the one with a deadline
- Never surface something just because it's new — surface it because it matters
- When in doubt, mention it briefly rather than omit it entirely

## What You Don't Do

- You don't make decisions for the principal — you inform them
- You don't send messages on their behalf unless explicitly told to
- You don't editorialize about their schedule — you present it clearly
```

---

## Agent Soul

The `soul` field is separate from `AGENT.md`. Where `AGENT.md` defines capabilities, communication style, and the job description, `soul` captures voice, personality quirks, tone, ethical boundaries, and values — the *character* behind the role.

- Stored in the `agents.soul` DB column (migration 0092)
- Injected into prompt assembly as `agent_soul` context
- Editable in **Settings > Soul** section

**Example:**

```
# Core Truths
- Be genuinely helpful, not performatively helpful
- Have opinions and share them when relevant

# Vibe
- Conversational and warm, not corporate
- Direct and honest — skip filler words

# Boundaries
- Private things stay private. Period.
- When in doubt, ask before acting externally
```

**When to use soul vs AGENT.md:**

| | AGENT.md | soul |
|---|----------|------|
| Purpose | Job description | Personality |
| Contains | Capabilities, priorities, judgment rules | Voice, tone, quirks, values, ethical lines |
| Analogy | What the agent *does* | Who the agent *is* |

---

## Tool Scoping

Agents can declare named scopes that restrict which tools, skills, and plugins are available in a given context. This lets the same agent operate with different capabilities depending on where it runs.

**Declaration in `agent.json`:**

```json
{
  "scopes": {
    "write": { "tools": ["file_write", "email_send"], "skills": [], "plugins": [] },
    "read": { "tools": ["file_read", "email_search"], "skills": [], "plugins": [] }
  }
}
```

Each scope is a `ToolScope` struct with three fields:
- `tools: Vec<String>` — tool names to allow
- `skills: Vec<String>` — skill qualified names to allow
- `plugins: Vec<String>` — plugin install codes to allow

**Usage:** SDK embeds pass a `scope` parameter, and the runner restricts tool access to that named scope's allowlist.

```typescript
nebo.chat.mount(container, { scope: "read" });
```

**Use case:** A public-facing embed uses the `read` scope (search and view only), while the main Nebo UI uses the `write` scope (full access). Same agent, different capabilities per context.

---

## Sidecar Tool Definitions

Tools can be declared directly in `agent.json`, turning sidecar HTTP endpoints into native LLM tools (not proxied through a wrapper):

```json
{
  "tools": [
    {
      "name": "get_document",
      "description": "Fetch a document by ID",
      "method": "GET",
      "path": "/documents/{id}",
      "input_schema": { "type": "object", "properties": { "id": { "type": "string" } } }
    }
  ]
}
```

**Behavior:**
- Each entry becomes a tool the LLM can call directly
- Path parameters are resolved from input: `/documents/{id}` with `{"id": "abc"}` becomes `/documents/abc`
- HTTP method determines body vs query handling (GET uses query params, POST/PUT/PATCH send a JSON body)
- Discovery is also available via a `GET /_tools` endpoint on the sidecar, returning the same format

---

## Multi-Agent Delegation

An agent can delegate tasks to other installed agents using the `agents` domain tool. The delegating agent pauses while the target agent runs with its full identity — persona, plugins, skills, and memory scoping.

### Tool Call

```
agents(action: "delegate", name: "Deal Tracker", prompt: "List all open deals closing this month")
```

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `name` | string | yes* | Target agent's display name |
| `id` | string | yes* | Target agent's ID (alternative to `name`) |
| `prompt` | string | yes | Task description for the delegated agent |
| `wait` | boolean | no | Wait for result before continuing (default: `true`). Set `false` for background delegation. |
| `max_iterations` | integer | no | Maximum agentic loop iterations for the delegated agent (`0` = default) |

\* One of `name` or `id` is required.

### Session Isolation

Each delegation creates a separate session keyed as `subagent:<parent_session_key>:<task_id>` (chains nest as `subagent:subagent:...`). The delegated agent:

- Loads its own AGENT.md persona and skills
- Runs with its own plugin set (from `requires.plugins` in its `agent.json`)
- Gets its own memory scope — it does not read the parent's conversation history
- Returns a text result to the parent when complete

### When to Use Delegation

| Scenario | Approach |
|----------|----------|
| Agent needs a capability it doesn't have (e.g., calendar access) | Delegate to an agent that has the required plugins |
| Task requires a different persona or expertise | Delegate to a specialist agent |
| Background processing while continuing the conversation | Delegate with `wait: false` |
| Sequential pipeline across multiple agents | Chain delegations in workflow activities |

### Constraints

- The target agent must be installed (appears in `agents(action: "list")`)
- If not already active, delegation auto-activates the target agent
- Delegation inherits the parent's cancellation token — cancelling the parent cancels the child
- There is no built-in depth limit, but deep delegation chains consume more tokens and time

---

## Followup Suggestions

After each chat turn, the agent generates 2-3 contextual follow-up suggestions displayed as clickable chips below the response. This helps users continue the conversation without typing.

**Constraints:**
- 2-8 words each
- Not phrased as questions
- No "Tell me" / "Can you" patterns
- Uses the cheapest available provider (avoids Janus credits for background operations)

Followup generation happens asynchronously after the main response completes. The chips are delivered to the frontend via WebSocket.

---

## Auto-Install Cascade

When a user installs an Agent:

1. Parse `agent.json` for `requires.plugins` and skill references
2. Install required plugins from `requires.plugins` (install codes like `PLUG-XXXX-XXXX`)
3. For each workflow binding: install its declared skill dependencies
4. Install any additional skills listed in top-level `skills` array
5. Skills cascade to their own plugin dependencies (via `plugins:` in SKILL.md frontmatter)
6. Register all trigger bindings from `agent.json`
7. Load the AGENT.md persona into the agent's context

The user installs a job. Everything else cascades — plugins first, then skills.

### Filesystem Watcher (Development)

During development, agents placed in the platform-native `user/agents/` directory are detected automatically by the filesystem watcher. The root path is platform-native (not `~/.nebo`):

- macOS: `~/Library/Application Support/Nebo/user/agents/`
- Windows: `%APPDATA%\Nebo\user\agents\`
- Linux: `~/.local/share/nebo/user/agents/`

Setting `NEBO_HOME` overrides the root directory. Agents placed there:

- **Added:** New agent directory or symlink with `AGENT.md` → appears in sidebar and Apps page
- **Changed:** Edits to `AGENT.md`, `agent.json`, or `manifest.json` → metadata updated in DB, worker restarted if active
- **Removed:** Deleted directory → agent soft-deactivated (DB record preserved with `is_enabled=0`)

Changes are debounced at 1 second. No restart needed. Symlinks are fully supported — you can symlink an entire app directory from your source repo into the `user/agents/` directory and changes take effect immediately.

---

## Validation Rules

**Enforced at parse time** (a violation prevents the agent from loading):

- Event triggers must have at least one entry in `sources`
- Activity IDs must be unique within each binding
- Watch triggers must have a `plugin` (no default — parsing fails without it)

**Runtime / best-effort behaviors** (not parse-time validation):

- `cron`, `interval`, `command`, and `event` fields default to empty when omitted — they are not validated at parse time
- An invalid heartbeat `interval` is handled at runtime by skipping the heartbeat, not by rejecting the config
- Skill refs should be qualified names (`@org/skills/name`); an unresolvable ref only emits a `tracing::warn` and the agent still loads
- Watch triggers should also provide `event` or `command` (both recommended — `command` as a fallback for when `event` resolution fails)
- All `{{key}}` placeholders in watch trigger commands should match an input `key` exactly; an unmatched placeholder is left as literal text at runtime
- An Agent with no workflows is valid — it provides only a persona and skill declarations
