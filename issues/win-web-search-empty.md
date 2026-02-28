# Web search returns no results on Windows

**Platform:** Windows 11
**Severity:** high
**Component:** `internal/agent/tools/web_tool.go`
**Status:** Fixed

## Summary
Web search returns "No results found" for all queries. The HTTP scraper used outdated CSS selectors and a detectable user-agent.

## Fix Applied
- Updated user-agent to realistic Chrome string with Accept/Accept-Language headers
- Rewrote `parseWebDuckDuckGoHTML()` with multiple selector patterns for resilience against DDG HTML changes
- Added generic `<a href>` fallback when no class selectors match
- Verified passing with manual test
