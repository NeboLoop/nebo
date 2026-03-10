# Go → Rust 100% Tool Parity Plan

## Status: DRAFT

**Go source:** `/Users/almatuck/workspaces/nebo/nebo/internal/agent/tools/`
**Rust target:** `/Users/almatuck/workspaces/nebo/nebo-rs/crates/tools/src/`

---

## Code Auditor Compliance

This plan follows every rule in `docs/sme/CODE_AUDITOR.md`:

- **Rule 1 (Reuse):** Every new tool implements `DynTool` trait, uses `ToolResult::ok()`/`ToolResult::error()`, receives `ToolContext`, and registers via `register_all_with_permissions()`.
- **Rule 2 (Create when needed):** New files only when no existing file covers the domain. Existing tools extended in-place.
- **Rule 3 (No dead code):** No files deleted — existing code preserved and extended with `#[cfg]` branches.
- **Rule 4 (Build):** `cargo build` verification at the end of every phase.
- **Rule 6 (Rust patterns):** Inline `#[cfg(target_os)]` matching `settings_tool.rs` pattern. No platform subdirectories.
- **Rule 8.1 (No competing pathways):** Each capability has ONE canonical home. No duplicated surfaces.
- **Rule 8.4 (Dependency direction):** New crate dependencies explicitly verified against existing graph.

---

## Gap Summary

| Domain | Go | Rust | Parity |
|--------|-----|------|--------|
| System (file, shell) | ✓ | ✓ | 100% |
| Settings (volume, brightness, wifi, etc.) | ✓ | ✓ | 100% |
| Bot (memory, task, session, profile, context, advisors, ask, vision) | ✓ | ~95% | profile.open_billing missing |
| Event (cron/scheduling) | ✓ | ✓ | 100% |
| Skill (catalog, CRUD, browse, install) | ✓ | ✓ | 100% |
| Work (workflow lifecycle) | ✓ | ✓ | 100% |
| Web (http, search, browser) | ✓ | ~85% | devtools resource missing |
| Execute (script runtime) | ✓ | ✓ | 100% |
| Emit (event bus) | ✓ | ✓ | 100% |
| Desktop (window, input, clipboard, capture, notification) | ✓✓✓ | macOS only | **0% Linux/Windows** |
| Desktop (ui, menu, dialog, space, shortcut, tts) | ✓✓✓ | **0%** | **Missing entirely** |
| Organizer (mail, contacts, calendar, reminders) | ✓✓✓ | **0%** | **Missing entirely** |
| App (list, launch, quit, activate, frontmost) | ✓✓✓ | **0%** | **Missing entirely** |
| Music (play, pause, next, playlists, shuffle) | ✓✓✓ | **0%** | **Missing entirely** |
| Keychain (get, find, add, delete) | ✓✓✓ | **0%** | **Missing entirely** |
| Loop (dm, channel, group, topic) | ✓ | **0%** | **Missing entirely** |
| Message — sms | ✓ (darwin) | **0%** | **Missing** |
| Message — notify (speak, dnd_status) | ✓✓✓ | partial | Missing actions |
| Spotlight / search | ✓✓✓ | macOS+Linux | **0% Windows** |

**Current parity: ~30%**

---

## Established Patterns (Must Follow)

### Tool Implementation Pattern

Every tool implements `DynTool` and follows this exact structure (from `settings_tool.rs`, `desktop_tool.rs`):

```rust
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

pub struct NewTool;

impl NewTool {
    pub fn new() -> Self { Self }
}

impl DynTool for NewTool {
    fn name(&self) -> &str { "toolname" }
    fn description(&self) -> String { /* STRAP resource/action list */ }
    fn schema(&self) -> serde_json::Value { /* JSON schema with resource + action enums */ }
    fn requires_approval(&self) -> bool { false }
    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let resource = input["resource"].as_str().unwrap_or("");
            let action = input["action"].as_str().unwrap_or("");
            match resource {
                "res1" => handle_res1(action, &input).await,
                _ => ToolResult::error(format!("Unknown resource '{}'", resource)),
            }
        })
    }
}
```

### Tools With DB Access

Tools that need `Arc<Store>` (like `BotTool`, `EventTool`, `MessageTool`) store it in the struct:

```rust
pub struct NewTool {
    store: Arc<db::Store>,
}
impl NewTool {
    pub fn new(store: Arc<db::Store>) -> Self { Self { store } }
}
```

The store is passed from `register_all_with_permissions()` in `registry.rs`.

### Cross-Platform Pattern

Inline `#[cfg(target_os)]` within the same file (established in `settings_tool.rs`, `desktop_tool.rs`, `spotlight_tool.rs`):

```rust
async fn handle_thing(action: &str, input: &serde_json::Value) -> ToolResult {
    #[cfg(target_os = "macos")]
    { /* macOS implementation */ }

    #[cfg(target_os = "linux")]
    { /* Linux implementation */ }

    #[cfg(target_os = "windows")]
    { /* Windows implementation */ }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Unsupported platform")
}
```

**No `platform/` subdirectories.** Platform code lives inline.

### Registration Pattern

New tools register in `register_all_with_permissions()` in `registry.rs`:

```rust
// New tool — requires "category" permission
if allowed("category") {
    self.register(Box::new(crate::new_tool::NewTool::new())).await;
}
```

And export from `lib.rs`:

```rust
pub mod new_tool;
pub use new_tool::NewTool;
```

---

## Phase 1: Desktop Cross-Platform (Priority: CRITICAL)

**File:** `crates/tools/src/desktop_tool.rs` (MODIFY existing — do NOT delete)

Existing macOS code stays. Add `#[cfg(target_os = "linux")]` and `#[cfg(target_os = "windows")]` branches to every `handle_*` function, matching how `settings_tool.rs` does it.

### 1.1 Window Management — Add Linux + Windows

**Go ref:** `window_darwin.go`, `window_linux.go`, `window_windows.go`

Existing actions (macOS works): list, focus, minimize, maximize, resize, close.
Add `move` action (missing from current Rust).

| Action | macOS (exists) | Linux (add) | Windows (add) |
|--------|----------------|-------------|----------------|
| list | osascript System Events | `wmctrl -l` or `xdotool search` | PowerShell `Get-Process \| Where MainWindowTitle` |
| focus | osascript `activate` | `wmctrl -ia` or `xdotool windowactivate` | PowerShell `Add-Type + SetForegroundWindow` |
| move | osascript `set position` | `xdotool windowmove` | PowerShell `MoveWindow` |
| resize | osascript `set size` | `xdotool windowsize` | PowerShell `MoveWindow` |
| minimize | osascript `set miniaturized` | `xdotool windowminimize` | PowerShell `ShowWindow SW_MINIMIZE` |
| maximize | osascript bounds | `wmctrl -b add,maximized` | PowerShell `ShowWindow SW_MAXIMIZE` |
| close | osascript click | `xdotool key alt+F4` or `wmctrl -ic` | PowerShell `CloseMainWindow()` |

### 1.2 Input — Add Linux + Windows + Missing Actions

**Go ref:** `desktop_darwin.go`, `desktop_linux.go`, `desktop_windows.go`

Existing actions (macOS): click, type, press, move.
Add: double_click, right_click, hotkey, scroll, drag, paste.

| Action | macOS (exists/extend) | Linux (add) | Windows (add) |
|--------|----------------------|-------------|----------------|
| click | cliclick / CGEvent | `xdotool click 1` | PowerShell SendInput |
| double_click | cliclick dc / CGEvent×2 | `xdotool click --repeat 2` | SendInput double |
| right_click | cliclick rc / CGEvent btn2 | `xdotool click 3` | SendInput right |
| type | System Events keystroke | `xdotool type` | SendInput / SendKeys |
| hotkey | CGEvent with modifiers | `xdotool key combo` | SendInput VK codes |
| scroll | CGEvent scroll | `xdotool click 4/5` | SendInput WHEEL |
| move | cliclick m | `xdotool mousemove` | SetCursorPos |
| drag | CGEvent down→move→up | `xdotool mousedown→move→mouseup` | SendInput sequence |
| paste | Cmd+V via hotkey | `xdotool key ctrl+v` | SendInput Ctrl+V |
| press | System Events key code | `xdotool key` | SendInput VK |

### 1.3 Clipboard — Add Linux + Windows + Missing Actions

**Go ref:** `clipboard_darwin.go`, `clipboard_linux.go`, `clipboard_windows.go`

Clipboard lives in `desktop_tool.rs` — ONE canonical home. No duplication.

Existing actions (macOS): read, write.
Add: clear.

| Action | macOS (exists) | Linux (add) | Windows (add) |
|--------|----------------|-------------|----------------|
| read | `pbpaste` | `xclip -o` / `xsel -ob` / `wl-paste` | PowerShell `Get-Clipboard` |
| write | `pbcopy` | `xclip -i` / `xsel -ib` / `wl-copy` | PowerShell `Set-Clipboard` |
| clear | `pbcopy < /dev/null` | `xclip -i /dev/null` | PowerShell `Set-Clipboard ''` |

### 1.4 Screenshot / Capture — Add Linux + Windows + "see" mode

**Go ref:** `snapshot_capture_darwin.go`, `snapshot_capture_linux.go`, `snapshot_capture_windows.go`

Existing actions (macOS): screenshot.
Add: `see` (annotated screenshot with element refs).

| Action | macOS (exists) | Linux (add) | Windows (add) |
|--------|----------------|-------------|----------------|
| screenshot | `screencapture -x` | `gnome-screenshot` / `scrot` / `grim` | PowerShell `CopyFromScreen` |
| see | capture + annotate refs | same + AT-SPI overlay | same + UI Automation overlay |

### 1.5 Notification — Add Linux + Windows

**Go ref:** `notification_darwin.go`, `notification_linux.go`, `notification_windows.go`

Existing actions (macOS): send.
Add: alert.

| Action | macOS (exists) | Linux (add) | Windows (add) |
|--------|----------------|-------------|----------------|
| send | osascript notification | `notify-send` | PowerShell BurntToast / Toast |
| alert | osascript dialog | `zenity` / `kdialog` | PowerShell `[MessageBox]::Show` |

### 1.6 New Resources — UI, Menu, Dialog, Space, Shortcut, TTS

Add these as new resources within the existing `desktop_tool.rs`. Update the `schema()` resource enum, `description()`, and `execute_dyn()` match.

#### UI / Accessibility

**Go ref:** `accessibility_darwin.go`, `accessibility_linux.go`, `accessibility_windows.go`
**Actions:** tree, find, click, get_value, set_value, list_apps

| Action | macOS | Linux | Windows |
|--------|-------|-------|---------|
| tree | AXUIElement | AT-SPI2 (D-Bus) | UI Automation |
| find | AXUIElement search | AT-SPI find | FindFirst/FindAll |
| click | AXUIElement performAction | AT-SPI Action | InvokePattern |
| get_value | AXUIElement value | AT-SPI Value | ValuePattern |
| set_value | AXUIElement set value | AT-SPI set | ValuePattern.SetValue |
| list_apps | AXUIElement system-wide | AT-SPI Registry | EnumDesktopWindows |

#### Menu

**Go ref:** `menubar_darwin.go`, `menubar_windows.go`
**Actions:** list, menus, click, status, click_status
**Platforms:** macOS, Windows (Go has no Linux impl)

#### Dialog

**Go ref:** `dialog_darwin.go`, `dialog_windows.go`
**Actions:** detect, list, click, fill, dismiss
**Platforms:** macOS, Windows (Go has no Linux impl)

#### Space / Virtual Desktop

**Go ref:** `spaces_darwin.go`, `spaces_windows.go`
**Actions:** list, switch, move_window
**Platforms:** macOS, Windows (Go has no Linux impl)

#### Shortcut

**Go ref:** `shortcuts_darwin.go`, `shortcuts_linux.go`, `shortcuts_windows.go`
**Actions:** list, run

| Action | macOS | Linux | Windows |
|--------|-------|-------|---------|
| list | `shortcuts list` | scan `~/.local/bin/` | PowerShell `Get-Command` |
| run | `shortcuts run "name"` | exec script | PowerShell `Start-Process` |

#### TTS

**Go ref:** `tts.go`
**Actions:** speak

| Action | macOS | Linux | Windows |
|--------|-------|-------|---------|
| speak | `say "text"` | `espeak` / `spd-say` | PowerShell SpeechSynthesizer |

### 1.7 Snapshot Infrastructure

**Go ref:** `snapshot_store.go`, `snapshot_annotator.go`, `snapshot_renderer.go`

Add to `desktop_tool.rs` (or private helper module if file exceeds ~800 lines — use `skills/` module pattern, NOT `platform/` pattern):

- `SnapshotStore` — in-memory `HashMap<String, SnapshotEntry>` keyed by element refs `[e1]`, `[e2]`
- Annotator — overlays element ref labels on screenshot images
- Renderer — renders accessibility tree to compact text for LLM context

### 1.8 Desktop Queue

**Go ref:** `desktop_queue.go`

Add `tokio::sync::Mutex<()>` guard in `DesktopTool` struct to serialize UI operations:

```rust
pub struct DesktopTool {
    queue: tokio::sync::Mutex<()>,
}
```

Acquire before any input/ui operation, release after. Prevents race conditions in multi-step automation.

### Phase 1 Verification

```bash
cargo build  # zero warnings
cargo test   # existing desktop tests still pass
```

---

## Phase 2: App Tool (Priority: HIGH)

**File:** NEW `crates/tools/src/app_tool.rs`

Standalone tool (matches Rust pattern where settings, spotlight, desktop are separate tools — NOT system sub-resources).

**Go ref:** `app_tool.go`, `app_darwin.go`, `app_linux.go`, `app_windows.go`
**Tool name:** `app`
**Actions (flat domain, no resource):** list, launch, quit, quit_all, activate, hide, info, frontmost

```rust
pub struct AppTool;

impl DynTool for AppTool {
    fn name(&self) -> &str { "app" }
    fn execute_dyn<'a>(...) -> ... {
        Box::pin(async move {
            let action = input["action"].as_str().unwrap_or("");
            match action {
                "list" => handle_app_list().await,
                "launch" => handle_app_launch(&input).await,
                // ...
            }
        })
    }
}
```

| Action | macOS | Linux | Windows |
|--------|-------|-------|---------|
| list | osascript `running applications` | `ps aux` + `/proc` | PowerShell `Get-Process` |
| launch | `open -a "App"` | `gtk-launch` / `xdg-open` | `Start-Process` |
| quit | osascript `quit app` | `kill PID` (graceful) | `Stop-Process` / `CloseMainWindow()` |
| quit_all | osascript loop | `killall` | PowerShell loop |
| activate | osascript `activate` | `wmctrl -a` / `xdotool` | `SetForegroundWindow` |
| hide | osascript `set visible false` | `xdotool windowminimize` | `ShowWindow SW_HIDE` |
| info | `mdls /Applications/X.app` | `.desktop` file parse | Registry `App Paths` |
| frontmost | osascript `frontmost application` | `xdotool getactivewindow` | `GetForegroundWindow` |

**Registry:** `register_all_with_permissions()` under `"desktop"` permission (app control is a desktop capability).

### Phase 2 Verification

```bash
cargo build  # zero warnings
```

---

## Phase 3: Organizer Tool (Priority: HIGH)

**File:** NEW `crates/tools/src/organizer_tool.rs`

**Go ref:** `pim_domain.go`, `mail_darwin.go`, `contacts_darwin.go`, `calendar_darwin.go`, `reminders_darwin.go` (+ linux + windows variants)
**Tool name:** `organizer`
**Resources:** mail, contacts, calendar, reminders

If the file grows beyond ~800 lines, split into a module using the `skills/` pattern:
```
crates/tools/src/organizer/
├── mod.rs          (OrganizerTool struct + DynTool impl + resource dispatch)
├── mail.rs         (handle_mail fn + per-platform #[cfg] blocks)
├── contacts.rs     (handle_contacts fn)
├── calendar.rs     (handle_calendar fn)
└── reminders.rs    (handle_reminders fn)
```

**No `platform/` subdirectory.** Each resource file contains its own inline `#[cfg(target_os)]` blocks.

```rust
pub struct OrganizerTool;

impl DynTool for OrganizerTool {
    fn name(&self) -> &str { "organizer" }
    fn execute_dyn<'a>(...) -> ... {
        Box::pin(async move {
            let resource = input["resource"].as_str().unwrap_or("");
            let action = input["action"].as_str().unwrap_or("");
            match resource {
                "mail" => handle_mail(action, &input).await,
                "contacts" => handle_contacts(action, &input).await,
                "calendar" => handle_calendar(action, &input).await,
                "reminders" => handle_reminders(action, &input).await,
                _ => ToolResult::error(format!("Unknown resource '{}'. Use: mail, contacts, calendar, reminders", resource)),
            }
        })
    }
}
```

### Mail

**Actions:** accounts, unread, read, send, search

| Action | macOS | Linux | Windows |
|--------|-------|-------|---------|
| accounts | AppleScript Mail.app | Evolution D-Bus / Thunderbird | Outlook COM |
| unread | AppleScript `unread count` | Evolution query | Outlook `GetDefaultFolder` |
| read | AppleScript `message id` | Evolution/Thunderbird | Outlook `MailItem` |
| send | AppleScript `new outgoing message` | `xdg-email` / Evolution | Outlook `CreateItem` |
| search | AppleScript `messages whose` | Evolution search | Outlook `Items.Find` |

### Contacts

**Actions:** search, get, create, groups

| Action | macOS | Linux | Windows |
|--------|-------|-------|---------|
| search | AppleScript Contacts.app | `folks` / Evolution | Outlook COM |
| get | AppleScript `person id` | `folks` | Outlook `ContactItem` |
| create | AppleScript `make new person` | `folks` / Evolution | Outlook `CreateItem` |
| groups | AppleScript `groups` | `folks` | Outlook `Folders` |

### Calendar

**Actions:** calendars, today, upcoming, create, list

| Action | macOS | Linux | Windows |
|--------|-------|-------|---------|
| calendars | AppleScript Calendar.app | Evolution D-Bus | Outlook `GetDefaultFolder(9)` |
| today | AppleScript `events of date today` | Evolution query | Outlook `Items.Restrict` |
| upcoming | AppleScript date range | Evolution query | Outlook `Items.Restrict` |
| create | AppleScript `make new event` | Evolution | Outlook `CreateItem` |
| list | AppleScript `every event` | Evolution | Outlook `Items` |

### Reminders

**Actions:** lists, list, create, complete, delete

| Action | macOS | Linux | Windows |
|--------|-------|-------|---------|
| lists | AppleScript Reminders.app | `gnome-todo` / Evolution Tasks | Outlook `GetDefaultFolder(13)` |
| list | AppleScript `reminders of list` | GNOME query | Outlook `Items` |
| create | AppleScript `make new reminder` | GNOME/Evolution | Outlook `CreateItem(olTaskItem)` |
| complete | AppleScript `set completed true` | GNOME update | Outlook `TaskItem.Complete` |
| delete | AppleScript `delete reminder` | GNOME/Evolution | Outlook `Delete` |

**Registry:** `register_all_with_permissions()` under `"organizer"` permission.

### Phase 3 Verification

```bash
cargo build  # zero warnings
```

---

## Phase 4: Music Tool (Priority: MEDIUM)

**File:** NEW `crates/tools/src/music_tool.rs`

Standalone tool (same pattern as SettingsTool, AppTool — separate `DynTool`, not a system sub-resource).

**Go ref:** `music_darwin.go`, `music_linux.go`, `music_windows.go`
**Tool name:** `music`
**Actions (flat domain):** play, pause, next, previous, status, search, volume, playlists, shuffle

```rust
pub struct MusicTool;

impl DynTool for MusicTool {
    fn name(&self) -> &str { "music" }
    // ...
}
```

| Action | macOS | Linux | Windows |
|--------|-------|-------|---------|
| play | osascript Music.app | `playerctl play` (MPRIS) | PowerShell SMTC |
| pause | osascript Music.app | `playerctl pause` | SMTC |
| next | osascript `next track` | `playerctl next` | SMTC |
| previous | osascript `previous track` | `playerctl previous` | SMTC |
| status | osascript state + track | `playerctl metadata` | SMTC |
| search | osascript search | MPRIS metadata | N/A |
| volume | osascript `set sound volume` | `playerctl volume` | SMTC |
| playlists | osascript `playlists` | MPRIS Playlists | N/A |
| shuffle | osascript `set shuffle` | `playerctl shuffle` | N/A |

**Registry:** `register_all_with_permissions()` under `"media"` permission.

### Phase 4 Verification

```bash
cargo build  # zero warnings
```

---

## Phase 5: Keychain Tool (Priority: MEDIUM)

**File:** NEW `crates/tools/src/keychain_tool.rs`

**Go ref:** `keychain_darwin.go`, `keychain_linux.go`, `keychain_windows.go`
**Tool name:** `keychain`
**Actions (flat domain):** get, find, add, delete

```rust
pub struct KeychainTool;

impl DynTool for KeychainTool {
    fn name(&self) -> &str { "keychain" }
    fn requires_approval(&self) -> bool { true } // credential access needs approval
    // ...
}
```

| Action | macOS | Linux | Windows |
|--------|-------|-------|---------|
| get | `security find-generic-password -w` | `secret-tool lookup` | `cmdkey /list` |
| find | `security find-generic-password -l` | `secret-tool search --all` | `cmdkey /list` + filter |
| add | `security add-generic-password` | `secret-tool store` | `cmdkey /add` |
| delete | `security delete-generic-password` | `secret-tool clear` | `cmdkey /delete` |

**Registry:** `register_all_with_permissions()` under `"system"` permission.

### Phase 5 Verification

```bash
cargo build  # zero warnings
```

---

## Phase 6: Loop Tool (Priority: MEDIUM)

**File:** NEW `crates/tools/src/loop_tool.rs`

**Go ref:** `loop_tool.go`
**Tool name:** `loop`
**Resources:** dm, channel, group, topic

### Dependency Direction Check

`tools` → `comm` is a NEW dependency. Current dependency graph:
```
tools depends on: types, ai, db, config, napp, browser, mcp, notify_crate
```
`comm` depends on: `types`

Adding `tools → comm` does NOT create a cycle. Verified safe.

Add to `crates/tools/Cargo.toml`:
```toml
comm = { workspace = true }
```

```rust
pub struct LoopTool {
    comm: Arc<dyn comm::CommPlugin>,
}

impl LoopTool {
    pub fn new(comm: Arc<dyn comm::CommPlugin>) -> Self { Self { comm } }
}

impl DynTool for LoopTool {
    fn name(&self) -> &str { "loop" }
    // ...
}
```

| Resource | Action | Implementation |
|----------|--------|----------------|
| dm | send | CommPlugin → NeboLoop WebSocket/REST |
| channel | send, messages, members, list | CommPlugin |
| group | list, get, members | CommPlugin |
| topic | subscribe, unsubscribe, list, status | CommPlugin pub/sub |

**Registry:** `register_all_with_permissions()` under `"loop"` permission. Only registered when a `CommPlugin` is provided.

### Phase 6 Verification

```bash
cargo build  # zero warnings — verify no circular deps
```

---

## Phase 7: Message Tool Completion (Priority: MEDIUM)

**File:** `crates/tools/src/message_tool.rs` (MODIFY existing)

### 7.1 SMS Resource (macOS only)

**Go ref:** `messages_darwin.go`
Add `sms` resource with actions: send, conversations, read, search.

| Action | macOS | Linux/Windows |
|--------|-------|---------------|
| send | osascript `tell app "Messages" to send` | ToolResult::error("SMS not supported") |
| conversations | osascript `every chat` | same |
| read | osascript `messages of chat` | same |
| search | SQLite on `~/Library/Messages/chat.db` | same |

### 7.2 Notify Resource — Add speak, dnd_status

| Action | macOS | Linux | Windows |
|--------|-------|-------|---------|
| speak | `say "text"` | `espeak` / `spd-say` | PowerShell SpeechSynthesizer |
| dnd_status | `defaults read com.apple.controlcenter` | D-Bus DoNotDisturb | PowerShell FocusAssist |

### Phase 7 Verification

```bash
cargo build  # zero warnings
```

---

## Phase 8: Web Tool Completion (Priority: MEDIUM)

**File:** `crates/tools/src/web_tool.rs` (MODIFY existing)

### 8.1 DevTools Resource

**Go ref:** `web_tool.go` devtools resource

Add `devtools` resource with actions: console, source, storage, dom, cookies, performance.

All actions route through Chrome DevTools Protocol via the browser extension bridge (same transport as `browser` resource).

### Phase 8 Verification

```bash
cargo build  # zero warnings
```

---

## Phase 9: Spotlight / Search — Windows (Priority: LOW)

**File:** `crates/tools/src/spotlight_tool.rs` (MODIFY existing)

**Go ref:** `spotlight_windows.go`

Add `#[cfg(target_os = "windows")]` branch:

```rust
#[cfg(target_os = "windows")]
{
    // PowerShell Get-ChildItem -Recurse -Filter
    // or Windows Search API (ISearchQueryHelper)
}
```

### Phase 9 Verification

```bash
cargo build  # zero warnings
```

---

## Phase 10: Skill Tool — Marketplace Browsing (Priority: LOW)

**File:** `crates/tools/src/skill_tool.rs` (MODIFY existing)

**No separate store_tool.** Skill marketplace browsing is part of `skill_tool.rs` — ONE canonical home for skill lifecycle.

**Go ref:** `neboloop_tool.go` (skills subset only)

Extend existing `browse` action to support: featured, popular, reviews.
The `install` action already handles SKILL-XXXX-XXXX codes.

App store browsing (apps resource from Go's `neboloop_tool.go`) is deferred — apps are .napp packages managed by the napp crate, not a tool concern.

### Phase 10 Verification

```bash
cargo build  # zero warnings
```

---

## Phase 11: Bot Tool — open_billing (Priority: LOW)

**File:** `crates/tools/src/bot_tool.rs` (MODIFY existing)

Add `open_billing` action to `profile` resource. Opens NeboLoop billing portal in default browser:

```rust
"open_billing" => {
    let url = "https://neboloop.com/billing";
    #[cfg(target_os = "macos")]
    { tokio::process::Command::new("open").arg(url).spawn(); }
    #[cfg(target_os = "linux")]
    { tokio::process::Command::new("xdg-open").arg(url).spawn(); }
    #[cfg(target_os = "windows")]
    { tokio::process::Command::new("cmd").args(["/c", "start", url]).spawn(); }
    ToolResult::ok("Opened billing portal")
}
```

### Phase 11 Verification

```bash
cargo build  # zero warnings
```

---

## Registry Updates (All Phases)

After all phases, `register_all_with_permissions()` in `registry.rs` will have:

```rust
// Existing (unchanged):
// system — always (core)
// web — "web" permission
// bot — always (core)
// event — always (core)
// skill — always (core)
// execute — when skill_loader + plan_tier
// message — always (core)
// desktop — "desktop" permission
// settings — "system" permission
// spotlight — "system" permission
// work — when workflow_manager

// New:
// app — "desktop" permission
if allowed("desktop") {
    self.register(Box::new(crate::app_tool::AppTool::new())).await;
}

// organizer — "organizer" permission
if allowed("organizer") {
    self.register(Box::new(crate::organizer_tool::OrganizerTool::new())).await;
}

// music — "media" permission
if allowed("media") {
    self.register(Box::new(crate::music_tool::MusicTool::new())).await;
}

// keychain — "system" permission
if allowed("system") {
    self.register(Box::new(crate::keychain_tool::KeychainTool::new())).await;
}

// loop — "loop" permission, only when comm is available
if allowed("loop") {
    if let Some(comm) = &comm_plugin {
        self.register(Box::new(crate::loop_tool::LoopTool::new(comm.clone()))).await;
    }
}
```

`lib.rs` additions:

```rust
pub mod app_tool;
pub mod keychain_tool;
pub mod loop_tool;
pub mod music_tool;
pub mod organizer_tool;

pub use app_tool::AppTool;
pub use keychain_tool::KeychainTool;
pub use loop_tool::LoopTool;
pub use music_tool::MusicTool;
pub use organizer_tool::OrganizerTool;
```

---

## File Change Summary

| File | Status | Description |
|------|--------|-------------|
| `crates/tools/src/desktop_tool.rs` | MODIFY | Add Linux/Windows to all resources + new resources (ui, menu, dialog, space, shortcut, tts) + snapshot infra + queue |
| `crates/tools/src/app_tool.rs` | NEW | App lifecycle (list, launch, quit, activate, frontmost) — cross-platform |
| `crates/tools/src/organizer_tool.rs` | NEW | PIM (mail, contacts, calendar, reminders) — cross-platform |
| `crates/tools/src/music_tool.rs` | NEW | Media control (play, pause, next, playlists) — cross-platform |
| `crates/tools/src/keychain_tool.rs` | NEW | Credential storage (get, find, add, delete) — cross-platform |
| `crates/tools/src/loop_tool.rs` | NEW | NeboLoop comms (dm, channel, group, topic) |
| `crates/tools/src/message_tool.rs` | MODIFY | Add SMS resource (macOS) + notify speak/dnd_status |
| `crates/tools/src/web_tool.rs` | MODIFY | Add devtools resource |
| `crates/tools/src/spotlight_tool.rs` | MODIFY | Add Windows search |
| `crates/tools/src/bot_tool.rs` | MODIFY | Add profile.open_billing |
| `crates/tools/src/skill_tool.rs` | MODIFY | Extend browse with featured/popular/reviews |
| `crates/tools/src/registry.rs` | MODIFY | Register new tools with permission gating |
| `crates/tools/src/lib.rs` | MODIFY | Export new modules |
| `crates/tools/Cargo.toml` | MODIFY | Add `comm` dependency |

**Total new files:** 5
**Total modified files:** 9
**Estimated new Rust LOC:** ~6,000-9,000

---

## Implementation Order

| # | Phase | Effort | Impact |
|---|-------|--------|--------|
| 1 | Phase 1: Desktop cross-platform (window, input, clipboard, capture, notification + new resources) | Large | Critical |
| 2 | Phase 2: App tool | Medium | High |
| 3 | Phase 3: Organizer (mail, contacts, calendar, reminders) | Large | High |
| 4 | Phase 4: Music tool | Small | Medium |
| 5 | Phase 5: Keychain tool | Small | Medium |
| 6 | Phase 6: Loop tool | Medium | Medium |
| 7 | Phase 7: Message completion | Small | Low |
| 8 | Phase 8: Web devtools | Small | Low |
| 9 | Phase 9-11: Search, Skill, Bot polish | Small | Low |

Each phase ends with `cargo build` — zero warnings required before proceeding.

---

## What This Plan Does NOT Do

- Does not create `platform/` subdirectories (Rule 8.1 — would introduce competing organizational pattern)
- Does not create a separate `store_tool.rs` (Rule 8.1 — skill marketplace is already in `skill_tool.rs`)
- Does not move clipboard to system tool (Rule 8.1 — clipboard already lives in `desktop_tool.rs`)
- Does not modify `system_tool.rs` to add resources (Rust pattern: separate tools, not system sub-resources)
- Does not create new crates (all new code in `crates/tools/src/`)
- Does not add `#[allow(dead_code)]` (Rule 3)
- Does not delete `desktop_tool.rs` and recreate (Rule 3 — preserve existing working code)
