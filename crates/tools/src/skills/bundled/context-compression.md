---
name: context-compression
description: Keep long conversations focused and productive by managing context degradation. Use when AI output quality drops in long conversations, instructions are being ignored, or you need to decide whether to summarize, checkpoint, or start a fresh session.
---

# Context Compression

Keep conversations focused and productive, especially long ones.

---

## When to Use

Trigger this skill when you hear:
- "The AI forgot what we were talking about"
- "This conversation is getting too long"
- "It's not following my instructions anymore"
- "Should I start a new conversation?"
- "The AI is repeating itself"
- "It seems confused now"
- When a conversation has gone back and forth more than 15-20 times

Long conversations are one of the most common reasons AI output degrades. This skill teaches how to keep things sharp.

---

## Why Conversations Lose Focus

AI has a limited attention span — not in the human sense, but in a very real technical sense. Every conversation has a maximum amount of information the AI can hold at once. Think of it like a desk: you can only spread out so many papers before things start falling off the edges.

As a conversation gets longer, three things happen:

1. **Early details fade.** Instructions you gave at the beginning carry less weight than what you said recently. The AI doesn't "forget" them exactly, but they get diluted by everything that came after.

2. **Contradictions creep in.** Over a long conversation, you might refine your thinking. The AI now has your original request AND your updated version, and it may blend them in confusing ways.

3. **Noise builds up.** Not everything in a conversation matters equally. Brainstorming tangents, rejected ideas, small talk — it all takes up space that could hold useful information.

The fix isn't to avoid long conversations. It's to manage them deliberately.

---

## Methodology

### Step 1: Recognize When Context Is Degrading

Help the user spot the warning signs:

- The AI stops following instructions you gave earlier
- Responses get more generic or repetitive
- The AI contradicts something it said before
- You find yourself re-explaining things you already covered
- Answers feel "off" even though you haven't changed what you're asking for

Ask the user: *"Have you noticed any of these happening? That's usually a sign the conversation has gotten too long and the AI is struggling to hold everything together."*

### Step 2: Front-Load What Matters

The most important information should go at the beginning of your request, not buried in the middle or tacked on at the end. The AI pays the most attention to:

- The first thing you say in a message
- The most recent thing you said
- Anything you explicitly label as important

**Before:** "So I was thinking about the email and maybe we should try a different angle, like what if we focused on the pain point we discussed earlier about onboarding being slow, and keep it short, probably under 150 words, and make sure the CTA is about booking a demo not signing up"

**After:** "Write a short email (under 150 words) about slow onboarding. CTA: book a demo. Tone: empathetic, not salesy."

The second version puts the key information up front and cuts the thinking-out-loud. Save the thinking for your own notes — give the AI the conclusion.

### Step 3: Summarize Before Continuing

When a conversation has been going for a while, pause and create a checkpoint. Say something like:

*"Let me summarize where we are before we continue."*

Then provide a clear summary:
- What we decided
- What's still open
- What we're doing next

This resets the AI's focus. It now has a clean, current picture instead of having to piece together 30 messages of back-and-forth.

**Example summary:**
"Here's where we are: We decided on a 5-email welcome sequence. Emails 1-3 are drafted and approved. Email 4 is about the premium features. We still need to write emails 4 and 5. The tone is casual and helpful, not pushy. Let's draft email 4 now."

### Step 4: Know When to Start Fresh

Sometimes the best move is a new conversation. Start fresh when:

- **The topic changed significantly.** You started with email copy but now you're doing competitive research. That's a new task.
- **You've made major revisions.** If you've gone back and forth on direction 5+ times, the conversation is full of outdated versions. Start clean with just the final direction.
- **The AI seems confused.** If responses are inconsistent or ignoring your instructions, a fresh start with a clear brief will work better than trying to course-correct.
- **You're past 30+ exchanges.** Even good conversations start to degrade at this length.

**Don't start fresh when:**
- You're in the middle of iterating on something and the AI is tracking well
- You've built up useful context that would take time to recreate
- The conversation is long but still focused on one topic

### Step 5: Save Before You Close

Before starting a new conversation, save anything important to memory. This way you don't lose the work — you just lose the noise.

```
agent(resource: "memory", action: "store", key: "project/[name]/decisions", value: "Key decisions made", layer: "tacit")
agent(resource: "memory", action: "store", key: "project/[name]/status", value: "Current status and next steps", layer: "tacit")
```

Then in the new conversation, you can pull it back:
```
agent(resource: "memory", action: "search", query: "project [name] decisions")
```

### Step 6: Use Focused Sessions

Teach the user to break big projects into focused sessions instead of one massive conversation:

- **Session 1:** Research and strategy (save findings to memory)
- **Session 2:** Draft first batch of content (pull strategy from memory)
- **Session 3:** Revise and finalize (pull drafts and feedback from memory)

Each session starts clean with just the relevant context loaded from memory. No noise from previous sessions, no degraded attention.

---

## Output Format

When helping someone compress their context, produce a clean summary they can copy into a new conversation:

```markdown
## Project Brief
**Goal:** [What we're trying to accomplish]
**Audience:** [Who it's for]
**Decisions made:**
- [Decision 1]
- [Decision 2]
- [Decision 3]

**Current status:** [Where things stand]
**Next step:** [What to do now]

**Constraints:**
- [Tone/voice requirements]
- [Length/format requirements]
- [Things to avoid]
```

---

## Quality Checks

Before finishing this skill, verify:

- [ ] **The user understands why conversations degrade** — they can explain it simply
- [ ] **They know how to front-load information** — key details go first, not buried
- [ ] **They can create a checkpoint summary** — they've done one or seen one done
- [ ] **They know when to start fresh** — and when to keep going
- [ ] **They saved important context to memory** — decisions, status, and constraints are preserved
- [ ] **They understand focused sessions** — big projects get broken into smaller conversations
- [ ] **No technical jargon was used** — "token limits" and "context windows" were avoided
- [ ] **Their current conversation was rescued** — if they came here because things went sideways, fix it now

---

## Examples

### Example 1: Conversation Gone Sideways

**User says:** "The AI was writing great emails earlier but now the tone is all wrong and it's ignoring my instructions."

**You do:**
1. Say: "That usually happens when a conversation gets long. The early instructions lose weight as new messages pile up. Let's fix it."
2. Ask: "What are the key decisions we've made so far? Tone, audience, what's been approved?"
3. Create a checkpoint summary with the user
4. Say: "Let's give the AI this summary as a reset." Paste the summary and re-request the current task.
5. If the conversation is very long, suggest: "We could also start a fresh conversation with just this summary. That way the AI has a clean slate with only the information that matters."
6. Save key decisions to memory either way

### Example 2: Big Project Planning

**User says:** "I need to create a whole marketing campaign — emails, landing page, ads, social posts."

**You do:**
1. Say: "That's a big project. Rather than trying to do it all in one conversation, let's break it into focused sessions. Each one will be sharper because the AI isn't juggling everything at once."
2. Map out the sessions:
   - Session 1: Campaign strategy and messaging (save to memory)
   - Session 2: Email sequence (pull strategy from memory)
   - Session 3: Landing page copy (pull strategy from memory)
   - Session 4: Ad copy and social posts (pull strategy from memory)
3. Start with Session 1
4. At the end, save all decisions to memory
5. Say: "When you're ready for the emails, start a new conversation and say 'pull up the campaign strategy for [name].' The AI will load everything we decided and we'll jump right into writing."

---

## Related Skills

- **context-fundamentals** — understand what context is and why it matters
- **memory-best-practices** — save the right things so fresh conversations start strong
- **multi-agent-patterns** — use sub-agents for focused tasks instead of one long conversation
- **evaluation** — check if output quality has degraded and decide whether to compress or restart
- **plan-writing** — structure big projects into manageable phases
