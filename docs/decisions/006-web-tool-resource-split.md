# ADR 006: Web Tool Resource Split + Browser Lifecycle

**Date:** 2026-02-09  
**Status:** Accepted  
**Deciders:** Alma Tuck, Nebo

## Context

The `web` domain tool is the only STRAP tool that doesn't use the `resource` field. It crams three fundamentally different concerns into a flat list of 18 actions:

1. **HTTP client** (`fetch`) — stateless, no browser, no JS rendering
2. **Web search** (`search`) — stateless query against DuckDuckGo/Google
3. **Browser automation** (`navigate`, `snapshot`, `click`, `fill`, `type`, `screenshot`, `text`, `evaluate`, `wait`, `scroll`, `hover`, `select`, `back`, `forward`, `reload`) — stateful, sequential, profile-aware

This causes real problems:
- The agent frequently fails browser operations because it doesn't understand the stateful workflow (navigate → snapshot → click)
- No way to check if a browser is available before attempting operations → silent failures
- No way to launch, stop, or manage the browser lifecycle from tool calls
- Inconsistent with `shell` (which has `bash`, `process`, `session` resources) and `agent` (which has `task`, `cron`, `memory`, `message`, `session`, `comm`)
- 18 actions in a flat enum is cognitively expensive for the LLM

Additionally, the agent has **no browser lifecycle control**. It can't:
- Check if a browser/profile is available before trying
- Launch the managed browser on demand
- Stop the browser when done (to save resources)
- List available profiles and their status

## Decision

### 1. Split `web` into three resources

| Resource | Actions | Purpose |
|----------|---------|---------|
| `http` | `fetch` | Raw HTTP requests (GET/POST/PUT/DELETE/PATCH/HEAD/OPTIONS) |
| `search` | `query` | Web search via DuckDuckGo or Google |
| `browser` | `navigate`, `snapshot`, `click`, `fill`, `type`, `screenshot`, `text`, `evaluate`, `wait`, `scroll`, `hover`, `select`, `back`, `forward`, `reload`, `status`, `launch`, `close`, `list_pages` | Full browser automation with lifecycle control |

### 2. Add browser lifecycle actions

| Action | Description | When to use |
|--------|-------------|-------------|
| `status` | Returns profile status: running, connected, page count, extension connected (for chrome profile) | Before any browser operation — agent should check first |
| `launch` | Starts the managed (nebo) browser for a profile. No-op if already running. Returns status. | When `status` shows browser not running |
| `close` | Stops the browser for a profile. Closes sessions and pages. | When done with browser tasks to free resources |
| `list_pages` | Lists all open pages/tabs with their target IDs, URLs, and titles | When managing multiple tabs |

### 3. Updated input schema

```go
type WebDomainInput struct {
    // STRAP fields
    Resource string `json:"resource,omitempty"` // http, search, browser
    Action   string `json:"action"`

    // HTTP fields
    URL     string            `json:"url,omitempty"`
    Method  string            `json:"method,omitempty"`
    Headers map[string]string `json:"headers,omitempty"`
    Body    string            `json:"body,omitempty"`

    // Search fields
    Query  string `json:"query,omitempty"`
    Engine string `json:"engine,omitempty"`
    Limit  int    `json:"limit,omitempty"`

    // Browser fields
    Profile  string `json:"profile,omitempty"`
    Ref      string `json:"ref,omitempty"`
    Selector string `json:"selector,omitempty"`
    Text     string `json:"text,omitempty"`
    Value    string `json:"value,omitempty"`
    Output   string `json:"output,omitempty"`
    Timeout  int    `json:"timeout,omitempty"`
    TargetID string `json:"target_id,omitempty"`
}
```

### 4. Updated call patterns

**Before:**
```
web(action: fetch, url: "https://api.example.com")
web(action: search, query: "golang tutorials")
web(action: navigate, url: "https://gmail.com", profile: "chrome")
web(action: snapshot)
web(action: click, ref: "e5")
```

**After:**
```
web(resource: http, action: fetch, url: "https://api.example.com")
web(resource: search, action: query, query: "golang tutorials")
web(resource: browser, action: status, profile: "chrome")
web(resource: browser, action: launch, profile: "nebo")
web(resource: browser, action: navigate, url: "https://gmail.com", profile: "chrome")
web(resource: browser, action: snapshot)
web(resource: browser, action: click, ref: "e5")
web(resource: browser, action: close, profile: "nebo")
web(resource: browser, action: list_pages, profile: "chrome")
```

### 5. Status response format

```json
{
  "profiles": [
    {
      "name": "nebo",
      "driver": "nebo",
      "running": false,
      "page_count": 0,
      "cdp_url": "http://127.0.0.1:9222"
    },
    {
      "name": "chrome",
      "driver": "extension",
      "running": true,
      "extension_connected": true,
      "page_count": 2,
      "cdp_url": "ws://127.0.0.1:27895/relay/cdp"
    }
  ]
}
```

When `profile` is specified, returns status for just that profile. When omitted, returns all profiles.

### 6. Error behavior changes

- `navigate`/`click`/`fill`/etc. on a non-running profile → clear error: "Browser not running for profile 'nebo'. Use `web(resource: browser, action: launch, profile: \"nebo\")` to start it."
- `launch` on `chrome` profile → error: "Cannot launch the chrome profile — it connects via the Chrome extension. Ensure the Nebo extension is active in Chrome."
- `close` on `chrome` profile → closes the Playwright session (disconnects from relay), does NOT close Chrome itself
- `status` never errors — always returns current state

## Implementation Plan

### Files to modify:

1. **`internal/agent/tools/web_tool.go`** — Main changes:
   - Add `Resource` field to `WebDomainInput`
   - Update `Resources()` → `["http", "search", "browser"]`
   - Update `ActionsFor()` for each resource
   - Update `Execute()` to route by resource first, then action
   - Add `handleStatus()`, `handleLaunch()`, `handleClose()`, `handleListPages()`
   - Update `Schema()` to include resource field and per-resource action enums
   - Update `Description()` with new patterns

2. **`internal/agent/tools/domain.go`** — No changes needed (already supports resource routing)

3. **`internal/browser/manager.go`** — Already has the needed methods:
   - `GetAllProfileStatuses()` → used by `status`
   - `GetProfileStatus(name)` → used by `status` with profile
   - `IsBrowserRunning(name)` → used by `status`
   - `ensureBrowserRunning()` → used by `launch`  
   - `StopBrowser(name)` → used by `close`
   - Need to expose `ListProfiles()` for status-all

4. **System prompt / CLAUDE.md** — Update web tool documentation with new resource pattern

5. **`internal/browser/relay.go`** — Add `ExtensionConnected()` to status response (already exists)

## Consequences

### Positive
- Consistent STRAP pattern across all domain tools
- Agent can check browser status before attempting operations (no more blind failures)
- Agent can manage browser lifecycle (launch/close on demand)
- Smaller action sets per resource → less LLM ambiguity
- Self-documenting: `resource: browser` makes it obvious this is stateful
- Resource-based tool policy becomes possible (e.g., allow `http` but deny `browser` for certain origins)

### Negative
- Slightly more verbose calls (one extra field)

### Neutral
- Total tool count stays the same (still one `web` tool)
- Browser package (`internal/browser/`) needs no structural changes — all lifecycle methods already exist

### Also fixed
- Duration bugs in `manager.go`: `IsChromeReachable(url, 1000)` was passing 1000 nanoseconds instead of 1 second. Fixed to use `time.Second` and `5*time.Second` properly.
- Added `GetSessionIfExists()` to browser session package for non-creating session lookups (used by `list_pages`).
