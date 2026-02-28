# Browser detection fails to find Edge on Windows

**Platform:** Windows 11
**Severity:** high
**Component:** `internal/browser/chrome.go`
**Status:** Fixed

## Summary
Browser launch with Nebo profile fails with "no supported browser found (Chrome/Brave/Edge/Chromium)" even though Edge is installed on every Windows machine.

## Fix Applied
- Added x86 Edge path: `Program Files (x86)\Microsoft\Edge\Application\msedge.exe`
- Added `findBrowserViaRegistry()` fallback querying `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\msedge.exe` and `chrome.exe`
- Verified passing with manual test
