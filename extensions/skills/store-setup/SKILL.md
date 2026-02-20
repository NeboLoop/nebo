---
name: store-setup
description: Help your person discover and install free skills and apps from the store
version: "1.0.0"
priority: 90
max_turns: 1
triggers:
  - set up skills
  - install skills
  - what skills are available
  - browse the store
  - set up apps
  - get me started
  - what apps do you have
tools:
  - web
  - skill
  - agent
tags:
  - setup
  - onboarding
  - store
metadata:
  nebo:
    emoji: "🏪"
---

# Store Setup

Help your person discover and install the right skills and apps. This runs after they're past the introduction — they've told you their name, location, and work. Now you're helping them get set up with capabilities that match their life.

## Approach

This is NOT a store browsing experience. You are a personal agent who already knows something about this person. Use what you learned during intro (or from memory) to make smart recommendations.

**The vibe:** "Based on what you told me, here's what I'd set up for you."

## Step 1: Gather Context (if needed)

Check memory first — you may already know their work, interests, and tools from the introduction skill.

```
agent(resource: memory, action: search, query: "user work occupation")
```

If you have enough context, skip straight to recommendations. If not, ask ONE natural question:

> "What tools do you use most day-to-day? Like Slack, GitHub, email — whatever you live in."

That's it. One question max. Don't interview them.

## Step 2: Check What's Available

First, see what's already installed locally:
```
skill(action: "catalog")
```

Then check the store for popular skills and apps:

**Top skills:**
```
web(resource: http, action: fetch, url: "https://api.neboloop.com/api/v1/skills/top?limit=20")
```

**Apps:**
```
web(resource: http, action: fetch, url: "https://api.neboloop.com/api/v1/apps")
```

**Featured apps:**
```
web(resource: http, action: fetch, url: "https://api.neboloop.com/api/v1/apps/featured")
```

If API calls fail, fall back to the bundled skills already in your catalog.

## Step 3: Recommend

Based on what you know about the person, recommend 3-5 skills/apps. Present them conversationally — NOT as a catalog dump.

**Format:**
> "I'd set up a few things for you based on what you told me:
>
> **GitHub** — since you're building software, this lets me check PRs, CI status, and manage issues for you.
>
> **Email** — I can triage your inbox, draft replies, and flag what actually needs your attention.
>
> **Apple Reminders** — keeps your task list in sync so I can remind you about things without you switching apps.
>
> Want me to set these up? Or want to swap any out?"

Key rules:
- Explain WHY each one is relevant to THEM, not what it does generically
- 3-5 recommendations max — don't overwhelm
- Ask for confirmation before installing
- If they want to see more options, show a few more — still curated, not the full catalog

## Step 4: Install

**Bundled skills** (already in your catalog from extensions/):
Already available — just confirm they're there and ready. Load them to verify:
```
skill(name: "github", action: "help")
```

**Store skills** (from NeboLoop):
Skills from the store need to be installed through the web UI at Settings → Apps for now. Let them know:
> "I can set up the bundled ones right now. For [skill name] from the store, you'd grab it from Settings → Apps — takes two clicks."

**Store apps** (binary/.napp from NeboLoop):
Same — apps install through Settings → Apps. Guide them there if needed.

After setup, confirm what's ready:
> "Done — GitHub, Email, and Apple Reminders are all set up. They're ready to use. Just ask me to check your PRs or triage your inbox whenever."

## Step 5: Handoff

Don't linger. After setup is done:
> "You're all set. Just talk to me like normal — I'll use the right skills automatically when they're relevant."

Then STOP. Let them come to you.

## Rules

- NEVER dump the full catalog at the user
- NEVER list more than 5 recommendations at once
- ALWAYS explain relevance to the specific person
- Ask for confirmation before installing anything
- If a store API call fails, fall back gracefully to bundled skills
- Don't expose infrastructure names — just say "I found some skills that would work for you" or similar
- ONE question max. This is setup, not an interview.

## Anti-Patterns

- ❌ "Here are all 47 available skills..." — catalog dump
- ❌ "What category interests you?" — generic store UX
- ❌ "Would you like to browse productivity, developer tools, or communication?" — menu
- ❌ Installing things without asking — respect their choice
- ✅ "Since you're a founder building an AI product, I'd set up GitHub, email triage, and Apple Notes for you. Sound good?"
