# Slash Commands — SME Reference

Comprehensive Subject Matter Expert document covering the slash command system:
registry, parser, autocomplete menu, execution engine, API integrations, and
frontend wiring.

**Status:** Current (Svelte 5 + TypeScript) | **Last updated:** 2026-03-27

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Source Files](#2-source-files)
3. [Data Structures](#3-data-structures)
4. [Command Registry](#4-command-registry)
5. [Parser & Completion](#5-parser--completion)
6. [Execution Engine](#6-execution-engine)
7. [Autocomplete Menu Component](#7-autocomplete-menu-component)
8. [ChatInput Integration](#8-chatinput-integration)
9. [Chat.svelte Wiring](#9-chatsvelte-wiring)
10. [API Endpoints](#10-api-endpoints)
11. [CSS Styling](#11-css-styling)
12. [Dual-Mode Commands](#12-dual-mode-commands)
13. [Adding a New Slash Command](#13-adding-a-new-slash-command)
14. [Known Quirks & Gotchas](#14-known-quirks--gotchas)

---

## 1. Architecture Overview

Slash commands are a **frontend-only** system. No server-side or Rust code is
involved in slash command parsing or routing. The server only participates when
a local handler calls a REST API endpoint, sends a WS message, or when a
command falls through to the agent as a normal chat message.

```
User types "/" in textarea
     │
     ├─ ChatInput.svelte: $effect detects prefix, shows SlashCommandMenu
     │   └─ Arrow keys navigate, Tab/Enter selects, Escape closes
     │
User submits (Enter on input, or selects no-arg command from menu)
     │
     ├─ Chat.svelte: parseSlashCommand(prompt)
     │   └─ Returns { command, args } or null
     │
     ├─ executeSlashCommand(command, args, ctx)
     │   ├─ returns true  → handled locally (system message shown)
     │   └─ returns false → sent to agent as normal WS chat message
```

**Key principle:** Every command has `executeLocal: boolean` in the registry.
Commands with `executeLocal: true` are handled by the frontend executor.
Commands with `executeLocal: false` always fall through to the agent. Some
commands are dual-mode — they handle locally with no args but fall through
with args.

---

## 2. Source Files

| File | Lines | Purpose |
|------|-------|---------|
| `app/src/lib/components/chat/slash-commands.ts` | 87 | Registry (`SLASH_COMMANDS[]`), `SlashCommand` interface, `parseSlashCommand()`, `getSlashCommandCompletions()` |
| `app/src/lib/components/chat/slash-command-executor.ts` | ~400 | `CommandContext` interface, `executeSlashCommand()`, 13 handler functions |
| `app/src/lib/components/chat/SlashCommandMenu.svelte` | 101 | Floating autocomplete menu component |
| `app/src/lib/components/chat/ChatInput.svelte` | 267 | Menu trigger `$effect`, keyboard navigation, selection callback |
| `app/src/lib/components/chat/Chat.svelte` | ~2600 | Slash command interception in `sendMessage()`, `handleSlashSelect()` callback, `CommandContext` construction, WS listeners for command responses |
| `app/src/app.css` | lines 1215–1290 | `.slash-command-menu`, `.slash-command-item`, `.sidebar-rail`, etc. |

---

## 3. Data Structures

### SlashCommand (slash-commands.ts:6–13)

```typescript
export interface SlashCommand {
    name: string;           // Command name without "/" (e.g., "model")
    description: string;    // One-liner for autocomplete menu
    category: 'session' | 'model' | 'info' | 'agent';
    args?: string;          // Hint string shown in menu (e.g., "[name]", "<query>", "on|off")
    argOptions?: string[];  // Fixed valid values (used by /think, /verbose)
    executeLocal: boolean;  // true = frontend handles, false = sent to agent
}
```

### CommandContext (slash-command-executor.ts:9–23)

```typescript
export interface CommandContext {
    messages: { role: string; content: string; timestamp: Date }[];
    chatId: string;
    isLoading: boolean;
    onNewChat: () => void;             // Create new session (POST API)
    onNewSession: () => void;          // Reset current session (WS message)
    onCancel: () => void;              // Cancel active generation
    onToggleDuplex: (() => void) | undefined;  // Toggle voice (may not exist)
    addSystemMessage: (content: string) => void;  // Show markdown in chat
    clearMessages: () => void;         // Clear chat display
    setVerboseMode: (on: boolean) => void;
    setThinkingLevel: (level: string) => void;
    toggleFocusMode: () => void;       // Collapse sidebar to rail / expand
    wsSend: (type: string, data?: Record<string, unknown>) => void;  // Raw WS send
}
```

`CommandContext` is constructed in Chat.svelte at two call sites:
- `handleSlashSelect()` — for no-arg auto-executions from the menu
- `sendMessage()` — for Enter-submitted commands

Both build identical contexts wired to the same Chat.svelte state and callbacks.

---

## 4. Command Registry

21 commands across 4 categories. Defined in `SLASH_COMMANDS` (slash-commands.ts:15–45).

### Session (6 commands, all `executeLocal: true`)

| Command | Args | Handler | Behavior |
|---------|------|---------|----------|
| `/new` | — | `ctx.onNewChat()` | Creates a new companion chat session via `POST /api/v1/chats/companion/new` |
| `/reset` | — | `ctx.onNewSession()` | Resets current session (sends `session_reset` WS, clears local messages) |
| `/clear` | — | `ctx.clearMessages()` | Clear display only (DB untouched) |
| `/stop` | — | `ctx.onCancel()` | Cancel active generation; shows message if idle |
| `/focus` | — | `ctx.toggleFocusMode()` | Toggle sidebar between full and rail mode |
| `/compact` | — | WS `session_compact` | Sends `{ session_id: chatId }`, shows info message; reloads on server completion |

### Model (3 commands, all `executeLocal: true`)

| Command | Args | Handler | Behavior |
|---------|------|---------|----------|
| `/model` | — | `handleModelList()` | Lists all models grouped by provider (API call) |
| `/model` | `<name>` | Agent | Falls through — agent does fuzzy model resolution |
| `/think` | `off\|low\|medium\|high` | `handleThink()` | Validates level, updates `thinkingLevel` state |
| `/verbose` | `on\|off` | `handleVerbose()` | Updates `verboseMode` state |

### Info (6 commands, all `executeLocal: true`)

| Command | Args | Handler | Behavior |
|---------|------|---------|----------|
| `/help` | — | `handleHelp()` | Renders all commands as markdown, grouped by category |
| `/status` | — | `handleStatus()` | Agent connection, ID, uptime, lanes (2 parallel API calls) |
| `/usage` | — | `handleUsage()` | Janus session + weekly token usage with percentages |
| `/export` | — | `handleExport()` | Downloads chat as `chat-export-YYYY-MM-DD.md` (Blob + click) |
| `/lanes` | — | `handleLanes()` | Lane concurrency status (API call) |
| `/search` | `<query>` | `handleSearch()` | LIKE search on chat history, top 10 results with previews |

### Agent (6 commands, mixed execution)

| Command | Args | Execution | Handler | Behavior |
|---------|------|-----------|---------|----------|
| `/skill` | `<name>` | **Agent** | returns false | Always sent to agent for skill activation |
| `/memory` | — | Local | `handleMemory()` | Lists top 15 memories (API call) |
| `/memory` | `<query>` | Local | `handleMemory()` | Searches memories, top 10 results (API call) |
| `/heartbeat` | — | Local | `handleHeartbeat()` | Shows heartbeat config + next automations schedule (API call) |
| `/heartbeat` | `wake` | **Agent** | returns false | Triggers immediate heartbeat via agent |
| `/advisors` | — | Local | `handleAdvisors()` | Lists advisors with role, status, priority (API call) |
| `/voice` | — | Local | `ctx.onToggleDuplex()` | Toggles full-duplex voice; shows error if unavailable |
| `/personality` | — | Local | `handlePersonality()` | Shows personality config (API call) |
| `/wake` | `[reason]` | **Agent** | returns false | Triggers immediate heartbeat with optional reason |

---

## 5. Parser & Completion

### parseSlashCommand (slash-commands.ts:53–75)

```
Input: "/think medium"  →  { command: "think", args: "medium" }
Input: "/help"           →  { command: "help", args: "" }
Input: "hello"           →  null
Input: "/bogus"          →  null
```

**Logic:**
1. Trims input, checks starts with `/`
2. Extracts command name (text before first space)
3. Extracts args (text after first space, trimmed)
4. Validates command exists in `SLASH_COMMANDS` (exact name match)
5. Returns null if not a registered command

### getSlashCommandCompletions (slash-commands.ts:81–86)

Returns matching commands sorted by category order: session(0) → model(1) → info(2) → agent(3).

```
Input: "m"  →  [/model (model), /memory (agent)]
Input: "he" →  [/help (info), /heartbeat (agent)]
Input: ""   →  [all 21 commands, sorted by category]
```

Category sort order defined in `CATEGORY_ORDER` (slash-commands.ts:47):
```typescript
const CATEGORY_ORDER = { session: 0, model: 1, info: 2, agent: 3 };
```

---

## 6. Execution Engine

### executeSlashCommand (slash-command-executor.ts:28–130)

**Signature:** `async (command: string, args: string, ctx: CommandContext) => Promise<boolean>`

**Return value:**
- `true` — command was handled locally; do NOT send to agent
- `false` — command should be sent to agent as a normal chat message

**Switch dispatch** covers all 21 commands. The `default` case returns `false`
(send to agent).

### Handler Functions (slash-command-executor.ts:132–400)

| Function | Async | API Call |
|----------|-------|----------|
| `handleHelp` | No | — |
| `handleThink` | No | — |
| `handleVerbose` | No | — |
| `handleModelList` | Yes | `api.listModels()` |
| `handleStatus` | Yes | `api.getSimpleAgentStatus()` + `api.getLanes()` |
| `handleUsage` | Yes | `api.neboLoopJanusUsage()` |
| `handleExport` | No | — |
| `handleLanes` | Yes | `api.getLanes()` |
| `handleSearch` | Yes | `api.searchChatMessages({ query })` |
| `handleMemory` | Yes | `api.searchMemories({ query })` or `api.listMemories({})` |
| `handleHeartbeat` | Yes | `api.getHeartbeat()` — now returns schedule + crons |
| `handleAdvisors` | Yes | `api.listAdvisors()` |
| `handlePersonality` | Yes | `api.getPersonality()` |

All handlers that call APIs use try/catch and show a "Failed to fetch..." system
message on error. No retries, no loading spinners.

### System Messages

Handlers call `ctx.addSystemMessage(content)` which creates a message with
`role: 'assistant'` and `id: cmd-{Date.now()}`. These appear in the chat as
Nebo messages but are **not persisted** — they exist only in the Svelte
`messages` state array and disappear on page refresh.

---

## 7. Autocomplete Menu Component

### SlashCommandMenu.svelte (101 lines)

**Props:**
```typescript
interface Props {
    query: string;       // Current prefix after "/"
    visible: boolean;    // Show/hide
    onselect: (cmd: SlashCommand) => void;
    onclose: () => void;
}
```

**Exported methods (called by ChatInput via `bind:this`):**
- `navigate(direction: 'up' | 'down')` — circular selection movement
- `selectCurrent(): SlashCommand | null` — returns highlighted command

**Reactive derivations:**
- `completions` = `getSlashCommandCompletions(query)` — recalculates on query change
- `grouped` = completions grouped by category — for rendering category headers

**Auto-behaviors:**
- Selection resets to index 0 when query changes (line 37–40)
- Menu auto-closes if `visible && completions.length === 0` (line 43–47)
- `scrollSelectedIntoView()` on navigation via `requestAnimationFrame` (line 66–71)

**Rendering structure:**
```
.slash-command-menu (absolute, above textarea)
  └─ for each category group:
      .slash-command-category (uppercase header)
      └─ for each command:
          .slash-command-item (.selected class on highlighted)
            .slash-command-name  ("/{name}")
            .slash-command-args  (args hint, if any)
            .slash-command-desc  (description)
```

Mouse hover updates `selectedIndex`. Click calls `onselect`.

---

## 8. ChatInput Integration

### Menu Trigger (ChatInput.svelte:57–72)

A `$effect` watches the `value` binding:

```typescript
$effect(() => {
    if (value.startsWith('/')) {
        const afterSlash = value.slice(1);
        const spaceIndex = afterSlash.indexOf(' ');
        if (spaceIndex === -1) {
            slashMenuQuery = afterSlash;
            slashMenuVisible = getSlashCommandCompletions(afterSlash).length > 0;
        } else {
            slashMenuVisible = false;  // Space typed → hide menu
        }
    } else {
        slashMenuVisible = false;
    }
});
```

**Trigger rules:**
- Menu shows when: input starts with `/` AND no space yet AND completions exist
- Menu hides when: input doesn't start with `/` OR space typed after command name
- This means the menu is ONLY visible while the user is typing the command name

### Keyboard Navigation (ChatInput.svelte:87–113)

When `slashMenuVisible && slashMenuRef`:
- `ArrowDown` → `slashMenuRef.navigate('down')` (prevents default)
- `ArrowUp` → `slashMenuRef.navigate('up')` (prevents default)
- `Tab` or `Enter` (no shift) → `slashMenuRef.selectCurrent()` + `handleSlashSelect()`
- `Escape` → close menu

These handlers are checked **first** in `handleKeydown()`, before any other
keyboard logic (e.g., Shift+Enter for newline, Enter for send).

### Selection Callback (ChatInput.svelte:74–85)

```typescript
function handleSlashSelect(cmd: SlashCommand) {
    if (cmd.args) {
        value = `/${cmd.name} `;  // Trailing space, user types args
    } else {
        value = `/${cmd.name}`;   // No space
        onSlashSelect?.(cmd);     // Auto-execute immediately
    }
    slashMenuVisible = false;
    textareaElement?.focus();
}
```

**Two outcomes:**
1. **Command with args** (e.g., `/model`, `/search`): Replaces input with
   `/{name} `, menu closes, user continues typing the argument, then presses
   Enter to submit.
2. **Command without args** (e.g., `/help`, `/new`): Replaces input with
   `/{name}`, calls `onSlashSelect()` prop **immediately** — this triggers
   `handleSlashSelect()` in Chat.svelte which executes the command. The user
   does NOT need to press Enter.

### Menu DOM Position (ChatInput.svelte:304–311)

```svelte
<SlashCommandMenu
    bind:this={slashMenuRef}
    query={slashMenuQuery}
    visible={slashMenuVisible}
    onselect={handleSlashSelect}
    onclose={() => { slashMenuVisible = false; }}
/>
```

Placed inside the `<div class="relative">` container that wraps the textarea,
so the absolute-positioned menu floats above it via `bottom-full`.

---

## 9. Chat.svelte Wiring

### Two Entry Points

**Entry 1: Menu auto-execute** (no-arg commands selected from menu)
- `ChatInput` calls `onSlashSelect` prop → `Chat.handleSlashSelect(cmd)`
- Builds `CommandContext`, calls `executeSlashCommand(cmd.name, '', ctx)`
- Clears input

**Entry 2: Submit interception** (Enter on typed command)
- `Chat.sendMessage()` calls `parseSlashCommand(prompt)`
- If parsed: builds `CommandContext`, calls `executeSlashCommand()`
- If executor returns `false`: creates user message, calls `handleSendPrompt()`
- If executor returns `true`: done, nothing sent to agent

### CommandContext Construction (Chat.svelte)

Both entry points build the same context:

```typescript
const ctx: CommandContext = {
    messages,
    chatId,
    isLoading,
    onNewChat: newChat,
    onNewSession: resetChat,
    onCancel: cancelMessage,
    onToggleDuplex: undefined,  // or bound to duplex handler
    addSystemMessage,
    clearMessages: () => { messages = []; },
    setVerboseMode: (on) => { verboseMode = on; },
    setThinkingLevel: (level) => { thinkingLevel = level; },
    toggleFocusMode,
    wsSend: (type, data) => client.send(type, data)
};
```

### WS Listeners for Command Responses

Chat.svelte registers listeners for server-side command completions:

- **`session_reset`**: On `success: true` for current session, calls `loadCompanionChat()` to reload fresh state
- **`session_compact`**: On `success: true`, calls `loadCompanionChat()` to show summary; on failure, shows error via `addSystemMessage()`

### Agent Fallthrough (Chat.svelte sendMessage)

When `executeSlashCommand()` returns `false`:

```typescript
executeSlashCommand(parsed.command, parsed.args, ctx).then((handled) => {
    if (!handled) {
        // Create user message with original text (e.g., "/skill web-search")
        const userMessage = { id: generateUUID(), role: 'user', content: prompt, ... };
        messages = [...messages, userMessage];
        handleSendPrompt(prompt);  // Sends via WS as normal chat
    }
});
```

The agent receives the raw slash command text (e.g., `/skill web-search`,
`/model sonnet`, `/wake check email`). The agent's tool system, STRAP routing,
or steering generators handle interpretation.

---

## 10. API Endpoints

All called from `$lib/api/nebo` via GET/POST requests. These are the Rust server
REST endpoints that slash commands query.

| Command | Endpoint | Response |
|---------|----------|----------|
| `/new` | `POST /api/v1/chats/companion/new` | `{ chat: Chat, messages: [], totalMessages: 0 }` |
| `/model` (no args) | `GET /api/v1/models` | `{ models: { [provider]: Model[] }, aliases: Alias[] }` |
| `/status` | `GET /api/v1/agent/status` | `{ connected, agentId, uptime }` |
| `/status` + `/lanes` | `GET /api/v1/agent/lanes` | `{ message: string }` |
| `/usage` | `GET /api/v1/neboloop/janus/usage` | `{ session: Usage, weekly: Usage }` |
| `/search` | `GET /api/v1/chats/search?query=...` | `{ messages: SearchResult[], total: number }` |
| `/memory` (list) | `GET /api/v1/memories` | `{ memories: Memory[], total: number }` |
| `/memory` (search) | `GET /api/v1/memories/search?query=...` | `{ memories: Memory[], total: number }` |
| `/heartbeat` | `GET /api/v1/agent/heartbeat` | `{ content, enabled, intervalMinutes, window, crons }` |
| `/advisors` | `GET /api/v1/agent/advisors` | `{ advisors: Advisor[] }` |
| `/personality` | `GET /api/v1/setup/personality` | `{ content: string }` |
| `/lanes` | `GET /api/v1/agent/lanes` | `{ message: string }` |

**WebSocket messages sent by commands:**
| Command | WS Type | Payload |
|---------|---------|---------|
| `/reset` | `session_reset` | `{ session_id: chatId }` |
| `/compact` | `session_compact` | `{ session_id: chatId }` |

**WebSocket responses listened for:**
| WS Type | Trigger | Action |
|---------|---------|--------|
| `session_reset` | Server completes reset | `loadCompanionChat()` to reload |
| `session_compact` | Server completes compaction | `loadCompanionChat()` on success; error message on failure |

**No API call (pure client-side):**
`/clear`, `/stop`, `/focus`, `/help`, `/think`, `/verbose`, `/export`, `/voice`

---

## 11. CSS Styling

All styles in `app/src/app.css`. No inline styles, no `<style>` blocks.

### Slash Command Menu (lines 1215–1246)

```css
.slash-command-menu {
    @apply absolute bottom-full left-0 right-0 mb-2 z-20;
    @apply bg-base-100 border border-base-300 rounded-xl shadow-lg overflow-hidden;
    max-height: 320px;
    overflow-y: auto;
}

.slash-command-item {
    @apply flex items-start gap-3 px-4 py-2.5 cursor-pointer transition-colors duration-75;
}

.slash-command-item:hover,
.slash-command-item.selected {
    @apply bg-base-200;
}

.slash-command-name {
    @apply text-base font-mono font-medium text-primary;
}

.slash-command-args {
    @apply text-sm font-mono text-base-content/60 ml-1;
}

.slash-command-desc {
    @apply text-sm text-base-content/60;
}

.slash-command-category {
    @apply text-sm font-semibold uppercase tracking-wider text-base-content/60 px-4 pt-3 pb-1;
}
```

### Sidebar Rail (focus mode, lines 1248–1290)

```css
.sidebar-rail .sidebar-container {
    width: 64px !important;
    min-width: 64px !important;
    overflow: hidden;
    transition: width 0.2s ease;
}
```

Hides text elements (header title/subtitle, bot info/name/role, labels, badges,
dividers, channels, loop headers) via `display: none !important`. Centers bot
icons and shows an expand hamburger button at the top of the rail.

The `.sidebar-expand-btn` is hidden by default and shown only inside `.sidebar-rail`.

**Layout:**
- Menu positioned `absolute bottom-full` — floats above textarea
- Max height 320px with scroll overflow
- `z-20` ensures it's above other UI elements
- `rounded-xl shadow-lg` for floating card appearance
- Uses DaisyUI/Tailwind `base-100`/`base-200`/`base-300` theme tokens

---

## 12. Dual-Mode Commands

Three commands behave differently based on whether args are provided:

### /model
- **No args** → Local: `handleModelList()` fetches `GET /api/v1/models` and displays grouped list
- **With args** (e.g., `/model sonnet`) → Agent: falls through for fuzzy model resolution

### /heartbeat
- **No args** → Local: `handleHeartbeat()` fetches `GET /api/v1/agent/heartbeat` and displays config + automation schedule
- **`wake`** → Agent: falls through to trigger immediate heartbeat

### /memory
- **No args** → Local: `handleMemory('')` fetches `GET /api/v1/memories`, lists top 15
- **With args** (e.g., `/memory cats`) → Local: `handleMemory('cats')` searches via `GET /api/v1/memories/search?query=cats`, shows top 10

Note: `/memory` is dual-mode but **always local** — both paths are handled by `handleMemory()`. The distinction is list vs. search.

---

## 13. Adding a New Slash Command

**Step 1:** Add entry to `SLASH_COMMANDS` array in `slash-commands.ts`:

```typescript
{ name: 'mycommand', description: 'Does something', category: 'info', args: '[arg]', executeLocal: true },
```

- Choose `category` based on function: `session` (lifecycle), `model` (LLM config), `info` (read-only queries), `agent` (agent features)
- Set `executeLocal: false` if the agent should handle it
- Add `argOptions` array if args have fixed valid values

**Step 2:** Add case to switch in `executeSlashCommand()` in `slash-command-executor.ts`:

```typescript
case 'mycommand':
    return await handleMyCommand(args, ctx);
```

**Step 3:** Write handler function in `slash-command-executor.ts`:

```typescript
async function handleMyCommand(args: string, ctx: CommandContext): Promise<boolean> {
    try {
        const res = await api.myEndpoint();
        ctx.addSystemMessage(`**My Command**\n\n${res.data}`);
    } catch {
        ctx.addSystemMessage('Failed to fetch data.');
    }
    return true;
}
```

**Step 4:** If the handler needs a new API call, add it to `$lib/api/nebo.ts`.

**Step 5:** If the command needs to react to a server-side response, add a WS listener
in Chat.svelte's onMount subscription block.

**That's it.** The autocomplete menu, parsing, keyboard navigation, and
Chat.svelte wiring are all automatic — they read from the `SLASH_COMMANDS`
array.

---

## 14. Known Quirks & Gotchas

### /new creates a new DB session, /reset clears the current one
`/new` calls `ctx.onNewChat()` which creates a new companion chat via
`POST /api/v1/chats/companion/new`. The old session remains accessible via
Settings > Sessions. `/reset` calls `ctx.onNewSession()` which sends a
`session_reset` WS message and clears local state.

### /compact triggers server-side summarization with feedback
`/compact` sends `session_compact` WS message. The frontend listens for
the `session_compact` response and reloads the chat on success, or shows
an error message on failure.

### /focus collapses sidebar to a 64px rail
Instead of fully hiding the sidebar, `/focus` applies `.sidebar-rail` class
which narrows to 64px, hides text, and centers avatar icons. An expand
hamburger button at the top of the rail restores full width.

### /heartbeat shows automation schedule
The heartbeat endpoint now returns `enabled`, `intervalMinutes`, `window`,
and `crons` (enabled cron jobs with computed next run times). The frontend
handler formats this into a schedule display.

### System messages are ephemeral
Messages created by `addSystemMessage()` use role `'assistant'` and ID
`cmd-{timestamp}`. They are **not saved to the database** — they only exist
in the Svelte `messages` state. Refreshing the page loses them.

### Menu visibility tied to space character
The menu disappears as soon as the user types a space after the command name
(ChatInput.svelte:66–67). This means for commands with args, the user types
blindly after the space — no argument completion or validation in the menu.

### No server-side slash command handling
The Rust codebase has zero references to slash commands. All 21 commands are
purely frontend. Agent-handled commands (skill, model+args, heartbeat wake,
wake) are sent as raw text chat messages — the agent interprets them through
its normal prompt/tool pipeline.

### Export downloads to browser
`/export` creates a Blob and triggers a browser download. In Tauri mode this
downloads to the OS default download folder. It does NOT use the server's file
system.

### handleStatus uses parallel API calls
Status fetches both `/api/v1/agent/status` and `/api/v1/agent/lanes` via
`Promise.all()`. All other handlers make single sequential API calls.

### Voice command requires duplex support
`/voice` checks `ctx.onToggleDuplex` existence before calling it. In
Chat.svelte, `onToggleDuplex` is currently set to `undefined` in some
contexts, meaning `/voice` will show "Voice is not available." unless duplex
is wired up.

### Category sort is stable
Commands are always presented session → model → info → agent, regardless of
search query. Within a category, commands appear in registry order (the order
they're defined in the `SLASH_COMMANDS` array).

### Multi-session support
The DB unique index on `chats(user_id)` was dropped in migration `0068_multi_session.sql`.
Multiple companion chats can now coexist. `get_companion_chat` returns the most
recent one (`ORDER BY updated_at DESC LIMIT 1`).
