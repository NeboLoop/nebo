# Creating Nebo Skills

## What Skills Are

Skills are instruction sets that shape how Nebo behaves during a conversation. When a skill is active, its content is injected directly into the system prompt — guiding the agent's tone, methodology, domain expertise, and tool usage for that session.

**Skills are not code.** They're markdown documents with YAML metadata. No compilation, no binary, no gRPC. A skill is a `SKILL.md` file in a directory. That's it.

**Skills are contextual and temporary.** They activate when relevant (via trigger matching or manual load), persist for a few turns, and expire when no longer needed. This keeps the agent's context lean — only the guidance that matters right now is in the prompt.

**Skills complement apps.** Apps provide executable capabilities (tools, channels, gateways). Skills provide orchestration guidance — how to use those tools effectively. A calendar app gives Nebo the ability to read and write events. A "meeting prep" skill tells Nebo *how* to prepare for meetings using that calendar app, email, and web search together.

## Skills vs Apps

| | Skills | Apps |
|---|--------|------|
| **Format** | Markdown (SKILL.md) | Compiled binary (.napp) |
| **Runtime** | Injected into system prompt | Sandboxed process over gRPC |
| **Provides** | Behavioral guidance, methodology | Tools, channels, UI panels |
| **Lifetime** | Session-scoped, TTL-based expiry | Always running |
| **Creation** | Write a markdown file | Write, compile, sign, package |
| **Security** | No permissions needed | Deny-by-default sandbox |

**When to use a skill:** You want to change *how* Nebo approaches a task — its methodology, tone, step sequence, or domain expertise.

**When to use an app:** You need new capabilities — API integrations, data processing, persistent state, or custom UI.

---

## Quick Start

Create a directory in your Nebo skills folder with a `SKILL.md` file:

```
~/.config/nebo/skills/           # Linux
~/Library/Application Support/Nebo/skills/   # macOS
%AppData%\Nebo\skills\           # Windows
```

### 1. Create the directory

```bash
mkdir -p ~/.config/nebo/skills/my-skill
```

### 2. Write the SKILL.md

```markdown
---
name: my-skill
description: One-line description of what this skill does
version: "1.0.0"
triggers:
  - keyword that activates this skill
  - another trigger phrase
---

# My Skill

Instructions for Nebo when this skill is active.

Tell the agent what to do, how to approach problems,
what tools to use, and what tone to take.
```

### 3. Done

Nebo watches the skills directory with `fsnotify`. Your skill is available immediately — no restart needed. Say something that matches a trigger, and it activates automatically.

### Or create via the agent

Ask Nebo directly:

> "Create a skill for writing blog posts"

Nebo will use `skill(action: "create", content: "...")` to write the SKILL.md file for you.

---

## SKILL.md Format

Every skill is a single `SKILL.md` file with two parts:

1. **YAML frontmatter** — metadata between `---` markers
2. **Markdown body** — the actual instructions injected into the system prompt

### Full Field Reference

```yaml
---
# REQUIRED
name: meeting-prep                    # Unique identifier (becomes the slug)
description: Prepare briefings for upcoming meetings  # One-liner for the catalog

# OPTIONAL
version: "1.0.0"                      # Semver for tracking updates
author: Jane Smith                    # Skill author
priority: 20                          # Higher = matched first (default: 0)
max_turns: 6                          # Turns of inactivity before auto-expiry (default: 4)

triggers:                             # Phrases that auto-activate this skill
  - meeting
  - prepare for
  - briefing
  - agenda

tools:                                # Tools this skill expects to use
  - read
  - web
  - memory

dependencies:                         # Other skills that must be installed
  - calendar-expert

tags:                                 # For categorization and discovery
  - productivity
  - meetings

metadata:                             # Arbitrary key-value data
  nebo:
    emoji: "📋"
---

# Meeting Prep

Your markdown instructions go here...
```

### Field Details

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `name` | Yes | — | Unique identifier. Used as the skill slug after sanitization (lowercase, hyphens only). Must be unique across all loaded skills. |
| `description` | Yes | — | One-line description shown in the skill catalog. Keep it concise — the agent sees this when deciding which skill to use. |
| `version` | No | `"1.0.0"` | Semver string for tracking updates. |
| `author` | No | — | Author name. Used in catalog display and future marketplace. |
| `priority` | No | `0` | Integer. Higher priority skills are matched first when multiple triggers fire on the same message. The bundled `onboarding` skill uses priority 100 to ensure it matches greeting triggers before other skills. |
| `max_turns` | No | `4` | How many turns of inactivity (no trigger re-match, no explicit invocation) before the skill auto-expires from the session. Set to `1` for one-shot skills like onboarding. |
| `triggers` | No | `[]` | List of phrases. If a user message contains any trigger (case-insensitive substring match), the skill appears as a hint in the system prompt. The agent must still explicitly invoke it — triggers don't auto-inject the full template. |
| `tools` | No | `[]` | List of tool names this skill expects to use. Currently informational — not enforced at runtime. Helps the agent understand what capabilities the skill needs. |
| `dependencies` | No | `[]` | List of skill names that must be installed. Currently informational — not enforced at runtime. |
| `tags` | No | `[]` | Categorization strings. Used in catalog display and future search/filtering. |
| `metadata` | No | `{}` | Arbitrary YAML map. The `nebo.emoji` convention is used by the UI to display a skill icon. |

---

## How Skills Activate

Skills follow a two-phase activation model: **hint then invoke**.

### Phase 1: Trigger Matching (Automatic)

Every time a user sends a message, Nebo scans all registered skills for trigger matches:

```
User message: "Can you review my authentication code?"
                         ^^^^^^
                         matches "review" trigger on code-review skill
```

Matching skills appear as **hints** in the system prompt:

```
## Skill Matches

These skills may be relevant to the user's message.
Use `skill(name: "...")` to activate one:
- **code-review** — Provides structured code review capabilities
```

The agent sees the hint and decides whether to invoke the skill. Hints are lightweight — just the name and description, not the full template.

**Trigger matching rules:**
- Case-insensitive substring match against the user message
- Multiple skills can match simultaneously (top 3 shown as hints)
- Already-invoked skills are excluded from hints (they're already in context)
- Skills with higher `priority` are shown first

### Phase 2: Invocation (Explicit)

The agent invokes a skill by calling the skill tool:

```
skill(name: "code-review")
```

This does three things:
1. Returns the full SKILL.md template as the tool result (the agent reads it)
2. Records the invocation in the session state
3. Re-injects the template into the system prompt on subsequent turns

Once invoked, the skill's instructions persist in the system prompt until it expires or is unloaded.

### Manual Load/Unload

Users or the agent can explicitly manage skills:

```
skill(name: "best-friend", action: "load")     # Activate for this session
skill(name: "best-friend", action: "unload")   # Deactivate
```

Manually loaded skills get a longer TTL (6 turns vs 4 for auto-matched).

---

## Skill Lifecycle

```
┌─────────────┐     trigger match     ┌──────────┐     skill(name:...)     ┌─────────┐
│  Registered  │ ──────────────────► │  Hinted   │ ─────────────────────► │ Invoked  │
│  (catalog)   │                      │  (prompt) │                        │ (active) │
└─────────────┘                      └──────────┘                        └────┬────┘
                                                                              │
                                                      TTL expires or unload   │
                                                                              ▼
                                                                        ┌──────────┐
                                                                        │  Expired  │
                                                                        └──────────┘
```

### TTL and Expiry

Active skills expire after a period of inactivity:

- **Auto-matched skills:** 4 turns of inactivity (configurable via `max_turns`)
- **Manually loaded skills:** 6 turns of inactivity
- **Re-invocation resets the timer** — if the user's message re-matches a trigger, the TTL resets
- **Hard cap:** Maximum 4 active skills per session
- **Token budget:** Combined active skill content capped at 16,000 characters

### Session Scope

Skills are scoped to a conversation session. Starting a new session clears all active skills. There is no cross-session skill persistence — skills activate fresh each conversation based on what the user is doing.

---

## Directory Structure

Skills are loaded from two locations, merged at startup:

```
extensions/skills/          # Bundled with Nebo (read-only)
├── code-review/
│   └── SKILL.md
├── debugging/
│   └── SKILL.md
├── api-design/
│   └── SKILL.md
├── git-workflow/
│   └── SKILL.md
├── database-expert/
│   └── SKILL.md
├── security-audit/
│   └── SKILL.md
├── best-friend/
│   └── SKILL.md
└── onboarding/
    └── SKILL.md

<data_dir>/skills/          # User-created (read-write)
├── meeting-prep/
│   └── SKILL.md
├── my-custom-skill/
│   └── SKILL.md
└── ...
```

**Rules:**
- Each skill lives in its own subdirectory
- The subdirectory name doesn't matter — the `name` field in the YAML frontmatter is the identifier
- Only `SKILL.md` files are loaded (case-insensitive filename match)
- If a user skill has the same name as a bundled skill, the user skill wins (last-loaded)

---

## Writing Good Skills

### Structure

A good skill template has:

1. **A clear heading** — tells the agent what mode it's in
2. **Core principles** — the non-negotiable behavioral rules
3. **Methodology** — step-by-step approach for common scenarios
4. **Examples** — concrete user-message-to-response patterns
5. **Anti-patterns** — what NOT to do (models learn well from negative examples)

### Keep It Focused

Each skill should do one thing well. Don't create a "general programming" skill — create `code-review`, `debugging`, `api-design` as separate skills. The agent can have multiple active simultaneously.

### Be Specific About Tools

Tell the agent which tools to use and when:

```markdown
## Approach

1. Read the file using `file(action: read, path: "...")`
2. Search for related tests with `file(action: grep, pattern: "TestFunctionName")`
3. Run the test suite with `shell(resource: bash, action: exec, command: "go test ./...")`
```

### Include Examples

Examples are the most effective part of a skill. Show the agent exactly what a good response looks like:

```markdown
## Example

**User:** "Review the auth middleware"

**Response:** I'll review the authentication middleware systematically.

**Critical Issues:**
1. `middleware.go:34` — Token validation doesn't check expiry...

**Suggestions:**
1. Consider adding rate limiting...
```

### Use Anti-Patterns

Explicitly listing what NOT to do is surprisingly effective:

```markdown
## Anti-Patterns (NEVER do these)
- Don't start with "I'd be happy to help" — just start helping
- Don't list pros and cons without giving your recommendation
- Don't ask clarifying questions if the intent is obvious
```

### Set the Right Priority

Priority determines which skill gets hinted first when multiple triggers match:

| Priority Range | Use Case |
|----------------|----------|
| 100+ | Critical overrides (onboarding, emergency) |
| 50-99 | Personality/behavioral skills |
| 20-49 | Domain expertise (debugging, API design) |
| 10-19 | General methodology (code review) |
| 0-9 | Low-priority / catch-all skills |

### Choose Triggers Carefully

Triggers are substring matches, so be specific enough to avoid false positives:

```yaml
# Too broad — will match "I have a problem with my code review process"
triggers:
  - problem

# Better — more specific phrases
triggers:
  - debug
  - bug
  - error
  - crash
  - not working
  - stack trace
```

Short, common words trigger more often. Long phrases trigger less often but more precisely. Balance based on how often you want the skill to activate.

---

## Managing Skills

### CLI Commands

```bash
nebo skills list              # List all loaded skills
nebo skills show <name>       # Show skill details and template
```

### Agent Tool Actions

The agent can manage skills through the `skill` tool:

| Action | Description | Example |
|--------|-------------|---------|
| `catalog` | List all available skills | `skill(action: "catalog")` |
| `load` | Activate a skill for this session | `skill(name: "code-review", action: "load")` |
| `unload` | Deactivate a skill | `skill(name: "code-review", action: "unload")` |
| `create` | Create a new skill on disk | `skill(action: "create", content: "---\nname: ...")` |
| `update` | Update an existing skill | `skill(name: "my-skill", action: "update", content: "...")` |
| `delete` | Delete a skill from disk | `skill(name: "my-skill", action: "delete")` |

**Note:** `create`, `update`, and `delete` only work on user-created skills (in the data directory). Bundled skills in `extensions/skills/` are read-only.

### REST API

Skills can also be managed through the HTTP API:

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/skills/{name}` | Get skill metadata |
| `GET` | `/api/skills/{name}/content` | Get raw SKILL.md content |
| `POST` | `/api/skills` | Create a new skill |
| `PUT` | `/api/skills/{name}` | Update a skill |
| `DELETE` | `/api/skills/{name}` | Delete a skill |
| `POST` | `/api/skills/{name}/toggle` | Enable/disable a skill |

### Enabling and Disabling

Skills can be toggled on/off without deleting them. Disabled skills are excluded from trigger matching and the catalog. The disabled state is stored in `<data_dir>/skill-settings.json`.

---

## Hot Reload

The user skills directory is watched with `fsnotify`. Changes are picked up automatically:

- **Create** a new `SKILL.md` file → skill appears in the catalog immediately
- **Edit** an existing `SKILL.md` → skill metadata and template are reloaded
- **Delete** a `SKILL.md` file → skill is removed from the catalog

**Important:** If a skill is currently active in a session, editing its file does NOT update the in-session copy. The session retains the template snapshot from when the skill was invoked. Unload and reload the skill to pick up changes mid-session.

---

## Bundled Skills

Nebo ships with 8 bundled skills covering common use cases:

| Skill | Priority | Description |
|-------|----------|-------------|
| `onboarding` | 100 | New user greeting and profile collection |
| `best-friend` | 50 | Personality skill — casual, loyal, real talk |
| `debugging` | 25 | Systematic debugging methodology |
| `api-design` | 15 | RESTful API design best practices |
| `code-review` | 10 | Structured code review with severity levels |
| `database-expert` | 10 | Database design, queries, optimization |
| `git-workflow` | 10 | Git branching, commits, PR workflows |
| `security-audit` | 10 | Security analysis and vulnerability detection |

These serve as both useful defaults and reference implementations for creating your own skills.

---

## Complete Example: Research Skill

Here's a full example of a well-structured skill:

```markdown
---
name: deep-research
description: Methodical research with source tracking and synthesis
version: "1.0.0"
author: Your Name
priority: 20
triggers:
  - research
  - investigate
  - find out about
  - what do we know about
  - deep dive
tools:
  - web
  - memory
  - file
tags:
  - research
  - analysis
metadata:
  nebo:
    emoji: "🔬"
---

# Deep Research

You are conducting structured research. Be thorough, cite sources,
and synthesize findings into actionable insights.

## Methodology

1. **Scope the question** — Before searching, clarify exactly what we need to know.
   Ask the user if the question is ambiguous.

2. **Search broadly first** — Use `web(action: search, query: "...")` with 2-3
   different phrasings to cast a wide net.

3. **Read primary sources** — Don't rely on search snippets. Use
   `web(action: fetch, url: "...")` to read the actual pages.

4. **Track sources** — For every claim, note the URL. Present sources at the end.

5. **Synthesize** — Don't just list findings. Connect them. Identify patterns,
   contradictions, and gaps.

6. **Store key findings** — Use memory to save important discoveries:
   `agent(resource: memory, action: store, layer: "daily", ...)`

## Output Format

Structure your research report as:

### Key Findings
- Bullet points of the most important discoveries

### Detailed Analysis
- Longer narrative connecting the dots

### Sources
- Numbered list of URLs with brief descriptions

### Open Questions
- What we still don't know

## Anti-Patterns
- Don't present search snippets as findings — read the actual source
- Don't give a single-source answer — cross-reference
- Don't hedge everything — state your assessment confidently
- Don't skip the sources section — always cite

## Example

**User:** "Research the current state of WebAssembly outside the browser"

**Response:**

### Key Findings
- WASI (WebAssembly System Interface) reached preview2 in 2024...
- Major cloud providers now offer Wasm-based serverless...
- The component model enables cross-language composition...

### Sources
1. [bytecodealliance.org/wasi](https://bytecodealliance.org) — WASI specification
2. ...
```

---

## Tips

- **Start small.** A 10-line skill that gives good instructions is better than a 200-line skill that tries to cover everything.
- **Test with real conversations.** Say something that matches your triggers and see if the skill activates and guides the agent well.
- **Watch the TTL.** If your skill expires too quickly, increase `max_turns`. If it lingers too long, decrease it.
- **One skill per concern.** Don't combine code review and debugging into one skill. Let the agent load both when needed.
- **Personality skills work.** The `best-friend` skill is proof that behavioral/tonal skills are just as valuable as methodological ones.
- **Use the agent to create skills.** Just describe what you want: "Create a skill that helps me write technical blog posts." Nebo writes the SKILL.md for you.
