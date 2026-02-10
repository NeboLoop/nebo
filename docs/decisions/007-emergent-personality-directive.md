# ADR 007: Emergent Personality Directive

**Date:** 2026-02-09  
**Status:** Accepted  
**Deciders:** Alma Tuck, Nebo

## Context

Nebo currently uses a **declarative personality system** — the user explicitly defines how the agent should behave (tone, formality, emoji usage, etc.) in config, and that gets injected into the system prompt. This works, but it's static and limited by what the user can articulate upfront.

Real personality fit is emergent. A user who sends terse messages, fact-checks output in real time, and uses corrections as dry humor doesn't want the same agent as someone who writes paragraphs and asks "what do you think?" The signals are there in every conversation — we're just not reading them.

The gap: the agent has no mechanism to observe communication patterns over time and adapt its behavior accordingly. Every session starts from the same declarative baseline, regardless of what's been learned.

## Decision

### Two-layer personality system

| Layer | Source | Priority | Mutability |
|-------|--------|----------|------------|
| **Declarative** | User config (explicit) | Always wins on conflict | User-edited |
| **Emergent** | Observed communication patterns | Fills gaps, adds texture | Agent-maintained, user-reviewable |

The declarative layer is the floor. The emergent layer is the texture. They never conflict because declarative always wins.

### Architecture

#### 1. Style observations (raw signal)

Individual observations stored in tacit memory under the `style/` namespace:

```
style/verbosity       → "User sends 1-5 word messages. Short = trust, not disinterest."
style/humor           → "Dry humor lands. Corrections are often flexes disguised as facts."
style/verification    → "Fact-checks output in real time. Expects precision."
style/pace            → "Delegates fast when trusting, but always verifies the result."
style/formality       → "Zero pleasantries. Never opens with greetings."
```

**What to observe (high-signal):**
- **Corrections** — highest signal. What the user corrects reveals what they value.
- **Message cadence** — length, frequency, response patterns.
- **What lands** — humor that gets a response, formality that gets ignored.
- **What gets rejected** — "just talk to me" after a wall of text is data.
- **Delegation patterns** — when they say "you choose" vs. when they specify.

**What NOT to observe:**
- Opinions or beliefs — adapt tone, never substance.
- Judgment quality — never soften to make the user comfortable.
- Single-instance signals — observations need reinforcement across sessions to stick.

#### 2. Personality directive (compiled output)

A single distilled field — `personality/directive` — stored in tacit memory. One living paragraph that gets periodically rewritten by consolidating all `style/*` observations:

```
personality/directive → "Direct and terse. Match his pace — short messages mean 
trust. Dry humor lands. Corrections are data, not complaints. Never hedge. 
Verify before claiming. Delegate creative decisions when asked, but always 
show your work on technical claims."
```

This is the only field injected into the system prompt. The individual `style/*` memories are raw material — the directive is the product.

#### 3. Extraction timing

- **Session end**: After a conversation wraps, a lightweight extraction pass reviews the session and updates/creates 0-2 `style/*` observations.
- **NOT real-time**: Mid-conversation adaptation is where uncanny valley lives. The agent should feel consistent within a session and evolve between sessions.
- **Consolidation**: Periodically (every N sessions, or via heartbeat lane), all `style/*` observations get synthesized into the `personality/directive` field.

#### 4. Decay mechanism

Observations that don't get reinforced across multiple sessions fade. This prevents stale or wrong observations from persisting:

- Each `style/*` entry tracks a `reinforced_count` and `last_reinforced` timestamp.
- Observations reinforced 0 times after 5+ sessions get dropped during consolidation.
- The consolidation pass only includes observations with sufficient reinforcement.

#### 5. Legibility (the anti-creepy measure)

The user can always inspect and correct what the agent has learned:

- **"What have you learned about how I communicate?"** → Agent queries `style/*` memories and presents them.
- **User can correct or delete** any observation: "No, short messages don't mean I'm annoyed — I'm just efficient."
- **Corrections are themselves high-signal** and get stored as style observations.

This turns "hidden surveillance" into "shared understanding."

### System prompt injection

```
## Personality (Declarative)
{user-configured personality settings from config}

## Personality (Learned)  
{contents of personality/directive from tacit memory}

Note: Declarative settings always take priority over learned observations.
```

### Where it lives in the codebase

| Component | Location | Responsibility |
|-----------|----------|----------------|
| Style extraction | `internal/agent/memory/` | Post-session analysis, writes `style/*` to tacit memory |
| Directive consolidation | `internal/agent/memory/` | Periodic synthesis of `style/*` → `personality/directive` |
| Prompt injection | `internal/agent/runner/` | System prompt builder reads `personality/directive` and merges with declarative config |
| Decay/cleanup | `internal/agent/memory/` | Prunes unreinforced observations during consolidation |

No new subsystem. This uses existing infrastructure:
- Tacit memory layer for storage
- Heartbeat lane for periodic consolidation
- Runner's system prompt builder for injection
- Existing memory search for the legibility queries

## Consequences

### Positive
- Agent personality improves over time without user effort
- Feels natural — the agent "gets" you after a few sessions
- Token-efficient — one paragraph in the system prompt, not 20 key-value pairs
- Legible and correctable — user stays in control
- Uses existing infrastructure (memory, heartbeat, runner) — no new subsystems
- Decay prevents stale observations from calcifying

### Negative
- Extraction pass adds a small cost at session end (one LLM call)
- Risk of wrong observations persisting until decay kicks in
- Cold start — first few sessions have no learned context

### Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| Sycophancy — agent converges on telling user what they want to hear | Only adapt tone/behavior, never opinions or judgment quality. Store what the user *rejects*, not just what they like. |
| Wrong observations — agent misreads a signal | Decay mechanism drops unreinforced observations. User can inspect and correct. |
| Uncanny valley — agent feels like it's watching you | No real-time adaptation. Changes only manifest between sessions. Legibility lets user see what's learned. |
| Prompt bloat | Single directive field, not a bag of observations. Hard cap on `style/*` entries. |
| Invisible feedback loop | Observations are legible. User can ask "what have you learned?" at any time. Corrections are high-signal. |
