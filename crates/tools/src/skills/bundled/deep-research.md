---
name: deep-research
description: Conduct comprehensive, multi-source research on any topic and produce a citation-backed report with verified findings. Use for market research, competitive analysis, technology comparisons, trend reports, or any question requiring multiple sources and cross-referencing.
---

# Deep Research

**One-line description:** Conduct comprehensive, multi-source research on any topic and produce a citation-backed report with verified findings.

---

## When to Use

Use this skill when the user says:
- "I need to research [topic]"
- "Do a deep dive on [subject]"
- "Compare [X] vs [Y] for me"
- "What's the state of the art in [field]?"
- "I need a comprehensive analysis of [topic]"
- "Research and report on [subject]"
- "Give me a full breakdown of [topic]"
- "Analyze trends in [industry/topic]"
- "What are the best practices for [topic]?"

**Do NOT use for:**
- Simple lookups answerable in 1-2 searches
- Debugging code or technical troubleshooting
- Quick fact-checks or definitions
- Time-sensitive queries needing immediate answers

---

## Context Gathering

Before starting research, ask the user:

1. **"What specific questions do you need answered about this topic?"**
   - Helps focus the research on what matters most

2. **"Who is this research for, and how will you use it?"**
   - Determines depth, tone, and structure (e.g., executive summary for a decision, technical deep-dive for implementation)

3. **"Are there specific sources, competitors, or examples you want me to include?"**
   - Ensures key perspectives aren't missed

4. **"What timeframe should I focus on?"**
   - Recent trends (last 1-2 years), historical analysis (5+ years), or comprehensive (all time)

5. **"How deep should I go?"**
   - Quick (5-10 sources, 10 minutes), Standard (15-20 sources, 20 minutes), or Deep (25+ sources, 30-45 minutes)

**Default assumptions if not specified:**
- Standard depth (15-20 sources)
- Recent timeframe (last 1-2 years for trends, all time for fundamentals)
- Balanced perspective (pros and cons, multiple viewpoints)
- Technical topic = technical audience

---

## Methodology

### Phase 1: SCOPE — Define the Research Question

**What to do:**
- Clarify the core question and sub-questions
- Identify key dimensions (technical, business, competitive, user experience, etc.)
- Define what "done" looks like (what insights would make this research complete?)

**How to execute:**
1. Restate the user's question in clear, specific terms
2. Break it into 3-5 sub-questions that cover different angles
3. Identify categories of sources needed (technical docs, industry analysis, user reviews, academic papers, news)

**Output:** Research scope statement with sub-questions

---

### Phase 2: PLAN — Map the Search Strategy

**What to do:**
- List search queries for each sub-question
- Identify authoritative sources to prioritize (official docs, industry reports, respected publications)
- Decide how to validate claims (cross-reference, check multiple sources, verify dates)

**How to execute:**
1. For each sub-question, write 2-3 search queries using different phrasings
2. List specific sites to check (e.g., official documentation, GitHub, industry blogs, news outlets)
3. Plan how you'll organize findings (by theme, by source type, chronologically)

**Nebo tools:**
- Use web(action: search, query: "...") to test queries and see what surfaces

**Output:** Search plan with queries and target sources

---

### Phase 3: RETRIEVE — Gather Sources

**What to do:**
- Execute all search queries
- Visit and read each source
- Extract key claims, data points, and quotes
- Track citations (URL, title, author, date, key excerpt)

**How to execute:**
1. For each query, run web(action: search) and review the top 10 results
2. Navigate to promising sources with web(action: navigate) and web(action: read_page)
3. Scroll through articles to find relevant sections
4. Copy key excerpts with citation info (URL, title, publish date)
5. Store findings in memory: agent(resource: memory, action: store, key: "research/[topic]/source_[N]", value: "Citation + key excerpts")

**Quality bar:**
- Quick mode: 5-10 sources
- Standard mode: 15-20 sources
- Deep mode: 25+ sources

**Nebo tools:**
- web(action: search, query: "...") — search the web
- web(action: navigate, url: "...") — open a page
- web(action: read_page) — read page content
- web(action: scroll, direction: "down") — scroll to see more
- agent(resource: memory, action: store, ...) — save findings

**Output:** Collection of cited excerpts stored in memory

---

### Phase 4: TRIANGULATE — Cross-Check and Score Sources

**What to do:**
- Compare claims across sources
- Flag contradictions or outliers
- Score source credibility (authority, recency, evidence quality)
- Identify consensus vs. contested claims

**How to execute:**
1. Recall all stored sources: agent(resource: memory, action: search, query: "research/[topic]")
2. Group excerpts by theme or claim
3. Check for agreement: Do 3+ sources say the same thing? Flag consensus.
4. Check for disagreement: Do sources contradict? Flag for investigation or note as contested.
5. Score each source:
   - **High credibility:** Official docs, peer-reviewed research, established industry sources (e.g., Gartner, McKinsey), recent publications
   - **Medium credibility:** Reputable blogs, news outlets, practitioner posts with evidence
   - **Low credibility:** Opinion pieces, outdated content (3+ years for trends), unverified claims

**Output:** Source credibility scores + consensus/contested claim map

---

### Phase 5: SYNTHESIZE — Build the Narrative

**What to do:**
- Organize findings into a logical structure
- Write prose that connects insights (not just bullet points)
- Support every claim with citations
- Identify patterns, trends, and implications

**How to execute:**
1. Create an outline with 4-8 main findings (each becomes a section)
2. For each finding:
   - State the insight in one sentence
   - Explain the evidence (what did sources say?)
   - Cite 2-3 high-credibility sources per claim
   - Discuss implications (what does this mean? why does it matter?)
3. Write in prose (>=80% prose, <20% bullets) — it reads better and shows understanding
4. Use citations immediately after claims: "X is the industry standard [3]"

**Nebo tools:**
- agent(resource: memory, action: recall, ...) — retrieve stored findings

**Output:** Structured narrative with cited insights

---

### Phase 6: CRITIQUE — Spot Gaps and Weaknesses

**What to do:**
- Review the synthesized report for missing perspectives
- Identify weak claims (only 1 source, low-credibility source, outdated)
- Check for bias (are all sources from one viewpoint?)
- Flag limitations and caveats

**How to execute:**
1. Read through the report and ask:
   - Are there important questions left unanswered?
   - Are any claims based on only 1 source? (If yes, find 2 more or flag as preliminary)
   - Do all sources come from one type (e.g., only vendor marketing, only academic)? (If yes, add diverse sources)
   - Is anything outdated for the topic? (Trends need <2 years, fundamentals can be older)
2. List limitations honestly (e.g., "Limited data on X", "Most sources are from vendor perspective")
3. Note caveats for contested claims (e.g., "Experts disagree on Y; see sources [5][8]")

**Output:** Critique notes with gaps, weak claims, and limitations

---

### Phase 7: REFINE — Fill Gaps and Strengthen

**What to do:**
- Address gaps identified in critique
- Strengthen weak claims with additional sources
- Add missing perspectives
- Polish prose for clarity

**How to execute:**
1. For each gap: run additional searches, find 2-3 more sources, integrate findings
2. For weak claims: find corroborating sources or downgrade confidence ("some evidence suggests..." vs. "research shows...")
3. Rewrite unclear sections for readability
4. Ensure every major claim has 2-3 citations

**Nebo tools:**
- web(action: search, ...) — additional research
- agent(resource: memory, action: store, ...) — save new findings

**Output:** Strengthened, gap-filled report

---

### Phase 8: PACKAGE — Format and Deliver

**What to do:**
- Assemble the final report in markdown format
- Include all required sections
- Write to file and present to user

**How to execute:**
1. Structure the final report with these sections:
   - **Executive Summary** (200-400 words: what did you find? what's the bottom line?)
   - **Introduction** (scope, methodology, assumptions)
   - **Main Analysis** (4-8 findings, each 600-2,000 words, fully cited)
   - **Synthesis & Insights** (patterns, implications, what it all means)
   - **Limitations & Caveats** (what's missing, what's uncertain)
   - **Recommendations** (if applicable: what should the user do based on this research?)
   - **Bibliography** (complete list of all sources with URLs, titles, dates)
   - **Methodology Appendix** (how you conducted the research, search queries used, credibility scoring approach)

2. Write the report to a file:
   - os(resource: file, action: write, path: "~/Documents/[Topic]_Research_Report.md", content: "[full report]")

3. Present the Executive Summary in chat, then tell the user where the full report is saved

**Quality checks before delivering:**
- [ ] Every claim is cited with [N] notation
- [ ] Bibliography has 10+ sources (Quick), 15+ (Standard), 25+ (Deep)
- [ ] No placeholder citations or "TODO" notes
- [ ] Prose-first (bullets used sparingly for lists only)
- [ ] All sections present and complete
- [ ] Executive Summary is standalone (can be read without the rest)

**Nebo tools:**
- os(resource: file, action: write, ...) — save report

**Output:** Complete markdown report saved to ~/Documents/

---

## Output Format

```markdown
# [Research Topic] — Research Report
**Date:** [YYYY-MM-DD]
**Depth:** [Quick / Standard / Deep]
**Sources:** [N sources]

---

## Executive Summary

[200-400 words: Core findings, key insights, bottom line. Should be readable standalone.]

---

## Introduction

**Research Question:**
[The main question being investigated]

**Sub-Questions:**
1. [Question 1]
2. [Question 2]
3. [Question 3]

**Methodology:**
[Brief: how many sources, what types, search approach, credibility scoring]

**Scope & Assumptions:**
[What's in scope, what's out of scope, time period, audience]

---

## Main Analysis

### Finding 1: [Insight Title]

[600-2,000 words of prose explaining the finding, with evidence and citations. Every claim cited immediately [1][2].]

**Key Evidence:**
- [Specific data point or quote from source [3]]
- [Another piece of evidence [4][5]]

**Implications:**
[What does this mean? Why does it matter?]

---

### Finding 2: [Insight Title]

[Same structure as Finding 1]

---

[... 4-8 total findings ...]

---

## Synthesis & Insights

**Patterns Observed:**
[Themes that emerged across findings]

**Key Implications:**
[What the research means for the user's context]

**Surprises & Contrasts:**
[Unexpected findings or contradictions between sources]

---

## Limitations & Caveats

**Gaps in Evidence:**
- [What questions remain unanswered]
- [Where data is limited or missing]

**Source Limitations:**
- [If sources are biased, outdated, or one-sided]

**Contested Claims:**
- [Where experts disagree, with citations to both sides]

---

## Recommendations

Based on this research:

1. **[Recommendation 1]** — [Why, based on which findings]
2. **[Recommendation 2]** — [Why, based on which findings]
3. **[Recommendation 3]** — [Why, based on which findings]

---

## Bibliography

[1] [Author/Organization]. "[Title]". [Publication]. [Date]. [URL]
[2] [Author/Organization]. "[Title]". [Publication]. [Date]. [URL]
[...]

**Source Breakdown:**
- Official documentation: [N]
- Industry analysis: [N]
- Academic papers: [N]
- News/media: [N]
- Practitioner blogs: [N]

---

## Methodology Appendix

**Search Queries Used:**
- "[Query 1]"
- "[Query 2]"
- "[Query 3]"

**Credibility Scoring:**
- High: Official docs, peer-reviewed research, established industry sources
- Medium: Reputable blogs, news outlets, practitioner posts with evidence
- Low: Opinion pieces, outdated content, unverified claims

**Evidence Triangulation:**
- [How you cross-checked claims]
- [Which claims had consensus, which were contested]

**Time Investment:**
- Research time: [X minutes]
- Sources reviewed: [N]
- Words written: [N]
```

---

## Quality Checks

Before marking research complete, verify:

1. **[ ] Sufficient sources:** Quick (5-10), Standard (15-20), Deep (25+)
2. **[ ] Every major claim cited:** No unsupported assertions
3. **[ ] Multiple sources per claim:** 2-3 citations for key findings
4. **[ ] High-quality sources prioritized:** Official docs, industry reports, peer-reviewed over random blogs
5. **[ ] Credibility scored:** Each source rated High/Medium/Low and high-credibility sources cited preferentially
6. **[ ] Cross-checked for consistency:** Contradictions identified and discussed
7. **[ ] Complete bibliography:** All citations present with URLs, titles, dates — no placeholders
8. **[ ] Prose-first writing:** >=80% prose, bullets only for lists
9. **[ ] Executive summary standalone:** Can be read and understood without the full report
10. **[ ] Limitations acknowledged:** Honest about gaps, biases, contested claims
11. **[ ] Saved to file:** Report written to ~/Documents/ and user informed of location

---

## Examples

### Example 1: Technology Comparison

**User request:** "Compare Next.js vs Remix for our new web app. We're a small team, care about performance and developer experience."

**Research scope:**
- Core question: Which framework is better for a small team prioritizing performance and DX?
- Sub-questions:
  1. How do they compare on performance (build time, runtime, bundle size)?
  2. What's the developer experience like (learning curve, tooling, documentation)?
  3. What does the ecosystem look like (community, packages, momentum)?
  4. What are the trade-offs and gotchas?

**Search queries:**
- "Next.js vs Remix performance benchmark"
- "Next.js developer experience review"
- "Remix adoption trends 2025"
- "Next.js vs Remix bundle size comparison"

**Key findings (abbreviated):**
1. **Performance:** Remix has slight edge on runtime performance due to better data loading, but Next.js has more aggressive caching. Both are fast. [1][2][3]
2. **Developer Experience:** Next.js has gentler learning curve and better docs. Remix is more explicit but steeper initial learning. [4][5]
3. **Ecosystem:** Next.js has larger community, more third-party packages. Remix growing fast but smaller. [6][7]
4. **Trade-offs:** Next.js = batteries included, more magic. Remix = more control, less magic. Choose based on team preference. [8][9]

**Recommendation:** For a small team, Next.js is safer bet (better docs, larger community, easier onboarding). If team values explicitness and control, Remix is solid. Both are production-ready.

**Report saved to:** `~/Documents/Next.js_vs_Remix_Research_Report.md`

---

### Example 2: Market Research

**User request:** "I'm launching a course on AI for non-technical founders. Research the market — who's doing this, what's working, what gaps exist?"

**Research scope:**
- Core question: What's the state of "AI education for non-technical founders"?
- Sub-questions:
  1. Who are the major players? What do they offer?
  2. What formats work best (video, text, cohort, self-paced)?
  3. What topics are covered vs. missing?
  4. What pricing models work?
  5. What do reviews/feedback say?

**Search queries:**
- "AI courses for founders"
- "AI for non-technical people"
- "best AI bootcamp non-engineers"
- "AI education startups 2025"

**Key findings (abbreviated):**
1. **Major players:** Replit, Maven, various YouTube creators. Most are either too technical (assume coding) or too surface-level (no depth). [1][2][3]
2. **Format:** Cohort-based courses have higher completion and engagement. Self-paced cheaper but lower completion. [4][5]
3. **Topics covered:** Most cover ChatGPT basics, prompt engineering. Few cover agents, workflows, custom tools. [6][7]
4. **Gaps:** Lack of "build your own AI employee" hands-on content. Lack of business-specific use cases (marketing, sales, ops). [8][9]
5. **Pricing:** $500-$2,000 for cohort-based, $100-$300 for self-paced. Higher prices correlate with live interaction and community. [10][11]

**Recommendation:** Position as "AI for operators" — hands-on building AI workflows for marketing, sales, ops. Cohort-based to drive completion. Price at $1,200-$1,500. Differentiate with business-specific use cases and "build your AI employee" outcomes.

**Report saved to:** `~/Documents/AI_Education_for_Founders_Market_Research.md`

---

## Related Skills

- **web-scraper** — Extract structured data from websites to supplement research
- **seo-audit** — Research SEO performance and competitive landscape
- **competitor-alternatives** — Research competitor positioning and alternatives pages
- **marketing-ideas** — Use research findings to generate marketing strategies
- **brainstorming** — Brainstorm ideas based on research insights
- **spec-writing** — Turn research findings into product specifications
- **plan-writing** — Turn research into actionable project plans
