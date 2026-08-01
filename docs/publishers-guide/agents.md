# Agents (`@org/agents/name`)

An Agent is a job description with a schedule. It bundles workflows and skills into a complete job profile — and it defines *when* each workflow runs. The Agent is the only artifact type that owns event bindings.

Only `AGENT.md` (persona) is required. `agent.json` (operational wiring) and `manifest.json` (identity metadata) are both optional — the loader checks for `AGENT.md`'s existence and loads the other two only if present. An optional `theme.css` is also picked up if present.

Two loader behaviors worth knowing: an agent you create by hand with no `manifest.json` gets a UUID minted and **written back into a new `manifest.json`**, so it keeps a stable identity across renames; and when a `manifest.json` is present, its `name` overrides the `AGENT.md` frontmatter name (unless it looks package-style, containing `@` or `/`), while its `description` only fills in when `AGENT.md` has none.

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
| `requires` | object | no | `{}` | Hard dependencies. `requires.plugins` is an array of plugin references (install codes like `PLUG-PJ3Z-ECFV`, or plugin slugs like `gws`) auto-installed during the cascade. `requires.interfaces` declares typed capability interfaces the agent binds (e.g. `["ledger", "mail"]`); each interface's gated operations become items in the agent's per-operation approval policy. |
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

> **Not yet enforced.** `defaults` is parsed and stored, but has no runtime consumer today — `user_local` is not resolved to a timezone, and `configurable` paths are not surfaced as user-editable overrides. Declare them for forward compatibility if you like, but do not rely on them taking effect. Schedule triggers currently evaluate against the machine's local timezone.

| Field | Type | Description |
|-------|------|-------------|
| `timezone` | string | Intended timezone for schedule triggers. `user_local` is intended to resolve to the user's system timezone; IANA names (e.g. `America/New_York`) are also accepted. |
| `configurable` | string[] | JSON paths within `agent.json` intended to be user-overridable after installation. |

### Input Fields

Input fields define a dynamic form rendered in the agent's Configure tab. Users fill in values after installation; the agent uses them at runtime.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `key` | string | yes | Unique reference key (used in `{{key}}` template substitution and system prompt injection). `name` is accepted as an alias — if `key` is omitted, `name` fills in. |
| `label` | string | yes | Display label shown to the user |
| `type` | string | yes | Field type: `text`, `textarea`, `number`, `select`, `checkbox`, `radio` |
| `description` | string | no | Help text displayed below the field |
| `required` | boolean | no | Whether the field must be filled before the agent can activate |
| `default` | any | no | Default value pre-filled in the form |
| `placeholder` | string | no | Placeholder text for text/textarea fields |
| `options` | array | no | For `select`/`radio` fields: `[{ "value": "...", "label": "..." }]` |

**How input values are used:**

1. **System prompt injection** — All filled input values are appended to the agent's system prompt as a "Configured Inputs" section. The LLM sees them and uses them without asking the user again.
2. **Trigger template substitution** — `{{key}}` placeholders are replaced with the corresponding input value at runtime, in both **watch trigger commands** (`gmail +watch --project {{gcp_project}}`) and **folder trigger paths**. **The placeholder name must exactly match an input `key`** — if the command uses `{{gcp_project}}`, there must be an input with `"key": "gcp_project"`. Unmatched placeholders are left as literal text (e.g., `--project {{gcp_project}}`), which will cause the trigger to fail or behave unexpectedly.
3. **Stored separately from schema** — The input field *schema* lives in `agent.json`. The user-supplied *values* are stored in the `input_values` DB column and updated via `PUT /agents/{id}/inputs`.
4. **Declared defaults apply at runtime** — a declared `default` is used even if the user never saved the form, and a value the user blanks out falls back to the default rather than substituting an empty string. **Only string defaults participate** in this merge: a numeric or boolean `default` is dropped, so give any input used in a `{{key}}` placeholder a string default (`"30"`, not `30`).

### Memory

Controls how the agent's memories are scoped. By default, each agent gets its own isolated memory pool (`user_id:agent:{agent_id}`). In addition, every agent always reads the owner's identity memories — the `tacit/preferences` and `tacit/personality` prefixes (timezone, language, communication style) — read-only. This inheritance is built in and needs no flag. (An earlier `inherit_user` flag has been removed; if present in older agent.json files it parses fine and is ignored.)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `context_isolated` | boolean | `false` | When `true`, memories are isolated per context. Each document/project/record gets its own memory pool. The context comes from the SDK embed's `contextId` when set, otherwise from the session's active chat — so each chat thread becomes its own context. |
| `topics` | array | `[]` | Declared memory topics that replace the generic `project` category in this agent's memory extraction prompt (see [Memory Topics](#memory-topics)). |

**Example:**

```json
{
  "memory": {
    "context_isolated": true
  }
}
```

**When to use `context_isolated`:**

Use this when your agent handles multiple independent contexts — legal clients, project documents, patient records — where facts from one context must never leak into another. The `contextId` comes from the SDK embed:

```typescript
nebo.chat.mount(container, { contextId: document.id });
```

Each context maintains its own memory pool. Only the agent-wide `tacit/` namespace is inherited into isolated contexts — working style crosses contexts, case facts do not.

**Fail-closed writes:** if `context_isolated` is set but no context can be derived for a run (no embed `contextId` and no active chat), memory **writes are refused** for that run rather than silently landing in the shared agent scope — where they would be readable from every other context. Reads fall back to the agent-wide scope.

### Memory Topics

By default the agent extracts durable facts into a generic `project` category. Declaring `memory.topics` replaces that category with your own domain vocabulary — each topic becomes a namespace prefix inside the agent's memory scope, and its description is injected verbatim into the extraction prompt as the category definition. The invariant layers (`tacit/*`, `entity/`) are never affected.

```json
{
  "memory": {
    "topics": [
      { "slug": "lead", "description": "A prospective buyer or seller — stage, budget, timeline, next action" },
      { "slug": "listing", "description": "A property on the market — address, price, status, showing history" }
    ]
  }
}
```

**Rules (all enforced at parse time — a violation prevents the agent from loading):**

| Rule | Limit |
|------|-------|
| Maximum topics | 8 (bounds extraction-prompt growth) |
| Slug format | kebab-case: lowercase letters, digits, hyphens; no leading/trailing hyphen, no `--` |
| Reserved slugs | `tacit`, `entity`, `daily`, `project`, `memory`, `style`, `artifact` |
| Description | Non-empty, max 120 characters |

Write descriptions the way you'd brief a new hire on what's worth writing down — they are the entire definition the extractor sees.

**Three-tier user_id convention:**

Memory scoping follows a layered naming convention:

| Layer | user_id format | Description |
|-------|---------------|-------------|
| Layer 1 (User) | `"user123"` | User-level preferences (timezone, language, style) |
| Layer 2 (Agent) | `"user123:agent:brief"` | Agent-wide memories |
| Layer 3 (Context) | `"user123:agent:brief:ctx:doc-123"` | Per-document/project memories |

How scoping resolves:
- **Default** — reads/writes Layer 2, plus read-only Layer 1 identity prefixes (always on)
- **`context_isolated: true`** — writes Layer 3; reads Layer 3 + the agent-wide `tacit/` namespace (Layer 2) + Layer 1 identity prefixes
- **`context_isolated: true` with no derivable context** — fail closed: writes refused, reads fall back to Layer 2

### Workflows Overview

The `workflows` map pairs triggers (when to run) with activities (what to do). Activities are inline; a binding with no activities is a chat-only binding — the trigger fires the agent without a fixed procedure.

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
- `tools: Vec<String>` — sidecar tool names to allow
- `skills: Vec<String>` — skill refs to load (a subset of the top-level `skills` array)
- `plugins: Vec<String>` — additional plugin slugs (e.g. `"gws"`) to pre-activate for this scope

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
- `agent.json` is the only source of tool definitions — there is no HTTP discovery endpoint (sidecar paths starting with `_` are reserved and blocked from external clients)

---

## Multi-Agent Delegation

An agent can delegate tasks to other installed agents using the `agent` domain tool's `registry` resource. The delegating agent pauses while the target agent runs with its full identity — persona, plugins, skills, and memory scoping.

### Tool Call

```
agent(resource: "registry", action: "delegate", name: "Deal Tracker", prompt: "List all open deals closing this month")
```

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `resource` | string | yes | Must be `"registry"`. (It can be inferred from the action, but the schema declares it required — always pass it.) |
| `action` | string | yes | `"delegate"` |
| `name` | string | yes* | Target agent's display name. Matching is slug-normalized both ways, so `"deal-tracker"` matches `"Deal Tracker"`. |
| `id` | string | yes* | Target agent's ID (alternative to `name`). Works at runtime, but is not declared in the tool's published schema — prefer `name`. |
| `prompt` | string | yes | Task description for the delegated agent |
| `wait` | boolean | no | Wait for result before continuing (default: `true`). Set `false` for background delegation. |
| `max_iterations` | integer | no | Maximum agentic loop iterations for the delegated agent (`0` = use the default) |

\* One of `name` or `id` is required.

Note: delegation is an action on the `agent` domain tool — there is no separate `agents` tool. Use `resource: "registry"` with `action: "delegate"` to run a *named* agent with its full identity; `resource: "task"` with `action: "spawn"` creates a blank sub-agent with no persona, plugins, or skills.

### Session Isolation

Each delegation creates a separate session keyed as `subagent:<parent_session_key>:<task_id>`, where `task_id` is `sa-<uuid>`. Chains nest as `subagent:subagent:...` up to the depth cap below. The delegated agent:

- Loads its own AGENT.md persona and skills
- Runs with its own plugin set (from `requires.plugins` in its `agent.json`)
- Gets its own memory scope (`{owner}:agent:{target_id}`) — it does not read the parent's conversation history
- Returns a text result to the parent when complete — this applies to `wait: true`. With `wait: false` the tool returns immediately ("Sub-agent spawned in background") and the output is delivered later through the parent's stream.

**Caveat — Soul is not loaded on the delegation auto-activate path.** When delegation has to register the target agent itself (because it wasn't already active), it does so without the agent's `soul` or `rules`. An agent first reached via delegation runs its AGENT.md persona but not its Soul. Activate the agent through the normal paths first if its Soul matters to the delegated task.

### When to Use Delegation

| Scenario | Approach |
|----------|----------|
| Agent needs a capability it doesn't have (e.g., calendar access) | Delegate to an agent that has the required plugins |
| Task requires a different persona or expertise | Delegate to a specialist agent |
| Background processing while continuing the conversation | Delegate with `wait: false` |
| Sequential pipeline across multiple agents | Chain delegations in workflow activities |

### Constraints

- The target agent must be resolvable — installed, or present as a directory under `user/agents/` (so a dev agent that was never formally installed is still delegable)
- If not already active, delegation registers the target in memory for the run. This is **not** a full activation: it does not flip `is_enabled` in the DB and does not register the target's triggers.
- Delegation inherits the parent's cancellation token — cancelling the parent cancels the child
- **Delegation chains are capped at 2 levels of nesting.** A top-level agent may delegate, and its child may delegate; a delegation attempted from the third level is refused with an instruction to do the work directly. Design multi-agent flows to fan out, not to chain deeply.

Note: this cap is distinct from the agent-to-agent **handoff** cap that governs @-mention chains between agents in Loop channels (a separate limit, enforced on inbound channel messages). Delegation depth and channel handoff depth are two different mechanisms with two different limits.

---

## Auto-Install Cascade

When a user installs an Agent, its declared dependencies are resolved and installed recursively.

**What gets collected:**

- `requires.plugins` — plugin dependencies
- Top-level `skills` — skill dependencies
- Skills referenced by inline workflow activities
- For marketplace-published agents, a `dependencies` block (`agents`, `skills`, `plugins`, `workflows`) — agents can depend on other agents, which install recursively

**Cascade rules:**

- Skills cascade to their own dependencies — plugins declared via `plugins:` in SKILL.md frontmatter, and other skills. Plugins cascade to their own plugin dependencies.
- **Dependencies marked `optional` are skipped** — only required dependencies install.
- **Only marketplace-resolvable refs install.** Skill refs must be qualified names (`@org/skills/name`) or install codes (`SKIL-XXXX-XXXX`). A bare name is treated as a tool binding provided by one of the agent's plugins and is silently skipped, not installed.

**Install order is not guaranteed to be plugins-first.** It depends on which declaration shape the agent uses — the `requires.plugins` path pushes plugins before skills, while the marketplace `dependencies` block and the `POST /agents` path collect skills first. Do not write an agent that depends on a specific dependency-install ordering.

**Failures are partial — there is no rollback.** If a dependency fails to install, the cascade broadcasts `dep_failed`, records the failure, and continues with the remaining dependencies. Two consequences:

- A failed dependency's own children are never attempted — one failed plugin orphans its whole subtree.
- **The agent itself stays installed and may auto-activate anyway.** The cascade runs as a detached background task, so its failure count cannot block the install. Users can end up with an installed, active agent missing plugins or skills. Recovery is manual, via the dependency approval/retry route.

Only a failure to persist the agent record itself aborts the install.

**Triggers and persona are not cascade steps.** The persona is persisted with the agent record up front. Cron rows are registered at install time, but live trigger registration (event subscriptions, heartbeat/watch/folder loops) is owned by the AgentWorker and happens when the agent is **activated** — cascade-installed agents install paused. If any installed plugin has unmet auth requirements, activation is deferred to the frontend setup wizard.

Note: agents discovered on disk at boot or by the filesystem watcher do **not** cascade by default — that path is gated behind an `auto_install_deps` setting which is off by default. Explicit installs always cascade.

### Filesystem Watcher (Development)

During development, agents placed in the platform-native `user/agents/` directory are detected automatically by the filesystem watcher. The root path is platform-native (not `~/.nebo`):

- macOS: `~/Library/Application Support/Nebo/user/agents/`
- Windows: `%APPDATA%\Nebo\user\agents\`
- Linux: `~/.local/share/nebo/user/agents/`

Setting `NEBO_HOME` overrides the root directory. Agents placed there:

- **Added:** New agent directory or symlink containing `AGENT.md` → appears in sidebar and Apps page
- **Changed:** Edits to `AGENT.md`, `agent.json`, or `theme.css` → metadata updated in DB, worker restarted if active
- **Removed:** Deleted directory → agent soft-deactivated (DB record preserved with `is_enabled=0`, so re-adding the directory restores it)

**Known limitations of the dev watcher:**

- **`manifest.json` edits are not picked up.** Change detection compares `AGENT.md`, `agent.json`, and `theme.css` only. Editing just `manifest.json` (version, description, `type`) updates nothing. Renaming the agent in `manifest.json` is worse — it changes the identity key, which registers the agent under the new name and then soft-deactivates it as a removal. Restart Nebo after changing `manifest.json`.
- **Rapid successive saves can be missed.** Events arriving within 1 second of the previous reload are discarded outright rather than deferred, so a multi-file save (editing `AGENT.md` and `agent.json` together) may only apply the first change. Save again if a change doesn't take.
- **Edits inside a symlinked directory do not reliably propagate.** Symlinking an agent directory from a source repo works for *discovery* — the agent is found and loaded. But the recursive watch does not follow symlinks into the target, so editing files through the symlink typically fires no event. Touch the symlink itself, or restart Nebo, to pick up changes.

For anything the watcher misses, `POST /agents/{id}/reload` re-reads `AGENT.md` and `agent.json` from disk on demand.

---

## Validation Rules

**Hard errors** (the whole agent fails to load):

- Event triggers must have at least one entry in `sources`
- Activity IDs must be unique within each binding
- All `memory.topics` rules (count, slug format, reserved slugs, description length)
- A malformed `agent.json` — a JSON syntax error or a schema violation outside the `workflows` map. The loader logs a warning and skips the agent entirely, so an agent that silently disappears after an edit is usually a typo in `agent.json`.

**Silently skipped bindings** (the agent still loads, minus that workflow):

Workflow parsing is lenient. If a binding doesn't match the schema, it is dropped with only a log warning and the rest of the agent loads normally. This is the failure mode to watch for, because the agent appears to install correctly:

- A `watch` trigger missing `plugin`, or a `heartbeat` missing `interval`, or a `folder` trigger missing `path` — these fields have no default, so the binding fails to parse and is discarded
- Any other structural mismatch inside a workflow binding

If a trigger silently never fires, check the logs for a skipped-workflow warning before anything else.

**Runtime / best-effort behaviors:**

- `cron`, `command`, and `event` default to empty when omitted — they are not validated at parse time
- An invalid heartbeat `interval` *value* is handled at runtime by skipping the heartbeat, not by rejecting the config
- Skill refs should be qualified names (`@org/skills/name`) or install codes (`SKIL-XXXX-XXXX`); anything else only emits a warning, the agent still loads, and the ref is **not** installed
- Watch triggers should also provide `event` or `command` (both recommended — `command` as a fallback for when `event` resolution fails)
- All `{{key}}` placeholders should match an input `key` exactly; an unmatched placeholder is left as literal text at runtime
- An Agent with no workflows is valid — it provides only a persona and skill declarations
