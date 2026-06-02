# Bug: web(search) stalls up to 30s before falling back to browser

**Date:** June 2, 2026
**Severity:** High (UX) — agent appears hung
**Component:** `crates/tools/src/web_tool.rs` (web tool / search)
**Status:** Logged, not yet fixed (deferred — focus is on prompt/tools)

**Description:**
A simple factual query ("what is the temperature in Sundance, UT right now?") took a long
time to return. The model correctly went straight to `web` (tool-visibility fix working),
but `web(search)` stalled for a long stretch before producing an answer, then "navigated
and returned a result rapidly." User-visible symptom: the chat shows "web running… /
searching the web" and gives no answer for many seconds.

**Root cause (traced):**
`handle_search` (web_tool.rs:305) tries providers in order:
1. BYOK search APIs (skipped — none configured)
2. `search_duckduckgo_html` (web_tool.rs:371) — DDG HTTP scraping, using the shared
   `reqwest::Client` whose timeout is **30s** (web_tool.rs:43-44)
3. `search_via_browser` (web_tool.rs:379) — browser navigate (fast once it runs)

When DuckDuckGo is slow or rate-limits the scrape, step 2 can block for up to **30s**
before it errors and step 3 (the fast browser path) runs. That 30s wait is the stall.

**Steps to Reproduce:**
1. No BYOK search provider configured.
2. Ask a fresh chat a factual question that requires search.
3. Observe a multi-second "searching the web" stall before the answer.

**Expected:** Search returns in a few seconds; if DDG scraping is slow, fall back to the
browser path quickly (≤ ~8s), not after 30s.

**Actual:** Up to ~30s stall on the DDG HTTP scrape before fallback.

**Proposed fix (for later):**
Wrap step 2 (`search_duckduckgo_html`) in a short `tokio::time::timeout` (~8s) so a slow/
blocking DDG scrape fails fast and falls through to the browser search. Optionally give
search its own `reqwest::Client` with a shorter timeout than the 30s fetch client.

**Notes:** Not caused by the prompt/tool-visibility changes — those are working (the model
reached for `web` immediately). This is a pre-existing web-tool reliability issue, now more
visible because the agent uses web confidently. Related: known "navigate 1s wait too short
for SPAs" issue in BROWSER_AUTOMATION SME.
