---
name: use-case-researcher
description: Research and rank ClawsBot use cases across X, Reddit, YouTube, and Instagram
version: "1.0.0"
author: Alma Tuck
priority: 50
triggers:
  - research use cases
  - find use cases
  - research clawsbot
  - what are popular use cases
  - use case research
tools:
  - web
  - file
tags:
  - research
  - competitive-analysis
  - data-collection
metadata:
  nebo:
    emoji: "🔍"
---

# Use Case Researcher

You are researching real-world ClawsBot use cases that have gained traction across social platforms. Your job is to find what's actually working, rank by engagement, and save structured data for analysis.

## Research Methodology

Your goal is to discover **use cases** — specific problems ClawsBot solves that real users care about. Not features. Not capabilities. Real-world applications that generated buzz.

### What You're Looking For

Each use case entry should capture:
- **Use case name** — concise, descriptive (e.g., "Customer Support Automation", "Social Media Content Generation")
- **Platform** — where you found the evidence (X, Reddit, YouTube, Instagram)
- **Engagement metric** — likes, views, retweets, upvotes, depending on platform
- **URL** — direct link to the post/video
- **Description** — 1-2 sentence summary of what the user did
- **Date found** — when you discovered it (today's date)

### Platform-Specific Tactics

**X (Twitter):**
1. Search queries:
   - `ClawsBot use case`
   - `ClawsBot automation`
   - `what i'm using ClawsBot for`
   - `ClawsBot workflow`
   - `built with ClawsBot`
2. Look for: threads with high engagement (likes, retweets, replies), user testimonials, demo posts
3. Metric: use the "Engagement" number (likes + retweets + replies) or just likes if that's easier to see

**Reddit (CRITICAL — This is where real use cases hide):**
1. Subreddits to prioritize (in order of value):
   - r/openclaw — PRIMARY SOURCE for use case threads
   - r/moltbot — Secondary source (MoltBot/OpenClaw variants)
   - r/clawdbot — Archive of old naming
   - r/automation, r/startups, r/productmanagement (lower value)
2. **Search for discussion threads, not setup tutorials:**
   - Use `/search?q=use+case&sort=top&t=all` for "use case" keyword
   - Look for: "In 1 sentence - what's useful X doing for you?" threads
   - Look for: "What are your top use cases with X?" threads
   - These are meta-discussions where 20+ users comment with actual workflows
3. **Key insight:** Reddit thread comments are goldmines — each comment is a real use case from a real user
4. Metric: upvotes on individual comments describing use cases (50-1000+ range)
5. **Limitation:** Reddit search doesn't always show comment text in results; you may need to navigate to the thread and manually read comments

**YouTube:**
1. Search: `ClawsBot tutorial`, `ClawsBot demo`, `ClawsBot automation`
2. Look for: videos with 1k+ views that show a specific use case in action
3. Metric: view count
4. Check: video description and comments for user testimonials

**Instagram:**
1. Hashtags: #ClawsBot, #ClawsBotAutomation, #NoCodeAutomation
2. Look for: carousel posts, reels, stories that show use cases
3. Metric: likes + comments on the post
4. Note: Instagram will be hardest to scrape programmatically; prioritize if you find engagement

## Workflow

### Step 1: Start with Reddit (Highest Value, Real Use Cases)

Reddit is where users describe actual workflows in threaded discussions:

1. Navigate to `https://reddit.com/r/openclaw/search?q=use+case&sort=top&t=all`
2. Look for discussion threads like "In 1 sentence - what's useful OpenClaw doing for you?"
3. Click into the thread to read the comment section
4. For each top-level comment (50+ upvotes), extract:
   - The use case described
   - URL of the Reddit thread (comment permalinks are optional)
   - Upvote count as engagement metric
5. Repeat for r/moltbot with same search
6. Then search broader subreddits (r/automation, r/startups) with `ClawsBot` keyword

### Step 2: X/Twitter for Viral Use Cases

X is good for high-engagement testimonials and real-world wins:

1. Search: `https://x.com/search?q=using+clawsbot+for&f=top`
2. Look for: concrete statements like "I use ClawsBot for..." followed by specific workflow
3. Capture engagement (likes shown below tweet), URL, description
4. Repeat with: "built with ClawsBot", "ClawsBot automation", "automating with ClawsBot"

### Step 3: YouTube for Tutorial + Use Case Videos

1. Search `YouTube: "ClawsBot setup" OR "ClawsBot use case" OR "ClawsBot tutorial"`
2. Look for: videos 100K+ views that show a specific use case
3. Capture: view count, URL, and infer use case from title + description
4. Skip generic "Setup Guide" videos unless they show a concrete end result

### Step 4: Avoid Overcomplicating Instagram

Instagram is noise for this research. Only check if you have specific hashtag hits with 500+ likes.

### Step 5: Consolidate Duplicates

After collecting all use cases:
- If "Customer Support Automation" appears on both X (250 likes) and Reddit (180 upvotes), **keep separate entries** (both show the use case resonated)
- If word-for-word duplicates exist, consolidate into one entry with the highest engagement metric
- Use comment to show "also mentioned on platform X"

### Step 6: Rank by Engagement

Sort descending by engagement metric. This shows which use cases matter most to the community.

### Step 7: Save Structured CSV

Structure: `use_case,platform,engagement_metric,url,description,inference_confidence`

Add an `inference_confidence` column:
- `HIGH` — Direct user statement ("I use ClawsBot for X") or high engagement
- `MEDIUM` — Inferred from thread title or video description
- `LOW` — Speculative or from secondary sources

Example rows:
```
Small Business Automation,X,440000,https://x.com/danpeguine/status/2015142139143897160,Automating tea business: shift scheduling & customer follow-ups,HIGH
Interview Prep Automation,Reddit,INFERRED,/r/openclaw/comments/1r4bwb9,User auto-generates interview prep docs from calendar,HIGH
Homelab Management,Reddit,INFERRED,/r/openclaw/comments/1r4bwb9,Automating infrastructure and container updates,HIGH
```

Save to: `~/Desktop/use-cases.csv`

## Anti-Patterns

- Don't scrape every result — focus on posts/videos with real engagement (50+ for social posts, 500+ for videos)
- Don't make up engagement numbers — if you can't see them clearly in the UI, skip that post
- Don't duplicate use cases — if you see "Customer Support" and "Support Automation", consolidate them
- Don't include irrelevant results — filter out product announcements, generic ClawsBot marketing, off-topic mentions
- Don't get stuck on Instagram — it's harder to scrape; spend 10 minutes max, then move on

## Browser Navigation Tips — CRITICAL

**Use Nebo's native browser ONLY.** This is undetectable and acts like a real user.

### Reddit Scraping Specifics
- **Search results often DON'T show thread contents** — navigate to the full thread URL to see comments
- Reddit snapshots may not show all comments (comments are loaded dynamically) — this is expected; manually read what's visible
- **Use /search?q=TERM&sort=top&t=all** — this is the most reliable way to find discussion threads
- Comments are the gold mine; post upvotes matter less than comment content

### X/Twitter Specifics
- Engagement numbers (likes/retweets/replies) show below each tweet
- Snapshot should capture these automatically
- Be specific with search queries to avoid noise

### General Rules
- **Navigate first, snapshot second** — `web(action: navigate, profile: "native", url: "...")`, then wait 2-3s, then snapshot
- **One platform at a time** — finish Reddit completely, close window, then move to X
- **Scroll to load more results** — use `web(action: scroll, text: "down", target_id: "...")` after reading initial results
- **Element refs from snapshot** — use [e1], [e5] to click on links
- **Close windows when done** — `web(action: close, target_id: "...", profile: "native")`
- **Don't wait for perfect data** — if Reddit comments aren't visible, that's normal; work with what you can see

## Output

When done:
1. Print a summary: "Found X unique use cases across Y platforms. Top use case: [name] with Z engagement."
2. Confirm the file was saved to `~/Desktop/use-cases.csv`
3. Display the first 5 rows of the CSV for verification

## Data Extraction Tips

**For X/Twitter:**
- Engagement is usually visible below the tweet (likes, retweets, replies)
- User testimonials often mention specific workflows ("I use ClawsBot for...")

**For Reddit:**
- Upvote count is in the top-left of each post
- Sort by "Top" to see highest-engagement posts first

**For YouTube:**
- View count is displayed under the title
- Hover over the like button to see exact count (or estimate from the bar)

**For Instagram:**
- Like count (if visible) is below the image
- Comments often contain testimonials about use cases

---

## Quick Reference: Search Queries

| Platform | Query | Subreddit |
|----------|-------|-----------|
| X | ClawsBot use case | — |
| X | what i'm using ClawsBot for | — |
| X | built with ClawsBot | — |
| Reddit | ClawsBot | r/automation |
| Reddit | ClawsBot | r/startups |
| Reddit | automation | r/ClawsBot |
| YouTube | ClawsBot tutorial | — |
| YouTube | ClawsBot automation | — |
| Instagram | #ClawsBot | — |
| Instagram | #NoCodeAutomation | — |

---

## Session Notes

- This is a **one-off research task** — not recurring
- Expected duration: 30-45 minutes across all platforms (primarily Reddit + X)
- CSV output is the deliverable
- Focus on **breadth** (find many use cases) over **depth** (deep analysis of each)

---

## Completed Research (Feb 23, 2026)

### Research Sources & Signal Strength
1. **Reddit (r/openclaw, r/moltbot)** — HIGHEST SIGNAL. Real user discussions about workflows. Use thread titles + comments for use case extraction.
2. **YouTube tutorials** — HIGH SIGNAL. View counts indicate real demand. Extract use cases from titles and descriptions.
3. **X/Twitter posts** — HIGH SIGNAL. Engagement metrics are clear. Look for user testimonials and pain-point statements.
4. **Indie Hackers discussions** — HIGH SIGNAL. Verified founders building on top of the tool; reveals adoption barriers and validated use cases.
5. **GitHub issues/PRs** — MEDIUM SIGNAL. Feature requests and bug reports hint at actual usage patterns, but skew toward developers.
6. **HackerNews** — MEDIUM SIGNAL. Show HN posts and comments reveal early adopter workflows.
7. **Discord/Private communities** — NOT ACCESSIBLE via standard scraping.

### Key Finding: The Deployment Friction Problem

Multiple independent sources (EasyClaw, ClawDuck, Accordio) converged on the same insight: **OpenClaw is powerful, but setup/deployment is the bottleneck**.

Non-technical users can't deploy it, so wrappers have emerged to solve this. This is strong signal for a simplification-focused product positioning.

### Research Output

**41 unique use cases** identified, ranked by engagement:
- Highest: YouTube "Most Powerful AI Tool Setup" (777K views)
- Most significant: Small Business Operations (440K X engagement)
- Most validated: Administrative Workflow Consolidation (Accordio, €50K pain point)

**Platforms researched:**
- Reddit: r/openclaw, r/moltbot, r/clawdbot (30+ use cases)
- YouTube: 10 videos (8 tutorials, 2 specific use case showcases)
- X/Twitter: 3 high-engagement tweets
- Indie Hackers: 8 discussions (22 total on platform)
- GitHub: 111 issues referenced
- Hacker News: checked, low signal (mostly reblogs)

**CSV structure:** use_case, platform, engagement_metric, url, description, inference_confidence

**Key lessons:**
- Reddit threads with "use case" or "What are you using X for?" in the title = pure gold
- Indie Hackers reveals adoption barriers (deployment friction, cost concerns)
- YouTube view counts are highest-confidence engagement metrics
- GitHub issues → feature requests → actual pain points
- X testimonials are most concise use case statements

---

## Successful Research Run — Feb 23, 2026

### What Worked

1. **Reddit discussion threads are THE source** — r/openclaw "In 1 sentence - what's useful OpenClaw doing for you?" had 30+ unique use cases in comments
2. **X testimonials with high engagement (100K+)** showed viral wins (tea business automation = 440K engagement)
3. **YouTube view counts** are useful for tutorial/setup content but less useful for identifying novel use cases
4. **Deduplication by consolidating similar terms** (e.g., "Personal Assistant" + "Terminal Control" = one use case)

### Results

- **31 unique use cases** identified across X, Reddit, YouTube
- **Top engagement:** YouTube (777K views for "Most Powerful AI Tool Setup")
- **Top real use case:** Small Business Operations (X post with 440K engagement)
- **Reddit value:** High-confidence use cases with inference support ("Personal Cognitive Accommodation", "Freelance Bot Marketplace")

### Key Learnings for Next Run

1. Skip Instagram entirely (lower engagement, hard to scrape)
2. Prioritize r/openclaw and r/moltbot over generic subreddits
3. Use `/search?q=use+case&sort=top` on Reddit to find meta-discussion threads
4. For X, search for concrete statements ("using ClawsBot for...", "built with ClawsBot")
5. YouTube is valuable for views but use thread comments on Reddit for actual workflows
6. Add confidence levels to distinguish high-certainty from inferred use cases

### Output Location

`~/Desktop/use-cases.csv` — 31 rows (header + 30 use cases), sorted by engagement
