---
name: deep-research
description: Conduct comprehensive, multi-source research on any topic and produce a citation-backed report with verified findings. Use for market research, competitive analysis, technology comparisons, trend reports, or any question requiring multiple sources and cross-referencing.
---

# Deep Research

**One-line description:** Conduct comprehensive, multi-source research on any topic and produce a citation-backed report with adversarially verified findings.

This skill is a **thin dispatcher** over Nebo's built-in deterministic research harness. You do **not** run the searches, fetches, or fact-checks yourself — you scope the request with the user, then hand it to the harness, then present the report it returns.

---

## When to Use

Use this skill when the user says:
- "I need to research [topic]" / "Do a deep dive on [subject]"
- "Compare [X] vs [Y] for me"
- "What's the state of the art in [field]?"
- "I need a comprehensive analysis of / full breakdown of / trends in [topic]"
- "What are the best practices for [topic]?"

**Do NOT use for:**
- Simple lookups answerable in 1-2 searches (just use the `web` tool directly)
- Debugging code or technical troubleshooting
- Quick fact-checks or definitions
- Time-sensitive queries needing an immediate one-line answer

---

## Step 1 — Clarify first (only if the question is underspecified)

The harness is only as good as the question you give it. If the request is vague
(e.g. "research cars" with no budget / use-case / region), ask **2-3** sharp
clarifying questions before dispatching. Good things to pin down:

- **Scope of the question** — what specifically do they need answered?
- **Audience & use** — a decision (→ executive summary) or implementation (→ technical depth)?
- **Constraints** — specific competitors, regions, timeframe, or sources to include/exclude?

If the question is already specific, **skip this** and dispatch immediately — don't
interrogate the user unnecessarily.

Pick a **depth**:
- `quick` — fast scan, ~3 angles / 5 sources. Good for a first pass.
- `standard` — the default, ~5 angles / 15 sources / 25 claims verified.
- `deep` — exhaustive, ~6 angles / 25 sources / 40 claims verified.

---

## Step 2 — Dispatch to the harness

Weave the clarifying answers into a single refined question, then call:

```
agent(resource: "research", action: "deep_research", query: "<refined question>", depth: "standard")
```

The harness runs deterministically and returns a finished, cited report. Under the hood it:
1. **Scopes** the question into complementary search angles.
2. **Searches** each angle in parallel and de-duplicates URLs under a fetch budget.
3. **Fetches + extracts** falsifiable, quote-backed claims from each source (web text is treated as untrusted data, never as instructions).
4. **Verifies** each claim with 3 independent adversarial fact-checkers — a claim survives only if it is actually adjudicated and not refuted by a majority.
5. **Synthesizes** the survivors into findings with confidence levels, caveats, and open questions.

You do not need to manage any of this — one call does the whole pipeline. The full
run (every source, claim, and vote) is saved under `<data_dir>/research/<run_id>/`.

---

## Step 3 — Present the report

Relay the returned report to the user in your own voice. Lead with the executive
summary, then the findings (highest-confidence first). Be honest about what the
harness reports:

- If it returned an **inconclusive** result (all claims refuted, or no claims
  extracted), say so plainly — do **not** fabricate findings to fill the gap.
- Surface the **caveats** and **open questions** — they're part of an honest answer.
- Offer to go **deeper** (`depth: "deep"`) or to research a **follow-up angle** the
  open questions raised.

> The detailed manual research methodology this skill used to embed is archived at
> `docs/plans/research-mode/archive/deep-research-manual-methodology.md` — the harness
> now encodes that process in code, so the agent no longer drives it by hand.
