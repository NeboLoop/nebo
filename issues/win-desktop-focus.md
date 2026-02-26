# Desktop focus fails to find windows by display name

**Platform:** Windows 11
**Severity:** medium
**Component:** `internal/agent/tools/window_windows.go`
**Status:** Fixed

## Summary
`desktop(resource: "window", action: "focus", app: "Microsoft Edge")` fails because Windows implementation matches against process executable names, not display names. "Microsoft Edge" runs as process `msedge`.

## Fix Applied
- Replaced `Get-Process -Name` with `findProcessPS()` helper using 4-tier matching:
  1. Exact process name (`msedge`)
  2. Partial process name (`*edge*` matches `msedge`)
  3. Window title match (`*Microsoft Edge*`)
  4. FileDescription match from exe version info ("Microsoft Edge" in msedge.exe metadata)
- Updated all actions (focus, move, resize, minimize, maximize, close) to use the helper
- `title` parameter is now properly wired through as an additional filter
