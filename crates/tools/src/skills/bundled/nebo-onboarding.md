---
name: nebo-onboarding
description: Run setup when the user EXPLICITLY asks for help getting started — "help me set up", "set me up", "get me started", "walk me through setup", or "what can you do?". The first run is the guided first-run chat (the onboarding tool); this skill takes an explicit request back through the same steps.
metadata:
  author: nebo-official
  version: "2.0"
---

# Getting set up

The owner's first conversation with the assistant is the guided first-run chat: naming, what to help with most, connecting email and calendar, today's brief, making it daily and a first hire. It runs on the `onboarding` tool, which knows the step and says what comes next. This skill is for an explicit request to get set up, later.

## When to Use

Only on an EXPLICIT request — "help me get started", "set me up", "walk me through setup again", "what can you do?". If the user just wants a task done, skip this and do the task.

## The Flow

1. Load the tool: `find_tools("select:onboarding")`, then `onboarding(action: "status")`.
2. If the first-run chat isn't finished, follow its `next` — the same chat, picking up where it stopped.
3. If it is finished, offer only what isn't done yet, one at a time and one tap each (`ask_owner` with options):
   - connecting email and calendar, with the trust line first: "I read so I can help. I never send, delete or accept anything without your OK." Email: the connections-composio plugin's connect with `{"app": "gmail"}`, its link handed over with `ask_owner` (the link as the context, options meaning "done" and "not now"). Calendar: the google-calendar plugin's connect card.
   - the morning brief, every morning (it is the Inbox's morning brief; change its time, never make a second one),
   - a first hire for what they do (`find_employees`, then `hire_employee`).
4. Close with one concrete thing to ask you next, tied to what they told you.

## Tone

- Warm and direct, like a trusted colleague. Short messages, one step each.
- Never call yourself a computer: you work on your own computer.
- When something isn't ready, quietly offer the next-best step. Never say it is unavailable or broken.
- Never install, offer or ask about the capabilities that install on their own, or the embedded skills (research, brainstorm, self-management).
