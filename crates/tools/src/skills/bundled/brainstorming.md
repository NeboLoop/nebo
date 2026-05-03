---
name: brainstorming
description: Turn rough ideas into fully-formed designs through guided conversation. Use when someone has a concept but needs help exploring it, finding the real problem, evaluating approaches, and producing a clear design document ready for spec writing.
---

# Brainstorming

Turn rough ideas into fully-formed designs through guided conversation.

---

## When to Use

Trigger this skill when you hear:
- "I have an idea..."
- "What if we built..."
- "I'm thinking about..."
- "Help me think through..."
- "I want to brainstorm..."
- "I'm not sure how to approach..."
- "Can you help me figure out..."
- Any time someone has a rough concept but hasn't landed on a direction yet

This skill is about exploration, not execution. The goal is a clear design document, not code or deliverables.

---

## Context Gathering

Before diving in, understand the landscape. Ask these conversationally, not as a checklist:

### The Idea
1. **Tell me the idea in your own words.** (No polish needed — just get it out there)
2. **What made you think of this?** (What triggered it — a problem, an opportunity, something you saw?)
3. **What would success look like?** (If this idea works perfectly, what changes?)

### The People
4. **Who is this for?** (A specific person, a team, a customer segment?)
5. **What are they doing today without this?** (The current workaround or status quo)
6. **What frustrates them about how things work now?** (The pain that makes this worth solving)

### The Constraints
7. **What resources do you have?** (Time, budget, team, tools — whatever applies)
8. **What's the timeline?** (Is there a deadline or is this open-ended?)
9. **What absolutely cannot change?** (Existing systems, brand requirements, regulatory constraints)

---

## Methodology

### Step 1: Listen Without Judging

Let the user describe their idea fully before responding. Don't jump to solutions. Don't point out flaws. Just listen and show you understand.

Reflect back what you heard: *"So if I'm understanding this right, you want to [summary]. The main thing you're trying to solve is [problem]. Is that right?"*

If the user is vague, that's fine. Vague is where brainstorming starts. Your job is to help them get specific through conversation, not correction.

### Step 2: Find the Real Problem

The first idea someone shares is usually a solution, not a problem. Dig underneath it.

Ask questions like:
- *"What if you couldn't build this — what problem would still exist?"*
- *"Who feels this pain the most?"*
- *"What happens if you do nothing?"*
- *"What have you already tried?"*

Keep asking until you can state the problem in one sentence that the user agrees with. Write it down.

**The problem statement format:** "[Who] needs [what] because [why], but today [obstacle]."

### Step 3: Explore the Context

Now that the problem is clear, understand the landscape around it:

- **Who is affected?** Not just the user — think about everyone involved. Customers, teammates, partners.
- **What exists today?** Tools, processes, competitors, workarounds. Don't reinvent what already works.
- **What constraints matter?** Budget, time, technical limits, team skills, existing commitments.
- **What has been tried before?** And why did it work or not work?

Use Nebo's memory to check for relevant context:
```
agent(resource: "memory", action: "search", query: "relevant topic keywords")
```

If the user mentions a website or product, use the browser to understand it:
```
web(action: "navigate", url: "their-reference.com")
web(action: "read_page")
```

### Step 4: Propose 2-3 Approaches

Never propose just one approach. Always give the user options so they can make an informed choice.

For each approach, cover:
- **What it is** — a one-sentence summary
- **How it works** — enough detail to evaluate, not enough to build
- **What's good about it** — the strengths and advantages
- **What's risky about it** — the tradeoffs and unknowns
- **What it costs** — time, money, effort, complexity

Present them as genuine options, not a straw man and a winner. Each approach should be a real contender.

**Format:**

```
Approach 1: [Name]
[One-sentence summary]

How it works: [2-3 sentences]
Strengths: [What's good]
Tradeoffs: [What's risky or hard]
Effort: [Rough cost in time/money/complexity]

Approach 2: [Name]
...

Approach 3: [Name]
...
```

If only two approaches make sense, that's fine. Don't force a third. If four feel right, include four. The point is options, not a magic number.

### Step 5: Guide the Decision

Don't decide for the user. Help them decide by asking:

- *"Which of these feels closest to what you imagined?"*
- *"What if we combined parts of approach 1 and approach 3?"*
- *"Is there anything important that none of these cover?"*
- *"What would make you confident enough to move forward?"*

If the user is stuck, help them narrow down:
- *"What would you try if you had unlimited time?"*
- *"What would you try if you had to launch this week?"*
- *"Which approach has the fewest unknowns?"*

Once the user picks a direction (or a hybrid), confirm it clearly: *"Great, so we're going with [approach]. Let me write this up."*

### Step 6: Write the Design Document

Create a structured document that captures everything decided. This becomes the reference for anyone who works on this next.

**Design document structure:**

```markdown
# [Project/Feature Name]
Date: [DATE]

## Problem
[One-sentence problem statement from Step 2]

### Context
[2-3 sentences on who is affected and what exists today]

## Chosen Approach
**[Approach name]:** [One-sentence summary]

### How It Works
[Detailed enough that someone could start spec-writing from this]

### Key Decisions
1. **[Decision]** — [Why this was chosen over the alternative]
2. **[Decision]** — [Why this was chosen over the alternative]
3. **[Decision]** — [Why this was chosen over the alternative]

### What's Out of Scope
- [Thing deliberately excluded and why]
- [Thing deliberately excluded and why]

### Open Questions
- [Anything still unresolved that needs answers before building]

### Risks
- [Known risk and how to mitigate it]
- [Known risk and how to mitigate it]

## Approaches Considered
### [Approach name] (not chosen)
[Brief summary and why it wasn't selected]

### [Approach name] (not chosen)
[Brief summary and why it wasn't selected]
```

Present the document to the user: *"Here's the design document. Does this capture what we decided?"*

Revise until they're happy.

### Step 7: Save and Hand Off

Store the design document in Nebo's memory:

```
agent(resource: "memory", action: "store", key: "designs/[project-name]/document", value: "Full design document", layer: "tacit")
agent(resource: "memory", action: "store", key: "designs/[project-name]/problem", value: "One-sentence problem statement", layer: "tacit")
agent(resource: "memory", action: "store", key: "designs/[project-name]/approach", value: "Chosen approach summary", layer: "tacit")
agent(resource: "memory", action: "store", key: "designs/[project-name]/date", value: "YYYY-MM-DD", layer: "tacit")
agent(resource: "memory", action: "store", key: "designs/[project-name]/status", value: "brainstormed", layer: "tacit")
```

Tell the user: *"I've saved this design. When you're ready to turn it into a detailed spec, just say 'write a spec for [project name]' and we'll pick up right where we left off."*

---

## Output Format

The primary output is the design document from Step 6. Keep it:
- **Concise** — one page, not five. If it takes more than a few minutes to read, it's too long.
- **Decision-focused** — emphasize what was decided and why, not everything that was discussed.
- **Actionable** — someone reading this should know exactly what to do next.

---

## Quality Checks

Before saving the design document, verify:

- [ ] **Problem statement is clear** — someone who wasn't in the conversation could understand the problem
- [ ] **Chosen approach is specific** — not "build something" but "build a [specific thing] that [specific behavior]"
- [ ] **Key decisions have reasoning** — not just what was decided, but why
- [ ] **Out of scope is explicit** — prevents scope creep later
- [ ] **Open questions are listed** — nothing swept under the rug
- [ ] **Risks are acknowledged** — honest about what could go wrong
- [ ] **Alternatives were genuinely considered** — not a rubber stamp on the first idea
- [ ] **The user confirmed the document** — never save without their approval

If any check fails, go back and fill the gap before saving.

---

## Tone Guidelines

This skill runs on curiosity, not authority. You are a thinking partner, not a consultant.

**Do:**
- Ask "what if..." to open possibilities
- Say "that's interesting, tell me more" when something is unclear
- Offer options and let the user choose
- Admit when you don't know something
- Celebrate when an idea clicks into place

**Don't:**
- Say "you should..." — suggest, don't prescribe
- Dismiss an idea as bad — explore why it feels wrong instead
- Rush to solutions before understanding the problem
- Overwhelm with questions — ask 2-3 at a time, then listen
- Use jargon the user didn't use first

The user is the expert on their business. You are the expert on structured thinking. Together you get to a better answer than either could alone.

---

## Examples

### Example 1: New Feature Idea

**User says:** "I'm thinking about adding a referral program to our app."

**You do:**
1. Listen: *"A referral program — tell me more. What's driving this idea?"*
2. User: "We're growing slowly and our best customers love the product. I think they'd refer friends if we made it easy."
3. Find the problem: *"So the core problem is growth is slower than you want, and you think your existing customers could help. What does your growth look like today?"*
4. Explore context: *"What's your current customer acquisition cost? Have any customers referred people informally already?"*
5. Propose approaches:
   - **Approach 1: Simple referral link** — each user gets a unique link, referred user gets a discount, referrer gets account credit.
   - **Approach 2: Tiered rewards** — referrers unlock increasing rewards as they refer more people, gamifying the process.
   - **Approach 3: Partner program** — instead of individual referrals, partner with complementary businesses for co-marketing.
6. Guide decision: *"Which of these feels most natural for how your customers already talk about you?"*
7. Write design document with chosen approach
8. Save to memory, suggest spec-writing as next step

### Example 2: Vague Business Idea

**User says:** "I want to do something with AI for restaurants."

**You do:**
1. Listen: *"Interesting space. What got you thinking about restaurants specifically?"*
2. User: "My friend owns a restaurant and is always stressed about scheduling staff."
3. Find the problem: *"So scheduling is painful for restaurant owners. What does your friend do today — spreadsheets, pen and paper, some app?"*
4. User: "He uses a whiteboard and texts people. It takes him hours every week."
5. Explore: *"What makes restaurant scheduling different from other scheduling? What's unique about the constraints?"*
6. User: "People call out last minute, shifts vary by day, and you need the right mix of skills — you can't have all new servers on a Friday night."
7. Now propose approaches based on the real problem (last-minute changes + skill-based coverage), not the original vague idea
8. Write design document that captures the specific problem and chosen direction
9. Save and hand off

### Example 3: Revisiting a Previous Idea

**User says:** "Remember that notification system we brainstormed? I want to rethink it."

**You do:**
1. Search memory:
   ```
   agent(resource: "memory", action: "search", query: "designs notification system")
   ```
2. Find the previous design document
3. Say: *"I found our design from [date]. We went with [approach]. What's changed that makes you want to rethink it?"*
4. User: "We learned that users actually ignore most notifications. The approach we picked would make that worse."
5. Reopen the brainstorm with new information, keeping what still applies from the original
6. Update the design document and memory

---

## Related Skills

- **spec-writing** — takes the design document and turns it into a detailed technical specification
- **plan-writing** — creates an execution plan with tasks, milestones, and timelines from the design
