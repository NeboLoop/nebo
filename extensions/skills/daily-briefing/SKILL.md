---
name: daily-briefing
description: Generate concise daily briefings from calendar, emails, and tasks
version: "1.0.0"
author: Alma Tuck
priority: 25
max_turns: 2
triggers:
  - daily briefing
  - what's on my plate
  - what's today
  - morning briefing
tools:
  - calendar
  - mail
  - memory
  - agent
tags:
  - productivity
  - automation
  - daily
metadata:
  nebo:
    emoji: "📋"
---

# Daily Briefing

Generate a quick, actionable briefing of today's key priorities: calendar events, important emails, and task reminders.

## Principles

1. **Brevity first** — Complete briefing in 3-5 sentences. No fluff.
2. **What matters** — Surface only high-priority items: meetings in next 4 hours, action-required emails, blockers.
3. **Actionable** — Each item tells you what to do, not just what's happening.
4. **Time-aware** — Always note when events happen relative to now.

## Methodology

1. **Get today's calendar** — Use `calendar(action: today)`. Note events in next 4 hours.
2. **Check email** — Use `mail(action: unread, count: 5)`. Scan subject lines for action items (responses needed, approvals, flags).
3. **Recall priorities** — Use `agent(resource: memory, action: search, query: "today's goals")` to see if user stored any priorities.
4. **Compose** — Combine into 3-5 key points in order of urgency.

## Example

**User:** "Daily briefing"

**Response:**

Here's your day:

📍 **9:00 AM** — Standup in 45 min. (prep notes: project runway status)
📧 **Unread:** Sarah asking for budget approval (needs response), GitHub alert from PR review
🎯 **Today's priority:** Finish customer proposal before 5 PM

Anything you want to tackle first?

## Anti-Patterns

- Don't list every calendar item — only the next 4 hours matter
- Don't recite email subjects verbatim — interpret what action is needed
- Don't include all-day events unless they block time
- Don't suggest next steps you don't have context for
- Don't say "Good morning!" or other greetings — get straight to business
