# TTS fails on Windows with exit status 1

**Platform:** Windows 11
**Severity:** medium
**Component:** `internal/agent/tools/tts.go`
**Status:** Fixed

## Summary
Text-to-speech fails with "exit status 1" on Windows due to missing `-NoProfile` flag and unsafe string escaping in the PowerShell command.

## Fix Applied
- Added single-quote escaping via `strings.ReplaceAll(params.Text, "'", "''")`
- Added `-NoProfile` flag to the PowerShell command
- Verified passing with manual test
