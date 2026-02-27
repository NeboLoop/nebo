# Windows Tools — SME Deep Dive

> Last updated: 2026-02-25

This document covers every Windows-specific tool, capability, and platform helper in Nebo. Read this file to become a Windows tools SME.

---

## Architecture Overview

Windows tools are **platform capabilities** — standalone `Tool` implementations in `internal/agent/tools/*_windows.go` files guarded by `//go:build windows` build tags. They auto-register via `init()` functions and are filtered at compile time.

### Registration Flow

```
Tool init() → RegisterCapability(&Capability{...})
  → CapabilityRegistry.Register() checks isAvailable(runtime.GOOS == "windows")
  → Global capabilities map stores tool
  ...later during agent startup...
  → RegisterPlatformCapabilitiesWithPermissions(registry, userPermissions)
  → Filters by category→permission mapping:
      productivity → "contacts"
      system       → "system"
      media        → "media"
      desktop      → "desktop"
      automation   → (no key — registered by default)
      search       → (no key — registered by default)
      security     → (no key — registered by default)
  → Registry.Register(tool) — tool becomes available to LLM
```

**Key files:**
- `capabilities.go:60-72` — `detectPlatform()` uses `runtime.GOOS`
- `capabilities.go:78-94` — `Register()` checks platform match
- `capabilities.go:189-219` — `RegisterPlatformCapabilitiesWithPermissions()` filters by user permission map

---

## The 19 Windows Files (18 Tools + 1 Helper)

### Tool Inventory

| # | File | Tool Name | Category | Approval | Backend |
|---|------|-----------|----------|----------|---------|
| 1 | `desktop_windows.go` | desktop | automation | Yes | PowerShell + user32.dll P/Invoke |
| 2 | `app_windows.go` | app | system | Yes | PowerShell + user32.dll P/Invoke |
| 3 | `window_windows.go` | window | system | Yes | PowerShell + user32.dll P/Invoke |
| 4 | `music_windows.go` | music | media | No | PowerShell SendKeys (media keys) |
| 5 | `clipboard_windows.go` | clipboard | system | No | PowerShell Get/Set-Clipboard |
| 6 | `spotlight_windows.go` | spotlight | search | No | Everything CLI or Get-ChildItem fallback |
| 7 | `notification_windows.go` | notification | system | No | WinRT Toast / BurntToast / MessageBox / TTS |
| 8 | `system_windows.go` | system | system | No | WMI + media keys + rundll32 |
| 9 | `shortcuts_windows.go` | shortcuts | automation | Yes | Task Scheduler + custom scripts |
| 10 | `mail_windows.go` | mail | productivity | Yes | Outlook COM (requires Outlook) |
| 11 | `contacts_windows.go` | contacts | productivity | No | Outlook COM (requires Outlook) |
| 12 | `calendar_windows.go` | calendar | productivity | No | Outlook COM (requires Outlook) |
| 13 | `reminders_windows.go` | reminders | productivity | No | Outlook Tasks / Task Scheduler fallback |
| 14 | `keychain_windows.go` | keychain | security | Yes | Windows Credential Manager (advapi32.dll) |
| 15 | `accessibility_windows.go` | accessibility | automation | Yes | Windows UI Automation API |
| 16 | `shell_windows.go` | (helper) | — | — | Returns `cmd.exe /C` for shell domain tool |
| 17 | `process_signal_windows.go` | (helper) | — | — | `process.Kill()` (no Unix signals) |
| 18 | `snapshot_capture_windows.go` | (helper) | — | — | `PrintWindow()` WinAPI for screenshot |
| 19 | `snapshot_accessibility_windows.go` | (helper) | — | — | UI Automation tree for element IDs |

---

## Detailed Tool Documentation

### 1. Desktop Tool (`desktop_windows.go`)

**Purpose:** Mouse and keyboard control via PowerShell + .NET P/Invoke.

**Actions:** `click`, `double_click`, `right_click`, `type`, `hotkey`, `scroll`, `move`, `drag`, `paste`, `get_mouse_pos`, `get_active_window`

**Input struct:**
```go
type desktopInputWin struct {
    Action     string `json:"action"`
    X, Y       int    `json:"x"` `json:"y"`
    Text       string `json:"text"`
    Keys       string `json:"keys"`
    Direction  string `json:"direction"` // up, down, left, right
    Amount     int    `json:"amount"`    // scroll clicks (default 3)
    ToX, ToY   int    `json:"to_x"` `json:"to_y"` // drag destination
    Delay      int    `json:"delay"`      // ms between keystrokes
    Element    string `json:"element"`    // e.g. "B3" from screenshot(see)
    SnapshotID string `json:"snapshot_id"`
}
```

**Implementation details:**
- **Click:** `SetCursorPos(x,y)` → 50ms sleep → `mouse_event(LEFTDOWN)` → 50ms → `mouse_event(LEFTUP)`. Right-click uses `RIGHTDOWN`/`RIGHTUP` flags.
- **Double-click:** Two click sequences with 100ms gap between them.
- **Type:** `System.Windows.Forms.SendKeys.SendWait()` with special char escaping (`+^%~()[]{}` all escaped to `{+}{^}` etc via `escapeDesktopSendKeys()`).
- **Hotkey:** Converts human format "ctrl+c" to SendKeys format "^c" via `convertToSendKeys()`. Modifiers: `^`=Ctrl, `%`=Alt, `+`=Shift. Special keys: `{ENTER}`, `{TAB}`, `{ESC}`, `{F1}`-`{F12}`, `{PGUP}`, `{PGDN}`, etc. Windows key approximated as `^{ESC}`.
- **Scroll:** `mouse_event(MOUSEEVENTF_WHEEL, dwData=amount*120)`. Negative for down.
- **Drag:** SetCursorPos(from) → LEFTDOWN → SetCursorPos(to) → LEFTUP.
- **Paste:** Saves old clipboard → sets new text via `Clipboard::SetText()` → sends Ctrl+V via SendKeys → restores old clipboard.
- **Element ID resolution:** `GetSnapshotStore().LookupElement(element, snapshotID)` resolves element IDs (e.g., "B3") to x/y center coordinates. If targeting with element, automatically clicks to focus before typing.

**All PowerShell commands use `-NoProfile` flag.**

---

### 2. App Tool (`app_windows.go`)

**Purpose:** Application lifecycle control.

**Actions:** `list`, `launch`, `quit`, `activate`, `info`, `frontmost`

**Implementation:**
- **list:** `Get-Process | Where-Object { $_.MainWindowTitle -ne '' }` → shows ProcessName, PID, Title, Memory (MB).
- **launch:** `cmd /C start "" name` or `cmd /C start "" path`.
- **quit:** `taskkill /IM name*` (graceful) or `taskkill /F /IM name*` (force). Falls back to `name.exe` suffix on failure.
- **activate:** P/Invoke `ShowWindow(handle, 9)` (SW_RESTORE) + `SetForegroundWindow(handle)`.
- **info:** `Get-Process -Name` → Path, WorkingSet64/MB, TotalProcessorTime, ThreadCount, StartTime.
- **frontmost:** P/Invoke `GetForegroundWindow()` → `GetWindowThreadProcessId()` → `Get-Process -Id`.

---

### 3. Window Tool (`window_windows.go`)

**Purpose:** Window management — positioning, sizing, state.

**Actions:** `list`, `focus`, `move`, `resize`, `minimize`, `maximize`, `close`

**Implementation:**
- **list:** `Get-Process | Where-Object { MainWindowHandle -ne 0 }`. Uses P/Invoke `GetWindowRect()` for position/size.
- **focus:** `ShowWindow(handle, 9)` + `SetForegroundWindow(handle)`.
- **move:** `SetWindowPos(handle, 0, x, y, 0, 0, SWP_NOSIZE=0x0001)`.
- **resize:** `SetWindowPos(handle, 0, 0, 0, width, height, SWP_NOMOVE=0x0002)`.
- **minimize:** `ShowWindow(handle, 6)` (SW_MINIMIZE).
- **maximize:** `ShowWindow(handle, 3)` (SW_MAXIMIZE).
- **close:** `.CloseMainWindow()` on the process (sends WM_CLOSE).

---

### 4. Music Tool (`music_windows.go`)

**Purpose:** Global media key control.

**Actions:** `play`, `pause`, `toggle`, `next`, `previous`, `stop`, `mute`, `volume_up`, `volume_down`

**Implementation:** Maps actions to Windows virtual key codes sent via `SendKeys`:
- MEDIA_PLAY_PAUSE, MEDIA_NEXT_TRACK, MEDIA_PREV_TRACK, MEDIA_STOP
- VOLUME_MUTE, VOLUME_UP, VOLUME_DOWN

Works with any app that responds to global media hotkeys (Spotify, Windows Media Player, etc.). No volume-level control — only up/down/mute.

---

### 5. Clipboard Tool (`clipboard_windows.go`)

**Purpose:** Clipboard management with in-memory history.

**Actions:** `get`, `set`, `clear`, `type`, `history`

**Implementation:**
- **get:** `Get-Clipboard` → stores in memory history (max 20 entries, `clipboardEntry{timestamp, content}`).
- **set:** `Set-Clipboard -Value`.
- **clear:** `Set-Clipboard $null`.
- **type:** Detects format via `Clipboard::GetDataObject()` — checks Bitmap, FileDrop, UnicodeText.
- **history:** Returns in-memory history (session-scoped, NOT persisted).
- Truncates display to 5000 chars.

---

### 6. Spotlight Tool (`spotlight_windows.go`)

**Purpose:** File search with dual backend.

**Actions:** `query` (with filters: `kind`, `dir`, `limit`, `name_only`)

**Two backends (auto-detected):**
1. **Everything CLI (`es.exe`):** Checks `PATH` then `ProgramFiles\Everything\`. Uses instant indexed search. Command: `es.exe -n {limit} {query}`.
2. **Windows Search (fallback):** `Get-ChildItem -Recurse` filtering by name/extension.

**File type filters (kind param):**
- `app` → .exe, .msi, .bat
- `document` → .doc, .docx, .xls, .xlsx, .ppt, .pptx
- `image` → .jpg, .jpeg, .png, .gif, .bmp, .svg
- `audio` → .mp3, .wav, .flac, .aac
- `video` → .mp4, .mkv, .avi, .mov
- `pdf` → .pdf
- `folder` → PSIsContainer filter

Default limit: 20 results. Falls back gracefully if Everything not running.

---

### 7. Notification Tool (`notification_windows.go`)

**Purpose:** Toast notifications, alert dialogs, text-to-speech.

**Actions:** `send`, `alert`, `speak`

**Implementation:**
- **send:** WinRT `Windows.UI.Notifications.ToastNotificationManager` for toast notifications. Falls back to BurntToast PowerShell module if native fails.
- **alert:** `System.Windows.Forms.MessageBox::Show(message, title)`.
- **speak:** `System.Speech.Synthesis.SpeechSynthesizer`. Supports voice selection via `SelectVoice()`. Lists voices via `GetInstalledVoices()`.

---

### 8. System Tool (`system_windows.go`)

**Purpose:** System-level controls.

**Actions:** `volume`, `brightness`, `sleep`, `lock`, `wifi`, `info`, `mute`, `unmute`

**Implementation:**
- **volume:** Sends media key presses repeatedly (calculates press count to approximate target percentage — 50 presses for full range, imprecise).
- **mute/unmute:** Sends `[char]173` (VK_VOLUME_MUTE).
- **brightness:** WMI `WmiMonitorBrightnessMethods.WmiSetBrightness(1, level)` — laptops only, fails silently on desktops.
- **sleep:** `rundll32.exe powrprof.dll,SetSuspendState 0,1,0`.
- **lock:** `rundll32.exe user32.dll,LockWorkStation`.
- **wifi:** `netsh wlan show interfaces` for status; `Enable-NetAdapter`/`Disable-NetAdapter` for control.
- **info:** `Get-WmiObject Win32_OperatingSystem` + `Win32_Processor` + uptime + memory (`TotalVisibleMemorySize/1MB`).

---

### 9. Shortcuts Tool (`shortcuts_windows.go`)

**Purpose:** Task scheduling and script management.

**Actions:** `list`, `run`, `create`, `delete`

**Two backends:**
1. **Task Scheduler:** Creates tasks in `\Nebo\` folder via `New-ScheduledTaskTrigger`.
2. **Custom Scripts:** Saves `.ps1`/`.bat`/`.cmd` to `<data_dir>/shortcuts/`.

**Scheduling formats:** `daily HH:MM`, `weekly DAY HH:MM`, `hourly`, `startup`, `logon`, `monthly DAY HH:MM`.

---

### 10. Mail Tool (`mail_windows.go`)

**Purpose:** Email via Outlook COM.

**Actions:** `read`, `send`, `unread`, `search`, `accounts`

**Requires:** Microsoft Outlook installed. Checks via COM object creation at `init()`.

**Outlook folder IDs:** 6=Inbox, 5=Sent, 16=Drafts, 3=Deleted, 4=Outbox.

- **send:** Creates mail item (type 0), sets To/CC/Subject/Body, calls `Send()`.
- **read:** Gets default Inbox (folder 6), sorts by ReceivedTime descending.
- **unread:** Queries `UnReadItemCount` on Inbox.
- **search:** Uses `Restrict()` filter on Subject/Body with LIKE pattern.
- **accounts:** Enumerates `namespace.Accounts` collection.

---

### 11. Contacts Tool (`contacts_windows.go`)

**Purpose:** Outlook contacts management.

**Actions:** `search`, `get`, `create`, `groups`

**Requires:** Outlook COM. Folder ID 10 = default Contacts.

- **search:** Partial match on FullName or Email1Address.
- **get:** Full details — name, company, email (1/2/3), phone (business/mobile/home), address, notes.
- **create:** New contact type 2.
- **groups:** Lists sub-folders in Contacts.

---

### 12. Calendar Tool (`calendar_windows.go`)

**Purpose:** Outlook calendar management.

**Actions:** `list`, `create`, `today`, `upcoming`, `calendars`

**Requires:** Outlook COM. Folder ID 9 = default Calendar.

- **today:** Filter events between today 00:00 and tomorrow 00:00.
- **upcoming:** Next N days (default 7).
- **create:** New appointment type 1. Sets Subject/Start/End/Location/Body, with ReminderSet.
- **Recurrence:** `.IncludeRecurrences = $true` includes recurring events.
- **Date parsing:** `YYYY-MM-DD HH:MM` or `YYYY-MM-DD`.

---

### 13. Reminders Tool (`reminders_windows.go`)

**Purpose:** Task/reminder management.

**Actions:** `list`, `create`, `complete`, `delete`, `lists`

**Dual backend (auto-detect Outlook):**
1. **Outlook Tasks (folder 13):** Full task management with categories, priority (2=High, 1=Normal, 0=Low), due dates. Task type 3.
2. **Task Scheduler (fallback):** Creates scheduled tasks with `msg.exe * "Reminder: taskname"` popups.

**Date parsing:** "tomorrow", "in 2 days", "in 3 hours", "YYYY-MM-DD".

---

### 14. Keychain Tool (`keychain_windows.go`)

**Purpose:** Windows Credential Manager access.

**Actions:** `get`, `find`, `add`, `delete`

- **Target format:** `nebo:service` or `nebo:service:account`.
- **get:** Native CredManager P/Invoke. Passwords masked (shows first 2 chars + asterisks).
- **find:** `cmdkey /list` filtering by target.
- **add:** `cmdkey /generic:target /user:account /pass:password`.
- **delete:** `cmdkey /delete:target`.

---

### 15. Accessibility Tool (`accessibility_windows.go`)

**Purpose:** UI Automation tree inspection and interaction.

**Actions:** `tree`, `find`, `click`, `get_value`, `set_value`, `list_apps`

- **tree:** Walks UI Automation hierarchy up to maxDepth (default 3), shows `[ControlType] Name`.
- **find:** Searches by role (Button, Edit, CheckBox) and/or label (case-insensitive). Returns first 20 matches.
- **click:** `InvokePattern.Invoke()` for buttons, `TogglePattern.Toggle()` for checkboxes.
- **get/set_value:** `ValuePattern.Current.Value` or `TextPattern.DocumentRange.GetText()`.
- **list_apps:** Enumerates top-level windows via `RootElement.FindAll()`.

**Key difference from screenshot(see):** Uses programmatic UI Automation tree vs visual accessibility overlay.

---

### 16-17. Shell & Signal Helpers

**`shell_windows.go`:**
- `ShellCommand()` → `"cmd.exe", []string{"/C"}`
- `ShellName()` → `"cmd"`
- Used by the `shell` domain tool to determine platform-specific invocation.

**`process_signal_windows.go`:**
- `KillProcessWithSignal()` → always `process.Kill()` (Windows has no Unix signals).
- `SignalSupported()` → `false`.
- `DefaultSignalName()` → `"KILL"`.

---

### 18-19. Snapshot Helpers

**`snapshot_capture_windows.go`:**
- `CaptureAppWindow(app, windowIndex)` → `(image.Image, Rect, error)`.
- Uses `PrintWindow()` WinAPI (better for inactive/partially-occluded windows than GDI).
- Saves to temp PNG, decodes with `png.Decode()`.
- `ListAppWindows()` returns window titles for process.

**`snapshot_accessibility_windows.go`:**
- `getUITreeWithBounds()` → `[]RawElement`.
- PowerShell UIAutomation API, max depth 5.
- Extracts: role, name, value, bounds (x, y, w, h), actionable (has supported patterns).
- Role normalization: Button→button, Edit→textfield, CheckBox→checkbox, etc.
- Used by `screenshot(action: see)` to build annotated overlay with element IDs (B=button, T=textfield, L=link).

---

## Windows-Specific Code Outside Tools

### Updater (`internal/updater/apply_windows.go`)

Binary update strategy for Windows:
1. Health check new binary (`--version`, 5s timeout)
2. Rename current exe to `.exe.old` (Windows allows renaming running exe)
3. **Copy** (not rename) new binary to current exe location (temp dir may be different filesystem)
4. Call `runPreApply()` to release resources
5. Spawn new process via `exec.Command(currentExe, args...)`
6. `os.Exit(0)`
7. **Rollback:** If copy fails, rename `.old` back to original

### App Process Supervision (`internal/apps/process_windows.go`)

- `isProcessAlive(pid)` — `os.FindProcess()` + `Signal(syscall.Signal(0))` test.
- `setProcGroup(cmd)` — `SysProcAttr.CreationFlags = CREATE_NEW_PROCESS_GROUP`.
- `killProcGroup()` — `taskkill.exe /t /f /pid` (tree kill with force).
- `killProcGroupTerm()` — `taskkill.exe /t /pid` (graceful, WM_CLOSE to GUI apps).

### Orphan Handling (`internal/apps/orphan_windows.go`)

- `killOrphansByBinary()` — **No-op** on Windows (Windows doesn't reparent to PID 1 like Unix).

---

## Security Model

### Approval Requirements

| Requires Approval (YES) | No Approval Needed |
|---|---|
| desktop, window, shortcuts, app, keychain, accessibility, mail | music, clipboard, notification, spotlight, system, contacts, calendar, reminders |

### Three Security Layers

1. **Safeguard** (unconditional, `safeguard.go`): Blocks dangerous shell commands (sudo, rm -rf, fork bombs). Windows tools bypass this (no shell involvement).
2. **Policy** (`policy.go`): Checks `RequiresApproval()` → prompts user via `ApprovalCallback` (web UI) or stdin (CLI). PolicyDeny blocks all, PolicyAllowlist (default) uses SafeBins + callback, PolicyFull allows all.
3. **Origin** (`origin.go`): Per-origin deny lists. `OriginComm` and `OriginApp` deny shell access. Windows platform tools are NOT currently in any origin deny list (all accessible from all origins).

### PowerShell Security

- All scripts use `-NoProfile` (skip `$PROFILE`, faster startup, avoids user profile side effects).
- Input escaping functions: `escapePowerShell()`, `escapePSContactsQuery()`, `escapeDesktopSendKeys()` handle backticks, quotes, dollar signs, SendKeys special chars.
- P/Invoke calls use marshaling but no additional input validation beyond JSON parsing.

### COM Object Dependencies

Mail, Contacts, Calendar, and Reminders all depend on Microsoft Outlook:
- `init()` functions check COM availability
- `Execute()` returns user-friendly "Outlook not installed" message on failure
- No elevated privileges required (runs as current user)

---

## Limitations & Quirks

| Tool | Limitation |
|------|-----------|
| Desktop | Windows key approximated as `^{ESC}`; element IDs require prior `screenshot(see)` |
| App | Launch via `cmd /C start` — limited to apps in PATH or full paths |
| Clipboard | History is in-memory only, lost on session end; max 20 entries |
| Spotlight | Falls back to slow `Get-ChildItem -Recurse` if Everything not installed/running |
| System | Volume is approximation (~2 key presses per 1%); brightness only works on laptops |
| Mail/Contacts/Calendar/Reminders | Requires Microsoft Outlook; no Gmail/Google Workspace/Thunderbird support |
| Accessibility | Max search depth 3-5; returns max 20 results |
| Keychain | Passwords always masked in output (security by design) |
| Music | No absolute volume control, only up/down/mute |
| Shortcuts | Limited to Task Scheduler scheduling patterns |
| Notifications | Toast requires app manifest; falls back to BurntToast module |
| Process signals | Only Kill(), no graceful termination signals (Windows limitation) |

---

## Category → Permission Mapping

```go
var categoryToPermission = map[string]string{
    "productivity": "contacts",  // mail, contacts, calendar, reminders
    "system":       "system",    // app, clipboard, notification, system, window
    "media":        "media",     // music
    "desktop":      "desktop",   // (currently unused — desktop/accessibility are "automation")
}
// Categories without mapping (automation, search, security) register by default
```

**Important:** The `automation` category (desktop, shortcuts, accessibility) and `search` category (spotlight) and `security` category (keychain) have NO permission key — they are registered unconditionally when the platform matches.
