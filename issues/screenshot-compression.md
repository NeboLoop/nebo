# Agent cannot view its own screenshots — data too large for inline rendering

## Current behavior
`os(resource: "capture", action: "screenshot")` returns raw PNG data that's too large to display inline in the agent's conversation context. The agent is effectively blind when taking screenshots.

## Desired behavior
Screenshots should be compressed to JPEG (quality ~60-70%) and resized to a reasonable max dimension (e.g., 1280px wide) before being returned, so the agent can actually see them inline and use them for visual feedback loops.

## Proposed changes

### 1. Compress in the capture pipeline
In the screenshot capture pipeline (likely in `crates/tools/` or wherever `os.capture.screenshot` is handled):
- After capturing the raw screenshot, convert PNG → JPEG at ~60-70% quality
- Resize to max 1280px width (maintain aspect ratio)
- This should dramatically reduce the payload size (typically 10-20x smaller)

### 2. Add optional `quality` parameter
- `os(resource: "capture", action: "screenshot", quality: "low")` → 800px wide, 50% JPEG
- `os(resource: "capture", action: "screenshot", quality: "high")` → full res PNG (current behavior)
- Default should be the compressed version since that's what the agent needs 99% of the time

### 3. File variant
The `format: "file"` variant should also save as compressed JPEG by default.

## Why this matters
The agent currently cannot do any visual design work, GUI verification, or screenshot-based debugging because it literally cannot see what's on screen. This is a significant capability gap — the agent has been "designing blind" when trying to help with visual/UI tasks.
