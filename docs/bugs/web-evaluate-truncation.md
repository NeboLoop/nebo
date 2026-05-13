# Bug Report: Web Tool Output Truncation Issue

**Date:** May 3, 2026  
**Severity:** High (blocks core functionality)  
**Confidence Grade:** High (reproducible pattern observed)

---

## Issue Summary
The `web.evaluate()` tool consistently truncates output from JavaScript execution on web pages, even when requesting small, specific data samples. This prevents extraction of meaningful content from dynamically loaded elements (e.g., YouTube transcripts, modals, dialogs).

---

## Reproduction Steps
1. Navigate to a page with dynamic content (e.g., YouTube video with transcript panel)
2. Trigger the dynamic element (click "Show Transcript")
3. Execute `web.evaluate()` with JavaScript that returns:
   - A single line of text
   - First 500 characters of content
   - Simple object with key statistics
4. **Result:** All outputs are truncated before reaching user

---

## Evidence Collected
- Confirmed transcript dialog exists in DOM (`[role="dialog"]` found via querySelector)
- JavaScript successfully accesses content (`dialog.innerText` returns non-empty string)
- Requested output sizes:
  - Single line → truncated
  - First 500 chars → truncated
  - Object with 4 fields → truncated
  - Line count + sample → truncated
- Pattern: **No amount of content reaches the user intact**

---

## Impact
- **Blocks OSINT investigations:** Cannot extract transcripts, comments, or dynamic content
- **Breaks workflow:** Requires manual browser interaction instead of automation
- **Reduces reliability:** User must verify findings manually
- **Wastes time:** Multiple failed attempts per investigation

---

## Technical Details
- **Tool:** `web.evaluate()` 
- **Environment:** Brave Browser (Chrome-based)
- **Expected behavior:** Return exact JavaScript return value
- **Actual behavior:** Output silently truncated mid-string
- **Suspected cause:** 
  - Internal buffer limit not documented
  - Character encoding issue with special characters
  - Dialog content being streamed rather than static

---

## Workarounds Attempted
1. ✅ Reduce output size (single line, 500 chars) → still truncated
2. ✅ Use structured objects → still truncated
3. ✅ Try different selectors → same result
4. ❌ No successful workaround identified

---

## Recommendations
1. **Immediate:** Document maximum output size for `web.evaluate()`
2. **Short-term:** Add pagination support for large content extraction
3. **Long-term:** Implement streaming response for very large outputs
4. **Alternative:** Create dedicated tool for transcript extraction (YouTube-specific)

---

## Next Steps Needed
- [ ] Confirm if this is a known limitation
- [ ] Determine actual output threshold
- [ ] Test with other dynamic content types (comments, reviews, etc.)
- [ ] Develop fallback strategy for blocked content

---

**Reporter:** Private Investigator AI Agent  
**Status:** Open — requires tool fix or documented workaround