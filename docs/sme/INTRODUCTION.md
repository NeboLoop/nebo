# Introduction Skill — SME Deep-Dive

> **Scope:** First-meeting conversational experience — skill definition, trigger pipeline, ask widgets, skill installs, memory storage, onboarding integration, and the full request→agent→frontend chain.

---

## Architecture Overview

The introduction is a **skill-driven conversational flow** that runs after the onboarding wizard completes. Onboarding handles technical setup (provider, permissions, NeboLoop OAuth). Introduction handles the *human* first-touch — building rapport, orienting the user, and installing personalized skills.

**Two distinct stages:**
- **Onboarding** (`OnboardingFlow.svelte`) — Technical setup wizard, full-screen overlay, gates app access
- **Introduction** (`extensions/skills/introduction/SKILL.md`) — Conversational first meeting, runs inside the chat UI

**Design principle:** The introduction should feel like five minutes with someone who already gets you — not a product tour.

---

## File Map

| File | Purpose |
|------|---------|
| `extensions/skills/introduction/SKILL.md` | The skill definition (v0.1.4) — frontmatter + 4-part template |
| `internal/agent/skills/skill.go` | Skill struct, `ParseSkillMD()` — YAML frontmatter + markdown body |
| `internal/agent/skills/loader.go` | Loads embedded skills from `extensions/skills/`, hot-reload via fsnotify |
| `internal/agent/tools/skill_tool.go` | `SkillDomainTool` — `ForceLoadSkill()`, `AutoMatchSkills()`, `ActiveSkillContent()` |
| `internal/agent/tools/neboloop_tool.go` | `store` tool — skill install via `RedeemSkillCode()` |
| `internal/agent/tools/agent_tool.go` | `agent` tool — `messageAsk()` with blocking `AskCallback`, `AskWidget` struct |
| `internal/agent/runner/runner.go` | Force-load decision (lines 423–437), belt-and-suspenders safeguard (lines 1112–1123) |
| `internal/agent/memory/dbcontext.go` | `NeedsOnboarding()` — checks `user_profiles.onboarding_completed` |
| `internal/realtime/chat.go` | `handleRequestIntroduction()` — routes browser request to agent hub |
| `internal/realtime/client.go` | `request_introduction` WebSocket message handler, `ask_response` handler |
| `cmd/nebo/agent.go` | `handleIntroduction()` (line 2596) — agent-side intro logic, dedup via `sync.Map` |
| `app/src/routes/(app)/agent/+page.svelte` | `requestIntroduction()` — frontend trigger on empty chat |
| `app/src/lib/components/chat/AskWidget.svelte` | Renders 6 widget types (buttons, text_input, select, confirm, radio, checkbox) |

---

## Skill Definition

**File:** `extensions/skills/introduction/SKILL.md`

```yaml
name: introduction
description: First meeting — make them feel seen, set them up for success
version: "0.1.4"
priority: 100          # Highest — runs before other skills
max_turns: 8           # Auto-expires after 8 turns of inactivity
triggers:
  - hello, hi, hey, start, help me get started
  - who are you, what can you do, introduce yourself
tools:
  - agent              # ask widgets + memory storage
  - store              # skill installs from NeboLoop catalog
metadata:
  nebo:
    emoji: "👋"
```

### Four-Part Flow (Mandatory Order)

| Part | Purpose | Exchanges | Key Rule |
|------|---------|-----------|----------|
| 1 — Connection | Build rapport: name, location, work | 3 | One question per message, 1-2 sentences max |
| 2 — Orientation | Explain how Nebo works | 1 message | Apple-like writing. **Mandatory — never skip** |
| 3 — Skill Picker | Recommend & install 3-4 personalized skills | Interactive | Always 3-4 options + "Skip for now" in single widget |
| 4 — Handoff | Warm close, let user come to agent | 1 message | No CTA, no pitch. Then STOP. |

### Part 1 — The Connection

Three exchanges to build emotional attachment via "unexpected understanding":

1. **Name** → First message: `"Hi! I'm Nebo."` + ask widget (`text_input`). Greet by name, ask location (plain text).
2. **Location** → React genuinely (not "cool!"). Ask what they do (plain text).
3. **Work** → Reflect back something they *didn't* say — the emotional truth underneath the facts.

Transition: `"Before I get out of your way — quick rundown on how things work, so nothing catches you off guard."`

### Part 2 — Orientation

One message. Short declarative sentences. Fragments that breathe.

Topics: lives on your computer (not cloud), windows may open/close (that's me working), approval prompts (you control), persistent memory (never repeat yourself).

End: `"One more thing — let me set you up."`

### Part 3 — Skill Picker

Map user's job/role to 3-4 skills from the catalog. Present via ask widget with buttons. Install silently via store tool.

**Skill Catalog (11 skills):**

| Skill | Install Code | Best For |
|-------|-------------|----------|
| Content Creator | `SKILL-F639-PJ5J-WT3W` | Writers, marketers |
| Family Hub | `SKILL-DSJ8-H4XG-ESP4` | Parents, family coordinators |
| Health & Wellness | `SKILL-7KRC-4JT8-N8VX` | Fitness, nutrition, habits |
| Interview Prep | `SKILL-ENXP-YGJZ-9GUN` | Job seekers |
| Job Search Coach | `SKILL-LNWY-Q7W2-KHVN` | Actively job hunting |
| Personal Finance | `SKILL-T5JE-JQLA-YJ5E` | Budgets, bills, savings |
| Research Assistant | `SKILL-GLXB-NNHJ-ZKCG` | Students, analysts |
| Small Business Ops | `SKILL-BVS3-UDJ3-C2JX` | Small business owners, freelancers |
| Student Learning | `SKILL-LLFN-BLT8-39GV` | Students at any level |
| Support Operations | `SKILL-TY54-HP5S-339D` | Customer support, ops |
| Travel Planner | `SKILL-YCST-9FLL-FL9V` | Travelers, trip planners |

### Part 4 — Handoff

`"That's it. Put me to work whenever you're ready."`

Then STOP. Let user come to agent.

---

## Trigger Pipeline

### How Introduction Gets Activated

There are **two paths** to activation:

**Path 1 — Force-load on first run (automatic):**

```
Browser loads /agent → empty chat detected → requestIntroduction()
  → WebSocket: "request_introduction" → realtime/client.go
  → handleRequestIntroduction() → agenthub Frame{Method: "introduce"}
  → cmd/nebo/agent.go: handleIntroduction()
  → runner.Run(RunRequest{ForceSkill: "introduction"})
  → runner.go line 426: skillProvider.ForceLoadSkill(sessionKey, "introduction")
```

**Path 2 — Auto-match on trigger words (re-trigger):**

```
User types "hello" or "introduce yourself"
  → runner.go line 441: skillProvider.AutoMatchSkills(sessionKey, "hello")
  → Trigger match → brief hint injected into system prompt
  → LLM decides to call skill(name: "introduction")
  → recordInvocation() → full template injected via ActiveSkillContent()
```

### Frontend Trigger Logic (`agent/+page.svelte`)

The frontend requests introduction when it detects an empty chat:

1. On page load, sends `check_stream` to see if there's an active response
2. If `check_stream` returns no active stream AND `messages.length === 0`:
   - Calls `requestIntroduction()` (line 1202)
3. **Fallback:** 5-second timeout on `check_stream` — if no response and chat is empty, requests introduction anyway (line 507)

```typescript
function doRequestIntroduction() {
    const client = getWebSocketClient();
    isLoading = true;
    client.send('request_introduction', { session_id: chatId || '' });
}
```

### Server-Side Routing (`realtime/chat.go:562`)

```go
func handleRequestIntroduction(c *Client, msg *Message, chatCtx *ChatContext) {
    // Wait up to 5s for agent to connect (handles startup race)
    agent := waitForAgent(chatCtx.hub, 5*time.Second)
    // Create pending request with marker prompt "__introduction__"
    requestID := fmt.Sprintf("intro-%d", time.Now().UnixNano())
    // Send Frame{Type: "req", Method: "introduce"} to agent hub
}
```

### Agent-Side Handler (`cmd/nebo/agent.go:2596`)

```go
func handleIntroduction(ctx, state, runner, sessions, requestID, sessionKey, userID) {
    // 0. Early exit: check onboarding_completed via memory.LoadContext
    //    If already onboarded → skip entirely (prevents re-introduction on page reload)
    // 1. Dedup via sync.Map — one introduction per session at a time
    // 2. Check for real user messages (skip if conversation exists)
    //    Filters out system-origin messages: heartbeats, triggers, "[User ..." prefixes
    // 3. Load DBContext → check if user is known
    //    Known user (has display_name): warm greeting by name, no introduction skill
    //    New user: ForceSkill = "introduction", runs full 4-part flow
    // 4. Stream events back via sendFrame
}
```

---

## Runner Integration

### Force-Load Decision (`runner.go:423-437`)

```go
if r.skillProvider != nil {
    if forceSkill != "" {
        // Explicit force-load (from handleIntroduction or RunRequest.ForceSkill)
        r.skillProvider.ForceLoadSkill(sessionKey, forceSkill)
    } else if needsOnboarding {
        // Fallback: auto-load for users who haven't completed onboarding
        // Only if session has NO existing messages (prevents loop)
        existingMsgs, _ := r.sessions.GetMessages(sessionID, 1)
        if len(existingMsgs) == 0 {
            r.skillProvider.ForceLoadSkill(sessionKey, "introduction")
        }
    }
}
```

### Belt-and-Suspenders Safeguard (`runner.go:1112-1123`)

After the agentic loop completes, if onboarding was needed and the session now has 4+ messages, mark `onboarding_completed = 1` programmatically:

```go
if needsOnboarding && userID != "" {
    if msgs, err := r.sessions.GetMessages(sessionID, 0); err == nil && len(msgs) >= 4 {
        r.sessions.GetDB().Exec(
            "UPDATE user_profiles SET onboarding_completed = 1, updated_at = ? WHERE user_id = ?",
            time.Now().Unix(), userID,
        )
    }
}
```

This prevents the introduction from looping forever if the LLM fails to store memories or the skill install doesn't complete.

### Onboarding Detection (`memory/dbcontext.go`)

```go
func (c *DBContext) NeedsOnboarding() bool {
    return c.OnboardingNeeded
}
```

Logic:
- No `user_profiles` row → `true`
- Row exists, `onboarding_completed` is NULL or 0 → `true`
- Row exists, `onboarding_completed` = 1 → `false`
- Query error → `true` (fail-open to ensure new users get introduced)

---

## Skill Lifecycle

### Constants (`skill_tool.go:17-26`)

```go
DefaultSkillTTL     = 4      // Turns before auto-matched skills expire
ManualSkillTTL      = 6      // Turns before manually/force-loaded skills expire
MaxActiveSkills     = 4      // Hard cap on concurrent active skills per session
MaxSkillTokenBudget = 16000  // Character budget for combined active skill content
```

### ForceLoadSkill (`skill_tool.go:490-509`)

```go
func (t *SkillDomainTool) ForceLoadSkill(sessionKey, skillName string) bool {
    // Looks up skill in entries map
    // Calls recordInvocation(sessionKey, skillName, true)  ← manual=true → ManualSkillTTL
    // Returns true if found and loaded
}
```

Records the skill as a "manual" invocation — stickier TTL (6 turns vs 4). Captures a snapshot of the skill template at invocation time (survives hot-reload edits mid-session).

### ActiveSkillContent (`skill_tool.go:346-389`)

Returns concatenated templates for all invoked skills in the session. Sorted by most recently invoked. Capped by `MaxSkillTokenBudget` (16,000 chars).

Injected into the system prompt as:

```
## Invoked Skills

The following skills were invoked in this session. Continue to follow their guidelines:

### Skill: introduction

[full SKILL.md template content]
```

### Skill Loading (`skills/loader.go`)

Embedded skills from `extensions/skills/` are loaded via `LoadFromEmbedFS()`:
- Walks the embedded filesystem looking for `SKILL.md` files
- Parses YAML frontmatter + markdown body via `ParseSkillMD()`
- Skips platform-mismatched skills
- Validates and stores in `skills` map

Hot-reload via fsnotify watches all skill directories. On WRITE/CREATE: reloads and validates. On DELETE/RENAME: removes from map. Calls `onChange` callback to sync with the skill tool registry.

---

## Tool Mechanisms

### Ask Widgets (`agent_tool.go`)

**AskWidget struct:**
```go
type AskWidget struct {
    Type    string   `json:"type"`              // "buttons", "select", "text_input", "confirm", "radio", "checkbox"
    Label   string   `json:"label,omitempty"`
    Options []string `json:"options,omitempty"`
    Default string   `json:"default,omitempty"` // Placeholder for text_input
}
```

**Usage by introduction skill:**
```
// Name prompt (Part 1)
agent(resource: message, action: ask, prompt: "What's your name?", widgets: [{type: "text_input", default: "Your name"}])

// Skill picker (Part 3)
agent(resource: message, action: ask, prompt: "Pick any that sound useful...", widgets: [{type: "buttons", options: ["Research Assistant", "Small Business Ops", "Personal Finance", "Skip for now"]}])
```

**Execution flow:**

1. `messageAsk()` validates prompt + widgets (defaults to confirm yes/no if none)
2. Generates UUID `requestID`
3. Calls `t.askCallback(ctx, requestID, prompt, widgets)` — **blocks until user responds**
4. Returns user's response as plain text string
5. CLI fallback: returns error `"Interactive prompts require the web UI"`

**WebSocket pipeline:**

```
agent_tool.go → askCallback(requestID, prompt, widgets)  [BLOCKS]
    → agenthub: sends ask frame to hub
    → chat.go: handleAskRequest() → stores requestID → broadcasts to all clients
    → AskWidget.svelte: renders widget → user interacts → submits
    → WebSocket: "ask_response" {request_id, value}
    → chat.go: handleAskResponse() → hub.SendAskResponse(agentID, requestID, value)
    → askCallback UNBLOCKS → returns value to agent
```

### Store Tool — Skill Install (`neboloop_tool.go`)

**Install code format:** `SKILL-XXXX-XXXX-XXXX` (20 chars, uppercase alphanumeric segments)

```go
func isSkillInstallCode(id string) bool {
    // Length must be 20
    // Must start with "SKILL-"
    // Dashes at positions 10 and 15
    // All other chars uppercase A-Z or 0-9
}
```

**Install flow:**
```go
func (t *NeboLoopTool) installSkill(ctx, client, params) {
    if isSkillInstallCode(params.ID) {
        // Redeem via NeboLoop API — resolves code to skill UUID, downloads SKILL.md
        resp, err := client.RedeemSkillCode(ctx, params.ID)
    } else {
        // Direct install by UUID
        resp, err := client.InstallSkill(ctx, params.ID)
    }
}
```

### Memory Storage (`agent_tool.go`)

During introduction, the skill stores 4 tacit memories silently:

```
agent(resource: memory, action: store, key: "user/name", value: "Alice", layer: "tacit")
agent(resource: memory, action: store, key: "user/location", value: "Denver", layer: "tacit")
agent(resource: memory, action: store, key: "user/work", value: "Real estate agent", layer: "tacit")
agent(resource: memory, action: store, key: "user/timezone", value: "America/Denver", layer: "tacit")
```

**Critical rule:** Memory operations are **invisible**. The skill explicitly forbids narrating saves — no "I've made a note" or "I'll remember that."

---

## Frontend Rendering

### AskWidget Component (`chat/AskWidget.svelte`)

Renders 6 widget types:

| Type | UI Control | Submit Behavior |
|------|-----------|-----------------|
| `buttons` | Button per option | Click any button → immediate submit |
| `confirm` | Yes/No buttons | Click → immediate submit |
| `select` | Dropdown | Select + submit button |
| `text_input` | Text field | Enter or submit button |
| `radio` | Radio button group | Select + submit button |
| `checkbox` | Checkbox group | Check any + submit button (shows count) |

After submit, shows response badge and disables re-submission.

### MessageGroup Integration (`chat/MessageGroup.svelte`)

Content blocks with `type: "ask"` render as `<AskWidget>`. The `onAskSubmit` callback sends `{type: "ask_response", data: {request_id, value}}` via WebSocket.

---

## End-to-End Flow

### New User First Chat

```
1. User completes onboarding wizard (provider, terms, permissions)
   → user_profiles.onboarding_completed stays 0 (wizard done, agent intro pending)

2. Browser navigates to /agent
   → Loads empty companion chat
   → check_stream returns no active stream
   → messages.length === 0 → requestIntroduction()

3. WebSocket: "request_introduction" {session_id}
   → realtime/client.go routes to handleRequestIntroduction()
   → Waits up to 5s for agent to connect
   → Creates pending request with marker "__introduction__"
   → Sends Frame{Method: "introduce"} to agent hub

4. cmd/nebo/agent.go: handleIntroduction()
   → Dedup check via introductionInProgress sync.Map
   → Checks for real user messages (skips if conversation exists)
   → New user: sets ForceSkill = "introduction"
   → Calls runner.Run(RunRequest{ForceSkill: "introduction", Origin: OriginSystem})

5. runner.go: prepareSystemPrompt()
   → ForceLoadSkill(sessionKey, "introduction") → recorded as manual (TTL=6)
   → ActiveSkillContent() → full SKILL.md injected into system prompt

6. Agent executes introduction skill
   Part 1: "Hi! I'm Nebo." + ask widget (text_input for name)
   → AskCallback blocks → AskWidget renders → user types name → unblocks
   → Stores: agent(resource: memory, action: store, key: "user/name", ...)
   → Asks location (plain text), then work (plain text)
   → Reflects back emotional truth

   Part 2: Orientation message (Apple-style writing)

   Part 3: Skill picker
   → ask widget (buttons: 3-4 skills + "Skip for now")
   → User picks → store(resource: "skills", action: "install", id: "SKILL-XXXX-XXXX-XXXX")
   → Silent confirm

   Part 4: "That's it. Put me to work whenever you're ready."

7. Runner safeguard (line 1112): session has 4+ messages
   → UPDATE user_profiles SET onboarding_completed = 1

8. Next page load: NeedsOnboarding() → false → introduction never force-loads again
```

### Returning Known User

```
Browser loads empty chat → requestIntroduction()
  → handleIntroduction() checks DBContext
  → UserDisplayName exists (stored during previous intro)
  → Warm greeting by name: "Hey Alice, good to see you. What can I help with?"
  → No skill loaded — just a regular greeting
```

### Re-triggering Introduction

User types "introduce yourself" or "who are you" in any session:
- `AutoMatchSkills()` detects trigger match
- Returns brief hint in system prompt
- LLM may invoke `skill(name: "introduction")` → full template loaded
- Runs the 4-part flow again (but safeguard won't re-flip onboarding flag since already 1)

---

## Deduplication & Safety

### Introduction Dedup (`cmd/nebo/agent.go:2591-2613`)

```go
var introductionInProgress sync.Map

// Only one introduction per session at a time
if _, running := introductionInProgress.LoadOrStore(sessionKey, true); running {
    // Skip duplicate, send skipped=true response
    return
}
defer introductionInProgress.Delete(sessionKey)
```

### Real Message Detection (`cmd/nebo/agent.go:2628-2657`)

Before running introduction, checks last 10 messages for "real" user messages. Filters out system-origin prefixes:
- `"You are running a scheduled"` (heartbeat/cron)
- `"[New user just opened"` (intro trigger)
- `"[User "` (greeting trigger)

If any real user message exists → skip introduction.

### Loop Prevention (`runner.go:1112-1123`)

Belt-and-suspenders: after 4+ messages in a session that started with `needsOnboarding=true`, force-mark `onboarding_completed = 1`. This prevents infinite introduction loops even if the LLM never calls the store tool or memory storage fails.

---

## Anti-Patterns (from SKILL.md)

The skill explicitly warns against these — they are coded into the template the LLM receives:

- No empty flattery reactions
- No recapping facts (parroting ≠ understanding)
- No canned availability phrases ("I'm here whenever you need me!")
- No transactional openers ("What would you like help with?")
- No dramatic emotional language
- No bullet point walls
- No ominous caution tone — matter-of-fact, not scary
- No dumping full skill catalog — curate 3-4 based on what you learned
- **No narrating memory saves** — zero commentary, completely invisible
- No inventing facts or fictional scenarios
- **No skipping Part 2** — orientation is mandatory before skill picker
- No offering only 1-2 skill options — always exactly 3-4 + "Skip for now"

---

## Gotchas & Known Issues

1. **Introduction loop bug (fixed in `d964349`):** If the LLM skipped Part 2 orientation, it confused the model into repeating the introduction. Fix was the belt-and-suspenders safeguard + better skill template structure.

2. **Ask state is not persistent.** If the browser refreshes mid-ask-widget, the widget is lost. User must re-send a message to re-trigger the exchange.

3. **CLI mode has no ask widgets.** `messageAsk()` returns error: `"Interactive prompts require the web UI"`. The skill says to fall back to plain text conversation, but this fallback is in the template instructions only — no code-level graceful degradation.

4. **Introduction dedup is per-session, not per-user.** The `introductionInProgress` sync.Map keys on session key. In single-user mode this is fine. Multiple users on same machine could theoretically race, but Nebo is single-user by design.

5. **Onboarding detection fail-open.** If the `user_profiles` query errors (e.g., migration not run), `NeedsOnboarding()` returns `true`. This means a database issue could trigger an unwanted introduction.

6. **5-second agent wait.** `waitForAgent()` in `handleRequestIntroduction()` blocks up to 5 seconds for the agent WebSocket to connect. On slow startup, this handles the race condition where the frontend loads before the agent connects.

7. **Skill install requires NeboLoop connection.** The `store` tool calls `client.RedeemSkillCode()` which hits the NeboLoop API. If user chose a non-Janus provider and skipped NeboLoop during onboarding, skill installs will fail silently during introduction Part 3.
