# Platform Tools - Complete Logic Deep-Dive

> Source: `nebo/internal/agent/tools/` (Go codebase)
> Covers every platform-specific tool, the STRAP domain pattern, capability system,
> policy/safeguard/origin infrastructure, and the snapshot pipeline.

---

## Table of Contents

1. [Core Infrastructure](#1-core-infrastructure)
   - [STRAP Pattern & Domain Tools](#11-strap-pattern--domain-tools)
   - [Capability System](#12-capability-system)
   - [Origin System](#13-origin-system)
   - [Policy System](#14-policy-system)
   - [Safeguard System](#15-safeguard-system)
   - [Desktop Queue](#16-desktop-queue)
2. [Desktop Domain Tool](#2-desktop-domain-tool)
   - [Desktop Input (DesktopTool)](#21-desktop-input-desktoptool)
   - [Window Management (WindowTool)](#22-window-management-windowtool)
   - [Menu Bar (MenubarTool)](#23-menu-bar-menubartool)
   - [Dialog (DialogTool)](#24-dialog-dialogtool)
   - [Virtual Desktops / Spaces (SpacesTool)](#25-virtual-desktops--spaces-spacestool)
   - [Dock (DockTool)](#26-dock-docktool)
   - [Shortcuts / Automations (ShortcutsTool)](#27-shortcuts--automations-shortcutstool)
   - [Screenshot (ScreenshotTool)](#28-screenshot-screenshottool)
   - [Text-to-Speech (TTSTool)](#29-text-to-speech-ttstool)
3. [System Domain Tool](#3-system-domain-tool)
   - [Settings (SettingsTool)](#31-settings-settingstool)
   - [OS App Control (OSAppTool)](#32-os-app-control-osapptool)
   - [File Search / Spotlight (SpotlightTool)](#33-file-search--spotlight-spotlighttool)
   - [Clipboard (ClipboardTool)](#34-clipboard-clipboardtool)
   - [Music (MusicTool)](#35-music-musictool)
   - [Keychain (KeychainTool)](#36-keychain-keychaintool)
   - [Notification (NotificationTool)](#37-notification-notificationtool)
4. [PIM (Organizer) Domain Tool](#4-pim-organizer-domain-tool)
   - [Contacts (ContactsTool)](#41-contacts-contactstool)
   - [Calendar (CalendarTool)](#42-calendar-calendartool)
   - [Mail (MailTool)](#43-mail-mailtool)
   - [Reminders (RemindersTool)](#44-reminders-reminderstool)
   - [Messages (MessagesTool)](#45-messages-messagestool)
5. [Standalone Tools](#5-standalone-tools)
   - [Vision (VisionTool)](#51-vision-visiontool)
   - [Accessibility (AccessibilityTool)](#52-accessibility-accessibilitytool)
   - [NeboLoop App Store (AppTool)](#53-neboloop-app-store-apptool)
6. [Snapshot Pipeline](#6-snapshot-pipeline)
   - [SnapshotStore](#61-snapshotstore)
   - [Element Annotation (AssignElementIDs)](#62-element-annotation-assignelementids)
   - [Rendering (RenderAnnotations)](#63-rendering-renderannotations)
   - [Window Capture (CaptureAppWindow)](#64-window-capture-captureappwindow)
   - [Accessibility Tree (getUITreeWithBounds)](#65-accessibility-tree-getuitreewithbounds)

---

## 1. Core Infrastructure

### 1.1 STRAP Pattern & Domain Tools

**File:** `domain.go`

STRAP (Single Tool Resource Action Pattern) consolidates ~35 individual tools into a handful
of domain tools. Each domain tool multiplexes requests via `resource` + `action` fields,
reducing the number of tool definitions the LLM must hold in context by ~80%.

**DomainTool interface** extends the base `Tool` interface:

```go
type DomainTool interface {
    Tool
    Domain() string                     // e.g. "desktop", "system", "organizer"
    Resources() []string                // e.g. ["input", "window", "menu", "dialog"]
    ActionsFor(resource string) []string // e.g. for "input": ["click", "type", "hotkey"]
}
```

**DomainInput** is the base input shared by all STRAP tools:

```go
type DomainInput struct {
    Resource string `json:"resource,omitempty"`
    Action   string `json:"action"`
}
```

**ResourceConfig** defines a resource and its actions:

```go
type ResourceConfig struct {
    Name        string
    Actions     []string
    Description string
}
```

**FieldConfig** defines a field in the domain schema:

- `Name`, `Type` ("string", "integer", "boolean", "array", "object"), `Description`
- `Required` (globally required) / `RequiredFor` (action-specific)
- `Enum` (allowed values), `Default`, `Items` (array item type), `ItemSchema` (full schema for array items)

**Key functions:**

| Function | Purpose |
|----------|---------|
| `ValidateResourceAction(resource, action, resources)` | Validates resource/action pair. Falls back to empty-string resource for flat domains. |
| `BuildDomainSchema(cfg)` | Generates JSON Schema from `DomainSchemaConfig`. Adds `resource` enum if >1 resource, collects all actions across resources for `action` enum. |
| `BuildDomainDescription(cfg)` | Generates a description string with resource/action docs and examples. |
| `ActionRequiresApproval(action, dangerousActions)` | Checks if an action is in a configurable dangerous-actions list. |

---

### 1.2 Capability System

**File:** `capabilities.go`

Replaces go-plugin architecture for mobile compatibility. Uses build tags for
platform-specific compilation.

**Platform constants:**

| Constant | Value |
|----------|-------|
| `PlatformDarwin` | `"darwin"` |
| `PlatformLinux` | `"linux"` |
| `PlatformWindows` | `"windows"` |
| `PlatformIOS` | `"ios"` |
| `PlatformAndroid` | `"android"` |
| `PlatformAll` | `"all"` |

**Capability struct:**

```go
type Capability struct {
    Tool          Tool
    Platforms     []string // Empty or "all" = available everywhere
    Category      string   // "system", "media", "productivity", "desktop"
    RequiresSetup bool
}
```

**CapabilityRegistry:**

- Singleton via `var capabilities = NewCapabilityRegistry()`
- `detectPlatform()` uses `runtime.GOOS`, with `isIOS` override for iOS build tag
- `Register(cap)` only registers if `isAvailable(cap)` returns true (checks platform match)
- `RegisterToToolRegistry(tr)` copies all registered capabilities into the tool registry
- `RegisterPlatformCapabilitiesWithPermissions(tr, permissions)` filters by category-to-permission mapping:
  - `"productivity"` -> `"contacts"`, `"system"` -> `"system"`, `"media"` -> `"media"`, `"desktop"` -> `"desktop"`

**Registration flow:** Platform-specific `init()` functions (via build tags) call `RegisterCapability()` to add tools. At startup, `RegisterPlatformCapabilities(tr)` copies them into the main tool registry.

---

### 1.3 Origin System

**File:** `origin.go`

Tracks the source of each request through context propagation.

| Origin | Source | Description |
|--------|--------|-------------|
| `OriginUser` | Web UI, CLI | Direct user interaction |
| `OriginComm` | NeboLoop DMs, loop channels | Inter-agent communication |
| `OriginApp` | External `.napp` binaries | App-initiated requests |
| `OriginSkill` | Matched YAML skills | Skill template execution |
| `OriginSystem` | Heartbeat, cron, recovery | Internal system tasks |

**Functions:**

- `WithOrigin(ctx, origin)` / `GetOrigin(ctx)` - Returns `OriginUser` if unset (safe default)
- `WithSessionKey(ctx, key)` / `GetSessionKey(ctx)` - Session key propagation
- `WithSessionID(ctx, id)` / `GetSessionID(ctx)` - Session DB ID propagation

**Context key type:** Unexported `contextKey int` with `iota` constants to avoid collisions.

---

### 1.4 Policy System

**File:** `policy.go`

Three-layer approval system for tool execution.

**PolicyLevel:**

| Level | Behavior |
|-------|----------|
| `PolicyDeny` | All operations require approval |
| `PolicyAllowlist` | Only allowlisted commands auto-approve (default) |
| `PolicyFull` | All operations auto-approve |

**AskMode:**

| Mode | Behavior |
|------|----------|
| `AskModeOff` | Never ask |
| `AskModeOnMiss` | Ask only for non-allowlisted (default) |
| `AskModeAlways` | Always ask |

**SafeBins (always allowed):**

```
ls, pwd, cat, head, tail, grep, find, which, type, jq, cut, sort, uniq, wc, echo,
date, env, printenv, git status, git log, git diff, git branch, git show,
go version, node --version, python --version
```

**Origin deny lists (default):**

| Origin | Denied Tools |
|--------|-------------|
| `OriginComm` | `shell`, `system:shell` |
| `OriginApp` | `shell`, `system:shell` |
| `OriginSkill` | `shell`, `system:shell` |

Remote agents, apps, and skills cannot execute shell commands.

**`IsDeniedForOrigin(origin, toolName, input...)`:** Checks bare tool name and `tool:resource` compound key.

**`RequiresApproval(cmd)`:**
1. If `IsAutonomous` callback returns true, skip approval
2. `PolicyFull` = no approval needed
3. `PolicyDeny` = always requires approval
4. If allowlisted, only ask if `AskModeAlways`
5. If not allowlisted, ask unless `AskModeOff`

**`RequestApproval(ctx, toolName, input)`:**
1. System origin auto-approves (no human to ask)
2. Autonomous mode auto-approves
3. `PolicyFull` auto-approves
4. For bash tools, checks allowlist
5. Uses `ApprovalCallback` if set (web UI approval)
6. Falls back to stdin prompt: `[y/N/a(lways)]`

**`IsDangerous(cmd)`:** Checks for: `rm -rf`, `sudo`, `chmod 777`, `dd`, `mkfs`, `curl | sh`, `eval`, fork bombs.

---

### 1.5 Safeguard System

**File:** `safeguard.go`

Hard safety limits that CANNOT be overridden by any setting, policy, or autonomous mode. Runs inside `registry.Execute()` before `tool.Execute()`.

**`CheckSafeguard(toolName, input)`** guards `file` and `shell` tools:

**File safeguard** (write/edit actions only):
- Resolves to absolute path
- Checks both original path and symlink-resolved path
- Calls `isProtectedPath()` per-platform

**Shell safeguard** (bash exec action only):
- **Blocks sudo** in all forms: prefix, piped, chained, subshell
- **Blocks su** (switch user) but allows words starting with "su" (suspend, surface, etc.)
- **Blocks destructive commands:**
  - `rm -rf /` (root wipe detection)
  - `dd` to block devices (`of=/dev/`)
  - All disk formatting: `mkfs`, `fdisk`, `gdisk`, `parted`, `sfdisk`, `cfdisk`, `wipefs`, `sgdisk`, `partprobe`, `diskutil erasedisk/erasevolume/partitiondisk/apfs deletecontainer`, `format`
  - Fork bombs
  - Writes to `/dev/` (except `/dev/null`, `/dev/stdout`, `/dev/stderr`)
  - `rm` targeting protected paths
  - `chmod`/`chown` on protected paths

**Protected paths by platform:**

**macOS (`isProtectedPathDarwin`):**
- `/System`, `/usr/bin`, `/usr/sbin`, `/usr/lib`, `/usr/libexec`, `/usr/share`
- `/bin`, `/sbin`, `/private/var/db`
- `/Library/LaunchDaemons`, `/Library/LaunchAgents`, `/etc`

**Linux (`isProtectedPathLinux`):**
- `/bin`, `/sbin`, `/usr/bin`, `/usr/sbin`, `/usr/lib`, `/usr/libexec`, `/usr/share`
- `/boot`, `/etc`, `/proc`, `/sys`, `/dev`, `/root`
- `/var/lib/dpkg`, `/var/lib/rpm`, `/var/lib/apt`

**Windows (`isProtectedPathWindows`):**
- `C:\Windows`, `C:\Program Files`, `C:\Program Files (x86)`
- `C:\ProgramData`, `C:\Recovery`, `C:\$Recycle.Bin`

**Cross-platform sensitive user paths:**
- `~/.ssh`, `~/.gnupg`, `~/.aws/credentials`, `~/.aws/config`
- `~/.kube/config`, `~/.docker/config.json`
- Nebo's own data directory (database, config) - prevents catastrophic self-harm

---

### 1.6 Desktop Queue

**File:** `desktop_queue.go`

Serializes desktop-category tool calls through `LaneDesktop` because they control the screen, mouse, or keyboard and cannot safely run in parallel.

**Desktop tool names:**

```
desktop, accessibility, screenshot, app, browser, window, menubar, dialog, shortcuts
```

**`DesktopQueueFunc`:** `func(ctx, execute func(ctx) *ToolResult) *ToolResult`

**Execution flow in `executeTool()`:**

1. Check origin-based restrictions (`IsDeniedForOrigin`)
2. Run `tool.pre_execute` hook (apps can intercept/modify tool calls)
3. Check approval (`RequiresApproval` + `RequestApproval`)
4. Execute the tool
5. Truncate results >100KB
6. Run `tool.post_execute` hook (apps can modify results)

**Hook system:** `tool.pre_execute` can fully handle a tool call (return a result directly) or modify the input. `tool.post_execute` can modify the result.

---

## 2. Desktop Domain Tool

The Desktop domain consolidates input, window, menu, dialog, spaces, shortcuts, screenshot, and TTS into one STRAP tool.

### 2.1 Desktop Input (DesktopTool)

**STRAP path:** `desktop(resource: "input", action: ...)`
**RequiresApproval:** `true`

**Actions:** `click`, `double_click`, `right_click`, `type`, `hotkey`, `scroll`, `move`, `drag`, `paste`

**Input fields:**
- `x`, `y` (int) - Screen coordinates
- `text` (string) - Text to type or paste
- `keys` (string) - Hotkey combo (e.g. "cmd+c")
- `direction` (string) - Scroll direction: up/down/left/right
- `amount` (int) - Scroll amount (default: 3)
- `to_x`, `to_y` (int) - Drag destination
- `element` (string) - Element ID from snapshot (e.g. "B1", "T2")

**Element ID resolution:** When `element` is set, calls `GetSnapshotStore().LookupElement("", element)` to find the element's center coordinates. Sets `x`/`y` from `element.Bounds.Center()`.

#### macOS (`desktop_darwin.go`)

**Backends:** `cliclick` (preferred, checked via `exec.LookPath`), AppleScript fallback

| Action | cliclick | AppleScript Fallback |
|--------|----------|---------------------|
| `click` | `cliclick c:X,Y` | `click at {X, Y}` on System Events |
| `double_click` | `cliclick dc:X,Y` | `click at {X, Y}` twice |
| `right_click` | `cliclick rc:X,Y` | `key code 0 using control down` (Ctrl+click) |
| `type` | `cliclick t:TEXT` | `keystroke TEXT` on System Events |
| `hotkey` | `cliclick kd:MODIFIER ku:MODIFIER kp:KEY` | `key code N using {modifiers}` |
| `scroll` | `cliclick cu:X,Y` (up), `cd:X,Y` (down) | Not supported via cliclick scroll |
| `move` | `cliclick m:X,Y` | `do shell script "cliclick m:X,Y"` |
| `drag` | `cliclick dd:X,Y du:TX,TY` | N/A |
| `paste` | Saves clipboard, copies text via AppleScript, Cmd+V keystroke, restores clipboard | Same flow |

**Modifier mapping (cliclick):**
- `cmd`/`command` -> `cmd`
- `alt`/`option` -> `alt`
- `ctrl`/`control` -> `ctrl`
- `shift` -> `shift`
- `fn` -> `fn`

**Key code mapping (AppleScript):** Special keys mapped to macOS virtual key codes:
- `return` -> 36, `tab` -> 48, `space` -> 49, `delete` -> 51, `escape` -> 53
- Arrow keys: `left` -> 123, `right` -> 124, `down` -> 125, `up` -> 126
- Function keys: `f1` -> 122, `f2` -> 120, etc.

#### Linux (`desktop_linux.go`)

**Backends:** `xdotool` (X11, preferred) or `ydotool` (Wayland fallback)

**Extra actions (Linux only):** `get_mouse_pos`, `get_active_window`

| Action | xdotool | ydotool |
|--------|---------|---------|
| `click` | `xdotool mousemove X Y click 1` | `ydotool mousemove --absolute -x X -y Y click 0xC0` |
| `double_click` | `click --repeat 2 --delay 50` | Two `click 0xC0` calls |
| `right_click` | `click 3` | `click 0xC1` |
| `type` | `xdotool type --clearmodifiers TEXT` | `ydotool type TEXT` |
| `hotkey` | `xdotool key combo` (e.g. `super+l`) | `ydotool key combo` |
| `scroll` | button 4 (up), 5 (down), 6 (left), 7 (right) | Only up/down supported |
| `move` | `xdotool mousemove X Y` | `ydotool mousemove --absolute -x X -y Y` |
| `drag` | `mousedown 1` + `mousemove` + `mouseup 1` | Not supported |
| `paste` | `xclip -selection clipboard` + `ctrl+v` | `wl-copy TEXT` + Wayland key codes |
| `get_mouse_pos` | `xdotool getmouselocation` | Not supported |
| `get_active_window` | `xdotool getactivewindow getwindowname` | Not supported |

**Modifier mapping (xdotool/ydotool):** `cmd`/`command`/`super`/`meta` -> `super`, `alt`/`option` -> `alt`

#### Windows (`desktop_windows.go`)

**Backend:** PowerShell + user32.dll P/Invoke

**P/Invoke functions:** `SetCursorPos`, `mouse_event`, `GetCursorPos`, `GetForegroundWindow`, `GetWindowText`

| Action | Implementation |
|--------|---------------|
| `click` | `SetCursorPos(X, Y)` + `mouse_event(0x0002, 0x0004)` (LEFTDOWN, LEFTUP) |
| `double_click` | Two click sequences |
| `right_click` | `mouse_event(0x0008, 0x0010)` (RIGHTDOWN, RIGHTUP) |
| `type` | `$wshell.SendKeys(TEXT)` with `escapeDesktopSendKeys()` (escapes `+`, `^`, `%`, `~`, `{`, `}`, `(`, `)`, `[`, `]`) |
| `hotkey` | `$wshell.SendKeys(combo)` via `convertToSendKeys()` |
| `scroll` | `mouse_event(0x0800, amount)` where MOUSEEVENTF_WHEEL = 0x0800, 120 per notch |
| `move` | `SetCursorPos(X, Y)` |
| `drag` | `SetCursorPos` + `mouse_event(LEFTDOWN)` + sleep 100ms + `SetCursorPos` + `mouse_event(LEFTUP)` |
| `paste` | `[System.Windows.Forms.Clipboard]::SetText(TEXT)` + `SendKeys("^v")` |

**`convertToSendKeys()` mapping:**
- `cmd`/`command`/`win` -> `^` (Ctrl in Windows, since there's no Cmd)
- `ctrl`/`control` -> `^`
- `alt`/`option` -> `%`
- `shift` -> `+`
- Keys wrapped in `{}` for SendKeys: `{ENTER}`, `{TAB}`, `{ESC}`, `{DELETE}`, `{LEFT}`, etc.

---

### 2.2 Window Management (WindowTool)

**STRAP path:** `desktop(resource: "window", action: ...)`
**RequiresApproval:** `false`

**Actions:** `list`, `focus`, `move`, `resize`, `minimize`, `maximize`, `close`

**Input fields:**
- `app` (string) - Application name or window title
- `x`, `y` (int) - Position for move
- `width`, `height` (int) - Size for resize

#### macOS (`window_darwin.go`)

**Backend:** AppleScript via System Events

| Action | AppleScript |
|--------|-------------|
| `list` | `name & "|||" & position & "|||" & size` of windows of process. Delimiter: `"|||"`. |
| `focus` | `tell process` + `perform action "AXRaise" of window 1` + `set frontmost to true` |
| `move` | `set position of window 1 to {X, Y}` |
| `resize` | `set size of window 1 to {W, H}` |
| `minimize` | `set value of AXMinimized attribute of window 1 to true` |
| `maximize` | Gets screen bounds from `tell Finder to get bounds of window of desktop`, then sets position to {0,0} and size to screen bounds |
| `close` | `click button 1 of window 1` (close button is button 1) |

**Output format (list):** Lines of `"<name> - Position: (X, Y), Size: WxH"`. Parses the "|||" delimiter to extract name, position, and size.

#### Linux (`window_linux.go`)

**Backends:** `wmctrl` (preferred) or `xdotool`

| Action | wmctrl | xdotool |
|--------|--------|---------|
| `list` | `wmctrl -l -G -p` (ID, desktop, PID, X, Y, W, H, host, title) | `xdotool search --name APP` + `getwindowgeometry` per window |
| `focus` | `wmctrl -a APP` | `xdotool search --name APP windowactivate` |
| `move` | `wmctrl -r APP -e 0,X,Y,-1,-1` | `xdotool search --name APP windowmove X Y` |
| `resize` | `wmctrl -r APP -e 0,-1,-1,W,H` | `xdotool search --name APP windowsize W H` |
| `minimize` | `wmctrl -r APP -b add,hidden` | `xdotool search --name APP windowminimize` |
| `maximize` | `wmctrl -r APP -b add,maximized_vert,maximized_horz` | `xdotool getdisplaygeometry` + `windowsize` + `windowmove 0 0` |
| `close` | `wmctrl -c APP` | `xdotool search --name APP windowclose` |

**`parseXdotoolGeometry(output)`:** Parses `xdotool getwindowgeometry --shell` output: `X=`, `Y=`, `WIDTH=`, `HEIGHT=` lines.

#### Windows (`window_windows.go`)

**Backend:** PowerShell + user32.dll

**P/Invoke functions:** `GetWindowRect`, `SetWindowPos`, `ShowWindow`, `GetForegroundWindow`, `GetWindowText`

**`findProcessPS(app)`:** 4-level matching:
1. Exact process name (`Get-Process -Name "app"`)
2. Partial process name (`-like "*app*"`)
3. Window title match (`MainWindowTitle -like "*app*"`)
4. FileDescription match (`FileVersionInfo.FileDescription -like "*app*"`)

| Action | Implementation |
|--------|---------------|
| `list` | `Get-Process` with `MainWindowTitle -ne ""`, then `GetWindowRect` for each |
| `focus` | `ShowWindow(hwnd, 9)` (SW_RESTORE) + `SetForegroundWindow(hwnd)` |
| `move` | `SetWindowPos(hwnd, 0, X, Y, 0, 0, 0x0001)` (SWP_NOSIZE flag) |
| `resize` | `GetWindowRect` to get current position, then `SetWindowPos(hwnd, 0, X, Y, W, H, 0)` |
| `minimize` | `ShowWindow(hwnd, 6)` (SW_MINIMIZE) |
| `maximize` | `ShowWindow(hwnd, 3)` (SW_MAXIMIZE) |
| `close` | `$proc.CloseMainWindow()` |

---

### 2.3 Menu Bar (MenubarTool)

**STRAP path:** `desktop(resource: "menu", action: ...)`
**RequiresApproval:** `false`

**Actions:** `list`, `menus`, `click`, `status`, `click_status`

**Input fields:**
- `app` (string) - Application name (default: frontmost)
- `menu` (string) - Top-level menu name (e.g. "File")
- `item` (string) - Menu item name (e.g. "Save")
- `menu_path` (string) - Combined path (e.g. "File > Save") -- resolved by `resolveMenuFields()`
- `status_item` (string) - Status bar item name

#### macOS (`menubar_darwin.go`)

**Backend:** AppleScript via System Events

**`resolveMenuFields(in)`:** If `menu_path` is set, splits by `" > "` to extract menu and item fields.

**`getAppProcess(app)`:** If app is empty, gets frontmost application name from System Events.

| Action | AppleScript |
|--------|-------------|
| `list` | `name of menu bar items of menu bar 1 of process` |
| `menus` | `name of menu items of menu MENU of menu bar item MENU of menu bar 1`. Also retrieves shortcuts via `AXMenuItemCmdChar` and `AXMenuItemCmdModifiers` attributes. |
| `click` | `click menu item ITEM of menu MENU of menu bar item MENU of menu bar 1` |
| `status` | Try ControlCenter process first, then SystemUIServer process: `name of menu bar items of menu bar 1` |
| `click_status` | `click menu bar item ITEM of menu bar 1` of ControlCenter/SystemUIServer |

**Menu shortcut display:** For each menu item, retrieves `AXMenuItemCmdChar` (the key) and `AXMenuItemCmdModifiers` (bitmask: 0=Cmd, bit0=Shift, bit1=Option, bit2=Ctrl). Formats as `(Cmd+Shift+S)`.

#### Windows (`menubar_windows.go`)

**Backend:** PowerShell + UI Automation API

**`findWindowPS(app)`:** 3-level matching:
1. Exact window name (`Current.Name -eq "app"`)
2. Partial window name (`-like "*app*"`)
3. Process name (`Get-Process -Name "app"`, then find window by PID)

| Action | Implementation |
|--------|---------------|
| `list` | Get MenuBar from window, then `Children` of MenuBar. Uses `collectMenuBarNamesPS()` filter. |
| `menus` | Find menu bar item by name, then `ExpandCollapsePattern` to expand, then read children. Handles both classic Win32 and WinUI 3 apps. |
| `click` | Find menu item by name hierarchy, use `InvokePattern` to click. |
| `status` | Navigate system tray: `Shell_TrayWnd` > `SysPager` hierarchy + `NotifyIconOverflowWindow`. |
| `click_status` | Find tray item, try `InvokePattern`, fallback to `SetFocus` + `{ENTER}` SendKeys. |

**WinUI 3 handling:** For modern Windows apps, flyout items are window descendants rather than MenuBarItem descendants. The code searches the Automation tree more broadly in this case.

**`collectMenuBarNamesPS(app)`:** Filters top-level menu bar items specifically from the window's direct descendants, avoiding false positives from nested UI elements.

---

### 2.4 Dialog (DialogTool)

**STRAP path:** `desktop(resource: "dialog", action: ...)`
**RequiresApproval:** `false`

**Actions:** `detect`, `list`, `click`, `fill`, `dismiss`

**Input fields:**
- `app` (string) - Application name
- `button` (string) - Button name to click
- `field` (string) - Field label for fill
- `text` (string) - Text to fill

#### macOS (`dialog_darwin.go`)

**Backend:** AppleScript via System Events

| Action | AppleScript |
|--------|-------------|
| `detect` | Checks for `sheet 1 of window 1` (sheets) and windows with role `AXDialog` or `AXSheet`. Returns boolean. |
| `list` | Lists buttons and text fields of detected dialog. Shows dialog title, buttons (`name of buttons of sheet 1`), and text fields. |
| `click` | `click button BUTTON of sheet 1 of window 1`. Falls back to window buttons if sheet not found. |
| `fill` | Tries text field by numeric index, then tries `set value of text field 1 to TEXT`. Uses `AXFocused` to find the right field when `field` label doesn't match. |
| `dismiss` | Tries buttons in order: Cancel, Close, OK, Done. Falls back to `key code 53` (Escape key). |

**Dialog detection logic:**
1. Check `sheet 1 of window 1` exists (AppleScript sheets)
2. Check windows with `role` of `AXDialog` or `AXSheet`
3. Returns `true` if either matches

#### Windows (`dialog_windows.go`)

**Backend:** PowerShell + UI Automation API

**`findDialogsPS(app)`:** Multi-strategy dialog detection:
1. Find sibling windows of same process (child windows, popup dialogs)
2. Search for `ControlType.Window` children of the main window
3. Search for modal `ControlType.Pane` patterns

| Action | Implementation |
|--------|---------------|
| `detect` | Runs `findDialogsPS()`, returns true if any dialogs found |
| `list` | Lists all controls in detected dialog: buttons, edit fields, text, checkboxes |
| `click` | Find button by name using exact match, then partial match (`-like "*button*"`), then search main window as fallback. Uses `InvokePattern`. |
| `fill` | Find edit control by label proximity or name, use `ValuePattern.SetValue()` |
| `dismiss` | Same button priority as macOS: Cancel, Close, OK, Done. No keyboard fallback. |

---

### 2.5 Virtual Desktops / Spaces (SpacesTool)

**STRAP path:** `desktop(resource: "space", action: ...)`
**RequiresApproval:** `false`

**Actions:** `list`, `switch`, `move_window`

**Input fields:**
- `direction` (string) - "left"/"right"/"new" or space number 1-9
- `number` (int) - Space number for direct switch (macOS)

#### macOS (`spaces_darwin.go`)

**Backend:** AppleScript key events

| Action | Implementation |
|--------|-------------|
| `list` | Returns informational message that macOS doesn't expose space list via API. Mentions `defaults read com.apple.spaces` as workaround. |
| `switch` by direction | `key code 123/124 using control down` (Ctrl+Left/Right arrows). "new" opens Mission Control via `key code 160`. |
| `switch` by number | `key code N using control down` where N maps to specific macOS virtual keycodes: 1->18, 2->19, 3->20, ..., 9->25. Supports spaces 1-9 only. |
| `move_window` | Opens Mission Control (`key code 160`), waits 500ms. Notes limitations -- recommends `yabai` window manager for reliable window-to-space movement. |

**Key codes:** Arrow keycodes: left=123, right=124. Number-to-keycode map: `{1:18, 2:19, 3:20, 4:21, 5:23, 6:22, 7:26, 8:28, 9:25}`.

#### Windows (`spaces_windows.go`)

**Backend:** PowerShell + keybd_event P/Invoke

**VK constants in `keybdEventPS`:**
- `VK_LWIN` = 0x5B, `VK_CONTROL` = 0x11
- `VK_LEFT` = 0x25, `VK_RIGHT` = 0x27
- `VK_D` = 0x44, `VK_SHIFT` = 0x10

| Action | Implementation |
|--------|-------------|
| `list` | Returns informational message that Windows doesn't expose virtual desktop list via standard API. |
| `switch` left/right | Win+Ctrl+Left/Right arrow via `keybd_event` |
| `switch` "new" | Win+Ctrl+D via `keybd_event` |
| `move_window` | Win+Ctrl+Shift+Left/Right via `keybd_event`. Notes this is Windows 11+ only. |

**Note:** No Linux implementation found -- spaces/virtual desktops are not implemented for Linux.

---

### 2.6 Dock (DockTool)

**STRAP path:** `desktop(resource: "dock", action: ...)`
**RequiresApproval:** `false`
**Platform:** macOS only

**Actions:** `list`, `launch`, `hide`, `show`, `running`

**Input fields:**
- `app` (string) - Application name (for launch)

#### macOS (`dock_darwin.go`)

| Action | Implementation |
|--------|-------------|
| `list` | `defaults read com.apple.dock persistent-apps` then parses plist to extract `"file-label"` values from `tile-data` entries. |
| `launch` | `open -a APP` via `exec.Command` |
| `hide` | `defaults write com.apple.dock autohide -bool true` + `killall Dock` |
| `show` | `defaults write com.apple.dock autohide -bool false` + `killall Dock` |
| `running` | AppleScript: `name of every process whose background only is false` from System Events |

**Plist parsing:** Reads raw plist text, splits by `"file-label"`, extracts quoted strings. Not using a plist library -- simple string parsing.

---

### 2.7 Shortcuts / Automations (ShortcutsTool)

**STRAP path:** `desktop(resource: "shortcut", action: ...)`
**RequiresApproval:** `true`

**Actions:** `run`, `list` (all platforms), `create`, `delete` (Linux and Windows only)

**Input fields:**
- `name` (string) - Shortcut/automation name
- `input` (string) - Input data for the shortcut
- `command` (string) - Shell command (for create on Linux/Windows)
- `schedule` (string) - Schedule expression (for create on Linux/Windows)

#### macOS (`shortcuts_darwin.go`)

**Backend:** `shortcuts` CLI (Apple Shortcuts)

| Action | Command |
|--------|---------|
| `run` | `shortcuts run NAME` or `shortcuts run NAME -i INPUT` |
| `list` | `shortcuts list` |

No `create` or `delete` -- Apple Shortcuts are managed via the Shortcuts app.

#### Linux (`shortcuts_linux.go`)

**Backends:** systemd (preferred), cron (fallback), plain scripts

| Action | Implementation |
|--------|---------------|
| `list` | Lists files in `<dataDir>/shortcuts/` directory + `systemctl --user list-timers` + `crontab -l` entries with `# Nebo:` marker |
| `run` | Executes `<dataDir>/shortcuts/NAME.sh` or `systemctl --user start NAME` |
| `create` | Creates `.sh` script in shortcuts dir. If `schedule` is set, creates systemd timer or cron entry. |
| `delete` | Removes script file. Removes systemd service+timer or cron entry. |

**systemd timer creation:**
1. Writes `nebo-NAME.service` to `~/.config/systemd/user/` with `ExecStart=/path/to/script.sh`
2. Writes `nebo-NAME.timer` with `OnCalendar=SCHEDULE`
3. Runs `systemctl --user daemon-reload` + `enable --now`

**Cron creation:** Adds entry with `# Nebo: NAME` comment suffix for identification. Removes by grepping out the marker line.

#### Windows (`shortcuts_windows.go`)

**Backend:** Task Scheduler + PowerShell scripts

| Action | Implementation |
|--------|---------------|
| `list` | Lists `.ps1` files in shortcuts dir + `Get-ScheduledTask -TaskPath "\Nebo\"` |
| `run` | `powershell -File NAME.ps1` or `Start-ScheduledTask -TaskName NAME -TaskPath "\Nebo\"` |
| `create` | Writes `.ps1` script. If `schedule` is set, creates scheduled task via `Register-ScheduledTask`. |
| `delete` | Removes `.ps1` file + `Unregister-ScheduledTask -TaskName NAME -TaskPath "\Nebo\"` |

**`parseSchedule(schedule)`:** Natural language to Task Scheduler trigger:

| Input | Trigger |
|-------|---------|
| `daily HH:MM` | `New-ScheduledTaskTrigger -Daily -At "HH:MM"` |
| `hourly` | Repetition interval 1 hour |
| `weekly DAY HH:MM` | `-Weekly -DaysOfWeek DAY -At "HH:MM"` |
| `monthly DAY HH:MM` | Monthly trigger |
| `startup` | `-AtStartup` |
| `logon` | `-AtLogOn` |

---

### 2.8 Screenshot (ScreenshotTool)

**STRAP path:** Standalone tool `screenshot(action: ...)`
**RequiresApproval:** `false`

**File:** `screenshot.go`

**Actions:** `capture`, `see`

**Input fields:**
- `action` (string) - "capture" (default) or "see"
- `display` (int) - Display number (0=primary, -1=all)
- `output` (string) - File path for saving
- `format` (string) - "file" (default), "base64", "both"
- `app` (string) - App name for window-level capture (see action)
- `window` (string) - "frontmost" (default) or 1-based index
- `snapshot_id` (string) - Retrieve previous snapshot by ID

#### capture action

1. Uses `github.com/kbinani/screenshot` library (cross-platform)
2. Gets display bounds via `screenshot.GetDisplayBounds(displayNum)`
3. For display=-1: unions all display bounds and captures combined rect
4. Saves to `<data_dir>/files/` for web UI serving via `/api/v1/files/<name>`
5. Returns `ImageURL` field for web UI display
6. If format="base64" or "both", also encodes as `data:image/png;base64,...`

#### see action

1. If `snapshot_id` set, retrieves from `GetSnapshotStore().Get(id)` -- returns cached snapshot
2. If `app` set, uses platform-specific `CaptureAppWindow(ctx, app, windowIndex)`
3. If no app, uses `screenshot.CaptureRect()` for full display
4. Gets accessibility tree via `getUITreeWithBounds(ctx, app, windowBounds)` (platform-specific)
5. Assigns element IDs via `AssignElementIDs(rawElements)`
6. Renders annotations via `RenderAnnotations(capturedImg, elements)`
7. Stores snapshot in `GetSnapshotStore().Put(snap)` with ID `snap-YYYYMMDD-HHMMSS`
8. Saves annotated image to `<data_dir>/files/`
9. Returns element list text + `ImageURL` for annotated image

**Output format (see):**
```
Snapshot snap-20260304-143025 captured for Finder
Saved to: /path/to/screenshot_see_20260304_143025.png

Interactive elements (12):
  B1  button   "Save"        (120, 45) 80x32
  T2  textfield "Search"     (200, 10) 150x28
  L3  link     "Help"        (300, 45) 40x20
  ...

Use desktop(action: "click", element: "B1") to interact with elements.
```

---

### 2.9 Text-to-Speech (TTSTool)

**STRAP path:** `desktop(resource: "tts", action: ...)` (registered as desktop domain resource)
**RequiresApproval:** `false`

**File:** `tts.go`

**Input:** `text` (string, required)

| Platform | Implementation |
|----------|---------------|
| macOS | `say TEXT` |
| Linux | `espeak TEXT` |
| Windows | PowerShell: `Add-Type -AssemblyName System.Speech; (New-Object System.Speech.Synthesis.SpeechSynthesizer).Speak('TEXT')` |

**Output:** `"Spoke: <text>"`

---

## 3. System Domain Tool

**File:** `system_tool.go`

**Core resources (always present):** `file` (FileTool), `shell` (ShellTool)

**Platform resources (registered via init()):** `app`, `clipboard`, `settings`, `music`, `search`, `keychain`

**Message resources (registered via init()):** `notify`

**Registration files:**
- `system_domain_darwin.go` - Registers all 6 platform resources + notify
- `system_domain_linux.go` - Same registrations
- `system_domain_windows.go` - Same registrations

**`inferResource(action)`:** Maps unique actions to resources when resource field is omitted:

| Actions | Inferred Resource |
|---------|------------------|
| `read`, `write`, `edit`, `glob`, `grep` | `file` |
| `exec`, `bg`, `kill`, `ps`, `send_input` | `shell` (maps to bash resource) |
| `copy`, `paste`, `clear` | `clipboard` |
| `send`, `alert`, `speak`, `dnd_status` | `notify` |
| `play`, `pause`, `next`, `previous`, `queue` | `music` |
| `volume`, `brightness`, `sleep`, `lock`, `wifi`, `bluetooth`, `darkmode`, `mute`, `unmute` | `settings` |
| `launch`, `quit`, `activate`, `frontmost`, `hide` | `app` |
| `search` | `search` |
| `get_secret`, `set_secret`, `delete_secret` | `keychain` |

---

### 3.1 Settings (SettingsTool)

**STRAP path:** `system(resource: "settings", action: ...)`
**RequiresApproval:** `false`

**Actions:** `volume`, `mute`, `unmute`, `brightness`, `sleep`, `lock`, `wifi`, `bluetooth`, `darkmode`, `info`

**Input fields:**
- `action` (string) - Required
- `value` (int) - For volume/brightness (0-100)
- `enable` (*bool) - For wifi/bluetooth/darkmode toggle. If nil, returns current status.

#### macOS (`system_darwin.go`)

| Action | Implementation |
|--------|---------------|
| `volume` | AppleScript: `set volume output volume N` (0-100) |
| `mute` | AppleScript: `set volume with output muted` |
| `unmute` | AppleScript: `set volume without output muted` |
| `brightness` | CLI: `brightness 0.N` (requires `brew install brightness`) |
| `sleep` | `pmset sleepnow` |
| `lock` | `pmset displaysleepnow` |
| `wifi` (get) | `networksetup -getairportpower en0` |
| `wifi` (set) | `networksetup -setairportpower en0 on/off` |
| `bluetooth` (get) | `blueutil -p` (requires `brew install blueutil`). Returns "1" for on. |
| `bluetooth` (set) | `blueutil -p 1/0` |
| `darkmode` (get) | AppleScript: `tell appearance preferences` + `dark mode` boolean check |
| `darkmode` (set) | AppleScript: `set dark mode to true/false` |
| `info` | AppleScript shell commands: `sysctl -n machdep.cpu.brand_string`, `sysctl -n hw.memsize`, `sw_vers -productVersion`, `hostname`, `uptime` |

**Helper:** `runOsascript(script)` runs `osascript -e script` and returns trimmed output.

#### Linux (`system_linux.go`)

| Action | Primary Backend | Fallback |
|--------|----------------|----------|
| `volume` | `pactl set-sink-volume @DEFAULT_SINK@ N%` (PulseAudio) | `amixer set Master N%` (ALSA) |
| `mute` | `pactl set-sink-mute @DEFAULT_SINK@ 1` | `amixer set Master mute` |
| `unmute` | `pactl set-sink-mute @DEFAULT_SINK@ 0` | `amixer set Master unmute` |
| `brightness` | `brightnessctl set N%` | `xbacklight -set N` |
| `sleep` | `systemctl suspend` | `pm-suspend` |
| `lock` | Tries in order: `loginctl lock-session`, `xdg-screensaver lock`, `gnome-screensaver-command -l`, `xflock4`, `i3lock`, `slock` |
| `wifi` (get) | `nmcli -t -f WIFI radio` + active connection info | `iwctl station wlan0 show` |
| `wifi` (set) | `nmcli radio wifi on/off` | `rfkill block/unblock wifi` |
| `bluetooth` (get) | `bluetoothctl show` (checks "Powered: yes") | `rfkill list bluetooth` |
| `bluetooth` (set) | `bluetoothctl power on/off` | `rfkill block/unblock bluetooth` |
| `info` | `hostname`, `/etc/os-release PRETTY_NAME`, `uname -r`, `/proc/cpuinfo model name`, `free -h`, `uptime -p` |

**No `darkmode` action on Linux.**

#### Windows (`system_windows.go`)

| Action | Implementation |
|--------|---------------|
| `volume` | PowerShell: `WScript.Shell.SendKeys` -- volume down 50 times then volume up (level/2) times. Approximate. |
| `mute`/`unmute` | `WScript.Shell.SendKeys([char]173)` -- toggles mute (same key for both) |
| `brightness` | `(Get-WmiObject -Namespace root/WMI -Class WmiMonitorBrightnessMethods).WmiSetBrightness(1, N)`. May not work on desktop monitors. |
| `sleep` | `rundll32.exe powrprof.dll,SetSuspendState 0,1,0` |
| `lock` | `rundll32.exe user32.dll,LockWorkStation` |
| `wifi` (get) | `netsh wlan show interfaces` |
| `wifi` (set) | `Get-NetAdapter -Physical` (filters Wireless/Wi-Fi/WiFi) + `Enable-NetAdapter`/`Disable-NetAdapter` |
| `darkmode` (get) | Registry: `HKCU:\...\Themes\Personalize\AppsUseLightTheme` (0 = dark mode) |
| `darkmode` (set) | Sets both `AppsUseLightTheme` and `SystemUsesLightTheme` registry values |
| `info` | `Get-WmiObject Win32_OperatingSystem/Win32_Processor`, `$env:COMPUTERNAME`, `Get-CimInstance` for uptime |

**No `bluetooth` action on Windows.**

---

### 3.2 OS App Control (OSAppTool)

**STRAP path:** `system(resource: "app", action: ...)`
**RequiresApproval:** `false`

**Actions:** `list`, `launch`, `quit`, `quit_all`, `activate`, `hide`, `info`, `frontmost`

**Input fields:**
- `id` (string) - App name or process name
- `force` (bool) - Force quit

#### macOS (`app_darwin.go`)

**Backend:** AppleScript + `open` CLI

| Action | Implementation |
|--------|---------------|
| `list` | AppleScript: `name of every process whose background only is false` |
| `launch` | `open -a APP` via exec.Command |
| `quit` | AppleScript: `tell application APP to quit`. If force=true, gets `unix id` and runs `kill -9`. |
| `quit_all` | Not implemented separately -- iterates apps. |
| `activate` | AppleScript: `tell application APP to activate` |
| `hide` | AppleScript: `set visible of process APP to false` |
| `info` | AppleScript: Gets `name`, path (`POSIX path of (path to application APP)`), `id` (unix PID), `frontmost`, `visible`, and `count of windows` |
| `frontmost` | AppleScript: `name of first application process whose frontmost is true` |

#### Linux (`app_linux.go`)

**Backends:** wmctrl, xdotool, ps

| Action | Implementation |
|--------|---------------|
| `list` | `wmctrl -l -p` or `ps -eo pid,comm --no-headers` |
| `launch` | `setsid APP &` (preferred) or `nohup APP &` |
| `quit` | `pkill -TERM APP`. If force=true: `pkill -KILL APP` |
| `quit_all` | Iterates running apps with `quit` |
| `activate` | `wmctrl -a APP` or `xdotool search --name APP windowactivate` |
| `hide` | `xdotool search --name APP windowminimize` |
| `info` | `ps -eo pid,ppid,user,%cpu,%mem,stat,start,time,comm --no-headers` filtered by app |
| `frontmost` | `xdotool getactivewindow getwindowname` |

#### Windows (`app_windows.go`)

**Backend:** PowerShell

| Action | Implementation |
|--------|---------------|
| `list` | `Get-Process` with `MainWindowTitle -ne ""`. Shows Name, PID, MainWindowTitle. |
| `launch` | `cmd /C start "" "APP"` |
| `quit` | `taskkill /IM APP.exe`. Force: `taskkill /IM APP.exe /F` |
| `quit_all` | Lists apps then quits each |
| `activate` | user32.dll: `ShowWindow(hwnd, 9)` + `SetForegroundWindow(hwnd)` |
| `hide` | `ShowWindow(hwnd, 0)` (SW_HIDE) |
| `info` | `Get-Process -Name APP`: Id, Name, MainWindowTitle, Path, CPU, WorkingSet64, StartTime |
| `frontmost` | user32.dll: `GetForegroundWindow()` + `GetWindowText()` |

---

### 3.3 File Search / Spotlight (SpotlightTool)

**STRAP path:** `system(resource: "search", action: "search")`
**RequiresApproval:** `false`

**Input fields:**
- `query` (string) - Search query
- `kind` (string) - File type filter
- `scope` (string) - Directory scope
- `limit` (int) - Max results (default: 20)
- `name_only` (bool) - Search file names only

#### macOS (`spotlight_darwin.go`)

**Backend:** `mdfind` (Spotlight)

**Kind filter mapping (`kindMap`):**
- `app`/`application` -> `kMDItemContentType == "com.apple.application-bundle"`
- `document`/`doc` -> `kMDItemKind == "Document"`
- `image`/`photo`/`picture` -> `kMDItemContentTypeTree == "public.image"`
- `video`/`movie` -> `kMDItemContentTypeTree == "public.movie"`
- `audio`/`music`/`sound` -> `kMDItemContentTypeTree == "public.audio"`
- `folder`/`directory` -> `kMDItemContentType == "public.folder"`
- `pdf` -> `kMDItemContentType == "com.adobe.pdf"`
- `presentation`/`slides` -> `kMDItemKind == "Presentation"`
- `spreadsheet`/`excel` -> `kMDItemKind == "Spreadsheet"`

**Command construction:**
- Base: `mdfind`
- If `scope` set: `-onlyin SCOPE`
- If `kind` set: appends kMDItem predicate + `" && "` prefix to query
- If `name_only`: `kMDItemDisplayName == "*QUERY*"wc` (Spotlight wildcard)
- `-0` flag for null-separated output

#### Linux (`spotlight_linux.go`)

**Backends:** `plocate` (preferred) -> `locate` -> `find` (fallback)

| Backend | Command | Options |
|---------|---------|---------|
| `plocate` | `plocate -l LIMIT -i QUERY` | Case-insensitive, limit results |
| `locate` | `locate -l LIMIT -i QUERY` | Same flags |
| `find` | `find SCOPE -maxdepth 10 -iname "*QUERY*"` | Case-insensitive, max depth 10 |

**Kind filtering (`getExtensionsForKind`):**
- `app`/`application` -> `.desktop`, `.AppImage`, `.snap`, `.flatpak`
- `document`/`doc` -> `.doc`, `.docx`, `.odt`, `.txt`, `.rtf`
- `image`/`photo` -> `.png`, `.jpg`, `.jpeg`, `.gif`, `.bmp`, `.svg`, `.webp`
- `video`/`movie` -> `.mp4`, `.mkv`, `.avi`, `.mov`, `.webm`, `.flv`
- `audio`/`music` -> `.mp3`, `.flac`, `.ogg`, `.wav`, `.m4a`, `.aac`
- `pdf` -> `.pdf`

Also includes `launchApp()` helper: `gtk-launch APP` or `xdg-open /usr/share/applications/APP.desktop`.

#### Windows (`spotlight_windows.go`)

**Backends:** Everything (`es.exe`, preferred) -> PowerShell Get-ChildItem

**Everything detection:** Checks 3 install paths:
1. `C:\Program Files\Everything\es.exe`
2. `C:\Program Files (x86)\Everything\es.exe`
3. `LOCALAPPDATA\Everything\es.exe`

| Backend | Implementation |
|---------|---------------|
| Everything | `es.exe -n LIMIT QUERY`. Kind: `ext:EXTENSIONS`. Scope: `folder:SCOPE`. |
| PowerShell | `Get-ChildItem -Path SCOPE -Recurse -Filter "*QUERY*"` with `Where-Object` for extension filtering |

---

### 3.4 Clipboard (ClipboardTool)

**STRAP path:** `system(resource: "clipboard", action: ...)`
**RequiresApproval:** `false`

**Actions:** `copy`, `paste`, `clear`

**Input fields:**
- `text` (string) - Text for copy action

#### macOS (`clipboard_darwin.go`)

| Action | Command |
|--------|---------|
| `copy` | `echo TEXT | pbcopy` via exec.Command |
| `paste` | `pbpaste` |
| `clear` | `echo "" | pbcopy` |

#### Linux (`clipboard_linux.go`)

**Backends:** `xclip` (X11) or `wl-copy`/`wl-paste` (Wayland)

| Action | xclip | wl-clipboard |
|--------|-------|-------------|
| `copy` | pipe to `xclip -selection clipboard` | pipe to `wl-copy` |
| `paste` | `xclip -selection clipboard -o` | `wl-paste` |
| `clear` | empty string to `xclip -selection clipboard` | empty string to `wl-copy` |

**Detection:** Checks for `wl-copy` first (Wayland preference), falls back to `xclip`.

#### Windows (`clipboard_windows.go`)

| Action | PowerShell |
|--------|-----------|
| `copy` | `Set-Clipboard -Value "TEXT"` |
| `paste` | `Get-Clipboard` |
| `clear` | `Set-Clipboard -Value ""` |

---

### 3.5 Music (MusicTool)

**STRAP path:** `system(resource: "music", action: ...)`
**RequiresApproval:** `false`

**Actions:** `play`, `pause`, `next`, `previous`, `now_playing`, `queue`, `search`, `volume`

**Input fields:**
- `query` (string) - Search query
- `value` (int) - Volume level (0-100)

#### macOS (`music_darwin.go`)

**Backend:** AppleScript targeting Music app (formerly iTunes)

| Action | AppleScript |
|--------|-------------|
| `play` | `tell application "Music" to play` |
| `pause` | `tell application "Music" to pause` |
| `next` | `tell application "Music" to next track` |
| `previous` | `tell application "Music" to previous track` |
| `now_playing` | Gets `name`, `artist`, `album`, `duration` of `current track` |
| `volume` | `set sound volume to N` |
| `search` | `search playlist "Library" for QUERY` -- returns first 10 tracks |

#### Linux (`music_linux.go`)

**Backends:** `playerctl` (MPRIS, preferred) or `mpc` (MPD)

| Action | playerctl | mpc |
|--------|-----------|-----|
| `play` | `playerctl play` | `mpc play` |
| `pause` | `playerctl pause` | `mpc pause` |
| `next` | `playerctl next` | `mpc next` |
| `previous` | `playerctl previous` | `mpc prev` |
| `now_playing` | `playerctl metadata --format "..."` (title, artist, album, status) | `mpc current` |
| `volume` | `playerctl volume 0.N` (0.0-1.0 scale) | `mpc volume N` |
| `search` | Not supported via playerctl | `mpc search any QUERY` |

#### Windows (`music_windows.go`)

**Backend:** PowerShell with Windows Media Player COM or Spotify detection

| Action | Implementation |
|--------|---------------|
| `play` | `$wshell.SendKeys([char]179)` (media play/pause key) |
| `pause` | Same key (toggle) |
| `next` | `$wshell.SendKeys([char]176)` (media next key) |
| `previous` | `$wshell.SendKeys([char]177)` (media previous key) |
| `now_playing` | Tries Windows Media Player COM, falls back to Spotify process detection |
| `volume` | Not directly controllable per-app; uses system volume |

---

### 3.6 Keychain (KeychainTool)

**STRAP path:** `system(resource: "keychain", action: ...)`
**RequiresApproval:** `true`

**Actions:** `get_secret`, `set_secret`, `delete_secret`, `list`

**Input fields:**
- `key` (string) - Secret name/key
- `value` (string) - Secret value (for set)
- `service` (string) - Service name (default: "nebo")

#### macOS (`keychain_darwin.go`)

**Backend:** `security` CLI (macOS Keychain)

| Action | Command |
|--------|---------|
| `get_secret` | `security find-generic-password -s SERVICE -a KEY -w` |
| `set_secret` | `security add-generic-password -s SERVICE -a KEY -w VALUE -U` (-U = update if exists) |
| `delete_secret` | `security delete-generic-password -s SERVICE -a KEY` |
| `list` | `security dump-keychain` + parse for service=SERVICE entries |

#### Linux (`keychain_linux.go`)

**Backends:** `secret-tool` (GNOME Keyring, preferred) or `pass` (password-store)

| Action | secret-tool | pass |
|--------|-------------|------|
| `get_secret` | `secret-tool lookup service SERVICE key KEY` | `pass show SERVICE/KEY` |
| `set_secret` | pipe VALUE to `secret-tool store --label=KEY service SERVICE key KEY` | `pass insert -f SERVICE/KEY` |
| `delete_secret` | `secret-tool clear service SERVICE key KEY` | `pass rm -f SERVICE/KEY` |
| `list` | `secret-tool search service SERVICE` | `pass ls SERVICE/` |

#### Windows (`keychain_windows.go`)

**Backend:** Windows Credential Manager via PowerShell

| Action | PowerShell |
|--------|-----------|
| `get_secret` | `cmdkey /list:SERVICE:KEY` or `(New-Object Net.NetworkCredential).Password` via `CredRead` |
| `set_secret` | `cmdkey /generic:SERVICE:KEY /user:KEY /pass:VALUE` |
| `delete_secret` | `cmdkey /delete:SERVICE:KEY` |
| `list` | `cmdkey /list:SERVICE:*` |

---

### 3.7 Notification (NotificationTool)

**STRAP path:** `system(resource: "notify", action: ...)`
**RequiresApproval:** `false`

**Actions:** `send`, `alert`, `speak`, `dnd_status`

**Input fields:**
- `title` (string) - Notification title
- `message` (string) - Notification body
- `subtitle` (string) - macOS subtitle
- `sound` (string) - Sound name
- `voice` (string) - Voice name for speak
- `urgency` (string) - Linux urgency: low/normal/critical

#### macOS (`notification_darwin.go`)

| Action | Implementation |
|--------|---------------|
| `send` | `display notification "MSG" with title "TITLE" subtitle "SUB" sound name "SOUND"` via osascript |
| `alert` | `display alert "TITLE" message "MSG"` via osascript |
| `speak` | `say "MSG"` or `say -v VOICE "MSG"` |
| `dnd_status` | `defaults read com.apple.notificationcenterui doNotDisturb` -- returns 1/0 |

#### Linux (`notification_linux.go`)

| Action | Primary | Fallback |
|--------|---------|----------|
| `send` | `notify-send "TITLE" "MSG" -u URGENCY -i ICON` | N/A |
| `alert` | `zenity --info --title="TITLE" --text="MSG"` (GTK) | `kdialog --msgbox "MSG" --title "TITLE"` (KDE) -> `notify-send -u critical` |
| `speak` | `spd-say "MSG"` (speech-dispatcher) | `espeak "MSG"` |
| `dnd_status` | `gsettings get org.gnome.desktop.notifications show-banners` (GNOME) | `qdbus org.freedesktop.Notifications ... Inhibited` (KDE) |

#### Windows (`notification_windows.go`)

| Action | Implementation |
|--------|---------------|
| `send` | WinRT `ToastNotificationManager` with XML template (`ToastGeneric`). Fallback: `BurntToast` module (`New-BurntToastNotification`). |
| `alert` | `[System.Windows.Forms.MessageBox]::Show("MSG", "TITLE")` |
| `speak` | `System.Speech.Synthesis.SpeechSynthesizer` with optional `SelectVoice("VOICE")` |
| `dnd_status` | Not implemented on Windows |

**Toast XML template:**
```xml
<toast><visual><binding template='ToastGeneric'>
  <text>TITLE</text>
  <text>MSG</text>
</binding></visual></toast>
```

---

## 4. PIM (Organizer) Domain Tool

The PIM (Personal Information Manager) / Organizer domain consolidates mail, contacts, calendar, and reminders into one STRAP tool. On macOS, it also includes messages (iMessage/SMS).

### 4.1 Contacts (ContactsTool)

**STRAP path:** `organizer(resource: "contacts", action: ...)`
**RequiresApproval:** `false`

**Actions:** `list`, `search`, `get`, `add`, `update`, `delete`

#### macOS (`contacts_darwin.go`)

**Backend:** AppleScript targeting Contacts app

| Action | AppleScript |
|--------|-------------|
| `list` | `name of every person` |
| `search` | `every person whose name contains QUERY` or `email contains QUERY` |
| `get` | `properties of person id ID` -- returns name, email, phone, address, note |
| `add` | `make new person with properties {first name:F, last name:L}` + email/phone sub-properties |
| `update` | Modifies existing person's properties |
| `delete` | `delete person id ID` |

#### Linux (`contacts_linux.go`)

**Backends:** `khard` (CardDAV, preferred) or `abook` (terminal addressbook)

| Action | khard | abook |
|--------|-------|-------|
| `list` | `khard list` | `abook --datafile ~/.abook/addressbook --mutt-query ""` |
| `search` | `khard list QUERY` | `abook --mutt-query QUERY` |
| `get` | `khard show UID` | Parse abook file for matching entry |
| `add` | Writes YAML to stdin of `khard new` | `abook --add-email` or direct file manipulation |
| `delete` | `khard remove UID` | Not directly supported |

#### Windows (`contacts_windows.go`)

**Backend:** PowerShell + Outlook COM automation

| Action | Implementation |
|--------|---------------|
| `list` | `$outlook.GetNamespace("MAPI").GetDefaultFolder(10).Items` (10 = olFolderContacts) |
| `search` | `.Items.Restrict("[FullName] LIKE '%QUERY%'")` |
| `get` | Direct property access: FullName, Email1Address, BusinessTelephoneNumber, etc. |
| `add` | `$folder.Items.Add()` + set properties + `.Save()` |
| `delete` | `.Delete()` on the contact item |

---

### 4.2 Calendar (CalendarTool)

**STRAP path:** `organizer(resource: "calendar", action: ...)`
**RequiresApproval:** `false`

**Actions:** `list`, `today`, `create`, `delete`, `upcoming`

#### macOS (`calendar_darwin.go`)

**Backend:** AppleScript targeting Calendar app

| Action | AppleScript |
|--------|-------------|
| `list` | `name of every calendar` |
| `today` | `every event of calendar 1 whose start date >= (current date)` and filtered by today |
| `upcoming` | Events in the next 7 days |
| `create` | `make new event at end of events of calendar CAL with properties {summary:TITLE, start date:START, end date:END}` |
| `delete` | `delete event id ID of calendar CAL` |

#### Linux (`calendar_linux.go`)

**Backends:** `khal` (CalDAV, preferred) or `gcalcli` (Google Calendar)

| Action | khal | gcalcli |
|--------|------|---------|
| `list` | `khal printcalendars` | `gcalcli list` |
| `today` | `khal list today today` | `gcalcli agenda today tomorrow` |
| `upcoming` | `khal list today 7d` | `gcalcli agenda` (default 7 days) |
| `create` | `khal new START END TITLE` | `gcalcli add --title TITLE --when START --duration DURATION` |
| `delete` | Not directly supported via CLI | `gcalcli delete --title TITLE` |

#### Windows (`calendar_windows.go`)

**Backend:** PowerShell + Outlook COM automation

| Action | Implementation |
|--------|---------------|
| `list` | `$outlook.GetNamespace("MAPI").Folders` -- iterates calendar folders |
| `today` | `.Items.Restrict("[Start] >= 'TODAY' AND [Start] < 'TOMORROW'")` |
| `upcoming` | `.Items.Restrict("[Start] >= 'TODAY' AND [Start] < 'NEXTWEEK'")` |
| `create` | `$folder.Items.Add(1)` (olAppointmentItem) + set Subject, Start, End + `.Save()` |
| `delete` | Find matching item and `.Delete()` |

---

### 4.3 Mail (MailTool)

**STRAP path:** `organizer(resource: "mail", action: ...)`
**RequiresApproval:** Depends on action (`send` requires approval)

**Actions:** `inbox`, `read`, `send`, `search`, `folders`

#### macOS (`mail_darwin.go`)

**Backend:** AppleScript targeting Mail app

| Action | AppleScript |
|--------|-------------|
| `inbox` | `messages of inbox` -- gets subject, sender, date of first N messages |
| `read` | `content of message id ID` |
| `send` | `make new outgoing message with properties {subject:S, content:C, visible:true}` + `make new to recipient with properties {address:ADDR}` + `send` |
| `search` | `messages of inbox whose subject contains QUERY or sender contains QUERY` |
| `folders` | `name of every mailbox` |

#### Linux (`mail_linux.go`)

**Backends:** `notmuch` (preferred) or `mutt`

| Action | notmuch | mutt |
|--------|---------|------|
| `inbox` | `notmuch search --format=json tag:inbox` | Parse mutt mailbox output |
| `read` | `notmuch show --format=json id:ID` | `mutt -f FOLDER -e 'push <display-message>'` |
| `send` | `sendmail` or `msmtp` | `mutt -s SUBJECT -a ATTACHMENT -- ADDR < BODY` |
| `search` | `notmuch search --format=json QUERY` | `notmuch` preferred for search |
| `folders` | `notmuch search --output=tags '*'` | N/A |

#### Windows (`mail_windows.go`)

**Backend:** PowerShell + Outlook COM automation

| Action | Implementation |
|--------|---------------|
| `inbox` | `$outlook.GetNamespace("MAPI").GetDefaultFolder(6).Items` (6 = olFolderInbox) |
| `read` | `.Items.Find("[EntryID] = 'ID'")` + `.Body` |
| `send` | `$outlook.CreateItem(0)` (olMailItem) + `.Subject`, `.Body`, `.To` + `.Send()` |
| `search` | `.Items.Restrict("[Subject] LIKE '%QUERY%' OR [SenderName] LIKE '%QUERY%'")` |
| `folders` | `$ns.Folders` recursive listing |

---

### 4.4 Reminders (RemindersTool)

**STRAP path:** `organizer(resource: "reminders", action: ...)`
**RequiresApproval:** `false`

**Actions:** `list`, `add`, `complete`, `delete`

#### macOS (`reminders_darwin.go`)

**Backend:** AppleScript targeting Reminders app

| Action | AppleScript |
|--------|-------------|
| `list` | `name of every reminder whose completed is false` of default list |
| `add` | `make new reminder with properties {name:TITLE, body:NOTES}` optionally with `due date` |
| `complete` | `set completed of reminder NAME to true` |
| `delete` | `delete reminder NAME` |

#### Linux (`reminders_linux.go`)

**Backend:** SQLite database at `<dataDir>/reminders.db`

Self-contained reminder storage since Linux has no native reminders API. Table schema:
```sql
CREATE TABLE reminders (id, title, notes, due_date, completed, created_at)
```

| Action | Implementation |
|--------|---------------|
| `list` | `SELECT * FROM reminders WHERE completed=0` |
| `add` | `INSERT INTO reminders (id, title, notes, due_date, completed, created_at)` |
| `complete` | `UPDATE reminders SET completed=1 WHERE title LIKE '%NAME%'` |
| `delete` | `DELETE FROM reminders WHERE title LIKE '%NAME%'` |

#### Windows (`reminders_windows.go`)

**Backend:** Same SQLite approach as Linux (no native Windows reminders API exposed via PowerShell/COM)

---

### 4.5 Messages (MessagesTool)

**STRAP path:** `organizer(resource: "sms", action: ...)`
**RequiresApproval:** `true` (for send)
**Platform:** macOS only

#### macOS (`messages_darwin.go`)

**Backend:** SQLite database (`~/Library/Messages/chat.db`) for reading + AppleScript for sending

| Action | Implementation |
|--------|---------------|
| `list` | SQLite query on `chat.db`: joins `message`, `handle`, `chat`, `chat_message_join`, `chat_handle_join` tables. Gets last N messages with sender, text, date. |
| `read` | Same SQLite query filtered by chat/handle ID |
| `send` | AppleScript: `tell application "Messages"` + `set targetService to 1st account whose service type = iMessage` + `set targetBuddy to participant PHONE of targetService` + `send MSG to targetBuddy` |
| `search` | SQLite: `WHERE message.text LIKE '%QUERY%'` |

**Date conversion:** macOS Messages uses "Apple Cocoa epoch" (2001-01-01) with nanosecond timestamps. Converts via `datetime(message.date/1000000000 + 978307200, 'unixepoch', 'localtime')`.

---

## 5. Standalone Tools

### 5.1 Vision (VisionTool)

**File:** `vision.go`
**RequiresApproval:** `false`

**Input fields:**
- `image` (string, required) - File path, URL, or `data:image/...;base64,...`
- `prompt` (string, default: "Describe this image in detail.")

**Image loading (`loadImage`):**

| Source Type | Handling |
|-------------|----------|
| `data:image/...` URI | Splits on `,`, extracts media type and base64 data |
| `http://` or `https://` | HTTP GET, reads body, base64 encodes. Media type from Content-Type header or file extension. Fallback: `image/jpeg`. |
| File path | Reads file. Supports `~/` expansion. Media type from extension. |

**Supported formats:** `.png` (image/png), `.jpg`/`.jpeg` (image/jpeg), `.gif` (image/gif), `.webp` (image/webp)

**Provider routing:** `AnalyzeFunc` callback, set via `SetAnalyzeFunc()` after provider loading. Receives base64 data, media type, and prompt. Returns analysis text.

---

### 5.2 Accessibility (AccessibilityTool)

**STRAP path:** `desktop(resource: "accessibility", action: ...)`
**RequiresApproval:** `false`

**Actions:** `read`, `elements`, `tree`

#### macOS (`accessibility_darwin.go`)

**Backend:** AppleScript via System Events

| Action | Implementation |
|--------|-------------|
| `read` | Gets focused element's `AXValue`, `AXTitle`, `AXDescription`, and `AXRoleDescription` |
| `elements` | Lists UI elements of the frontmost window with roles, titles, and values |
| `tree` | Recursive accessibility tree dump (limited depth) |

#### Linux (`accessibility_linux.go`)

**Backend:** AT-SPI via Python3/gi (same infrastructure as `getUITreeWithBounds`)

#### Windows (`accessibility_windows.go`)

**Backend:** UI Automation API via PowerShell (same infrastructure as `getUITreeWithBounds`)

---

### 5.3 NeboLoop App Store (AppTool)

**File:** `app_tool.go`
**RequiresApproval:** `false`

This is NOT the OS app control tool -- it manages Nebo's own app ecosystem via the NeboLoop store.

**Actions:** `list`, `launch`, `stop`, `settings`, `browse`, `install`, `uninstall`

**Input fields:**
- `id` (string) - App ID (required for launch, stop, install, uninstall)
- `query` (string) - Search query (for browse)
- `category` (string) - Category filter: "featured", "popular", or custom
- `page` (int), `page_size` (int) - Pagination

**Interfaces:**
- `AppManager` - Local app lifecycle: `ListInstalled()`, `LaunchApp()`, `StopApp()`
- `AppInstaller` - Downloads and installs: `InstallFromURL(ctx, downloadURL)`
- `NeboLoopClientProvider` - Creates API client for store operations

| Action | Implementation |
|--------|---------------|
| `list` | `appManager.ListInstalled()` -- returns ID, name, version, status (running/stopped/error) |
| `launch` | `appManager.LaunchApp(ctx, id)` |
| `stop` | `appManager.StopApp(ctx, id)` |
| `settings` | Not yet implemented |
| `browse` | `client.ListApps(ctx, query, category, page, pageSize)`. If `id` set, uses `client.GetApp(ctx, id)` for details. Category shortcuts: "featured", "popular". |
| `install` | `client.InstallApp(ctx, id)` (registers with NeboLoop) + `appInstaller.InstallFromURL(ctx, downloadURL)` (downloads binary). Fallback download URL: `/api/v1/apps/{id}/download?version=V`. |
| `uninstall` | `client.UninstallApp(ctx, id)` |

---

## 6. Snapshot Pipeline

The snapshot pipeline enables the "see + interact" workflow: capture a screenshot, overlay numbered element IDs on interactive UI elements, and allow the agent to interact by element ID.

### 6.1 SnapshotStore

**File:** `snapshot_store.go`

**Types:**

```go
type Rect struct {
    X, Y, Width, Height int
}

type Element struct {
    ID       string // e.g. "B1", "T2", "L3"
    Role     string // e.g. "button", "textfield", "link"
    Label    string // Human-readable label
    Bounds   Rect
    Value    string
    Actionable bool
}

type Snapshot struct {
    ID           string
    CreatedAt    time.Time
    App          string
    WindowTitle  string
    RawPNG       []byte
    AnnotatedPNG []byte
    Elements     map[string]*Element
    ElementOrder []string
}
```

**`Rect.Center()`:** Returns `(X + Width/2, Y + Height/2)` -- used by desktop tool for element clicks.

**SnapshotStore:**
- **Singleton:** `GetSnapshotStore()` with `sync.Once`
- **Storage:** `map[string]*Snapshot` protected by `sync.RWMutex`
- **TTL:** 1 hour. Cleanup runs every 5 minutes via background goroutine.
- **`Put(snap)`:** Stores snapshot by ID
- **`Get(id)`:** Returns snapshot or nil if expired/missing
- **`LookupElement(snapshotID, elementID)`:** If `snapshotID` is empty, searches the most recent snapshot. Returns `*Element` or nil.

---

### 6.2 Element Annotation (AssignElementIDs)

**File:** `snapshot_annotator.go`

**RawElement struct:**
```go
type RawElement struct {
    Role        string
    Title       string
    Description string
    Value       string
    Position    Rect
    Actionable  bool
}
```

**Role prefix map (`rolePrefixMap`):**

| Role | Prefix | Role | Prefix |
|------|--------|------|--------|
| button | B | textfield | T |
| link | L | checkbox | C |
| menu | M | slider | S |
| tab | A | radio | R |
| popup | P | image | G |
| static text | X | toolbar | O |
| list | I | table | W |
| scrollbar | Z | group | U |
| window | N | combobox | P |
| menu item | M | | |

**`AssignElementIDs(raw []RawElement) []*Element`:**

1. **Filter:** Keep only elements with `Actionable == true` and `Position.Width > 0 && Position.Height > 0`
2. **Sort by position:** Primary sort by Y coordinate (grouped into 10px vertical bands to handle slight misalignments), secondary sort by X coordinate within each band
3. **Assign IDs:** Each element gets `PREFIX + counter` where prefix comes from `rolePrefixMap` (default "E" for unknown roles). Counter is per-prefix: B1, B2, T1, T2, etc.
4. **Build label:** Prefers `Title`, falls back to `Description`, then `Value`. Truncates to 40 chars.

**`FormatElementList(elements []*Element) string`:**

Formats as human-readable table:
```
Interactive elements (N):
  B1  button   "Save"        (120, 45) 80x32
  T2  textfield "Search"     (200, 10) 150x28
```

---

### 6.3 Rendering (RenderAnnotations)

**File:** `snapshot_renderer.go`

**Library:** `github.com/fogleman/gg` (2D graphics)

**Colors:**
- Overlay: `rgba(0, 120, 255, 0.15)` (semi-transparent blue)
- Border: `rgba(0, 120, 255, 0.6)`
- Pill background: `rgba(30, 30, 30, 0.85)` (dark)
- Pill text: white

**`RenderAnnotations(img image.Image, elements []*Element) (image.Image, error)`:**

1. Creates a `gg.Context` from the source image
2. For each element, calls `drawElementOverlay()`:
   - Draws filled rectangle (overlay color) at element bounds
   - Draws stroke rectangle (border color, 1.5px width)
   - Draws label pill with element ID
3. Returns annotated image

**Pill placement (`drawElementOverlay`):** Tries 4 positions in order:
1. **Above-left:** Pill above the element, left-aligned
2. **Above-right:** Pill above, right-aligned
3. **Below-left:** Pill below, left-aligned
4. **Inside top-left:** Pill inside element at top-left corner (fallback)

The pill is a rounded rectangle containing the element ID text (e.g., "B1") in white on dark background.

---

### 6.4 Window Capture (CaptureAppWindow)

Platform-specific implementations for capturing a specific application window.

#### macOS (`snapshot_capture_darwin.go`)

**Multi-step process:**
1. **Get window info via AppleScript:** Position and size of `window 1` of the target app process
2. **Get CGWindowID via Python3/Quartz:**
   ```python
   from Quartz import CGWindowListCopyWindowInfo, kCGWindowListOptionOnScreenOnly, kCGNullWindowID
   ```
   Iterates window list, matches by `kCGWindowOwnerName` and `kCGWindowBounds`, extracts `kCGWindowNumber`
3. **Capture via screencapture:** `screencapture -l <windowID> -x <tmpFile>` (-l = specific window, -x = no sound)

#### Linux (`snapshot_capture_linux.go`)

**Multi-step process:**
1. **Find window ID:** `xdotool search --name APP` -- returns list of window IDs. Selects by `windowIndex` (1-based, clamped to available range).
2. **Get geometry:** `xdotool getwindowgeometry --shell WINDOW_ID` -- parses `X=`, `Y=`, `WIDTH=`, `HEIGHT=` lines
3. **Capture screenshot** (3 backends tried in order):
   - **ImageMagick:** `import -window WINDOW_ID <tmpFile>`
   - **scrot:** `xdotool windowactivate --sync WINDOW_ID` then `scrot -u <tmpFile>` (focused window)
   - **gnome-screenshot:** `xdotool windowactivate --sync WINDOW_ID` then `gnome-screenshot -w -f <tmpFile>`

**`ListAppWindows(ctx, app)`:** Uses `xdotool search --name APP` + `xdotool getwindowname ID` for each.

#### Windows (`snapshot_capture_windows.go`)

**Single PowerShell script using .NET + P/Invoke:**
1. `Add-Type` with `WindowCapture` class: `GetWindowRect`, `PrintWindow`, `GetForegroundWindow`
2. Find process by exact name (`Get-Process -Name APP`) or window title match (`-like "*APP*"`)
3. `GetWindowRect(hwnd, rect)` for bounds
4. Create `System.Drawing.Bitmap` with window dimensions
5. `PrintWindow(hwnd, hdc, 2)` -- flag 2 = `PW_RENDERFULLCONTENT` (captures off-screen portions)
6. Save bitmap as PNG
7. Output `"Left,Top,Width,Height"` for parsing

**`ListAppWindows(ctx, app)`:** PowerShell `Get-Process` with `ProcessName -like "*APP*"` and `MainWindowTitle -ne ""`.

---

### 6.5 Accessibility Tree (getUITreeWithBounds)

Platform-specific implementations for getting the UI element tree with positions.

#### macOS (`snapshot_accessibility_darwin.go`)

**Backend:** AppleScript recursive traversal

**AppleScript logic:**
1. `getElements(elem, depth, maxD, offsetX, offsetY)` -- recursive function, max depth 5
2. For each element: gets `role`, `title`, `description`, `value`, `position`, `size`, `actions`
3. Only includes elements with `sw > 0 and sh > 0` (valid size)
4. Output format: `role|title|description|value|x|y|w|h|actionable` (pipe-delimited, one line per element)
5. Entry point: iterates windows of the target process

**`normalizeRole(role)`:** Maps AppleScript AX role names to simplified names:
- `AXButton` -> `button`, `AXTextField`/`AXTextArea` -> `textfield`, `AXStaticText` -> `static text`
- `AXCheckBox` -> `checkbox`, `AXRadioButton` -> `radio`, `AXPopUpButton` -> `popup`
- `AXMenuButton`/`AXMenu` -> `menu`, `AXMenuItem` -> `menu item`, `AXSlider` -> `slider`
- `AXTabGroup` -> `tab`, `AXLink` -> `link`, `AXImage` -> `image`, `AXToolbar` -> `toolbar`
- `AXList` -> `list`, `AXTable` -> `table`, `AXScrollBar` -> `scrollbar`, `AXGroup` -> `group`
- `AXWindow` -> `window`, `AXComboBox` -> `combobox`, `AXDisclosureTriangle` -> `button`
- Unknown: strips `AX` prefix and lowercases

#### Linux (`snapshot_accessibility_linux.go`)

**Backend:** AT-SPI via Python3/gi

**Prerequisites check:** `python3 -c "import gi; gi.require_version('Atspi', '2.0')"` -- if fails, returns nil (AT-SPI not available).

**Python script logic:**
1. `get_elements(obj, depth, max_depth)` -- recursive, max depth 5
2. Gets: `role_name`, `name`, `description`, text via `get_text_iface()`, value via `get_value_iface()`, actions via `get_action_iface()`
3. Position/size via `get_component_iface().get_extents(Atspi.CoordType.SCREEN)`
4. Escapes pipes in values (replaces `|` with space)
5. Entry point: `Atspi.get_desktop(0)` -> find app by name (case-insensitive) -> iterate windows

**`parseLinuxElements(output)`:** Same pipe-delimited format as macOS. Does NOT normalize roles (AT-SPI role names are already simple strings like "push button", "text", etc.)

**`escapeAtspyPy(app)`:** Escapes the app name for safe embedding in Python string.

#### Windows (`snapshot_accessibility_windows.go`)

**Backend:** UI Automation API via PowerShell

**PowerShell script logic:**
1. `Get-Elements` function with `$element`, `$depth`, `$maxDepth` params
2. Gets: `ControlType.ProgrammaticName` (strips "ControlType." prefix), `Name`, `BoundingRectangle`
3. Checks `GetSupportedPatterns()` for actionability
4. Gets value via `ValuePattern.Current.Value`
5. Entry point: `AutomationElement.RootElement.FindAll(Children)` -> find window by name (`-like "*APP*"`)

**`normalizeWinRole(role)`:** Maps UI Automation control types to simplified names:
- `Button` -> `button`, `Edit` -> `textfield`, `Text` -> `static text`
- `CheckBox` -> `checkbox`, `RadioButton` -> `radio`, `ComboBox` -> `combobox`
- `MenuItem` -> `menu item`, `Menu` -> `menu`, `Slider` -> `slider`
- `Tab`/`TabItem` -> `tab`, `Hyperlink` -> `link`, `Image` -> `image`
- `ToolBar` -> `toolbar`, `List`/`ListItem` -> `list`, `Table` -> `table`
- `ScrollBar` -> `scrollbar`, `Group` -> `group`, `Window` -> `window`
- Unknown: lowercases

**`parseElementOutput(output)`:** Same pipe-delimited parsing as other platforms.

**`escapeUIAutoPS(app)`:** Escapes the app name for safe embedding in PowerShell strings.

---

## Appendix A: Platform Backend Summary

| Tool | macOS | Linux | Windows |
|------|-------|-------|---------|
| **Desktop Input** | cliclick / AppleScript | xdotool / ydotool | PowerShell + user32.dll |
| **Window Mgmt** | AppleScript System Events | wmctrl / xdotool | PowerShell + user32.dll |
| **Menu Bar** | AppleScript System Events | N/A | PowerShell + UI Automation |
| **Dialog** | AppleScript System Events | N/A | PowerShell + UI Automation |
| **Spaces** | AppleScript key events | N/A | PowerShell + keybd_event |
| **Dock** | defaults + AppleScript | N/A | N/A |
| **Shortcuts** | `shortcuts` CLI | systemd / cron / scripts | Task Scheduler + PowerShell |
| **Screenshot** | screencapture + kbinani | ImageMagick / scrot / gnome-screenshot + kbinani | PrintWindow + kbinani |
| **TTS** | `say` | `espeak` | System.Speech.Synthesis |
| **Settings** | AppleScript + pmset + networksetup + blueutil | pactl/amixer + brightnessctl/xbacklight + nmcli/rfkill + bluetoothctl | PowerShell + WMI + SendKeys + registry |
| **OS App** | AppleScript + `open` | wmctrl / ps / pkill | PowerShell + taskkill + user32.dll |
| **File Search** | `mdfind` (Spotlight) | plocate / locate / find | Everything (es.exe) / PowerShell |
| **Clipboard** | pbcopy / pbpaste | xclip / wl-copy | PowerShell Set-Clipboard |
| **Music** | AppleScript Music app | playerctl / mpc | PowerShell + WScript.Shell media keys |
| **Keychain** | `security` CLI | secret-tool / pass | cmdkey / Credential Manager |
| **Notification** | osascript display notification | notify-send / zenity / kdialog | WinRT Toast / BurntToast / MessageBox |
| **Contacts** | AppleScript Contacts | khard / abook | PowerShell + Outlook COM |
| **Calendar** | AppleScript Calendar | khal / gcalcli | PowerShell + Outlook COM |
| **Mail** | AppleScript Mail | notmuch / mutt | PowerShell + Outlook COM |
| **Reminders** | AppleScript Reminders | SQLite (self-contained) | SQLite (self-contained) |
| **Messages** | SQLite + AppleScript | N/A | N/A |
| **Accessibility Tree** | AppleScript recursive | AT-SPI via Python3/gi | UI Automation via PowerShell |
| **Window Capture** | screencapture -l + Quartz CGWindowID | ImageMagick / scrot / gnome-screenshot | PrintWindow via PowerShell |

---

## Appendix B: Error Handling Patterns

All tools follow a consistent error handling pattern:

1. **Input validation:** Returns `ToolResult{IsError: true}` with descriptive message for missing required fields
2. **Backend detection:** Linux tools check `exec.LookPath()` for CLI backends, return helpful install instructions when not found
3. **Graceful degradation:** Multiple backends tried in priority order (e.g., pactl -> amixer, wmctrl -> xdotool)
4. **Provider unavailable:** Returns descriptive error pointing to Settings (e.g., "No vision provider configured. Add an AI provider with vision support in Settings > Providers.")
5. **No nil error returns:** Tool execution errors are wrapped in `ToolResult{IsError: true}`, the `error` return value is only used for truly exceptional situations
6. **Result truncation:** Results exceeding 100KB are truncated by the registry to prevent context window pollution

---

## Appendix C: Approval Requirements

| Tool | RequiresApproval |
|------|-----------------|
| Desktop Input (DesktopTool) | **true** |
| Window (WindowTool) | false |
| MenuBar (MenubarTool) | false |
| Dialog (DialogTool) | false |
| Spaces (SpacesTool) | false |
| Dock (DockTool) | false |
| Shortcuts (ShortcutsTool) | **true** |
| Screenshot (ScreenshotTool) | false |
| TTS (TTSTool) | false |
| Settings (SettingsTool) | false |
| OS App (OSAppTool) | false |
| File Search (SpotlightTool) | false |
| Clipboard (ClipboardTool) | false |
| Music (MusicTool) | false |
| Keychain (KeychainTool) | **true** |
| Notification (NotificationTool) | false |
| Contacts (ContactsTool) | false |
| Calendar (CalendarTool) | false |
| Mail (MailTool) | Depends on action |
| Reminders (RemindersTool) | false |
| Messages (MessagesTool) | **true** (send) |
| Vision (VisionTool) | false |
| NeboLoop AppTool | false |

Note: The shell tool uses the Policy system's allowlist-based approval rather than the `RequiresApproval()` method. File tool does not require approval but is guarded by safeguards.
