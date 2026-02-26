# Native browser navigation times out on Windows

**Platform:** Windows 11
**Severity:** medium
**Component:** `internal/browser/snapshot.go`, `cmd/nebo/desktop.go`
**Status:** Fixed

## Summary
Taking a snapshot of a page in the native Wails webview times out on Windows. Two separate causes: Playwright snapshot blocking, and Wails bootstrap JS not injecting.

## Fix Applied

### Fix 1: Playwright snapshot timeout (`snapshot.go`)
- Wrapped `WaitForLoadState()` in goroutine with context deadline
- Changed from `load` to `domcontentloaded` state (avoids blocking on persistent network activity)
- Set 5s Playwright-level timeout as safety net

### Fix 2: Wails WebView2 bootstrap JS (`desktop.go`)
- Root cause: Wails v3 Windows bug — `WebviewWindowOptions.JS` only applied for HTML-mode windows, not URL-mode. macOS/Linux inject via NavigationCompleted event, but Windows only registers JS inside `chromium.Init()` which runs inside `if HTML != ""`.
- Without bootstrap JS, `neboBootstrapJS` never runs, `"wails:runtime:ready"` never fires, `runtimeLoaded` stays false, all `ExecJS()` calls queue forever in `pendingJS`.
- Workaround: Create browser windows with `HTML: " "` to trigger `chromium.Init()` which registers JS via `AddScriptToExecuteOnDocumentCreated` (persists across all navigations), then call `SetURL()` to navigate.
