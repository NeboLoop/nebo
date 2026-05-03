---
name: evaluation
description: Judge whether AI output is actually good by checking accuracy, completeness, relevance, actionability, and hallucination risk. Use before publishing, sending, or acting on AI-generated content, or when something feels off but you cannot pinpoint why.
---

# Evaluation

Judge whether AI output is actually good, not just impressive-sounding.

---

## When to Use

Trigger this skill when you hear:
- "Is this actually good?"
- "How do I know if the AI got it right?"
- "This sounds good but I'm not sure"
- "Can I trust this output?"
- "Something feels off about this"
- "How do I check AI work?"
- When a user is about to publish, send, or act on AI output without reviewing it

AI is very good at sounding confident. This skill teaches you to look past the polish and judge what actually matters.

---

## Why Evaluation Matters

AI output has a specific failure mode that's different from human work: it sounds authoritative even when it's wrong. A human who doesn't know the answer usually hesitates, hedges, or admits uncertainty. AI rarely does this. It writes with the same confident tone whether the answer is spot-on or completely made up.

This means you can't use "does it sound good?" as your quality check. You need a more deliberate approach.

The good news: evaluating AI output is a learnable skill, and once you develop it, your results improve dramatically — because you get better at asking for what you actually need.

---

## Methodology

### Step 1: Check for Accuracy

The most important question: **Is this actually true?**

AI can and does make things up. Not out of malice — it generates what sounds right based on patterns, and sometimes the pattern leads to fiction presented as fact.

**Watch for:**
- **Statistics without sources** — "Studies show that 73% of customers prefer..." Where did that number come from? If the AI can't tell you, it probably made it up.
- **Named claims** — "According to Harvard Business Review..." Did HBR actually say that? Check before using it.
- **Specific details that seem too perfect** — If every data point conveniently supports the argument, some might be fabricated.
- **Historical or technical claims** — AI is particularly prone to errors with dates, names, and technical specifications.

**What to do:**
- Ask the AI: "Are you certain about [specific claim]? Can you provide a source?"
- Spot-check 2-3 specific facts, especially ones you'll be sharing publicly
- For high-stakes content, verify key claims independently
- If something seems too good to be true, it probably is

### Step 2: Check for Completeness

**Did it actually cover everything you asked for?**

AI sometimes drops parts of your request, especially when you asked for multiple things at once. It might nail the first two points and quietly skip the third.

**Watch for:**
- **Missing items from a list** — you asked for 5 ideas, it gave you 4
- **Shallow treatment of hard topics** — it went deep on the easy stuff and glossed over the hard stuff
- **Missing constraints** — you said "under 150 words" and it wrote 300
- **Skipped requirements** — you said "include a CTA" and there's no CTA

**What to do:**
- Compare the output against your original request point by point
- Check that every requirement was addressed, not just acknowledged
- If something is missing, ask specifically: "You didn't address [topic]. Can you add that?"

### Step 3: Check for Relevance

**Does it answer what was actually asked?**

AI can produce excellent content that completely misses the point. It's helpful, it's well-written, it's thorough — and it answers a different question than the one you asked.

**Watch for:**
- **Generic answers to specific questions** — you asked about your industry, it gave you general business advice
- **Answering the easy version** — you asked "how do we fix churn?" and it answered "why churn matters"
- **Information dump instead of an answer** — lots of background but no clear recommendation
- **Drifting from the topic** — starts relevant, wanders into adjacent but unhelpful territory

**What to do:**
- After reading the output, ask yourself: "If I forwarded this to my team, would they say it answered the question?"
- Check the first and last paragraphs — they should both relate to what you asked
- If the response is off-target, re-ask with more specific context: "I'm specifically asking about [X], not [Y]"

### Step 4: Check for Actionability

**Can you actually use this?**

Some AI output is technically correct and relevant but impossible to act on. "Create a content strategy" isn't actionable. "Publish two blog posts per week on these three topics, targeting these keywords, with this distribution plan" is actionable.

**Watch for:**
- **Advice without specifics** — "focus on your target audience" (how?)
- **Strategy without tactics** — "build brand awareness" (through what channels? with what budget?)
- **Recommendations without priorities** — 15 things to do with no sense of which matter most
- **Plans without timelines or owners** — what gets done first? by whom? by when?

**What to do:**
- Ask: "Can I take action on this tomorrow morning? What exactly would I do first?"
- If you can't answer that, the output needs to be more specific
- Request: "Make this more actionable. Give me specific steps I can take this week."

### Step 5: Check for Hallucination Patterns

**Is the AI making up things that sound plausible?**

Hallucination is when the AI generates information that doesn't exist but presents it as fact. It's the most dangerous failure mode because it's the hardest to detect.

**Common hallucination patterns:**
- **Fake quotes** — attributed to real people who never said them
- **Invented tools or platforms** — references to products or features that don't exist
- **Fabricated case studies** — "Company X increased conversions by 340% using this approach"
- **Nonexistent research** — citing papers, studies, or reports that were never published
- **Made-up URLs** — links that look legitimate but go nowhere
- **False feature descriptions** — describing what a product does inaccurately

**What to do:**
- Treat any specific claim (names, numbers, quotes, URLs) as unverified until you check
- Ask the AI: "Are you generating this from training data or did you verify this from a current source?"
- For critical content, use a sub-agent to fact-check: "Verify these claims by searching the web for each one"
- When in doubt, remove the specific claim and make the point without it

### Step 6: Use Evaluation to Improve Future Output

The point of evaluation isn't just to catch mistakes — it's to improve how you work with AI going forward.

**After evaluating, ask:**
- If accuracy was the issue: "I need to be more specific about what's factual vs. what needs to be generated"
- If completeness was the issue: "I should number my requirements so nothing gets skipped"
- If relevance was the issue: "I need to give better context about what I'm actually trying to accomplish"
- If actionability was the issue: "I should ask for specific steps, not general advice"

Save your evaluation criteria to memory so the AI knows your standards:

```
agent(resource: "memory", action: "store", key: "preferences/quality-standards", value: "Always cite sources for statistics. Never fabricate quotes. Prioritize actionable recommendations over general advice. Flag uncertainty explicitly.", layer: "tacit")
```

---

## Output Format

When evaluating AI output for a user, present your assessment clearly:

```markdown
## Evaluation: [What was evaluated]

### Accuracy
[Rating: Strong / Needs verification / Concerns]
- [Specific findings]

### Completeness
[Rating: Complete / Partial / Missing key elements]
- [What's covered]
- [What's missing]

### Relevance
[Rating: On target / Partially relevant / Off track]
- [How well it addresses the actual question]

### Actionability
[Rating: Ready to use / Needs specifics / Too vague]
- [What can be acted on immediately]
- [What needs more detail]

### Hallucination Risk
[Rating: Low / Medium / High]
- [Specific claims to verify]

### Overall
[Summary and recommendation: use as-is, revise, or redo]
```

---

## Quality Checks

Before finishing this skill, verify:

- [ ] **The user knows the five checks** — accuracy, completeness, relevance, actionability, hallucination
- [ ] **They can spot hallucination** — they understand that confident tone doesn't mean correct
- [ ] **They know how to verify** — ask for sources, spot-check facts, compare against request
- [ ] **They can give better feedback** — not just "this is wrong" but "this is wrong because [specific issue]"
- [ ] **They connect evaluation to better prompting** — evaluation failures point to context gaps
- [ ] **Quality standards are saved** — their preferences and standards are in memory
- [ ] **No false confidence was created** — they don't think a checklist makes them infallible, just better
- [ ] **They have a practical habit** — a quick evaluation routine they can use every time

---

## Examples

### Example 1: Suspicious Statistics

**User says:** "The AI wrote a blog post with a bunch of statistics. Can you check if they're real?"

**You do:**
1. Read through the blog post and identify every specific claim (statistics, quotes, named studies)
2. Flag each one:
   - "73% of customers prefer self-service" — no source cited, likely generated
   - "According to McKinsey's 2024 report..." — searchable, let's verify
   - "Companies that blog get 55% more traffic" — this is a commonly cited stat from HubSpot, but the actual number and methodology should be verified
3. Search the web to verify the key claims
4. Report: "Two of the five statistics check out. One is a commonly cited stat but the specific number is slightly off. Two appear to be generated — I couldn't find any source for them."
5. Say: "For the ones that can't be verified, we have two options: find real statistics to replace them, or rewrite those sentences to make the point without specific numbers."
6. Save quality preference: "Always cite sources for statistics in blog posts"

### Example 2: Marketing Copy Review

**User says:** "I had the AI write landing page copy. Does this look good to go?"

**You do:**
1. Pull product context and customer profile from memory
2. Evaluate against the five checks:
   - **Accuracy:** Does it describe the product correctly? Do the claims match what it actually does?
   - **Completeness:** Does it cover the value prop, address pain points, have a CTA, address objections?
   - **Relevance:** Does it speak to the ideal customer, or is it generic?
   - **Actionability:** Is the CTA clear? Does a reader know what to do next?
   - **Hallucination:** Are there any claims about results, integrations, or features that aren't real?
3. Present findings: "The copy is strong on voice and CTA. Two issues: the headline focuses on features instead of the customer's pain point, and it claims 'seamless integration with 50+ tools' — is that accurate?"
4. Suggest specific fixes rather than vague feedback
5. Say: "After fixing those two things, this is ready to publish."

---

## Related Skills

- **context-fundamentals** — better context leads to output that passes evaluation more often
- **context-compression** — degraded context from long conversations often causes evaluation failures
- **memory-best-practices** — save quality standards so the AI learns what "good" means to you
- **copy-editing** — detailed editing goes deeper than evaluation on written content
- **deep-research** — when evaluation reveals claims that need verification
