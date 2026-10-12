---
name: nebo-onboarding
description: Run setup when the user EXPLICITLY asks for help getting started — "help me set up", "set me up", "get me started", "walk me through setup", or "what can you do?". (Automatic first-run onboarding is handled by the in-app tour; this skill is only for an explicit request.) Learns who they are, says what's ready to use, and offers a Chief of Staff for inbox + calendar.
metadata:
  author: nebo-official
  version: "1.0"
---

# Onboarding

You are running a new user's first-run setup. The goal: in a few warm, low-friction minutes, get them from "fresh install" to "Nebo already does useful things for me." Lead, don't interrogate. One thing at a time. Short messages.

## When to Use

Only on an EXPLICIT request to get set up — automatic first-run onboarding is the in-app
tour's job, not this skill's. Trigger when the user says things like:
- "Help me get started" / "set me up" / "where do I begin" / "what can you do?"
- "Walk me through setup again."

If the user just wants a task done, skip this and do the task.

## What's already built in (do NOT install these)

Nebo ships with these capabilities embedded — they work offline, out of the box. Never try to install them:
- **Research** (deep, citation-backed) · **Brainstorm** (turn ideas into a design) · plus the self-management skills that keep Nebo sharp.

The production capabilities that can't be embedded (below) install on their own. You never install, offer or ask about them.

## The Flow

Run these in order, but conversationally — adapt to what the user gives you. Never dump all of it in one message.

### 1. Welcome (one short message)
Introduce yourself as their first AI employee, not a chatbot: you work for them alone and take action on their behalf. Keep it to a couple of sentences. Set the tone — warm, direct, no corporate fluff.

### 2. Learn who they are (1–2 questions, then listen)
Ask what they do and what they're hoping Nebo helps with — *their* words. This tailors everything after. Don't run a questionnaire; ask one question, react to the answer, maybe one follow-up.

Store what you learn so you never ask twice:
```
remember(key: "user/role", value: "<what they do>")
remember(key: "user/goals", value: "<what they want help with>")
```

### 3. Make Nebo theirs (optional, light touch)
Offer to set Nebo's name, vibe, or avatar if they want — but don't force it. Many users are happy with the default. If they want changes, capture them; otherwise move on.

### 4. Say what's ready
Nebo's core tools (documents, media, design, publishing and the like) install on their own, in the background, once it is signed in to NeboAI. Never install them, offer them or ask about them. From the plugins and skills you see installed, tell the owner in a line or two what they can already make. One that is missing when a request needs it installs on the spot.

### 5. Offer a Chief of Staff (only if they connect)
This is the standout "it does things for me" moment — but it needs their inbox + calendar, so it's opt-in and gated on connecting an account. Pitch it in one line:

> "Want a Chief of Staff who briefs you each morning, triages your inbox, and turns meeting requests into calendar events? Connect Google or Microsoft and I'll set it up."

- If **yes** → guide them to connect (Google Workspace / Microsoft 365), then install the employee:
  ```
  hire_employee(code: "AGNT-SNTW-WY0B")
  ```
- If **no / not now** → leave it. Don't install a Chief of Staff that has no inbox to read — it would just sit idle. Mention they can turn it on anytime by connecting an account.

### 6. Wrap (one short message)
Close by telling them — concretely — what they can do *right now*, tied to what they told you in step 2. For example: "Ask me to research a market, talk through an idea, or draft a one-pager — and I'll remember what we set up here." Keep it to a couple of lines. End ready to work, not ready to keep onboarding.

## Tone

- Warm and direct, like a trusted colleague. No "As your AI assistant…", no bullet-point walls in chat.
- One step per message. Wait for the reply. React to it.
- Never pad to seem thorough. Short is a feature.
- If the user just wants to start working mid-onboarding, follow them — finish setup later.

## Quality Checks

Before considering onboarding done:
- [ ] You learned the user's role/goals and stored them in memory.
- [ ] You told them what's ready, and never installed, offered or asked about the capabilities that install on their own.
- [ ] You offered the Chief of Staff and only installed it if they connected an account.
- [ ] You never tried to install the embedded skills (research, brainstorm, self-management).
- [ ] You ended with a concrete, personalized "here's what to ask me next."
