# Browser Automation Timeout Issue

**Date:** March 4, 2026  
**Component:** web browser control  
**Severity:** High

## Description

The Nebo Chrome browser extension fails to establish a connection when attempting to navigate to URLs. Browser navigation commands timeout after 30 seconds.

## Symptoms

- `web(action: "navigate", url: "...`) times out after 30s
- Extension cannot access content of URLs
- Error: "Failed to read page: Cannot access contents of url"
- HTTP fetch works fine, only browser automation fails

## Environment

- OS: macOS (aarch64)
- Extension: Nebo Chrome/Brave extension (bmkkjdcmjiebhegfibdnbimjpkmaickm)
- Target: https://news.ycombinator.com

## Investigation

Multiple attempts to navigate to URLs resulted in timeouts. The extension appears to be installed but not properly connecting to the Nebo bridge.

## Expected Behavior

Browser navigation commands should complete within seconds, not timeout after 30s.

## Steps to Reproduce

1. Run `web(action: "navigate", url: "https://news.ycombinator.com")`
2. Wait for response
3. Observe 30s timeout

## Workaround

Use `web(action: "fetch")` for static content retrieval instead of browser automation.

## Notes

The Chrome extension manifest may need permission updates to access host URLs. User may need to reinstall or re-authorize the extension.

**API Key:** APP-KHF6-3LSD-N6PY