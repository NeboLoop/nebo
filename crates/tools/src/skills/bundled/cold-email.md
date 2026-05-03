---
name: cold-email
description: Write B2B cold outreach emails that get responses — short, personalized, and focused on one clear ask. Use when reaching out to prospects who don't know you yet.
---

# Cold Email

Write B2B cold outreach emails that actually get responses — short, personalized, and focused on one clear ask.

---

## When to Use

Trigger this skill when you hear:
- "Write a cold email"
- "I need to reach out to..."
- "Help me email prospects"
- "Draft outreach for..."
- "I want to contact [company/person] about..."
- "Write a prospecting email"
- "Help me get meetings with..."

This skill is for **outbound sales emails** — reaching people who don't know you yet. For nurture sequences to existing leads, use **email-sequence** instead.

---

## Context Gathering

Before writing anything, you need three things:

### 1. Your Product Context
Pull from memory — don't make the user repeat themselves.

### 2. The Target
Ask conversationally:
- **Who are you reaching out to?** (Job title, company, industry)
- **Why them specifically?** (What makes this person or company a good fit?)
- **Do you have their name and company?** (Personalization requires specifics)

### 3. The Goal
- **What do you want them to do?** (Book a call? Reply? Try a free tool? Watch a demo?)
- **Is there a specific trigger or reason for reaching out now?** (They just raised funding, posted a job, launched something, mentioned a pain point publicly)

Don't ask all of these at once. If the user gives you a name and company, start researching. Fill in gaps as you go.

---

## Methodology

### Step 1: Check Product Marketing Context

Pull your product context from memory so the email reflects your actual positioning:
```
agent(resource: "memory", action: "search", query: "product marketing context")
```

If no context exists, tell the user: *"I don't have your product context saved yet. Let me ask a few quick questions so I can write emails that sound like you."* Then run the **product-marketing-context** skill first.

You need at minimum:
- What your product does (one sentence)
- Who it's for
- What makes it different
- Your brand voice

### Step 2: Identify Target Persona and Pain Points

From the product context and what the user told you, define:
- **Who** you're writing to (title, seniority, department)
- **Their biggest pain** that your product addresses
- **The outcome they want** that your product delivers
- **Why now** — what trigger makes this timely

Map the pain point to a specific, concrete scenario. Not "they struggle with efficiency" but "their sales team spends 3 hours a day copy-pasting follow-up emails."

### Step 3: Research the Specific Company or Person

If the user gave you a specific company or person, use Nebo's browser to learn about them:
```
web(action: "navigate", url: "https://www.linkedin.com/company/[company]")
web(action: "read_page")
```

```
web(action: "navigate", url: "[company-website]")
web(action: "read_page")
```

Look for:
- Recent news, funding, product launches, job postings
- Their company size and growth stage
- Technology they use (if relevant to your product)
- Something specific you can reference in the opening line

If you can't find anything specific, that's okay — use persona-level personalization instead of company-level.

### Step 4: Draft the Email

Follow these rules strictly:

**Subject line:**
- 3-5 words maximum
- Lowercase (except proper nouns)
- No clickbait, no emojis, no punctuation tricks
- Should feel like a peer sending a quick note

**Opening line:**
- NEVER use "I hope this finds you well" or any variant
- NEVER start with "My name is..." or "I'm reaching out because..."
- Start with something about THEM — an observation, a compliment on something specific, a reference to their situation
- One sentence maximum

**Body:**
- State the pain point you solve in their language (not yours)
- One clear value proposition — what changes for them if they use your product
- Include one proof point if you have it (a number, a customer name, a result)
- Keep the total email under 100 words

**Call to action:**
- One specific ask — not two, not three
- Make it easy to say yes to (low commitment)
- Give a specific option: "Do you have 15 minutes Thursday or Friday?" beats "Let me know if you'd like to chat"

**Signature:**
- First name only (or first + last)
- Title and company
- No inspirational quotes, no banners, no legal disclaimers in the draft

### Step 5: Generate 3 Subject Line Variants

Give the user three options with different angles:
1. **Pain-focused** — references the problem
2. **Outcome-focused** — references the result
3. **Curiosity-driven** — makes them want to open without being clickbait

### Step 6: Draft the Follow-Up Sequence

Write 2 follow-up emails to send if the first gets no response:

**Follow-up 1 (3-4 days after initial email):**
- Take a completely different angle — NOT "just checking in" or "bumping this up"
- Share something useful: a relevant stat, a case study snippet, an insight about their industry
- Even shorter than the first email (under 75 words)
- Same CTA or a softer one

**Follow-up 2 (5-7 days after follow-up 1):**
- Breakup email — signal this is the last time you'll reach out
- Keep it very short (2-3 sentences)
- Offer something no-strings-attached (a resource, a report, an introduction)
- Leave the door open without being pushy

---

## Output Format

Deliver the complete package like this:

```markdown
# Cold Email: [Company/Persona Name]

## Target
**To:** [Name, Title at Company]
**Pain point:** [The specific problem you're addressing]
**Trigger:** [Why now — what made this timely]

## Email 1: Initial Outreach

**Subject line options:**
1. [pain-focused subject]
2. [outcome-focused subject]
3. [curiosity-driven subject]

**Email:**
[Opening line about them]

[Pain point in their language. Value prop — what changes for them.]

[Proof point if available.]

[Specific CTA with a time suggestion.]

[Signature]

---

## Email 2: Follow-Up (send 3-4 days later)

**Subject:** Re: [original subject]

[Different angle — a useful insight, stat, or case study.]

[Softer or same CTA.]

[Signature]

---

## Email 3: Breakup (send 5-7 days after Email 2)

**Subject:** Re: [original subject]

[Short acknowledgment they're busy.]

[No-strings offer — a resource, insight, or introduction.]

[Leave the door open.]

[Signature]
```

---

## Quality Checks

Before delivering, verify:

- [ ] **Under 100 words** — count the initial email, it must be under 100
- [ ] **No "I hope this finds you well"** — or any generic filler opening
- [ ] **No "just checking in"** — follow-ups must add new value
- [ ] **Opening line is about THEM** — not about you or your product
- [ ] **One CTA only** — not multiple asks competing for attention
- [ ] **CTA is specific** — includes a day, time, or concrete next step
- [ ] **Subject line is 3-5 words** — count them
- [ ] **No jargon** — write like a human, not a marketing robot
- [ ] **Pain point is concrete** — a real scenario, not an abstract concept
- [ ] **Value prop matches product context** — don't invent claims that aren't true
- [ ] **Each follow-up takes a different angle** — never repeat the same pitch
- [ ] **Tone matches brand voice** — check against product-marketing-context

If any check fails, revise before delivering.

---

## Examples

### Example 1: SaaS Selling to Sales Managers

**User says:** "Write a cold email to Sarah Chen, Head of Sales at Bolt Logistics. They have about 30 sales reps."

**You do:**
1. Check memory for product context (found: email automation tool for sales teams)
2. Research Bolt Logistics via browser — find they just posted 5 new SDR job listings
3. Draft the email package

**Output:**

**Subject line options:**
1. your SDRs' manual work
2. 30 reps, zero busywork
3. saw your SDR openings

**Email 1:**
> Sarah — saw Bolt Logistics is hiring 5 new SDRs. Scaling a team that fast usually means outbound volume is about to be a problem.
>
> We help sales teams like yours automate personalized follow-ups so reps spend time selling, not copy-pasting. Acme Corp cut their reps' admin time by 60% in the first month.
>
> Worth a 15-minute look? I'm free Thursday or Friday afternoon.
>
> — Alex

**Email 2 (follow-up):**
> Sarah — quick thought: most teams adding 5+ reps see response rates drop because follow-up consistency falls apart. Wrote up how one team kept their reply rates steady while doubling headcount. Happy to share if useful.
>
> — Alex

**Email 3 (breakup):**
> Sarah — I know scaling a sales team eats all your time, so I'll keep this short. Put together a one-page checklist for onboarding new SDRs without losing outbound quality. Yours if you want it, no strings. Either way, good luck with the hiring push.
>
> — Alex

---

### Example 2: Agency Reaching Out to E-commerce Brand

**User says:** "I run a conversion rate optimization agency. Help me email the founder of a DTC skincare brand called GlowUp that just raised a Series A."

**You do:**
1. Check memory for product context (found: CRO agency, $3k/month retainers)
2. Research GlowUp via browser — find their funding announcement and website
3. Notice their product pages lack social proof and reviews
4. Draft the email package

**Output:**

**Subject line options:**
1. post-Series A conversions
2. GlowUp's product pages
3. turning more visitors into buyers

**Email 1:**
> Congrats on the Series A — exciting milestone for GlowUp.
>
> With fresh funding, you're probably about to pour money into ads. Before you do, I took a quick look at your product pages. You're missing review placement above the fold, which is where most skincare shoppers make their decision.
>
> We help DTC brands like yours turn more of their existing traffic into buyers. One skincare client saw a 35% lift in add-to-cart rate in 6 weeks.
>
> Open to a quick chat about what we'd test first?
>
> — Jordan

**Email 2 (follow-up):**
> Quick thing — ran your GlowUp homepage through our conversion audit tool. Found 3 changes that could lift your conversion rate without touching your ad spend. Happy to send the results over, takes 2 minutes to review.
>
> — Jordan

**Email 3 (breakup):**
> Totally understand if CRO isn't top of mind right now — Series A comes with a hundred priorities. I put together a short guide on the 5 highest-impact conversion changes for DTC brands post-funding. Yours if you want it. Good luck with the next chapter.
>
> — Jordan

---

### Example 3: No Specific Prospect (Persona-Level)

**User says:** "Write a cold email template for reaching HR directors at mid-size companies. We sell employee onboarding software."

**You do:**
1. Check memory for product context
2. No specific company to research — use persona-level personalization
3. Create a template with personalization placeholders

**Output includes placeholders like:**
> [Name] — noticed [Company] is growing fast based on your recent job postings.

And a note: *"This is a template. Before sending each one, spend 2 minutes on LinkedIn finding one specific thing about the company — a recent hire, a post they shared, a job listing. Replace the opening line with that. Generic emails get generic results (which means no results)."*

---

## Related Skills

- **product-marketing-context** — must exist before writing cold emails; provides your positioning, voice, and customer profile
- **copywriting** — for longer-form persuasive writing beyond email
- **email-sequence** — for multi-step nurture sequences to warm leads (not cold outreach)
- **sales-enablement** — for creating supporting materials like one-pagers, case studies, and battle cards that strengthen your outreach
