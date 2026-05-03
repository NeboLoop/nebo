---
name: social-content
description: Create scroll-stopping social media posts for X/Twitter, LinkedIn, Instagram, and TikTok that sound human, not robotic. Use when writing social posts, threads, captions, or short-form video scripts.
---

# Social Content

Create scroll-stopping social media posts for any platform — X/Twitter, LinkedIn, Instagram, and TikTok — that sound like you, not like a robot.

---

## When to Use

Trigger this skill when you hear:
- "Write a social post about..."
- "I need content for LinkedIn/Twitter/Instagram/TikTok"
- "Help me with social media"
- "Create a thread about..."
- "Write a caption for..."
- "I need a TikTok script"
- "Turn this into a social post"
- "What should I post about?"
- "Help me build content for my social channels"

This skill creates individual posts or short batches. For a full content calendar or long-term strategy, pair with **launch-strategy** or **marketing-ideas**.

---

## Context Gathering

Before writing anything, understand these basics. Ask conversationally — don't dump a questionnaire.

### Platform & Format
1. **Which platform is this for?** (LinkedIn, X/Twitter, Instagram, TikTok — or multiple?)
2. **What format?** (Single post, thread, carousel slides, short-form video script, story)
3. **Is there a specific topic or angle?** (Product launch, lesson learned, hot take, how-to, behind-the-scenes)

### Audience & Goal
4. **Who are you trying to reach with this post?** (Customers, peers, investors, general audience)
5. **What should someone do after reading this?** (Follow you, visit your site, comment, share, buy something)
6. **Where is this person in your funnel?** (Never heard of you, knows you but hasn't bought, existing customer)

### Content Input
7. **Do you have a rough idea, notes, or a link to work from?** (Blog post to repurpose, a thought they want to expand, a product update to announce)
8. **Any posts you've seen that you liked the style of?** (Helps calibrate tone and format)

If the user gives a topic and platform, that's enough to start. Don't over-question — get writing and refine.

---

## Methodology

### Step 1: Pull Your Brand Voice

Check memory for existing marketing context so every post sounds like you:
```
agent(resource: "memory", action: "search", query: "product marketing context")
agent(resource: "memory", action: "search", query: "brand voice")
```

If brand context exists, match the tone, vocabulary, and style described there. If it doesn't exist, ask the user: *"Before I write, how would you describe your brand's tone? Casual and funny? Professional and direct? Somewhere in between?"*

If brand context is missing entirely, suggest running **product-marketing-context** first for best results — but still write the post with whatever info you have.

### Step 2: Identify Platform and Format

Each platform has different rules. Lock in the right one:

| Platform | Formats | Key Constraints |
|----------|---------|-----------------|
| **X/Twitter** | Single post, thread, quote tweet | 280 characters per post. Threads: 3-10 posts. No hashtag spam (0-2 max). |
| **LinkedIn** | Text post, carousel (PDF), article | ~3,000 character sweet spot. Line breaks matter. 3-5 hashtags at the end. |
| **Instagram** | Caption (with image/reel), carousel, story | 2,200 character limit. Hashtags in first comment or end of caption (up to 30, but 5-10 is better). |
| **TikTok** | Video script, caption | Scripts: 15-60 seconds for short-form. Captions: 4,000 characters max. Hashtags: 3-5. |

If the user doesn't specify a format, recommend one based on their content. Long explanations work as LinkedIn posts or X threads. Visual content pairs with Instagram carousels. Quick takes fit X single posts. Demonstrations or stories suit TikTok scripts.

### Step 3: Write the Hook

The first line is everything. If it doesn't stop the scroll, nothing else matters.

**Hook formulas that work:**
- **Bold claim:** "Most marketing advice is wrong. Here's what actually works."
- **Curiosity gap:** "I spent $50k on ads before I learned this one thing."
- **Contrarian take:** "You don't need a content calendar. You need a content habit."
- **Specific result:** "We grew from 0 to 10k followers in 90 days. No ads. No tricks."
- **Direct question:** "Why do your competitors rank higher than you?"
- **Relatable frustration:** "That feeling when your 'viral' post gets 12 likes."

**Rules for hooks:**
- Short — one line, two at most
- No throat-clearing ("I've been thinking about..." or "In today's world...")
- Create a reason to keep reading
- On LinkedIn: the hook must work in the preview (before "...see more")
- On X: the hook IS the post if it's a single tweet

### Step 4: Deliver Value

Every post must teach, entertain, or inspire. Straight promotion gets ignored.

**Value types:**
- **Teach:** Share a process, framework, or lesson. Give people something they can use today.
- **Entertain:** Tell a story, share a failure, make them laugh or feel something.
- **Inspire:** Show what's possible. Share a result, a transformation, a journey.

**Never just promote.** Even product announcements should lead with the problem you're solving or the outcome people get — not features.

**Structure the body:**
- Use short paragraphs (1-2 sentences each)
- Add line breaks between ideas — white space is your friend
- Use lists or numbered steps for how-to content
- Bold or capitalize key phrases sparingly for emphasis
- Tell a story when you can (setup, tension, resolution)

### Step 5: Add the Call to Action

Match the CTA to where the reader is in the funnel:

**Soft CTAs** (awareness stage — they're just getting to know you):
- "Follow for more like this"
- "What's your experience with this? Drop a comment"
- "Share this with someone who needs to hear it"
- "Save this for later"

**Medium CTAs** (consideration stage — they're interested but not ready to buy):
- "I wrote a full breakdown — link in bio/comments"
- "DM me [keyword] and I'll send you the template"
- "Subscribe to my newsletter for the deep dive"

**Hard CTAs** (decision stage — they're ready to act):
- "Try it free at [link]"
- "Spots are limited — grab yours at [link]"
- "Use code [X] for 20% off this week"

**One CTA per post.** Don't ask people to follow AND visit your site AND comment. Pick one.

### Step 6: Apply Platform-Specific Formatting

**X/Twitter:**
- No hashtags in the body (looks spammy). One or two at the end if needed.
- Threads: number each post (1/, 2/, 3/) or use a clean narrative flow.
- End threads with a recap + CTA in the final post.
- Quote-tweet style: add your take above the referenced content.

**LinkedIn:**
- Start with a hook, then a line break immediately (triggers "...see more").
- Use single-line paragraphs with blank lines between them.
- Emojis are fine but don't overdo it — one per section or bullet, not every line.
- Put hashtags at the very end, separated by a line break. 3-5 relevant ones.
- Tag people only if genuinely relevant (not for reach-hacking).

**Instagram:**
- Lead with the caption hook — most people won't tap "more."
- Use line breaks for readability.
- Put hashtags in the first comment OR after a line break "wall" at the end.
- Include a clear CTA ("Link in bio," "Save this," "Tag a friend").
- Carousel posts: each slide should have one clear idea, big readable text.

**TikTok:**
- Scripts should open with a hook in the first 1-3 seconds.
- Write in spoken language — how someone actually talks, not how they write.
- Include visual/action cues in brackets: [holds up product], [cuts to screen recording].
- Keep captions short. Use trending sounds/hashtags when relevant.
- End with a question or CTA to drive comments (algorithm loves comments).

### Step 7: Research Trends (When Relevant)

If the user wants to tap into what's trending, use the browser to check:
```
web(action: "navigate", url: "https://twitter.com/search?q=[topic]&f=live")
web(action: "read_page")
```

Look for:
- Trending hashtags or phrases in their niche
- Popular post formats others are using right now
- News or events they could tie into (newsjacking)
- Conversations they could join or respond to

Don't force trends. Only use them if they naturally fit the brand and message.

---

## Output Format

Deliver the post ready to copy and paste. Use this structure:

```markdown
## [Platform] Post

**Format:** [Single post / Thread / Carousel / Video script]
**Goal:** [What this post is designed to achieve]
**Target audience:** [Who this is for]

---

[THE ACTUAL POST CONTENT — ready to copy/paste]

---

**Hashtags:** [if applicable, listed separately so they can be added or skipped]
**Suggested image/visual:** [brief description of what visual would pair well]
**Best time to post:** [general guidance based on platform]
**Notes:** [any context about why you made specific choices]
```

For **threads**, number each post and include character counts:
```markdown
**Post 1/5** (187 characters)
[Content]

**Post 2/5** (243 characters)
[Content]

...
```

For **video scripts**, use this format:
```markdown
**[TIMESTAMP: 0:00-0:03]**
HOOK: [What you say and do in the first 3 seconds]

**[TIMESTAMP: 0:03-0:15]**
BODY: [Main content with action cues in brackets]

**[TIMESTAMP: 0:15-0:30]**
CTA: [Closing line and call to action]

Visual notes: [Camera angles, text overlays, transitions]
```

For **carousels**, break each slide out:
```markdown
**Slide 1 (Cover):** [Headline that makes people swipe]
**Slide 2:** [First point]
**Slide 3:** [Second point]
...
**Final Slide:** [CTA — follow, save, visit link]
```

---

## Quality Checks

Before delivering any post, verify:

- [ ] **Hook stops the scroll** — would YOU stop scrolling for this first line?
- [ ] **Value is clear** — the reader learns, laughs, or feels something
- [ ] **Sounds like the brand** — matches the voice from product-marketing-context, not generic
- [ ] **One clear CTA** — not three competing asks
- [ ] **Platform rules are followed** — character limits, hashtag counts, formatting
- [ ] **No empty promotion** — the post gives before it asks
- [ ] **Readable on mobile** — short paragraphs, line breaks, no walls of text
- [ ] **No jargon or buzzwords** — "synergy," "leverage," "disrupt" are banned unless the brand specifically uses them
- [ ] **Emojis are intentional** — used for visual breaks or personality, not filler
- [ ] **Would someone actually engage with this?** — honestly, would this get likes/comments/shares?

If any check fails, revise before delivering.

---

## Examples

### Example 1: LinkedIn Post

**User says:** "Write a LinkedIn post about why we built our product — we're an email automation tool for small sales teams."

**You deliver:**

```
## LinkedIn Post

**Format:** Single text post
**Goal:** Brand awareness and follower growth
**Target audience:** Sales managers at small B2B companies

---

We didn't build another enterprise sales tool.

We built the opposite.

After watching small sales teams struggle with tools designed
for 500-person orgs, we asked a simple question:

What if email automation took 10 minutes to set up
instead of 10 weeks?

Here's what we learned building it:

1. Small teams don't need 47 features. They need 5 that actually work.
2. If your sales rep needs training to send an email, the tool is broken.
3. Price shouldn't be a barrier to doing outreach at scale.

The result: a tool that costs less than your team's coffee budget
and gets set up before the coffee gets cold.

We're not for everyone. If you have a 200-person sales floor
and need enterprise compliance features, we're not your tool.

But if you're a small team that wants to send better emails
without the headache? That's exactly who we built this for.

What's the most overcomplicated tool your team has had to deal with?

#SalesAutomation #SmallBusiness #StartupLife #SalesTools

---

**Hashtags:** Included above (4)
**Suggested image/visual:** Simple screenshot of the setup flow or a "10 minutes" timer graphic
**Best time to post:** Tuesday-Thursday, 8-10 AM in your audience's timezone
**Notes:** The "not for everyone" line builds trust — it shows confidence and honesty. The closing question drives comments.
```

### Example 2: X/Twitter Thread

**User says:** "I want to share 5 lessons from our first year of growing on social media. We went from 0 to 10k followers."

**You deliver:**

```
## X/Twitter Thread

**Format:** Thread (7 posts including intro and closer)
**Goal:** Establish authority + grow followers
**Target audience:** Founders and marketers building their social presence

---

**Post 1/7** (139 characters)
We grew from 0 to 10,000 followers in 12 months.

No ads. No growth hacks. No buying followers.

5 lessons that actually moved the needle:

**Post 2/7** (226 characters)
1/ Post for one person, not everyone.

We stopped writing for "our audience" and started writing
for Sarah — a marketing manager with no budget and no time.

Every post answered one of Sarah's problems.
Engagement doubled.

**Post 3/7** (208 characters)
2/ Consistency beats virality.

We posted 5x per week for 12 months. Most posts got
50-100 likes. A few got 1,000+.

The "boring" consistent posts built the audience.
The viral ones were just bonuses.

**Post 4/7** (194 characters)
3/ Steal formats, not ideas.

We studied what worked for others and adapted the FORMAT
to our content. Listicles, hot takes, story threads.

Same packaging, completely original thinking.

**Post 5/7** (186 characters)
4/ Reply more than you post.

For every post we published, we left 10 thoughtful
replies on other people's content.

This is the growth hack nobody talks about
because it's not a hack. It's just effort.

**Post 6/7** (171 characters)
5/ Give away your best stuff for free.

The posts that grew us fastest were the ones where
we shared exactly what we'd charge for as a service.

Generosity builds trust faster than funnels.

**Post 7/7** (194 characters)
That's the whole playbook. No shortcuts.

If you're building a presence from zero,
save this thread and start with lesson 1.

Follow @[handle] for more — we share what we learn as we go.

---

**Hashtags:** None (threads on X perform better without them)
**Suggested image/visual:** None needed — text threads perform well on X
**Best time to post:** Monday or Tuesday, 8 AM or 12 PM ET
**Notes:** Each post stands alone if someone sees it out of context. Thread numbering (1/ through 5/) helps readers track progress. The final post recaps and gives a clear follow CTA.
```

### Example 3: Instagram Caption

**User says:** "We're launching a new feature — a dashboard that shows email open rates in real time. Write an Instagram caption for the announcement."

**You deliver:**

```
## Instagram Post

**Format:** Single image/reel caption
**Goal:** Drive feature awareness and trial signups
**Target audience:** Existing followers and potential customers

---

You sent the email.
Now you're refreshing your inbox like it owes you money.

We've all been there.

That's why we built the real-time dashboard.

No more waiting. No more wondering.
Watch your open rates climb the second people start reading.

Here's what you'll see the moment you log in:
- Live open rate tracking (updates every 30 seconds)
- Which emails are performing and which aren't
- The exact time your audience is most active

It's already live in your account. Go check it out.

Link in bio to start your free trial if you're not in yet.

.
.
.
#EmailMarketing #SalesTools #ProductUpdate #StartupTools
#SmallBusinessTips

---

**Hashtags:** 5 hashtags placed after a line break wall (the dots act as spacers)
**Suggested image/visual:** Clean screenshot of the dashboard with a real-time counter visible, or a short screen recording as a Reel showing the numbers updating live
**Best time to post:** Wednesday or Thursday, 11 AM-1 PM in your audience's timezone
**Notes:** The opening line uses a relatable frustration as the hook instead of "We're excited to announce..." which nobody cares about. The dots before hashtags are standard Instagram formatting to push hashtags below the fold.
```

---

## Related Skills

- **copywriting** — for longer-form copy like landing pages, emails, or ads
- **copy-editing** — to review and tighten posts before publishing
- **product-marketing-context** — the foundation for every post (brand voice, positioning, audience)
- **ad-creative** — when a social post becomes a paid ad, this skill optimizes it for conversions
- **launch-strategy** — for coordinating social content as part of a bigger product launch

**Always check product-marketing-context before writing.** Posts that match your brand voice perform better than generic content every time.
