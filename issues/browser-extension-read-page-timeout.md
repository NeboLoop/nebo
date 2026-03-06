# Bug: Browser Extension read_page Operations Timeout

## Summary
The Nebo browser extension's `read_page` command consistently times out after 30 seconds, even though the extension reports as connected and navigation works correctly.

## Symptoms
- `web(action: "status")` returns: "Browser extension connected: true, Ready. Use read_page to see the current page."
- `web(action: "navigate", url: "...")` succeeds
- `web(action: "read_page")` times out after 30 seconds with error: "Tool 'read_page' timed out after 30s (connected: true, pending: 0)"
- This happens on every call, regardless of page content

## Impact
Cannot use the browser automation features that require reading page content:
- Cannot click buttons or links (no element refs available)
- Cannot fill forms
- Cannot interact with any web pages
- Cannot execute the browser-based workflows the user requests

## Work Attempted
- Verified extension is connected via `web(action: "status")`
- Navigation works - can successfully navigate to any URL
- read_page fails consistently across multiple pages
- Issue persists after extension update per user report

## Expected Behavior
- `read_page` should return the page accessibility tree with element references
- Should complete within a few seconds, not timeout after 30s
- Element refs should be usable for subsequent click/fill actions

## Steps to Reproduce
1. Call `web(action: "status")` - confirms connected
2. Call `web(action: "navigate", url: "https://www.google.com")` - succeeds
3. Call `web(action: "read_page")` - times out after 30s

## Environment
- Date: March 5, 2026
- OS: macOS (aarch64)
- Browser: Brave (user's default)
- Extension: Updated version per user

## Priority
HIGH - This blocks all browser automation functionality that the user explicitly requests.
