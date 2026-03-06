# Desktop and Webview: Deep-Dive Reference

This document provides a comprehensive, implementation-level reference for the Go desktop
mode, native webview management, and single-instance lock system. It contains enough detail
to reimplement these systems in Rust.

Source files covered:

- `nebo/internal/webview/` (manager.go, actions.go, callback.go, cursor.go, fingerprint.go, js.go)
- `nebo/cmd/nebo/desktop.go`
- `nebo/cmd/nebo/desktop_stub.go`
- `nebo/cmd/nebo/webview_navigation_darwin.go`
- `nebo/cmd/nebo/webview_navigation_linux.go`
- `nebo/cmd/nebo/webview_navigation_other.go`
- `nebo/cmd/nebo/webview_permissions_darwin.go`
- `nebo/cmd/nebo/webview_permissions_other.go`
- `nebo/cmd/nebo/lock_unix.go`
- `nebo/cmd/nebo/lock_windows.go`

---

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [Single-Instance Lock System](#single-instance-lock-system)
3. [Desktop Command (desktop.go)](#desktop-command)
4. [Webview Manager](#webview-manager)
5. [Actions Dispatching](#actions-dispatching)
6. [Callback System](#callback-system)
7. [JavaScript Injection](#javascript-injection)
8. [Cursor Simulation](#cursor-simulation)
9. [Fingerprint Generation](#fingerprint-generation)
10. [Platform-Specific Navigation Handling](#platform-specific-navigation-handling)
11. [Platform-Specific Media Permissions](#platform-specific-media-permissions)
12. [Desktop Stub (non-desktop builds)](#desktop-stub)
13. [Wails Window Handle Adapter](#wails-window-handle-adapter)

---

## Architecture Overview

The desktop system is structured as follows:

```
desktop.go (RunDesktop)
  |
  +-- acquireLock()             -- Single instance enforcement
  +-- svc.NewServiceContext()   -- Shared DB, auth, config
  +-- application.New()         -- Wails v3 app with native event loop
  +-- InjectWebViewMediaPermissions()   -- macOS media auto-grant
  +-- Window creation           -- Main window with saved position/size
  +-- System tray               -- Show/Hide/Update/Quit menu
  +-- Background goroutines:
  |     +-- HTTP server         -- server.Run()
  |     +-- Agent               -- runAgent() with reconnect
  |     +-- Heartbeat daemon    -- Proactive heartbeat ticks
  |     +-- Background updater  -- Auto-update checker
  +-- wailsApp.Run()            -- Main thread event loop (blocks)
```

The webview subsystem (`internal/webview/`) is an independent package that manages
agent-controlled browser windows. It is used by the `web` domain tool when the agent
needs to interact with external websites:

```
Agent tool call: web(action: navigate, url: "...")
  |
  +-- webview.Manager.CreateWindow()
  |     +-- Calls creator callback (set by desktop.go)
  |     +-- GenerateFingerprint() + InjectJS()
  |
  +-- webview.Navigate() / Click() / Snapshot() / etc.
  |     +-- Generates JS code with unique request ID
  |     +-- Registers callback channel
  |     +-- Calls WindowHandle.ExecJS(js)
  |     +-- Waits for callback delivery (channel or timeout)
  |
  +-- JS in webview sends result back via:
        +-- window.__nebo_cb() (native bridge, preferred)
        +-- HTTP POST to /internal/webview/callback (fallback)
        +-- Wails RawMessageHandler receives "nebo:cb:{json}"
```

---

## Single-Instance Lock System

### Purpose

Ensures only one Nebo process runs per computer. Uses file locking (not named mutexes)
so it works across terminals, Finder launches, and login items.

### Lock File Location

`{dataDir}/nebo.lock` where `dataDir` is the platform-standard data directory:
- macOS: `~/Library/Application Support/Nebo/nebo.lock`
- Linux: `~/.config/nebo/nebo.lock`
- Windows: `%AppData%\Nebo\nebo.lock`

### Lock File Format

Plain text file containing the PID of the owning process followed by a newline:
```
12345
```

### API (same signature on all platforms)

```
func acquireLock(dataDir string) (*os.File, error)
func releaseLock(file *os.File)
```

### Unix Implementation (lock_unix.go)

Build tags: `darwin || linux`

**acquireLock(dataDir string) (*os.File, error):**

1. Compute `lockPath = dataDir + "/nebo.lock"`.
2. Call `tryLock(lockPath)`. If succeeds, return the file handle.
3. If lock fails, read the PID from the lock file via `readLockPID(lockPath)`.
4. If PID > 0 and the process is dead (checked via `isProcessAlive`), remove the stale
   lock file, sleep 100ms to let the OS release the file handle, then retry `tryLock`.
5. If PID > 0 and the process is alive, return error: `"cannot acquire lock (held by PID %d)"`.
6. If no PID readable, return generic error: `"cannot acquire lock"`.

**tryLock(lockPath string) (*os.File, error):**

1. `os.OpenFile(lockPath, O_CREATE|O_RDWR, 0600)` -- creates or opens the file.
2. `syscall.Flock(file.Fd(), LOCK_EX|LOCK_NB)` -- exclusive, non-blocking lock.
3. If lock succeeds: truncate file to 0, seek to beginning, write `"{PID}\n"`, sync.
4. Return the open file handle (must be kept open for the duration of the process).

**readLockPID(lockPath string) int:**

Reads the entire file, trims whitespace, parses as integer. Returns 0 on any error.

**isProcessAlive(pid int) bool:**

`syscall.Kill(pid, 0)` -- signal 0 checks existence without killing. Returns true if
no error (process exists and caller has permission to signal it).

**releaseLock(file *os.File):**

`syscall.Flock(file.Fd(), LOCK_UN)` then `file.Close()`. Safe to call with nil.

### Windows Implementation (lock_windows.go)

Build tags: `windows`

**acquireLock:** Identical logic to Unix (stale detection, retry).

**tryLock(lockPath string) (*os.File, error):**

1. `os.OpenFile(lockPath, O_CREATE|O_RDWR, 0600)`.
2. `windows.LockFileEx(handle, LOCKFILE_EXCLUSIVE_LOCK|LOCKFILE_FAIL_IMMEDIATELY, 0, 1, 0, &overlapped)`.
   - Locks 1 byte starting at offset 0.
   - `LOCKFILE_FAIL_IMMEDIATELY` makes it non-blocking.
3. Write PID and sync, same as Unix.

**isProcessAlive(pid int) bool:**

1. `windows.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, uint32(pid))`.
2. `windows.GetExitCodeProcess(handle, &exitCode)`.
3. Returns true if `exitCode == 259` (`STILL_ACTIVE`).

**releaseLock(file *os.File):**

`windows.UnlockFileEx(handle, 0, 1, 0, &overlapped)` then `file.Close()`.

### Integration with Updater

Before the updater applies a binary restart, the lock must be released so the new
process can acquire it:

```go
updater.SetPreApplyHook(func() { releaseLock(lockFile) })
```

### What Happens When Another Instance Tries to Start

1. `acquireLock` is called.
2. `tryLock` fails because the file is already locked by the running process.
3. The PID is read and verified alive.
4. Error is returned, and the process prints:
   ```
   Error: cannot acquire lock (held by PID 12345)
   Nebo is already running. Only one instance allowed per computer.
   ```
5. `os.Exit(1)`.

---

## Desktop Command

### File: `nebo/cmd/nebo/desktop.go`

Build tag: `desktop` (CGO required for Wails, WebKit, and native APIs).

### Embedded Assets

Two icons are embedded at compile time:

```go
//go:embed icons/appicon.png
var appIcon []byte

//go:embed icons/tray-icon.png
var trayIcon []byte
```

### Window State Persistence

**Struct:**

```go
type windowState struct {
    X      int `json:"x"`
    Y      int `json:"y"`
    Width  int `json:"width"`
    Height int `json:"height"`
}
```

**File location:** `{dataDir}/window-state.json`

**loadWindowState(dataDir string) *windowState:**

- Reads JSON from disk.
- Returns nil if file does not exist, cannot be read, cannot be parsed, or if
  `Width < 400` or `Height < 300` (sanity check for nonsensical sizes).

**saveWindowState(dataDir string, window *WebviewWindow):**

- Reads current `Size()` and `Position()` from the Wails window.
- Does NOT save if `width < 400` or `height < 300` (minimized/invisible state).
- Writes JSON to disk with mode 0644.

### RunDesktop() -- Full Flow

1. **PATH augmentation:** `ensureUserPath()` adds common directories (/opt/homebrew/bin,
   ~/.local/bin, ~/go/bin, ~/.cargo/bin, etc.) to PATH so CLI tools launched from
   Finder/Dock are discoverable.

2. **Quiet mode:** Sets `migrations.QuietMode = true` and `neboapp.QuietMode = true`
   to suppress startup output.

3. **Data directory:** `defaults.EnsureDataDir()` creates the data directory with
   default files if it does not exist. Fatal on failure.

4. **Logging:** `logging.Init(WithFile(...))` initializes unified tint console + file
   logging to `{dataDir}/logs/agent.log`.

5. **Lock acquisition:** `acquireLock(dataDir)`. Fatal on failure. Deferred release.

6. **Updater hook:** `updater.SetPreApplyHook(func() { releaseLock(lockFile) })`.

7. **ServiceContext:** `svc.NewServiceContext(*ServerConfig)` -- single owner of the
   database connection. Version is set to `AppVersion`. UpdateManager is installed.
   Deferred `svcCtx.Close()`.

8. **Server URL computation:**
   ```
   serverURL = config.App.BaseURL || "http://{domain}:{port}"
   healthURL = "http://localhost:{port}"
   ```

9. **Wails application creation:**

   ```go
   application.New(application.Options{
       Name: "Nebo",
       Icon: appIcon,
       Mac: MacOptions{
           ApplicationShouldTerminateAfterLastWindowClosed: false,
       },
       Windows: WindowsOptions{DisableQuitOnLastWindowClosed: true},
       Linux: LinuxOptions{
           DisableQuitOnLastWindowClosed: true,
           ProgramName: "nebo",
       },
       RawMessageHandler: <see below>,
       OnShutdown: <prints "Nebo stopped.">,
   })
   ```

   **RawMessageHandler:** This is a critical integration point. Every message sent via
   `window._wails.invoke()` from any webview window is delivered here. The handler:
   - Checks for the `"nebo:cb:"` prefix.
   - Strips the prefix, parses the remaining string as `CallbackResult` JSON.
   - Validates that `RequestID` is non-empty.
   - Calls `webview.GetCollector().Deliver(result)` to route the result to the
     waiting Go function.

10. **Media permissions injection:** `InjectWebViewMediaPermissions()` -- macOS-specific
    ObjC runtime manipulation (see [Media Permissions](#platform-specific-media-permissions)).

11. **Main window creation:**

    Default size: 1280x860. Minimum: 800x600.
    If saved state exists, width/height come from the saved state.

    ```go
    WebviewWindowOptions{
        Name:      "main",
        Title:     "Nebo",
        Width:     winWidth,
        Height:    winHeight,
        MinWidth:  800,
        MinHeight: 600,
        URL:       serverURL,
        Mac: MacWindow{Backdrop: MacBackdropTranslucent},
        Windows: WindowsWindow{HiddenOnTaskbar: false},
    }
    ```

12. **Position restore:** Deferred to a goroutine with a short delay:
    - macOS/Linux: 200ms
    - Windows (WebView2): 500ms

    A `stateRestored` atomic bool gates auto-save until after restore completes.

13. **Event hooks for auto-save:**

    - `Common.WindowDidMove` and `Common.WindowDidResize` -- save window state.
    - On Windows, additionally hooks `Windows.WindowDidMove` and `Windows.WindowDidResize`
      for reliability (Wails v3 alpha event mapping can be unreliable).
    - Only saves if `stateRestored == true && quitting == false`.

14. **Close-to-tray behavior:**

    The `WindowClosing` hook cancels the close event and hides the window instead:
    ```go
    closeHandler := func(event *WindowEvent) {
        if quitting.Load() { return }  // let close proceed during quit
        saveWindowState(dataDir, window)
        window.Hide()
        event.Cancel()
    }
    ```
    Same platform-specific dual-hook pattern as above.

15. **System tray:**

    ```
    systray = wailsApp.SystemTray.New()
    systray.SetIcon(trayIcon)
    systray.SetLabel("")  -- empty label (icon only)
    ```

    **Tray menu items:**

    | Item | Behavior |
    |------|----------|
    | Show | `window.Show()` + `window.Focus()` |
    | Hide | Save state + `window.Hide()` |
    | --- | Separator |
    | Status: Starting... | Disabled label, updated by lifecycle events |
    | Check for Updates | Triggers manual update check (see below) |
    | --- | Separator |
    | Quit Nebo | Save state, set `quitting=true`, `safeQuit(app)` |

    **Update check logic (onClick):**

    1. Detect install method (`updater.DetectInstallMethod()`).
    2. If homebrew: show "Managed by Homebrew" for 3s, return.
    3. If package_manager: show "Use apt upgrade" for 3s, return.
    4. If a pending update is already downloaded (check `um.PendingPath()`): apply immediately.
    5. Otherwise: check for update, download with progress, verify checksum, store pending,
       update label to "Restart to Update (vX.Y.Z)" with a click handler that applies.

16. **Background services (spawned in a goroutine):**

    All background services are started in a single goroutine so `wailsApp.Run()` can start
    the macOS event loop immediately on the main thread.

    **Service registration on svcCtx:**
    - `SetBrowseDirectory`: Native directory picker via `wailsApp.Dialog.OpenFile()`.
    - `SetBrowseFiles`: Native multi-file picker.
    - `SetOpenDevWindow`: Creates or focuses a "dev" window (1400x900, URL: `/dev`).
    - `SetOpenPopup`: Creates a popup window for OAuth flows (unique name with timestamp).

    **Webview Manager setup:**
    - `wvm.SetCreator(...)` -- installs the window creation callback.
    - `wvm.SetCallbackURL("http://localhost:{port}/internal/webview/callback")`.
    - Creator callback builds a Wails `WebviewWindow` with `neboBootstrapJS` injected.
    - On Windows, uses a workaround: creates with `HTML: " "` to trigger `chromium.Init()`
      which registers JS via `AddScriptToExecuteOnDocumentCreated`, then navigates to URL.
    - On macOS/Linux, JS is injected via `WebviewWindowOptions.JS` which runs after
      every navigation at the impl level.

    **HTTP server:** `server.Run(ctx, config, opts)` in a goroutine. WaitGroup tracked.

    **Server readiness:** `waitForServer(healthURL, 10s)` polls `GET /api/v1/csrf-token`
    every 100ms until 200 OK or timeout.

    **Agent:** `runAgent(ctx, agentCfg, serverURL, agentOpts)` in a goroutine with
    exponential backoff reconnection (1s initial, 30s max, doubles each retry).

    **Heartbeat daemon:** Started once after first agent connection via `sync.Once`.
    Interval defaults to 30 minutes, configurable via settings. Sends a "run" frame
    to any connected agent. Respects quiet hours.

    **Lifecycle callbacks:**
    - `OnAgentConnected`: Updates status to "Connected", starts heartbeat on first connect.
    - `OnAgentDisconnected`: Updates status to "Disconnected".

17. **Error handling goroutine:**

    Reads from `errCh`. Fatal errors (containing "server error" or "server failed to start")
    cancel context and quit. Non-fatal errors log a warning and update status to
    "Reconnecting...".

18. **Event loop:** `wailsApp.Run()` blocks the main thread until `app.Quit()`.

19. **Shutdown:** Cancel context, wait for all goroutines via WaitGroup.

### safeQuit(app *application.App)

Wraps `app.Quit()` with panic recovery. Wails v3 alpha.67 can panic with nil pointer
dereference on `windowsSystemTray.destroy()`. Recovery calls `os.Exit(0)`.

### neboBootstrapJS Constant

This JavaScript is injected into every agent-controlled browser window via
`WebviewWindowOptions.JS`. It runs at the impl level after EVERY navigation, bypassing
the `runtimeLoaded` gate that blocks the public `ExecJS` method.

**What it does:**

1. **Defines `window.__nebo_cb(d)`:** The universal callback function that sends
   results back to Go via the native platform message handler. Uses the `"nebo:cb:"`
   prefix so `RawMessageHandler` can identify callback messages.

   Priority order:
   - `window._wails.invoke(m)` -- Wails bridge
   - `window.webkit.messageHandlers.external.postMessage(m)` -- macOS WebKit
   - `window.chrome.webview.postMessage(m)` -- Windows WebView2

2. **Forces `wails:runtime:ready`:** After a 200ms delay, sends the
   `"wails:runtime:ready"` message via the same native handlers. This forces
   `runtimeLoaded = true` in the Wails framework, allowing queued `ExecJS` calls
   to flush. Without this, external pages where the Wails JS runtime fails to
   initialize would leave ExecJS permanently queued.

**Full source:**

```javascript
(function(){
  window.__nebo_cb = function(d) {
    var m = "nebo:cb:" + JSON.stringify(d);
    try {
      if (window._wails && window._wails.invoke) {
        window._wails.invoke(m);
      } else if (window.webkit && window.webkit.messageHandlers &&
                 window.webkit.messageHandlers.external) {
        window.webkit.messageHandlers.external.postMessage(m);
      } else if (window.chrome && window.chrome.webview) {
        window.chrome.webview.postMessage(m);
      }
    } catch(e) {}
  };

  setTimeout(function() {
    try {
      if (window._wails && window._wails.invoke) {
        window._wails.invoke("wails:runtime:ready");
      } else if (window.webkit && window.webkit.messageHandlers &&
                 window.webkit.messageHandlers.external) {
        window.webkit.messageHandlers.external.postMessage("wails:runtime:ready");
      } else if (window.chrome && window.chrome.webview) {
        window.chrome.webview.postMessage("wails:runtime:ready");
      }
    } catch(e) {}
  }, 200);
})();
```

---

## Webview Manager

### File: `nebo/internal/webview/manager.go`

### WindowHandle Interface

The abstraction over native webview windows. In desktop mode, Wails WebviewWindow
implements this via the `wailsWindowHandle` adapter. In tests, `mockHandle` implements it.

```go
type WindowHandle interface {
    SetURL(url string)
    ExecJS(js string)
    SetTitle(title string)
    Show()
    Hide()
    Focus()
    Close()
    SetSize(width, height int)
    Reload()
    Name() string
}
```

### WindowCreatorOptions

Configuration for creating a new native browser window:

```go
type WindowCreatorOptions struct {
    Name   string
    Title  string
    URL    string
    Width  int
    Height int
}
```

### Window Struct

```go
type Window struct {
    ID          string
    Title       string
    URL         string
    Owner       string       // Session key for cleanup
    seq         int64        // Monotonic sequence for "most recent" queries
    CreatedAt   time.Time
    Handle      WindowHandle
    Fingerprint *Fingerprint
}
```

### Manager Struct

```go
type Manager struct {
    mu          sync.RWMutex
    creator     func(opts WindowCreatorOptions) WindowHandle
    windows     map[string]*Window      // ID -> Window
    owners      map[string]map[string]bool  // owner -> set of window IDs
    callbackURL string
}
```

### Singleton Access

Global singleton via `sync.Once`:

```go
var (
    managerOnce sync.Once
    mgr         *Manager
)

func GetManager() *Manager
```

### Global Window Counter

```go
var windowCounter atomic.Int64
```

Used for both unique ID generation and monotonic sequencing of windows.

### Methods

**SetCreator(fn func(WindowCreatorOptions) WindowHandle):**

Installs the window creation callback. Called from `desktop.go` during initialization.
When nil, `IsAvailable()` returns false and `CreateWindow()` returns an error.

**SetCallbackURL(url string):**

Sets the base URL for JS-to-Go result callbacks. Typically
`"http://localhost:27895/internal/webview/callback"`.

**CallbackURL() string:**

Returns the configured callback URL.

**IsAvailable() bool:**

Returns `true` if a creator callback is installed (i.e., running in desktop mode).

**CreateWindow(url, title, owner string) (*Window, error):**

1. If `creator == nil`, return error (headless mode).
2. Generate ID: `"win-{unixNano}-{counter}"`.
3. Default title: `"Nebo Browser"` if empty.
4. Call `creator(WindowCreatorOptions{Name: id, Title: title, URL: url, Width: 1200, Height: 800})`.
5. Generate fingerprint: `GenerateFingerprint()`.
6. Inject fingerprint JS: `handle.ExecJS(fp.InjectJS())`.
7. Create `Window` struct with all metadata.
8. Store in `windows` map.
9. If owner is non-empty, track in `owners` map.
10. Return the window.

**GetWindow(id string) (*Window, error):**

- If `id` is empty, returns the window with the highest `seq` value (most recently created).
- If `id` is non-empty, looks up by exact ID.
- Returns error if not found.

**GetWindowByOwner(owner string) (*Window, error):**

Returns the most recently created window for the given owner. Returns `nil, nil` if
no windows exist for this owner (not an error).

**ListWindows() []*Window:**

Returns all open windows as a slice.

**CloseWindow(id string) error:**

1. Look up window by ID.
2. Call `Handle.Close()`.
3. Remove from `windows` map.
4. Clean up `owners` map (remove from owner set, delete empty sets).

**CloseWindowsByOwner(owner string) int:**

Closes all windows belonging to a session. Returns count of windows closed.
Deletes the owner entry from the owners map.

**CloseAll():**

Closes all windows and clears the map.

**WindowCount() int:**

Returns `len(windows)`.

---

## Actions Dispatching

### File: `nebo/internal/webview/actions.go`

This file contains the high-level action functions that the agent's web tool calls.
Each action follows a common pattern:

1. Get the window by ID.
2. Generate JavaScript with a unique request ID and callback URL.
3. Register the request ID with the callback collector.
4. Execute the JS via `WindowHandle.ExecJS()`.
5. Wait for the result on the callback channel (with timeout and context cancellation).

### Constants

```go
const defaultTimeout = 15 * time.Second
```

### Core Pattern: execJS

```go
func execJS(ctx context.Context, m *Manager, windowID string,
    jsGen func(reqID, cbURL string) string, timeout time.Duration) (json.RawMessage, error)
```

1. `m.GetWindow(windowID)` -- resolve window.
2. `newRequestID()` -- generate `"req-{unixNano}"`.
3. `m.CallbackURL()` -- get callback URL.
4. `jsGen(reqID, cbURL)` -- generate the action-specific JS.
5. `collector.Register(reqID)` -- register pending channel.
6. `win.Handle.ExecJS(js)` -- execute in webview.
7. `select` on: result channel, timeout, context done.
8. On result: `collector.Cleanup(reqID)`, check for JS error.
9. On timeout: cleanup, return error.
10. On context done: cleanup, return context error.

### Actions

**Navigate(ctx, m, windowID, url, timeout) (json.RawMessage, error):**

1. `win.Handle.SetURL(url)` -- navigate the webview.
2. Update `win.URL`.
3. **Sleep 1500ms** -- wait for page load and Wails runtime re-injection.
4. Re-inject fingerprint (JS context resets on navigation).
5. Call `GetInfo()` to collect page metadata.
6. Update tracked window title from response.

**GetInfo(ctx, m, windowID, timeout) (json.RawMessage, error):**

Executes `pageInfoJS` and returns `{url, title, scrollY, documentHeight, viewportHeight}`.

**Snapshot(ctx, m, windowID, timeout) (json.RawMessage, error):**

Executes `snapshotJS` which walks the DOM and produces an accessible text representation
with interactive element refs (e1, e2, ...).

**Click(ctx, m, windowID, ref, selector, timeout) (json.RawMessage, error):**

Executes `cursorClickJS` which simulates realistic cursor movement via bezier curves
and then clicks. Uses `ref` (data-nebo-ref attribute) or CSS `selector`.

**Fill(ctx, m, windowID, ref, selector, value, timeout) (json.RawMessage, error):**

Executes `fillJS` which sets input/textarea values using the native value setter trick
for React/Vue/Angular compatibility.

**Type(ctx, m, windowID, ref, selector, text, timeout) (json.RawMessage, error):**

Executes `typeJS` which types text character by character with keydown/keypress/input/keyup
events per character.

**GetText(ctx, m, windowID, selector, timeout) (json.RawMessage, error):**

Executes `getTextJS`. If selector is empty, returns `document.body.textContent`.

**Evaluate(ctx, m, windowID, code, timeout) (json.RawMessage, error):**

Executes arbitrary JavaScript via `evalJS` and returns the result.

**Scroll(ctx, m, windowID, direction, timeout) (json.RawMessage, error):**

Executes `scrollJS`. Directions: up, down, left, right, top, bottom.

**Wait(ctx, m, windowID, selector, timeout) (json.RawMessage, error):**

Executes `waitJS` which polls every 200ms for the selector to exist. The Go-side timeout
adds a 2-second buffer beyond the JS polling timeout.

**Hover(ctx, m, windowID, ref, selector, timeout) (json.RawMessage, error):**

Executes `cursorHoverJS` for realistic hover simulation (movement without clicking).

**Select(ctx, m, windowID, ref, selector, value, timeout) (json.RawMessage, error):**

Executes `selectJS` to set a value on a `<select>` element.

**Back(ctx, m, windowID, timeout) (json.RawMessage, error):**

Fires `history.back()` directly (no callback), sleeps 1 second, then calls `GetInfo()`.

**Forward(ctx, m, windowID, timeout) (json.RawMessage, error):**

Fires `history.forward()` directly, sleeps 1 second, then calls `GetInfo()`.

**Reload(ctx, m, windowID) error:**

Calls `win.Handle.Reload()` directly. No callback, no result.

**Screenshot(ctx, m, windowID) (string, error):**

Executes `screenshotJS` which attempts to use `html2canvas` if available, otherwise
falls back to a basic canvas-based capture. Returns base64-encoded PNG data.
Timeout: 30 seconds.

### Helper: updateWindowTitle

```go
func updateWindowTitle(win *Window, info json.RawMessage)
```

Parses a `{title: string}` from the GetInfo response and updates both `win.Title`
and `win.Handle.SetTitle(title)`.

### Helper: newRequestID

```go
func newRequestID() string
// Returns "req-{time.Now().UnixNano()}"
```

---

## Callback System

### File: `nebo/internal/webview/callback.go`

### CallbackResult

The wire format for JS-to-Go communication:

```go
type CallbackResult struct {
    RequestID string          `json:"requestId"`
    Data      json.RawMessage `json:"data"`
    Error     string          `json:"error,omitempty"`
}
```

### CallbackCollector

```go
type CallbackCollector struct {
    mu      sync.Mutex
    pending map[string]chan CallbackResult
}
```

Global singleton (not lazily initialized, created at package init):

```go
var collector = &CallbackCollector{
    pending: make(map[string]chan CallbackResult),
}

func GetCollector() *CallbackCollector
```

**Register(requestID string) chan CallbackResult:**

Creates a buffered channel (capacity 1), stores it in `pending`, returns the channel.

**Deliver(result CallbackResult):**

Looks up the channel by `result.RequestID`. If found, removes from pending and sends
the result on the channel. If not found (unknown or already cleaned up), silently drops.

**Cleanup(requestID string):**

Removes the entry from `pending`. Called after a result is received or on timeout.

### Dual Delivery Paths

Results can arrive via two paths:

1. **Native bridge (preferred):** JS calls `window.__nebo_cb(d)` which sends
   `"nebo:cb:{json}"` via the native message handler. The Wails `RawMessageHandler`
   in desktop.go receives it and calls `GetCollector().Deliver(result)`.

2. **HTTP POST (fallback):** JS calls `fetch(callbackURL, {method: "POST", body: json})`.
   The `CallbackHandler()` HTTP handler receives it and calls `collector.Deliver(result)`.

### WaitForResult

```go
func WaitForResult(ctx context.Context, requestID string, timeout time.Duration) (json.RawMessage, error)
```

Convenience function that registers, waits (with timeout and context), and cleans up.
Returns the data or an error. Used for standalone callback waiting.

### CallbackHandler() http.HandlerFunc

HTTP handler for `POST /internal/webview/callback`.

**CORS:** Allows any origin (mirrors the requesting origin in `Access-Control-Allow-Origin`).
This is necessary because the webview loads external sites whose JS must POST results
back to this localhost endpoint.

**Request flow:**
1. OPTIONS: Return 204 with CORS headers.
2. Non-POST: Return 405.
3. POST: Decode JSON body as `CallbackResult`.
4. Validate `requestId` is non-empty.
5. Call `collector.Deliver(result)`.
6. Return 200.

**Security model:** Localhost-only. The callback URL is always `http://localhost:{port}/...`.
The request ID is generated by Go and is unpredictable (nanosecond timestamp), so external
sites cannot forge valid callbacks.

---

## JavaScript Injection

### File: `nebo/internal/webview/js.go`

### Architecture

All JS templates are self-contained IIFEs (Immediately Invoked Function Expressions).
They follow a pattern:

1. Define `__cb` (the callback function).
2. Execute the action code.
3. Send result back via `__cb({requestId: ..., data: ...})`.

### callbackJS(callbackURL string) string

Returns a JS snippet that defines `__cb(data)`:

```javascript
var __cb = window.__nebo_cb || function(d) {
    var m = "nebo:cb:" + JSON.stringify(d);
    try {
        // Try Wails bridge first
        if (window._wails && window._wails.invoke) {
            window._wails.invoke(m);
        }
        // Then macOS WebKit
        else if (window.webkit && window.webkit.messageHandlers &&
                 window.webkit.messageHandlers.external) {
            window.webkit.messageHandlers.external.postMessage(m);
        }
        // Then Windows WebView2
        else if (window.chrome && window.chrome.webview) {
            window.chrome.webview.postMessage(m);
        }
        // Finally HTTP fetch (headless fallback)
        else {
            fetch(callbackURL, {
                method: "POST",
                headers: {"Content-Type": "application/json"},
                body: JSON.stringify(d)
            }).catch(function(){});
        }
    } catch(e) {}
};
```

**Priority:** `window.__nebo_cb` (pre-defined by bootstrap JS) > native bridge > HTTP fetch.

### wrapJS(requestID, callbackURL, actionCode string) string

Standard wrapper for synchronous actions:

```javascript
(function(){
    <callbackJS>
    try {
        <actionCode>
        __cb({requestId: "<requestID>", data: __result});
    } catch(e) {
        __cb({requestId: "<requestID>", error: e.message || String(e)});
    }
})();
```

The `actionCode` must assign its result to `var __result`.

### jsonString(s string) string

Returns `json.Marshal(s)` as a string -- safely JSON-encodes strings for embedding
in JavaScript. Handles quotes, backslashes, newlines, etc.

### Action JS Generators

**pageInfoJS(requestID, callbackURL):**

```javascript
var __result = {
    url: location.href,
    title: document.title,
    scrollY: window.scrollY,
    documentHeight: document.documentElement.scrollHeight,
    viewportHeight: window.innerHeight
};
```

**snapshotJS(requestID, callbackURL):**

Walks the DOM recursively via `__walk(el, depth)`. For each element:

1. Skip hidden elements (`offsetParent === null`, except body/html).
2. Detect interactive elements: `a`, `button`, `input`, `textarea`, `select`, `details`,
   `summary`, plus elements with `role="button"`, `tabindex`, `onclick`, or
   `contenteditable="true"`.
3. Assign refs to interactive elements: `[e1]`, `[e2]`, etc. Sets
   `data-nebo-ref="eN"` attribute on the DOM element for later targeting.
4. Build description strings by tag:
   - `a`: `[eN] link "text" -> href`
   - `button`: `[eN] button "text"`
   - `input`: `[eN] input[type] name=... label=... value=... placeholder=... required`
   - `textarea`: `[eN] textarea name=... label=... value=...`
   - `select`: `[eN] select name=... label=... value=... options=[...]`
   - `img`: `img alt=...`
   - `h1-h6`: `hN: text`
   - `p/span/li/td/th/label`: `tag: text` (leaf nodes under 200 chars)
   - `form`: `form method=GET/POST action=...`
5. Labels are resolved via: `<label for="id">`, parent `<label>`, `aria-label`,
   `aria-labelledby`.
6. Select options are enumerated (max 10): `value="display text"`.

Output format:
```
Page: <title>
URL: <url>
---
<indented tree of elements>
```

**clickJS(requestID, callbackURL, ref, selector):**

Simple click -- scrolls element into view, calls `el.click()`, returns
`{ok: true, tag: ..., text: ...}`.

**fillJS(requestID, callbackURL, ref, selector, value):**

Framework-compatible fill using native value setter:

```javascript
var proto = tag === "textarea" ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
var nativeSetter = Object.getOwnPropertyDescriptor(proto, "value").set;
nativeSetter.call(el, value);
el.dispatchEvent(new Event("input", {bubbles: true}));
el.dispatchEvent(new InputEvent("input", {bubbles: true, inputType: "insertText", data: value}));
el.dispatchEvent(new Event("change", {bubbles: true}));
```

This bypasses React/Vue/Angular's value tracking by using the native HTMLInputElement
prototype setter directly.

**typeJS(requestID, callbackURL, ref, selector, text):**

Character-by-character typing with full keyboard event sequence per character:

```javascript
for (var i = 0; i < text.length; i++) {
    var c = text[i];
    el.dispatchEvent(new KeyboardEvent('keydown', {key: c, bubbles: true}));
    el.dispatchEvent(new KeyboardEvent('keypress', {key: c, bubbles: true}));
    nativeSetter.call(el, (el.value || "") + c);
    el.dispatchEvent(new InputEvent('input', {bubbles: true, inputType: 'insertText', data: c}));
    el.dispatchEvent(new KeyboardEvent('keyup', {key: c, bubbles: true}));
}
el.dispatchEvent(new Event('change', {bubbles: true}));
```

**getTextJS(requestID, callbackURL, selector):**

With selector: `el.textContent.trim()`.
Without selector: `document.body.textContent.trim()`.

**evalJS(requestID, callbackURL, code):**

Wraps arbitrary code in a function: `var __result = (function(){ <code> })();`

**scrollJS(requestID, callbackURL, direction):**

| Direction | Code |
|-----------|------|
| up | `window.scrollBy(0, -window.innerHeight * 0.8)` |
| down | `window.scrollBy(0, window.innerHeight * 0.8)` |
| left | `window.scrollBy(-window.innerWidth * 0.8, 0)` |
| right | `window.scrollBy(window.innerWidth * 0.8, 0)` |
| top | `window.scrollTo(0, 0)` |
| bottom | `window.scrollTo(0, document.documentElement.scrollHeight)` |

Returns: `{scrollY, scrollHeight, viewportHeight}`.

**waitJS(requestID, callbackURL, selector, timeoutMs):**

Polls every 200ms:

```javascript
function __poll() {
    var el = document.querySelector(selector);
    if (el) {
        __cb({requestId: ..., data: {found: true, elapsed: ...}});
    } else if (Date.now() - start > timeout) {
        __cb({requestId: ..., data: {found: false, elapsed: ..., error: "Timeout..."}});
    } else {
        setTimeout(__poll, 200);
    }
}
__poll();
```

Uses its own callback pattern (not the standard `wrapJS` wrapper) because it is
asynchronous with polling.

**hoverJS(requestID, callbackURL, ref, selector):**

Simple hover -- scrolls into view, dispatches `mouseenter` and `mouseover`.
Returns `{ok: true, tag: ...}`.

**selectJS(requestID, callbackURL, ref, selector, value):**

Sets `el.value`, dispatches `change` and `input` events.
Returns `{ok: true, value: ...}`.

**screenshotJS(requestID, callbackURL):**

Tries `html2canvas(document.body)` if available (async, with its own callback).
Otherwise creates a blank canvas at document dimensions with white background.
Returns `{data: "data:image/png;base64,..."}`.

### escapeSingleQuote(s string) string

Replaces `'` with `\'`. Used for embedding selectors in error messages.

---

## Cursor Simulation

### File: `nebo/internal/webview/cursor.go`

Provides realistic mouse movement simulation to avoid bot detection.

### cursorClickJS(requestID, callbackURL, ref, selector) string

Generates JavaScript that simulates a human clicking an element:

1. **Find element** by `data-nebo-ref` attribute or CSS selector.
2. **Scroll into view** with smooth behavior.
3. **Calculate target point**: Random position within the element's bounding rect
   (30-70% of width/height -- avoids exact center).
4. **Random start point**: Random position anywhere on screen.
5. **Bezier control point**: Midpoint between start and target, offset by random
   +/-200px in each axis.
6. **Parameters** (randomized per call):
   - Steps: 15-34
   - Jitter: 1-3px per step
   - Base delay: 5-14ms between steps
7. **Easing function**: Quadratic ease-in-out:
   ```javascript
   t < 0.5 ? 2*t*t : (1 - Math.pow(-2*t+2, 2) / 2)
   ```
8. **Movement loop** (`moveStep()`):
   - For each step: calculate bezier position, apply jitter, dispatch `mousemove`.
   - Delay between steps: `baseDelay + Math.random() * baseDelay`.
9. **Arrival events**: `mouseover`, `mouseenter`, `mousemove` on target.
10. **Click sequence** with human-like delays:
    - `mousedown` after 30-90ms.
    - `mouseup` after 20-60ms.
    - `click` event.
    - `el.click()` (redundant call for framework compatibility).
11. **Result**: `{ok: true, tag: ..., text: ..., path: {steps, startX, startY, endX, endY}}`.

**MouseEvent construction** includes realistic properties:
- `bubbles: true`, `cancelable: true`, `view: window`
- `clientX/clientY` and `screenX/screenY` (offset by `window.screenX/Y`)
- `button: 0` for click events, `-1` for movement
- `buttons: 1` for mousedown, `0` otherwise

### cursorHoverJS(requestID, callbackURL, ref, selector) string

Similar to click but without the click sequence:

1. Same element finding and bezier path generation.
2. Parameters:
   - Steps: 12-27
   - Jitter: 1-3px
   - Base delay: 5-14ms
3. Same easing function and movement loop.
4. Arrival events: `mouseover`, `mouseenter`, `mousemove`.
5. No mousedown/mouseup/click.
6. Result: `{ok: true, tag: ...}`.

---

## Fingerprint Generation

### File: `nebo/internal/webview/fingerprint.go`

### Purpose

Each native browser window gets a unique, randomized fingerprint to appear as a
different browser to fingerprinting scripts. This prevents correlation between
different agent browsing sessions and makes the webview harder to detect as automated.

### Fingerprint Struct

```go
type Fingerprint struct {
    UserAgent           string
    Platform            string
    Language            string
    Languages           []string
    TimezoneOffset      int       // minutes from UTC
    Timezone            string    // IANA timezone name
    ScreenWidth         int
    ScreenHeight        int
    ColorDepth          int
    PixelRatio          float64
    HardwareConcurrency int
    MaxTouchPoints      int
    WebGLVendor         string
    WebGLRenderer       string
    CanvasNoise         float64   // 0.0001 to 0.0011
}
```

### Randomization Pools

**Screen resolutions (10 options):**

| Width | Height |
|-------|--------|
| 1920 | 1080 |
| 2560 | 1440 |
| 1366 | 768 |
| 1440 | 900 |
| 1536 | 864 |
| 1680 | 1050 |
| 1280 | 720 |
| 1600 | 900 |
| 2560 | 1600 |
| 1920 | 1200 |

**Timezones (12 options):**

America/New_York (-300), America/Chicago (-360), America/Denver (-420),
America/Los_Angeles (-480), America/Phoenix (-420), Europe/London (0),
Europe/Berlin (60), Europe/Paris (60), Asia/Tokyo (540), Asia/Shanghai (480),
Australia/Sydney (660), Pacific/Auckland (780).

**User agents (7 options):**

- Safari 18.3 on macOS (MacIntel)
- Chrome 131 on macOS (MacIntel)
- Chrome 131 on Windows (Win32)
- Firefox 133 on Windows (Win32)
- Chrome 131 on Linux (Linux x86_64)
- Chrome 130 on macOS (MacIntel)
- Edge 130 on Windows (Win32)

**WebGL renderers (8 options):**

- Apple M1 Pro, M2, M3 (Metal)
- NVIDIA RTX 3060, RTX 4070 (D3D11)
- Intel UHD 770, Iris Xe (D3D11)
- AMD Radeon RX 7600 (D3D11)

All use `"Google Inc. (Vendor)"` format for the vendor string.

**Languages (5 options):**

- `["en-US", "en"]`
- `["en-US", "en", "es"]`
- `["en-GB", "en"]`
- `["en-US", "en", "fr"]`
- `["en-US", "en", "de"]`

**Additional randomized values:**

- `PixelRatio`: one of 1.0, 1.25, 1.5, 2.0
- `HardwareConcurrency`: one of 4, 6, 8, 10, 12, 16
- `MaxTouchPoints`: always 0 (desktop browser)
- `ColorDepth`: always 24
- `CanvasNoise`: `rand.Float64() * 0.001 + 0.0001` (range 0.0001 to 0.0011)

### GenerateFingerprint() *Fingerprint

Selects one random value from each pool and returns a complete fingerprint.
Uses `math/rand` (not crypto/rand).

### InjectJS() string

Returns JavaScript that overrides browser fingerprint APIs. All overrides use
`Object.defineProperty` with getter functions, making them appear native to
fingerprinting scripts.

**What is overridden:**

1. **Navigator properties:**
   - `navigator.userAgent`
   - `navigator.platform`
   - `navigator.language`
   - `navigator.languages`
   - `navigator.hardwareConcurrency`
   - `navigator.maxTouchPoints`

2. **Screen properties:**
   - `screen.width` / `screen.height`
   - `screen.availWidth` / `screen.availHeight` (availHeight = height - 40 for taskbar)
   - `screen.colorDepth`
   - `window.devicePixelRatio`

3. **Timezone:**
   - `Intl.DateTimeFormat` constructor: wraps to inject default timezone.
   - `Date.prototype.getTimezoneOffset`: returns configured offset.

4. **WebGL:**
   - `WebGLRenderingContext.prototype.getParameter`: intercepts
     `UNMASKED_VENDOR_WEBGL` and `UNMASKED_RENDERER_WEBGL` from
     `WEBGL_debug_renderer_info` extension.
   - Same for `WebGL2RenderingContext` if available.

5. **Canvas fingerprint:**
   - `HTMLCanvasElement.prototype.toDataURL`: Before calling the original,
     reads pixel data, adds tiny random noise to each red channel value
     (`noise * 255 * random`), writes back, then calls original `toDataURL`.

### When Fingerprints are Injected

1. **On window creation:** `CreateWindow()` calls `handle.ExecJS(fp.InjectJS())`
   immediately after the window handle is created.
2. **On navigation:** `Navigate()` re-injects `win.Fingerprint.InjectJS()` after
   a 1500ms delay (JS context resets on navigation).

---

## Platform-Specific Navigation Handling

### macOS (webview_navigation_darwin.go)

Build tags: `desktop && darwin`

```go
func InjectWebViewNavigationHandler() {}  // No-op
```

On macOS, external URL handling is done via JavaScript in the frontend
(`+layout.svelte`), not at the native level.

### Linux (webview_navigation_linux.go)

Build tags: `desktop && linux`

Uses cgo to interact with GTK3 and WebKit2GTK at the C level.

**C functions:**

- `isLocalURI(uri)`: Returns TRUE if URI starts with `http://localhost` or
  `http://127.0.0.1`.
- `openInBrowser(uri)`: Spawns `xdg-open` via `g_spawn_async`.
- `decidePolicyHook(ihint, n_params, params, data)`: GObject emission hook on
  `WebKitWebView::decide-policy` signal. Applied globally to all WebKitWebView instances.
  - Only intercepts `NAVIGATION_ACTION` and `NEW_WINDOW_ACTION` types.
  - Only intercepts from localhost pages (Nebo's own UI).
  - If target URI is external (`http://` or `https://` but not localhost):
    prints redirect message, opens in system browser, calls
    `webkit_policy_decision_ignore(decision)`.
  - Allows all localhost navigation.
  - Does NOT intercept navigation from agent-controlled browser windows (which
    load external sites).
- `injectLinuxNavigationHandler()`: Looks up the `"decide-policy"` signal on
  `webkit_web_view_get_type()`, then installs `decidePolicyHook` as a global
  emission hook.

**Go function:**

```go
func InjectWebViewNavigationHandler() {
    C.injectLinuxNavigationHandler()
}
```

Called after `application.New()` which initializes GTK.

### Windows/Other (webview_navigation_other.go)

Build tags: `desktop && !darwin && !linux`

```go
func InjectWebViewNavigationHandler() {}  // No-op
```

Windows uses JavaScript-based handling in `+layout.svelte` with the Navigation API
(Chromium-native in WebView2).

---

## Platform-Specific Media Permissions

### macOS (webview_permissions_darwin.go)

Build tags: `desktop && darwin`

Uses cgo with Objective-C runtime manipulation to auto-grant microphone/camera permissions.

**C functions:**

- `grantMediaCapturePermission(self, _cmd, webView, origin, frame, type, decisionHandler)`:
  WKUIDelegate method implementation. Calls `decisionHandler(WKPermissionDecisionGrant)`
  to auto-grant without showing a permission dialog.

- `injectMediaPermissionHandler()`:
  1. Gets Wails' `WebviewWindowDelegate` class via `objc_getClass`.
  2. Checks if `webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:`
     already exists. If so, skips (future Wails version may add it).
  3. Adds the method via `class_addMethod` with the correct ObjC type encoding:
     `"v@:@@@@q@?"` (void return, id, SEL, 4 objects, int64 enum, block).

**Go function:**

```go
func InjectWebViewMediaPermissions() {
    C.injectMediaPermissionHandler()
}
```

Called after `application.New()` (so the `WebviewWindowDelegate` class exists) but
before the window loads content.

### Non-macOS (webview_permissions_other.go)

Build tags: `desktop && !darwin`

```go
func InjectWebViewMediaPermissions() {}  // No-op
```

Only macOS WebKit requires this runtime delegate injection. Windows WebView2 and
Linux WebKit2GTK handle media permissions differently.

---

## Desktop Stub

### File: `nebo/cmd/nebo/desktop_stub.go`

Build tag: `!desktop`

```go
func RunDesktop() {
    fmt.Println("Desktop mode not available in this build. Running headless...")
    RunAll()
}
```

When built without the `desktop` tag (e.g., `CGO_ENABLED=0`), `RunDesktop` falls back
to headless mode by calling `RunAll()`.

---

## Wails Window Handle Adapter

### File: `nebo/cmd/nebo/desktop.go` (wailsWindowHandle type)

Adapts `*application.WebviewWindow` to the `webview.WindowHandle` interface:

```go
type wailsWindowHandle struct {
    win *application.WebviewWindow
}

func (w wailsWindowHandle) SetURL(url string)          // Delegates with debug logging
func (w wailsWindowHandle) ExecJS(js string)            // Delegates with debug logging (truncated preview)
func (w wailsWindowHandle) SetTitle(title string)       // Direct delegation
func (w wailsWindowHandle) Show()                       // Direct delegation
func (w wailsWindowHandle) Hide()                       // Direct delegation
func (w wailsWindowHandle) Focus()                      // Direct delegation
func (w wailsWindowHandle) Close()                      // Direct delegation
func (w wailsWindowHandle) SetSize(width, height int)   // Direct delegation
func (w wailsWindowHandle) Reload()                     // Direct delegation
func (w wailsWindowHandle) Name() string                // Returns win.Name()
```

`SetURL` and `ExecJS` include debug-level timing logs. `ExecJS` truncates the JS
preview to 80 characters.

### openURL(url string)

Opens a URL in the default system browser:

| OS | Command |
|----|---------|
| macOS | `open <url>` |
| Windows | `rundll32 url.dll,FileProtocolHandler <url>` |
| Linux/other | `xdg-open <url>` |

---

## Key Implementation Notes for Rust Port

1. **Thread model:** Wails requires the event loop on the main thread (macOS requirement).
   All background services run in goroutines. In Rust, the equivalent would be a Tao/Wry
   event loop on the main thread with tokio tasks for background work.

2. **Callback collector:** The request-ID-to-channel mapping is the core synchronization
   primitive. In Rust, use a `DashMap<String, oneshot::Sender<CallbackResult>>` or similar.

3. **JS injection timing:** The bootstrap JS and fingerprint JS must run before any page
   scripts. In Wry, use `initialization_scripts` or `WebViewBuilder::with_initialization_script`.

4. **Native bridge priority:** The native message handler (Wails invoke / WebKit postMessage /
   WebView2 postMessage) is strongly preferred over HTTP fetch because it bypasses CORS
   and mixed-content blocking. The Rust port should use Wry's IPC mechanism.

5. **Fingerprint re-injection:** After every navigation, the JS context resets. The
   fingerprint must be re-injected. The 1500ms delay in Navigate() is a pragmatic wait
   for page load. Consider using a navigation-complete event instead.

6. **Windows WebView2 workaround:** The blank-HTML-then-navigate pattern for JS injection
   is a Wails v3 alpha bug. In Wry, `AddScriptToExecuteOnDocumentCreated` works directly.

7. **Lock file:** The Rust port should use `fs2::FileExt::try_lock_exclusive()` for
   cross-platform file locking, which wraps `flock` on Unix and `LockFileEx` on Windows.

8. **Window state persistence:** Simple JSON file. The sanity checks (min 400x300) are
   important to handle corrupted state or minimized-window saves.

9. **Close-to-tray:** The window close event must be interceptable and cancellable.
   On macOS, `applicationShouldTerminateAfterLastWindowClosed` must be false.

10. **Agent reconnection:** Exponential backoff from 1s to 30s with doubling. The agent
    goroutine runs in an infinite retry loop until context cancellation.
