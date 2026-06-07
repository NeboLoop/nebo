# Nebo Install Experience — Test Report

**Build:** local dev (`localhost:5173`) · **Driver:** Playwright/Chromium, real GUI flow (no API/DB shortcuts) · **Persona:** non-technical pool-business owner, first time · **Date:** 2026-06-06

> **Environment notes:** signed-in, populated dev machine (like iOS — you're already on your device), with 7 pre-existing agents + ~25 threads. The backend was being actively rebuilt mid-test (it shipped a fix during the run), so some failures below are dev-server churn, not the product — flagged where so.
>
> **Tester correction:** the run initially reported the install as "silent, no modal." That was wrong — a capture-timing miss. There **is** an install modal; confirmed with a DOM MutationObserver + screenshots. Corrected throughout.

---

## Headline
The core flow is **genuinely smooth, and for a non-technical ICP it's easy** — browse → click Install → it's there → use it, with **zero API keys** and no setup. That's the hard part, and it's delivered. The issues found are either already fixed (realtime sidebar), a known regression (configure), or one real silent-failure to harden.

---

## Step-by-step (what / what happened / screenshot)
1. **`/assistant/threads`** *(step1-threads.png)* — Threads + left "Agents" rail + "New thread… clean context, fresh start" composer. Reads as a chat app; "what's an Agent" isn't explained, but a returning user is oriented.
2. **`/marketplace`** *(step2-marketplace.png)* — Catalog: left rail (**Install code** `NEBO-XXXX-XXXX`, Category w/ counts, Pricing), top tabs (All · Agents · Apps · Skills · Plugins · Connectors · Collections · Shared · Installed), Featured + "Top in Marketplace." Jobs-to-be-done categories ("Run your business," "Manage money") are excellent and ICP-friendly.
3. **Agent — "Research Report"** *(step3-agent-detail.png)* — `🤖 Agent · Research & decide` / *"Research a topic and produce a structured summary"* / 9 installs / Free / code `AGNT-SW4Z-5XKN` / **Install** + *"Paste into Nebo's chat to install on any companion."*
4. **Install (agent)** *(step4-installed.png)* — Button → **"Installed."** Server registered it (appears in `/marketplace/installed`).
5. **Verify in sidebar** *(step5-agents-list.png)* — Initially **did not appear** in the Agents rail. **Root cause:** WS `tool_quarantined` — *"signature verification failed: read signatures.json: No such file or directory"* — agent **quarantined on install but UI still showed "Installed."** Separately, the rail wasn't realtime. **After rebuild, the rail now shows it** → realtime path fixed.
6. **Skill — "PDF"** install — near-instant; modal flashes <1s (no deps).
7. **Plugin — "X (Twitter)"** install *(install-modal.png)* — modal captured verbatim:
   > **Installing Plugin** ✕ · *(spinner)* **Installing plugin…** · `PLUG-EQ3H-BJQ4` · **Cancel** · ─── · **DEPENDENCIES** · *(spinner)* **X (Twitter)** — plugin

   No credential prompt — by design: **X drives your already-logged-in browser** (zero-config connector).
8. **Plugin — "Denticon (Planet DDS)"** *(dds-install-modal2.png)* — first attempt 502'd (backend mid-reboot); after reboot it installed:
   > **Plugin Installed** ✓ · **Denticon (Planet DDS) installed!** · **DEPENDENCIES (1/1)** ✓ Denticon (Planet DDS) — plugin

   (Denticon's own config errors because its backend isn't wired — separate from install.)
9. **Configure — Chief of Staff → Settings → Configure** *(cos-configure.png)* — agent.json-driven fields: YOUR NAME, AUTO-REPLY SENDER TYPES, LABEL RULES (`pattern -> label`), MORNING/EVENING BRIEFING TIME (24h), BUSINESS TYPE (Retail/Services/SaaS/E-commerce/Professional Services/Other). **Known regression — fix slated for next release.**

---

## Install modal — full lifecycle (verbatim)
Every install opens a modal; it flashes <1s for a dep-less skill/agent, holds open for plugins with dependencies:
- **Progress:** `Installing Plugin` / spinner / `Installing plugin…` / `<INSTALL-CODE>` / **Cancel** / `DEPENDENCIES` (per-dep spinner)
- **Success:** `Plugin Installed` / ✓ / `<Name> installed!` / `DEPENDENCIES (n/n)` ✓ per dep

---

## Promise check — *"ready in minutes, no complex setup, no tech skills needed"*
**Holds.** 1–2 clicks, instant, **0 API keys**, dependency cascade auto-resolves with a visible checklist, browser-driven connectors need no config. The iOS-simple promise is actually delivered — and it's the wall every competitor fails at (key-paste, dev-console setup). **Cracks at:** silent failure (the quarantined agent still read "Installed"), and the configure screen (known regression).

## End state — is it obvious how to use it?
After install the agent appears in the rail (now that realtime is fixed) and you talk to it in a thread. Remaining gap: a **post-install signpost** — install happens on the marketplace, configure lives under Agent → Settings → Configure, with nothing connecting the two.

## Time + clicks
**Marketplace → installed: 2 clicks (open + Install), ~instant.** Speed is not a problem anywhere.

---

## Friction log — ranked
1. **Silent failure dressed as success (highest priority).** Quarantined agent (`signatures.json` sig-fail) still showed **"Installed."** When the happy path is this frictionless, a *silent* failure makes a trusting non-technical user think it worked when it didn't. **Make failures as loud as successes.** (Likely dev-env root cause; the UX behavior is product-level.)
2. **Configure discoverability + dev-language** *(known regression, next release)* — install and configure are different places with no signpost; "Configuration from agent.json" and `pattern -> label` leak developer language; 24h time fields; BUSINESS TYPE has no home/field-services option.
3. **Thin listings + name collision** — one vague line per item, no "what it does for *you*"/example; "install on any **companion**" is undefined jargon; near-identical **"Researcher"** vs **"Research Report"**.

## Bugs / errors (specifics)
- **🔴 Silent quarantine on install** — WS `tool_quarantined` / `tool_error`: *"signature verification failed: signing error: read signatures.json: No such file or directory (os error 2)"*; UI showed "Installed," no error surfaced. **Make the quarantine state visible.**
- **🟢 Realtime sidebar — FIXED during the session.** Installed agent now appears in the Agents rail without reload.
- **🟡 Configure screen — known regression**, fix slated for next release.
- **⚪️ Backend instability during test (dev-only)** — repeated `502` on `/health`, `/api/v1/setup/status`, `/api/v1/.../install` and `ws://…/ws` handshake drops, from active rebuilds shipping fixes. Not a product defect.
- **⚪️ (live site, separate)** `neboai.com` threw `Failed to fetch dynamically imported module … nodes/146…js (404)` — looks like a stale-deploy chunk mismatch on production; unrelated to local build.

---

## Verdict
**The easy button works.** For the non-technical ICP, "found it → installed it → it's working," with no keys and no setup, is category-leading onboarding and a real moat. The install gesture, the cascade, and the zero-key connectors all deliver. Highest-leverage fix: **honesty on the unhappy path** — make a quarantine/sig-fail as visible as the green "Installed!" — and the experience is trustworthy end-to-end.

## Artifacts
Screenshots (Playwright output dir): `step1-threads.png`, `step2-marketplace.png`, `step3-agent-detail.png`, `step4-installed.png`, `step5-agents-list.png`, `install-modal.png`, `dds-install-modal2.png`, `cos-configure.png`.
