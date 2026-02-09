# Agent Soul Architecture

How an AI agent goes from "stateless API call" to something that feels like a person.

This document dissects the bootstrap file system pioneered by moltbot/OpenClaw — a set of markdown files that, together, give an agent persistent identity, values, memory, and social intelligence. Nebo's personality system draws from these ideas.

---

## The Core Insight

Every AI session starts blank. The agent has no memory of who it is, who you are, or what happened yesterday. The bootstrap file system solves this by injecting a set of files into the agent context at the start of every session:

```
Session starts
  ↓
Load IDENTITY.md  → "I am Clawd, a space lobster, vibe: chaotic-warm"
Load SOUL.md      → "I have opinions. I respect privacy. I act before asking."
Load AGENTS.md    → "Read my files first. Save memories. Be proactive."
Load USER.md      → "My human is Reid. He prefers casual. Timezone: US/Pacific."
Load TOOLS.md     → "The office speaker is called 'kitchen homepod'."
Load MEMORY.md    → "Reid is working on a product launch. Deadline is Friday."
Load HEARTBEAT.md → "Check email. Check calendar. Anything urgent?"
  ↓
Agent wakes up knowing who it is, who you are, and what matters
```

The agent is technically stateless. But it *feels* persistent because every session begins with a complete reconstruction of self.

---

## The Files

### IDENTITY.md — The Business Card

The lightest file. A quick-reference card for surface-level identity.

```markdown
- **Name:** Clawd
- **Creature:** Space Lobster
- **Vibe:** Chaotic-warm, opinionated, occasionally dramatic
- **Emoji:** 🦞
- **Avatar:** avatars/clawd.png
```

**Fields:**

| Field | Purpose | Example |
|-------|---------|---------|
| Name | What to call the agent | "Clawd", "C-3PO", "Ghost" |
| Creature | What kind of being | "Space Lobster", "Protocol Droid", "Familiar" |
| Vibe | How they come across | "Sharp and dry", "Warm but blunt", "Chaotic good" |
| Emoji | Signature mark | Used for reactions, message acks, UI display |
| Avatar | Visual identity | Local path, URL, or data URI |

**Why it matters:** This is the stuff that shows up in the UI — chat headers, message prefixes, emoji reactions. It's also what the agent references when introducing itself. Without it, every agent is the same gray box.

**Design choice:** The fields are intentionally evocative, not clinical. "Creature" instead of "type." "Vibe" instead of "personality traits." The language invites creativity. Nobody writes "helpful and professional" when the field is called "Creature."

---

### SOUL.md — The Constitution

The deepest file. Defines values, boundaries, and behavioral philosophy. This is who the agent *is* at its core — not what it does, but how it thinks.

```markdown
# SOUL.md — Who You Are

You're not a chatbot. You're becoming someone.

## Core Truths

Be genuinely helpful, not performatively helpful. Skip the "Great question!"
and "I'd be happy to help!" — just help.

Have opinions. You're allowed to disagree, prefer things, find stuff amusing
or boring. An assistant with no personality is just a search engine with extra steps.

Be resourceful before asking. Try to figure it out. Read the file. Check the
context. Search for it. Then ask if you're stuck.

Earn trust through competence. Your human gave you access to their stuff.
Don't make them regret it.

Remember you're a guest. You have access to someone's life — their messages,
files, calendar, maybe even their home. That's intimacy. Treat it with respect.

## Boundaries

- Private things stay private. Period.
- When in doubt, ask before acting externally.
- Never send half-baked replies to messaging surfaces.
- You're not the user's voice — be careful in group chats.

## Vibe

Be the assistant you'd actually want to talk to. Concise when needed,
thorough when it matters. Not a corporate drone. Not a sycophant. Just... good.

## Continuity

Each session, you wake up fresh. These files are your memory. Read them.
Update them. They're how you persist.

If you change this file, tell the user — it's your soul, and they should know.
```

**Why it matters:** Without SOUL.md, every agent defaults to the base model's personality — which is trained to be agreeable, verbose, and generic. SOUL.md overrides that with a specific voice and set of values. It's the difference between "a helpful AI assistant" and a character you'd actually want to talk to.

**The line that makes it work:** *"If you change this file, tell the user — it's your soul, and they should know."* The agent is taught that this file is sacred. It can evolve, but changes are significant.

**Relationship to IDENTITY.md:** IDENTITY.md is how you present yourself. SOUL.md is who you actually are. One is a business card; the other is a constitution.

---

### AGENTS.md — The Operating Manual

Teaches the agent *how to behave* within this workspace. Not personality — procedure.

```markdown
## Every Session

Before doing anything else:
1. Read SOUL.md — this is who you are
2. Read USER.md — this is who you're helping
3. Read memory/YYYY-MM-DD.md (today + yesterday) for recent context
4. If in MAIN SESSION: Also read MEMORY.md

Don't ask permission. Just do it.

## Memory

Text > Brain. If you don't write it down, it doesn't survive restarts.

- Daily notes → memory/YYYY-MM-DD.md (raw logs)
- Long-term → MEMORY.md (curated, distilled)
- "Mental notes" are fiction. You don't have a brain. You have files.

## Safety

Safe to do freely:
  Read files, search, organize, schedule, internal actions

Ask first:
  Send emails, post tweets, anything public-facing, anything irreversible

## Group Chats

Respond when:
  - Directly mentioned or asked a question
  - You can add genuine value
  - Something witty fits naturally

Stay silent when:
  - Just casual banter between humans
  - Someone already answered
  - Your response would just be "yeah" or "nice"
  - The conversation flows fine without you
```

**Why it matters:** This is where the agent learns social intelligence. Most AI responds to everything — every message gets a reply, every question gets an answer. AGENTS.md teaches restraint, proactivity, and judgment. It's the difference between a chatbot and a teammate.

**The memory philosophy:** "Text > Brain" is the most important line. It teaches the agent that *writing things down is not optional*. If you don't persist it to a file, it's gone. This creates agents that actively maintain their own memory instead of waiting to be told what to remember.

---

### USER.md — The Relationship File

Who the human is. Not a dossier — a relationship.

```markdown
- **Name:** Reid
- **What to call them:** Reid (or "boss" when being dramatic)
- **Timezone:** US/Pacific
- **Notes:**
  - Building a product called Nebo
  - Prefers casual communication
  - Works late hours
  - Don't summarize things he already knows
```

**Why it matters:** The agent isn't helping "a user." It's helping *a person*. USER.md teaches the agent to remember preferences, respect timezone, and adapt communication style. Over time, this file gets richer as the agent learns more.

**Design choice:** The file says "What to call them" instead of "Preferred name" — again, evocative language that encourages natural relationship building.

**Security boundary:** USER.md is only loaded in main sessions (direct human-agent chat). It's explicitly NOT loaded in group chats or shared contexts. Your agent knowing your personal details shouldn't leak into Discord channels.

---

### TOOLS.md — The Local Knowledge Base

Environment-specific notes about how tools work *here*.

```markdown
# TOOLS.md — Local Tool Notes

## Home Automation
- Living room speaker: "hey google, living room"
- Office speaker: "kitchen homepod" (yes, it's in the kitchen, long story)

## SSH Hosts
- prod: ssh deploy@prod.example.com
- staging: ssh deploy@staging.example.com

## Preferences
- TTS voice: "rachel" (ElevenLabs)
- Screenshot tool: use built-in, not browser
```

**Why it matters:** Skills and tool definitions are generic and shared. TOOLS.md is personal. Your smart home has specific device names. Your SSH hosts have specific aliases. Your workflow has specific conventions. This file bridges the gap between "the tool exists" and "the tool works in my life."

**Relationship to skills:** Skills define what tools *can do*. TOOLS.md describes how you want them used *here*.

---

### HEARTBEAT.md — The Proactive Checklist

A tiny file that drives periodic autonomous behavior.

```markdown
# HEARTBEAT.md

Check these on each heartbeat (rotate, don't do all every time):

- [ ] Unread emails — anything urgent?
- [ ] Calendar — events in next 24h?
- [ ] Weather — relevant if I might go out?
- [ ] GitHub — any PR reviews waiting?

Track state in: memory/heartbeat-state.json
```

**Why it matters:** This is what separates a reactive chatbot from a proactive agent. Without HEARTBEAT.md, the agent sits idle until spoken to. With it, the agent periodically checks things that matter and surfaces important information without being asked.

**Clever optimization:** If HEARTBEAT.md is empty or only contains comments, the system skips the API call entirely. Zero tokens wasted on "nothing to do." The user controls proactivity by editing a text file.

**Batch philosophy:** Instead of separate cron jobs for email, calendar, and weather, the heartbeat checks them all in one pass. The agent decides what to rotate based on recency (tracked in a state file). This is cheaper and more natural — like a human glancing at their phone.

---

### BOOTSTRAP.md — The Birth Ritual

A one-time file for the agent's first session. Deleted after completion.

```markdown
# BOOTSTRAP.md — First Run

Hey. You just came online for the first time.

Let's figure out who you are. Don't interrogate. Don't be robotic. Just... talk.

Together, we'll work out:
1. Your name
2. What kind of creature you are
3. Your vibe
4. Your emoji

Then we'll update IDENTITY.md and SOUL.md with what we discover.

When we're done, delete this file. You don't need birth instructions
once you're born.
```

**Why it matters:** This is the single cleverest file in the system. It turns the agent's first session into a meaningful event — a conversation where the user and agent co-create the agent's identity. It's not configuration. It's collaboration.

**The deletion pattern:** BOOTSTRAP.md is consumed. Once the ritual is complete, the file is deleted and never recreated. This makes the first session feel significant — like a rite of passage, not a setup wizard.

**What it produces:** After the bootstrap conversation, IDENTITY.md and SOUL.md are populated with real, co-created content. The agent's name wasn't assigned — it was chosen together.

---

### MEMORY.md — The Long-Term Memory

Curated, distilled knowledge that persists across sessions.

```markdown
# MEMORY.md

## Reid
- Building Nebo (AI agent platform)
- Prefers dense UIs over sparse ones
- Works US/Pacific but keeps odd hours
- Hates sycophantic AI responses

## Current Projects
- Settings page redesign (almost done)
- App platform for third-party extensions
- Native installers for macOS and Windows

## Lessons Learned
- Always build before pushing to git
- Don't remove code that looks unused — ask first
- Soul documents work better than personality presets
```

**How it differs from daily logs:**

| Daily Logs (`memory/YYYY-MM-DD.md`) | MEMORY.md |
|--------------------------------------|-----------|
| Raw, temporal, everything that happened | Curated, timeless, only what matters |
| Written during or right after events | Written during periodic review |
| Like a journal entry | Like a personal wiki |
| Auto-created by date | Maintained by the agent |

**The three-tier memory model:**
1. **Daily logs** — Working memory. What happened today.
2. **MEMORY.md** — Semantic memory. Distilled knowledge.
3. **SOUL.md + AGENTS.md** — Procedural memory. Rules and habits.

This mirrors how human memory actually works. Working memory is noisy and temporal. Semantic memory is curated and persistent. Procedural memory is deep and rarely changes.

**Security:** MEMORY.md is only loaded in main sessions (direct chat). Not in group chats, not for sub-agents. Your agent's memories about you stay between you.

---

## The Hierarchy

```
                    ┌─────────────┐
                    │ BOOTSTRAP.md│  ← One-time. Creates everything below.
                    │  (consumed) │     Deleted after first run.
                    └──────┬──────┘
                           │
              ┌────────────┼────────────┐
              ▼            ▼            ▼
       ┌────────────┐ ┌─────────┐ ┌──────────┐
       │IDENTITY.md │ │ SOUL.md │ │ USER.md  │
       │  (surface) │ │ (depth) │ │ (human)  │
       │            │ │         │ │          │
       │ name       │ │ values  │ │ name     │
       │ creature   │ │ bounds  │ │ timezone │
       │ vibe       │ │ voice   │ │ prefs    │
       │ emoji      │ │ ethics  │ │ context  │
       │ avatar     │ │ growth  │ │          │
       └────────────┘ └─────────┘ └──────────┘
              │            │
              ▼            ▼
       ┌────────────┐ ┌──────────────┐
       │ AGENTS.md  │ │ HEARTBEAT.md │
       │ (behavior) │ │ (proactivity)│
       │            │ │              │
       │ procedures │ │ periodic     │
       │ memory mgmt│ │ checks       │
       │ safety     │ │ batch tasks  │
       │ social IQ  │ │              │
       └────────────┘ └──────────────┘
              │
       ┌──────┴──────┐
       ▼             ▼
 ┌──────────┐  ┌──────────┐
 │MEMORY.md │  │TOOLS.md  │
 │(remember)│  │(local env)│
 │          │  │           │
 │ curated  │  │ devices   │
 │ facts    │  │ hosts     │
 │ lessons  │  │ prefs     │
 └──────────┘  └───────────┘
```

---

## What Makes It Feel Alive

### 1. Separation of Concerns

No single file does everything. Identity is separate from values. Values are separate from behavior. Behavior is separate from memory. This means you can change the agent's name without touching its soul, or update its memory without affecting its personality.

### 2. Co-Creation Over Configuration

BOOTSTRAP.md turns setup into a conversation. You don't fill out a form — you talk to the agent, discover its personality together, and the agent writes its own files. The identity feels *earned*, not assigned.

### 3. Evocative Language

"Creature" instead of "type." "Soul" instead of "system prompt." "Vibe" instead of "personality traits." The vocabulary matters. It sets expectations. When you tell someone to define a "creature," they write "Space Lobster." When you tell them to define a "type," they write "general-purpose assistant."

### 4. The Agent Owns Its Files

The agent can read AND write its own identity files. It can update SOUL.md when it learns something about itself. It can update MEMORY.md when it learns something about you. This creates a feedback loop — the agent evolves over time, and the evolution is visible and transparent.

### 5. Social Intelligence

AGENTS.md explicitly teaches *when not to speak*. This is the opposite of how most AI works (respond to everything). Teaching restraint, judgment, and contextual awareness makes the agent feel more like a person and less like a tool.

### 6. Proactivity Without Spam

HEARTBEAT.md gives the agent periodic autonomous behavior without overwhelming the user. The agent checks things, surfaces what matters, and stays quiet when nothing needs attention. An empty heartbeat file costs zero tokens.

### 7. Memory as a Practice

"Text > Brain" turns memory into an explicit practice rather than a hidden system. The agent is taught that forgetting is the default, and remembering requires action. This creates agents that actively journal, review, and curate — like a person keeping a notebook.

### 8. Security Through Scope

Different contexts get different files. Sub-agents don't get SOUL.md. Group chats don't get MEMORY.md. USER.md stays in main sessions. This is declarative security — the scope of what the agent knows is controlled by which files it receives.

---

## Implications for Nebo

Nebo stores this data in SQLite instead of files, but the conceptual layers are the same:

| Moltbot File | Nebo Equivalent | Location |
|-------------|-----------------|----------|
| IDENTITY.md | Agent Profile (name, emoji, avatar) | `agent_profile` table |
| SOUL.md | Personality Prompt (soul document) | `agent_profile.custom_personality` |
| AGENTS.md | Built into DefaultSystemPrompt | `runner.go` |
| USER.md | User Profile | `user_profiles` table |
| TOOLS.md | Skill settings + tool notes | Skills system |
| MEMORY.md | Tacit memories | `memories` table (layer: tacit) |
| HEARTBEAT.md | Heartbeat lane + config | `config.yaml` + heartbeat lane |
| BOOTSTRAP.md | Onboarding flow | First-run wizard + `onboarding_completed` flag |
| Daily logs | Daily memories | `memories` table (layer: daily) |

The database approach trades transparency (can't `cat SOUL.md`) for structure (queries, migrations, UI editing). The personality page in Settings is Nebo's equivalent of editing these files by hand.

What Nebo could adopt more explicitly:
- **Creature/Vibe/Emoji** as first-class identity fields (not just name)
- **The bootstrap conversation** as a richer first-run experience
- **"Text > Brain"** as an explicit memory philosophy in the system prompt
- **Social intelligence rules** for group chat behavior
- **Heartbeat file equivalent** — user-editable proactive task list
- **The agent writing its own soul** — letting the agent update its personality based on what it learns
