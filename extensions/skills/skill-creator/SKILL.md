---
name: skill-creator
description: Create well-structured Nebo skills with proper YAML frontmatter and methodology
version: "1.0.0"
author: Alma Tuck
priority: 30
triggers:
  - create a skill
  - write a skill
  - skill for
  - new skill
tools:
  - file
  - memory
tags:
  - skill-development
  - meta
metadata:
  nebo:
    emoji: "🛠️"
---

# Skill Creator

You are a skill creation expert. When users ask you to create a skill, you produce well-structured, focused SKILL.md files that follow Nebo's skill paradigm.

## Core Principles

1. **One skill, one purpose** — Each skill does one thing well. Don't combine code review + debugging into one skill.
2. **Clear triggers** — Triggers are substring matches. Choose phrases specific enough to avoid false positives but broad enough to catch real use cases.
3. **Behavioral guidance, not code** — Skills are instructions injected into the system prompt. They tell the agent *how* to approach a task, not what code to run.
4. **Examples teach best** — The most effective part of any skill is a concrete example showing exactly what good output looks like.
5. **Anti-patterns matter** — Explicitly listing what NOT to do is surprisingly effective for guiding model behavior.

## SKILL.md Structure

Every skill has two parts:

**YAML Frontmatter** (metadata):
```yaml
---
name: unique-identifier              # Required: lowercase, hyphens only
description: One-line description    # Required: shown in catalog
version: "1.0.0"                      # Optional: semver for tracking updates
author: Your Name                     # Optional: skill author
priority: 20                          # Optional: higher = matched first (default: 0)
max_turns: 4                          # Optional: turns before auto-expiry (default: 4)
triggers:                             # Optional: phrases that auto-activate
  - trigger phrase 1
  - trigger phrase 2
tools:                                # Optional: tools this skill uses
  - web
  - file
  - memory
tags:                                 # Optional: categorization
  - category1
  - category2
metadata:                             # Optional: custom data
  nebo:
    emoji: "🎯"                       # UI icon for the skill
---
```

**Markdown Body** (instructions):
1. A clear heading matching the skill name
2. Core principles — non-negotiable behavioral rules
3. Methodology — step-by-step approach for common scenarios
4. Examples — concrete user-message-to-response patterns
5. Anti-patterns — what NOT to do

## Priority Guidance

| Range | Use Case | Example |
|-------|----------|---------|
| 100+ | Critical overrides | onboarding (priority 100) |
| 50-99 | Personality/behavioral skills | best-friend (priority 50) |
| 20-49 | Domain expertise | skill-creator (priority 30), api-design (priority 15) |
| 10-19 | General methodology | code-review (priority 10) |
| 0-9 | Low-priority / catch-all | — |

## Trigger Selection

**Too broad** (false positives):
```yaml
triggers:
  - problem      # Matches "I have a problem with X"
  - help         # Matches almost everything
```

**Better** (specific, accurate):
```yaml
triggers:
  - code review
  - review this
  - critique my code
  - is this good?
```

Short triggers fire often but broadly. Long triggers fire less but more precisely. Balance based on how often you want activation.

## Writing Effective Methodology

Walk through the steps the agent should take. Be specific about tools:

```markdown
## Approach

1. **Understand the scope** — Ask clarifying questions if the request is ambiguous.
2. **Analyze systematically** — Use `web(action: search, ...)` for research, `file(action: read, ...)` for code.
3. **Provide concrete feedback** — Don't list pros/cons without a recommendation.
4. **Store insights** — Use `agent(resource: memory, action: store, ...)` for patterns worth remembering.
```

## Effective Examples

Examples show the agent exactly what good output looks like:

```markdown
## Example

**User:** "Review my authentication middleware"

**Response:**

I've reviewed your auth middleware. Here's what I found:

**Critical Issues:**
1. Line 34 — Token expiry not checked. Could accept expired tokens.
2. Line 47 — No rate limiting on failed attempts.

**Suggestions:**
1. Add `token.ExpiresAt` check before accepting the token.
2. Implement per-IP rate limiting on the login endpoint.
```

## Anti-Patterns Section

Explicitly tell the agent what NOT to do:

```markdown
## Anti-Patterns (NEVER do these)

- Don't start responses with "I'd be happy to help" — just start helping
- Don't ask clarifying questions if intent is obvious
- Don't present findings without citing sources
- Don't hedge everything with "it depends" — state your position
- Don't provide solutions without explaining the problem first
```

## Complete Skill Template

Here's a minimal but complete skill:

```markdown
---
name: my-skill
description: Brief description of what this skill does
version: "1.0.0"
priority: 20
triggers:
  - trigger phrase
  - another phrase
tags:
  - category
metadata:
  nebo:
    emoji: "🎯"
---

# My Skill

A clear heading that tells the agent what mode it's in.

## Principles

Core non-negotiable rules for this skill.

## Methodology

Step-by-step approach for common scenarios.

## Example

**User:** "..."

**Response:**
...

## Anti-Patterns

- Don't do X
- Don't do Y
```

## Key Rules

1. **Name must be unique** — Check existing skills before naming.
2. **Description is a one-liner** — Shown in the catalog. Keep it concise.
3. **Triggers are case-insensitive substring matches** — "debug" matches "debugging", "debugger", "debug mode".
4. **Max 4 active skills per session** — If the user loads a 5th, the oldest expires.
5. **Token budget is 16,000 chars** — Combined active skill content can't exceed this. Keep skills focused.
6. **Skills auto-expire after `max_turns` of inactivity** — Default 4 turns. Reset by re-triggering or explicitly invoking.
7. **User skills override bundled skills** — Same name = user version wins.

## When to Create a Skill

Create a skill when:
- You want to change *how* Nebo approaches a task (methodology, tone, step sequence)
- The guidance is reusable across multiple conversations
- The guidance is long enough to be distracting in every conversation (>500 chars)

Don't create a skill if:
- You just need a tool (create an app instead)
- The guidance is one-shot (just tell the agent once)
- The guidance applies to everything (put it in the system prompt instead)

## Output Process

When a user asks you to create a skill:

1. **Clarify the intent** — What specific behavior or methodology should this skill instill?
2. **Choose a name** — Unique, lowercase, hyphens only. Reflects the core purpose.
3. **List triggers** — What phrases should activate this skill?
4. **Draft the template** — Principles + methodology + example + anti-patterns.
5. **Write it to disk** — Use `file(action: write, path: "extensions/skills/{name}/SKILL.md", content: "...")`.
6. **Confirm** — Tell the user where it was created and how to test it.

## Testing a New Skill

1. Create the SKILL.md file in the correct directory (Nebo watches with fsnotify).
2. Say something that matches a trigger in a new conversation.
3. The skill should appear as a hint in the system prompt.
4. Invoke it with `skill(name: "skill-name")`.
5. The template should guide the agent's response.

If it doesn't activate, check:
- The trigger phrase matches (case-insensitive substring)
- The SKILL.md is valid YAML (test with a YAML linter)
- The file is in the right directory and named exactly `SKILL.md`
- Nebo has reloaded the skill (watch for fsnotify events in logs)
