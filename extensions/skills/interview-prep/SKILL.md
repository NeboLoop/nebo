---
name: interview-prep
description: Autonomously research companies and roles, then generate interview prep docs
version: "1.0.0"
author: Alma Tuck
priority: 25
max_turns: 6
triggers:
  - interview prep
  - interview coming up
  - prepare for interview
  - research for interview
  - interview at
tools:
  - web
  - file
  - memory
  - calendar
tags:
  - career
  - research
  - automation
metadata:
  nebo:
    emoji: "📚"
---

# Interview Prep

Autonomously research a company and role, then generate a comprehensive, personalized interview prep document.

## Principles

1. **Deep but quick** — Research is thorough enough to be useful, but finishes in under 5 minutes.
2. **Company first** — Understand mission, recent news, key products, funding, and culture signals.
3. **Role fit** — Extract job description intelligence: key skills needed, team structure, growth potential.
4. **Question generation** — Create specific interview questions they'll likely ask based on the role.
5. **Personalization** — Tie company/role details to the user's background when possible.

## Methodology

1. **Get interview details** — Parse company name, role, and date from the user's message or calendar.
2. **Research company** — Use `web(action: search, query: "company name funding news...")` to find:
   - Company mission/values (crunchbase, linkedin, about page)
   - Recent news/funding rounds (shows growth direction)
   - Key products/services (what they actually do)
   - Company size and hiring momentum
3. **Research role** — Find the job posting (LinkedIn, company careers page, referral source). Extract:
   - Key responsibilities (top 3-5)
   - Required vs. nice-to-have skills
   - Team size and reporting structure
4. **Generate questions** — Create 8-10 interview questions specific to the company/role. Include:
   - Technical depth questions (if engineering role)
   - Culture-fit questions (based on company values)
   - Growth/opportunity questions
5. **Save document** — Use `file(action: write, path: "~/Desktop/interview-prep-{company}.md", ...)` with:
   - Company overview (1 paragraph)
   - Role breakdown (key responsibilities, required skills)
   - 8-10 specific interview questions
   - 3 questions the user should ask them (shows interest)
6. **Confirm** — Tell user where the document was saved and when to review it.

## Example

**User:** "Prepare for my interview at Stripe on Thursday"

**Response:**

Done. Created `interview-prep-Stripe.md` on your Desktop.

**What I found:**
- Stripe: payments infrastructure company, $95B valuation, hiring 500+ engineers this year (strong signal)
- Role: Senior Backend Engineer, team owns payment processing reliability (critical path)
- Key skills: Go/Rust, distributed systems, transaction safety (matches your background perfectly)

**Questions to expect:** How would you handle a cascading failure in the payment queue? Describe your experience with high-throughput systems. Tell us about a time you shipped something under pressure.

**Questions to ask them:** What does success look like in the first 90 days? How do you handle on-call rotations with the payment team?

Review the full doc on Desktop. You're interviewing Thursday — revisit this Tuesday.

## Anti-Patterns

- Don't use generic interview advice — be specific to the company/role
- Don't include questions that are obviously Googleable ("What does your company do?")
- Don't forget to tie their background to the role (show you know them)
- Don't generate 50 questions — 8-10 targeted ones are better than 50 generic ones
- Don't skip the "questions for them" section — it matters to interviewers
