# Platform Tools SME Document

Deep-dive reference for Nebo's platform-specific tools: Organizer (mail, contacts,
calendar, reminders), Music, Spotlight, Keychain, Desktop, and Settings. Covers
architecture, conditional compilation, platform dispatch, AppleScript/JXA/native
integration, cross-platform fallbacks, and security considerations.

---

## 1. Architecture Overview

Platform tools live under the unified `OsTool` which acts as a single STRAP domain
tool exposing 23 resources. Each resource delegates to a specialized inner tool or
handler module. Platform-specific code uses Rust's `#[cfg(target_os = "...")]` for
compile-time selection.

```
                            ┌──────────────────────┐
                            │      OsTool          │
                            │  (STRAP domain tool) │
                            │  resource + action   │
                            └──────────┬───────────┘
                                       │
        ┌──────────┬───────────┬───────┴────────┬──────────┬───────────┐
        │          │           │                │          │           │
   ┌────▼───┐ ┌───▼────┐ ┌───▼─────┐   ┌──────▼───┐ ┌───▼────┐ ┌───▼────────┐
   │FileTool│ │ShellTool│ │Desktop  │   │Organizer │ │Music   │ │Spotlight   │
   │        │ │        │ │Tool     │   │Module    │ │Tool    │ │Tool        │
   └────────┘ └────────┘ └────┬────┘   └────┬─────┘ └───┬────┘ └────┬───────┘
                               │              │           │           │
                          ┌────▼────┐    ┌────▼────┐  ┌──▼───┐  ┌───▼─────┐
                          │Settings │    │Keychain │  │      │  │         │
                          │Tool     │    │Tool     │  │macOS │  │macOS    │
                          └─────────┘    └─────────┘  │Music │  │mdfind   │
                                                      │.app  │  └─────────┘
                                                      └──────┘

                    ┌────────────────────────────────────────────┐
                    │          Platform Dispatch Layer           │
                    │                                            │
                    │  #[cfg(target_os = "macos")]  → AppleScript│
                    │                                 + Swift    │
                    │  #[cfg(target_os = "linux")]  → CLI tools  │
                    │  #[cfg(target_os = "windows")]→ PowerShell │
                    └────────────────────────────────────────────┘
```

### Crate Location

All platform tool source code lives in `crates/tools/src/`:

```
crates/tools/src/
├── os_tool.rs                 # Unified OsTool (STRAP: 23 resources)
├── music_tool.rs              # MusicTool (DynTool impl)
├── spotlight_tool.rs          # SpotlightTool (DynTool impl)
├── keychain_tool.rs           # KeychainTool (DynTool impl)
├── settings_tool.rs           # SettingsTool (DynTool impl)
├── desktop_tool.rs            # DesktopTool (DynTool impl)
├── app_tool.rs                # AppTool (DynTool impl)
└── organizer/
    ├── mod.rs                 # OrganizerInput + platform dispatch
    ├── shared.rs              # Cross-platform: date parsing, escaping, subprocess
    ├── macos.rs               # macOS: AppleScript handlers
    ├── native.rs              # macOS: Swift PIM helper (EventKit/Contacts)
    ├── pim_helper.swift       # Swift source (compiled at runtime)
    ├── linux.rs               # Linux: CLI backends (khal, notmuch, etc.)
    └── windows.rs             # Windows: PowerShell/Outlook COM
```

---

## 2. OsTool — The Unified Entry Point

`OsTool` is registered as a single STRAP domain tool named `"os"`. It owns instances
of every platform sub-tool and dispatches based on the `resource` field.

### Key Struct

```rust
// crates/tools/src/os_tool.rs
pub struct OsTool {
    file_tool: FileTool,
    shell_tool: ShellTool,
    desktop_tool: DesktopTool,
    app_tool: AppTool,
    settings_tool: SettingsTool,
    music_tool: MusicTool,
    keychain_tool: KeychainTool,
    spotlight_tool: SpotlightTool,
    store: Option<Arc<db::Store>>,
}
```

### Resource Routing

The `execute_dyn` method routes based on `resource`:

| Resource     | Inner Target                     | Platform-Specific? |
|-------------|----------------------------------|--------------------|
| `file`      | `FileTool`                       | No                 |
| `shell`     | `ShellTool`                      | No                 |
| `window`    | `DesktopTool`                    | Yes                |
| `input`     | `DesktopTool`                    | Yes                |
| `clipboard` | `DesktopTool`                    | Yes                |
| `capture`   | `DesktopTool`                    | Yes                |
| `notification`| `DesktopTool`                  | Yes                |
| `ui`        | `DesktopTool`                    | Yes                |
| `menu`      | `DesktopTool`                    | Yes                |
| `dialog`    | `DesktopTool`                    | Yes                |
| `space`     | `DesktopTool`                    | Yes                |
| `shortcut`  | `DesktopTool`                    | Yes                |
| `tts`       | `DesktopTool`                    | Yes                |
| `dock`      | `DesktopTool` (macOS only)       | Yes                |
| `app`       | `AppTool`                        | Yes                |
| `settings`  | `SettingsTool`                   | Yes                |
| `music`     | `MusicTool`                      | Yes                |
| `keychain`  | `KeychainTool`                   | Yes                |
| `search`    | `SpotlightTool`                  | Yes                |
| `mail`      | `organizer::handle_mail()`       | Yes                |
| `contacts`  | `organizer::handle_contacts()`   | Yes                |
| `calendar`  | `organizer::handle_calendar()`   | Yes                |
| `reminders` | `organizer::handle_reminders()`  | Yes                |

### Resource Inference

When an LLM omits the `resource` field (common), OsTool applies two inference layers:

1. **Action-based inference** (`infer_resource`): Maps action names to resources
   (e.g. `"play"` -> `"music"`, `"unread"` -> `"mail"`, `"screenshot"` -> `"capture"`)

2. **Context-based inference** (`infer_resource_from_context`): Examines parameter
   presence to disambiguate shared actions like `"create"`:
   - `date`/`calendar`/`end_date`/`days` present -> `"calendar"`
   - `list`/`due_date`/`priority` present -> `"reminders"`
   - `phone`/`company` present -> `"contacts"`
   - `to`/`subject`/`mailbox` present -> `"mail"`

### Approval Policy

Per-resource approval avoids blanket confirmation dialogs:

```
AUTO_APPROVE (no confirmation):
  file, shell, clipboard, capture, search, notification, tts, dock

ORGANIZER (read auto-approve, write requires approval):
  Read:  unread, accounts, read, search, today, upcoming, calendars, etc.
  Write: send, create, delete, complete, accept, decline

ALWAYS REQUIRE APPROVAL:
  input, window, ui, menu, dialog, app, settings, music, keychain, space, shortcut
```

---

## 3. Registration and Conditional Loading

### Compile-Time Platform Selection

Rust's `#[cfg(target_os = "...")]` gates are used at the module and function level.
The `organizer/mod.rs` uses conditional `pub use` to export the correct platform
handlers:

```rust
// crates/tools/src/organizer/mod.rs
#[cfg(target_os = "macos")]
pub use macos::{handle_calendar, handle_contacts, handle_mail, handle_reminders};

#[cfg(target_os = "linux")]
pub use linux::{handle_calendar, handle_contacts, handle_mail, handle_reminders};

#[cfg(target_os = "windows")]
pub use windows::{handle_calendar, handle_contacts, handle_mail, handle_reminders};
```

Unsupported platforms get stub implementations that return `ToolResult::error(...)`.

### Runtime Registration

OsTool is registered as a **deferred** tool during `register_all_with_permissions`:

```rust
// crates/tools/src/registry.rs  (register_all_with_permissions)
let mut os_tool = crate::os_tool::OsTool::new(policy, self.process_registry.clone())
    .with_store(store.clone());
// ...
self.register_deferred(Box::new(os_tool)).await;
```

"Deferred" means the tool's schema is not sent to the LLM until it is discovered
via `tool_search` or activated by keyword context matching in the conversation.

### Contextual Activation via tool_filter

The `tool_filter.rs` in `crates/agent/src/` defines keyword groups that control
when OS sub-contexts inject their STRAP documentation:

```rust
// crates/agent/src/tool_filter.rs
const CONTEXTUAL_GROUPS: &[(&str, &[&str])] = &[
    ("organizer", &["calendar", "reminder", "contact", "email", "schedule", ...]),
    ("music",     &["music", "play", "pause", "song", "playlist", ...]),
    ("settings",  &["volume", "brightness", "wifi", "bluetooth", ...]),
    ("keychain",  &["password", "credential", "keychain", "secret", ...]),
    ("spotlight", &["find file", "search file", "locate", "spotlight", ...]),
    ("desktop",   &["click", "mouse", "keyboard", "window", "screenshot", ...]),
    ("app",       &["launch", "open app", "close app", "running app", ...]),
];
```

When the user's message matches keywords from a group, the `os` tool is included
in the filtered tool list, and the corresponding STRAP sub-documentation is injected
into the system prompt for that turn.

---

## 4. Organizer Tool (Mail, Contacts, Calendar, Reminders)

The organizer is the most complex platform tool, spanning four PIM resources across
three platforms with a dual-path architecture on macOS.

### 4.1 Shared Infrastructure

**File:** `crates/tools/src/organizer/shared.rs`

```
┌───────────────────────────────────────────────────────┐
│                   shared.rs                            │
│                                                        │
│  parse_date()       — Natural language + ISO dates     │
│  escape_applescript() — macOS string escaping           │
│  escape_powershell()  — Windows string escaping         │
│  run_osascript()    — macOS subprocess (30s timeout)   │
│  run_command()      — Linux/Windows subprocess         │
│  run_command_with_stdin() — Pipe data to stdin          │
│  run_powershell()   — Windows PowerShell               │
│  which_exists()     — Binary availability check         │
└───────────────────────────────────────────────────────┘
```

**Date parsing** supports multiple formats:
- ISO: `"2024-06-15 14:00"`, `"2024-06-15"`
- US: `"06/15/2024 14:00"`, `"06/15/2024"`
- Natural: `"today"`, `"tomorrow"`, `"in 2 hours"`, `"in 3 days"`, `"in 1 week"`

**AppleScript escaping** handles backslash, double-quote, newline, and tab characters:

```rust
pub fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}
```

**Subprocess execution** uses `tokio::process::Command` with:
- `kill_on_drop(true)` — process is killed if the Future is dropped
- 30-second timeout via `tokio::time::timeout`
- Direct argument passing (no shell interpolation) for injection safety

### 4.2 OrganizerInput Type

```rust
// crates/tools/src/organizer/mod.rs
pub struct OrganizerInput {
    pub action: String,
    pub resource: String,          // routing hint (handled by OsTool caller)

    // Mail fields
    pub to: Vec<String>,           // custom deserializer: string OR array
    pub cc: Vec<String>,
    pub subject: String,
    pub body: String,
    pub mailbox: String,

    // Contacts fields
    pub query: String,
    pub name: String,
    pub email: String,
    pub phone: String,
    pub company: String,
    pub notes: String,

    // Calendar fields
    pub calendar: String,
    pub date: String,
    pub end_date: String,
    pub location: String,
    pub title: String,
    pub days: Option<i64>,
    pub repeat: String,
    pub interval: Option<i32>,
    pub end_repeat: String,
    pub repeat_days: String,       // alias: "weekdays"

    // Reminder fields
    pub list: String,
    pub due_date: String,
    pub priority: Option<i32>,

    // Shared
    pub limit: Option<i64>,
}
```

The `to` field uses a custom `string_or_vec` deserializer that accepts both
`"user@example.com"` (single string) and `["a@b.com", "c@d.com"]` (array).

### 4.3 macOS Implementation — Dual Path Architecture

macOS uses a two-tier approach: native Swift helper (fast, preferred) with
AppleScript fallback (universal, slower).

```
                    ┌─────────────────────────────────┐
                    │   macOS Organizer Request        │
                    │   handle_calendar/contacts/etc   │
                    └───────────────┬─────────────────┘
                                    │
                              ┌─────▼──────┐
                              │ Try native │
                              │ pim-helper │
                              └─────┬──────┘
                                    │
                         ┌──────────▼───────────┐
                         │ native::run_pim()    │
                         │                      │
                         │ Returns Some(result) │──────► Done (fast path)
                         │ Returns None         │──────► AppleScript fallback
                         └──────────────────────┘
                                    │
                              ┌─────▼──────────┐
                              │ run_osascript() │
                              │ (30s timeout)   │
                              └────────────────┘
```

#### Native Swift PIM Helper

**File:** `crates/tools/src/organizer/native.rs` + `pim_helper.swift`

The Swift helper binary is compiled at runtime from embedded source and cached at
`~/.nebo/data/bin/pim-helper`. It uses Apple's native frameworks directly:

- **EventKit** — Calendar events and Reminders
- **CNContactStore** (Contacts framework) — Contact records

```
Compilation and Caching Flow:
═══════════════════════════════

1. ensure_helper() called on first organizer request
2. Checks HELPER_PATH (OnceLock<Mutex<Option<PathBuf>>>)
3. If cached and binary exists → return path immediately
4. Otherwise:
   a. Write PIM_HELPER_SOURCE (include_str!) to ~/.nebo/data/bin/pim_helper.swift
   b. Run: swiftc -O -framework EventKit -framework Contacts -o pim-helper pim_helper.swift
   c. Write FNV-1a hash of source to pim-helper.hash
   d. Cache path in HELPER_PATH
5. On Nebo update: source hash changes → auto-recompile
```

The hash comparison ensures the binary is recompiled when the embedded Swift source
changes (e.g. after a Nebo update), but never unnecessarily recompiled otherwise:

```rust
const PIM_HELPER_SOURCE: &str = include_str!("pim_helper.swift");
const SOURCE_HASH: u64 = const_fnv1a(PIM_HELPER_SOURCE.as_bytes());
```

The helper is invoked as a CLI:

```
pim-helper <domain> <action> [--key value ...]

Examples:
  pim-helper calendar today --days 1
  pim-helper contacts search --query "John"
  pim-helper reminders create --name "Buy milk" --due_date "tomorrow"
```

**Permission handling** in Swift:
- macOS 14+: Uses `requestFullAccessToEvents` / `requestFullAccessToReminders`
- Earlier macOS: Uses `requestAccess(to: .event)` / `requestAccess(to: .reminder)`
- Contacts: Uses `requestAccess(for: .contacts)`
- On denial, prints `ERROR: CALENDAR_PERMISSION_REQUIRED` (etc.) and exits with code 1

**Output protocol**: The helper prints pipe-separated text to stdout. Lines starting
with `ERROR:` are treated as errors by `native::run_pim()`:

```rust
if stdout.starts_with("ERROR: ") {
    Some(ToolResult::error(stdout.trim_start_matches("ERROR: ").to_string()))
} else if o.status.success() {
    Some(ToolResult::ok(stdout))
}
```

If `run_pim()` returns `None` (helper unavailable / swiftc missing / compilation
failure), the caller falls through to AppleScript.

#### AppleScript Fallback (macOS)

AppleScript is used as the universal fallback. Scripts are constructed as Rust format
strings and executed via `osascript -e`:

```rust
pub async fn run_osascript(script: &str) -> ToolResult {
    let child = tokio::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    // ... 30s timeout, parse stdout/stderr ...
}
```

### 4.4 Mail (macOS)

**File:** `crates/tools/src/organizer/macos.rs` (lines 12-105)

| Action    | AppleScript Target    | Notes                           |
|----------|-----------------------|---------------------------------|
| `accounts`| Mail → every account  | Returns account names           |
| `unread`  | Mail → inbox unread   | Count of unread messages        |
| `read`    | Mail → messages 1..N  | Configurable limit (1-50)       |
| `send`    | Mail → new outgoing   | To + CC + subject + body        |
| `search`  | Mail → search inbox   | Query string, limit 1-50        |

Mail does NOT use the native helper — all operations go through AppleScript since
Apple does not expose Mail.framework for third-party use.

### 4.5 Contacts (macOS)

**File:** `crates/tools/src/organizer/macos.rs` (lines 110-257)

Contacts uses a native-first approach:

```rust
pub async fn handle_contacts(action: &str, input: &OrganizerInput) -> ToolResult {
    // Try native Contacts framework first
    if let Some(result) = super::native::run_pim("contacts", action, &args).await {
        return result;
    }
    // AppleScript fallback
    match action { ... }
}
```

| Action   | Native (CNContactStore) | AppleScript Fallback          |
|----------|------------------------|-------------------------------|
| `search` | Yes — fast, no app     | Contacts.app search           |
| `get`    | Yes                    | Contacts.app person detail    |
| `create` | Yes                    | Contacts.app make new person  |
| `groups` | Yes                    | Contacts.app every group      |

### 4.6 Calendar (macOS)

**File:** `crates/tools/src/organizer/macos.rs` (lines 260-745)

Calendar is the most complex organizer resource with:
- Native EventKit (preferred) with AppleScript fallback
- Calendar preference persistence (DB + legacy file migration)
- Auto-accept for calendar invitations
- Configurable calendar filtering via UI widget
- Recurrence rule support (daily/weekly/monthly/yearly with day-of-week and end date)

```
Calendar Request Flow:
═══════════════════════

1. Auto-accept check (for today/upcoming/list):
   - Load CalendarPrefs from DB
   - If auto_accept=true, fire native::run_pim("calendar", "accept", [])
   - Log any accepted invites

2. Try native EventKit path:
   - Build args from OrganizerInput fields
   - Call native::run_pim("calendar", action, &args)
   - If Some(result) → return

3. AppleScript fallback:
   - For "configure": list calendars, show checkbox widget, save prefs
   - For event queries: query_calendar_events() with per-calendar timeout
   - For create/delete: build AppleScript with date formatting
```

**Calendar Preferences** (stored in DB `plugin_settings` table):

```rust
struct CalendarPrefs {
    calendars: Vec<String>,    // which calendars to track
    auto_accept: bool,         // auto-accept pending invitations
}
```

The preferences system migrates from a legacy `~/.nebo/data/calendar_preferences.json`
file to the DB on first access. Preferences control which calendars are queried,
reducing latency from querying all 18+ calendars to only 3-5 selected ones.

**The bulk calendar query** uses a single `osascript` process with AppleScript's
`with timeout of 15 seconds` per calendar, avoiding 18+ separate subprocess spawns:

```applescript
tell application "Calendar"
    repeat with cal in allCals
        try
            with timeout of 15 seconds
                set evts to (every event of cal whose start date >= today ...)
            end timeout
        on error
            set skippedCals to skippedCals & (name of cal) & ", "
        end try
    end repeat
end tell
```

Overall timeout: 60s with saved preferences, 180s without.

| Action       | Native (EventKit) | AppleScript Fallback       |
|-------------|-------------------|----------------------------|
| `today`     | Yes               | query_calendar_events(1)   |
| `upcoming`  | Yes               | query_calendar_events(N)   |
| `list`      | Yes               | query_calendar_events(N)   |
| `create`    | Yes (+ recurrence)| AppleScript event create   |
| `delete`    | Yes               | AppleScript event delete   |
| `calendars` | Yes               | Calendar → every calendar  |
| `configure` | No (needs UI)     | Checkbox widget + save     |
| `pending`   | Yes               | N/A                        |
| `accept`    | Yes               | N/A                        |
| `decline`   | Yes               | N/A                        |
| `auto_accept`| DB toggle only   | DB toggle only             |

### 4.7 Reminders (macOS)

**File:** `crates/tools/src/organizer/macos.rs` (lines 750-912)

| Action    | Native (EventKit) | AppleScript Fallback         |
|----------|-------------------|------------------------------|
| `lists`  | Yes               | Reminders → every list       |
| `list`   | Yes               | Reminders → incomplete items |
| `create` | Yes               | Reminders → make new reminder|
| `complete`| Yes              | Reminders → set completed    |
| `delete` | Yes               | Reminders → delete reminder  |

Priority mapping (AppleScript): 1-4 = High (1), 5 = Medium (5), 6-9 = Low (9).

---

## 5. Music Tool

**File:** `crates/tools/src/music_tool.rs`

Standalone `DynTool` implementation. Cross-platform with compile-time dispatch.

### Struct and DynTool Interface

```rust
pub struct MusicTool;

impl DynTool for MusicTool {
    fn name(&self) -> &str { "music" }
    fn requires_approval(&self) -> bool { false }
    // ...
}
```

Note: `requires_approval` returns `false` on MusicTool itself, but OsTool overrides
this — when routed through `os(resource: "music")`, approval IS required (because
`"music"` is not in `AUTO_APPROVE_RESOURCES`).

### Actions

| Action     | macOS                    | Linux (playerctl)        | Windows           |
|-----------|--------------------------|--------------------------|-------------------|
| `play`    | `Music.app → play`       | `playerctl play`         | Not supported     |
| `pause`   | `Music.app → pause`      | `playerctl pause`        | Not supported     |
| `next`    | `Music.app → next track` | `playerctl next`         | Not supported     |
| `previous`| `Music.app → previous`   | `playerctl previous`     | Not supported     |
| `status`  | current track + state    | `playerctl metadata`     | Process list      |
| `search`  | search Library playlist  | Not supported            | Not supported     |
| `volume`  | get/set sound volume     | `playerctl volume`       | Not supported     |
| `playlists`| every playlist name     | Not supported            | Not supported     |
| `shuffle` | toggle shuffle enabled   | `playerctl shuffle`      | Not supported     |

### macOS AppleScript Patterns

```applescript
-- Status: get current track info
tell application "Music"
    set s to player state as text
    if s is "stopped" then return "Not playing"
    try
        return (name of current track) & " - " & (artist of current track) & " [" & s & "]"
    on error
        return s
    end try
end tell

-- Search: query library
tell application "Music"
    set results to search playlist "Library" for "query"
    ...
end tell
```

The music tool has its own `run_osascript` helper (not shared with organizer) because
it does not need the 30s timeout or `kill_on_drop` behavior of the organizer's version.

---

## 6. Spotlight Tool

**File:** `crates/tools/src/spotlight_tool.rs`

File search using the OS-native search index.

### Struct

```rust
pub struct SpotlightTool;

impl DynTool for SpotlightTool {
    fn name(&self) -> &str { "spotlight" }
    fn requires_approval(&self) -> bool { false }
    // ...
}
```

### Single Action: `search`

Parameters:
- `query` (required): Search query or filename pattern
- `dir` (optional): Directory to search within
- `limit` (optional, default 50): Maximum results

### Platform Backends

| Platform | Backend     | Details                                    |
|----------|------------|---------------------------------------------|
| macOS    | `mdfind`   | Native Spotlight CLI. Supports `-onlyin dir`|
| Linux    | `plocate`  | Fast indexed search. Falls back to `find`   |
| Windows  | PowerShell | `Get-ChildItem -Recurse -Filter` pattern    |

macOS `mdfind` is the most powerful — it searches the full Spotlight metadata index
including file content, metadata attributes, and filename patterns. No special
permissions required beyond file system access.

```rust
// macOS path
let mut cmd = tokio::process::Command::new("mdfind");
if !dir.is_empty() {
    cmd.arg("-onlyin").arg(dir);
}
cmd.arg(query);
```

---

## 7. Keychain Tool

**File:** `crates/tools/src/keychain_tool.rs`

Cross-platform credential storage management. This is the ONLY platform tool that
sets `requires_approval` to `true` at the DynTool level.

### Struct

```rust
pub struct KeychainTool;

impl DynTool for KeychainTool {
    fn name(&self) -> &str { "keychain" }
    fn requires_approval(&self) -> bool { true }  // ALL actions need approval
    // ...
}
```

### Actions

| Action   | macOS (`security` CLI)                    | Linux (`secret-tool`)         | Windows (`cmdkey`)        |
|---------|-------------------------------------------|-------------------------------|---------------------------|
| `get`   | `find-generic-password -s SVC -a ACCT -w` | `lookup service SVC account`  | `/list:SVC` (metadata)    |
| `find`  | `find-generic-password -l LABEL`          | `search --all label LABEL`    | `/list` + grep            |
| `add`   | `add-generic-password -s -a -w -U`        | `store --label SVC account`   | `/add:SVC /user: /pass:`  |
| `delete`| `delete-generic-password -s -a`           | `clear service SVC account`   | `/delete:SVC`             |

### macOS Details

Uses the `security` CLI which interacts with the macOS Keychain. The `-U` flag on
`add-generic-password` means "update if exists" (upsert behavior).

```rust
// Get password
run_command("security", &["find-generic-password", "-s", service, "-a", account, "-w"])

// Add/update password
run_command("security", &[
    "add-generic-password", "-s", service, "-a", account, "-w", password, "-U"
])
```

Note: `security` may trigger a macOS Keychain authorization dialog asking the user
to allow access. This is an OS-level prompt, not a Nebo prompt.

---

## 8. Desktop Tool and Settings Tool

These are not macOS-exclusive but have significant platform-specific internals.

### DesktopTool

**File:** `crates/tools/src/desktop_tool.rs`

Handles 12 resources: window, input, clipboard, capture, notification, ui, menu,
dialog, space, shortcut, tts, dock.

macOS uses a combination of:
- **AppleScript** for window management, notifications, TTS
- **Accessibility API** (`AXUIElement`) for UI tree inspection via a Swift helper
- **screencapture** CLI for screenshots
- **pbcopy/pbpaste** for clipboard

Windows uses a persistent PowerShell daemon (`DesktopDaemon`) to avoid the 1-2s
startup cost of launching a new PowerShell process for each operation.

### SettingsTool

**File:** `crates/tools/src/settings_tool.rs`

System settings control. macOS implementations use `osascript` for volume, brightness,
dark mode, and `networksetup` / `blueutil` CLI tools for connectivity.

---

## 9. Cross-System Interactions

### Tool Registry Integration

```
┌──────────┐     register_all_with_permissions()     ┌──────────────┐
│ Registry │◄──── register_deferred(OsTool) ─────────│ OsTool       │
│          │                                          │ (deferred)   │
└────┬─────┘                                          └──────────────┘
     │
     │  execute()
     │  ├─ acquire ResourcePermit (Screen/Browser)
     │  ├─ call tool.execute_dyn(ctx, input)
     │  └─ release permit
     │
     ▼
┌──────────────────┐
│  ResourcePermits │  Screen mutex serializes:
│  screen: Mutex   │  window, input, ui, menu,
│  browser: Mutex  │  dialog, space, shortcut
└──────────────────┘
```

OsTool declares `resource_permit()` to return `ResourceKind::Screen` for physical
input resources (window, input, ui, menu, dialog, space, shortcut). This ensures
concurrent agents cannot fight over the same mouse/keyboard.

Parallelizable resources (file, shell, capture, app, music, keychain, search, mail,
contacts, calendar, reminders) return `None` — no permit needed.

### Agent Runner Integration

The agent runner calls tools via the registry. Platform tools participate in:

1. **Tool filtering** — `tool_filter.rs` decides which tools appear in each turn
2. **STRAP doc injection** — When an organizer/music/keychain context is active,
   the corresponding sub-documentation is injected into the system prompt
3. **Approval flow** — `requires_approval_for()` is checked before execution;
   if true, the agent pauses for user confirmation via `ctx.ask_user()`

### Store Dependencies

Only two platform tools use the DB store:
- **OsTool** holds `Option<Arc<db::Store>>` for calendar preferences
- **Calendar configure** action uses `ctx.ask_user()` for the checkbox widget

---

## 10. AppleScript/JXA Execution Patterns

### Pattern 1: Simple One-Liner

```rust
run_osascript("tell application \"Mail\" to return name of every account").await
```

Used for read-only queries that return simple values.

### Pattern 2: Multi-Line Script with Formatting

```rust
let script = format!(r#"tell application "Calendar"
    set today to current date
    set evts to (every event of cal whose start date >= today ...)
    repeat with e in evts
        set output to output & (name of cal) & " | " & (summary of e) & linefeed
    end repeat
    return output
end tell"#);
run_osascript(&script).await
```

Used for queries that iterate over collections and build formatted output.

### Pattern 3: Script with Dynamic Properties

```rust
let mut script = format!(r#"tell application "Contacts"
    set newPerson to make new person with properties {{first name:"{first}", last name:"{last}"}}"#);
if !input.email.is_empty() {
    script.push_str(&format!("..."));
}
script.push_str("...");
run_osascript(&script).await
```

Used for create operations where properties are conditionally added.

### Pattern 4: Custom Timeout (Calendar)

```rust
let child = tokio::process::Command::new("osascript")
    .arg("-e").arg(&script)
    .kill_on_drop(true)
    .spawn()?;

tokio::time::timeout(overall_timeout, child.wait_with_output()).await
```

Used when the default 30s timeout is insufficient (calendar can take up to 180s).

### Injection Prevention

All user-supplied strings pass through `escape_applescript()` before interpolation
into AppleScript templates. The function escapes:
- `\` -> `\\` (backslash)
- `"` -> `\"` (double quote)
- `\n` -> `\\n` (newline)
- `\t` -> `\\t` (tab)

Additionally, the `run_command()` family uses direct argument passing
(`Command::new("osascript").arg("-e").arg(script)`) rather than shell interpolation,
preventing command injection at the subprocess level.

---

## 11. Security Considerations

### Keychain Access

The keychain tool requires approval for ALL operations (`requires_approval: true`).
On macOS, `security find-generic-password -w` may trigger an OS-level Keychain
authorization dialog. Nebo cannot bypass this — the OS controls access.

The `-U` flag on `add-generic-password` enables upsert (update-if-exists) behavior,
which avoids duplicate entries but means an LLM could overwrite existing credentials.
The approval gate prevents this from happening without user consent.

### Privacy Permissions (macOS)

macOS enforces per-app privacy permissions. Nebo (or the terminal running it) needs:

| Permission                     | Required For                          | System Settings Path                           |
|-------------------------------|---------------------------------------|-----------------------------------------------|
| **Contacts**                  | Organizer contacts                    | Privacy & Security > Contacts                 |
| **Calendars**                 | Organizer calendar                    | Privacy & Security > Calendars                |
| **Reminders**                 | Organizer reminders                   | Privacy & Security > Reminders                |
| **Mail** (Automation)         | Organizer mail (AppleScript)          | Privacy & Security > Automation > Mail.app    |
| **Accessibility**             | Desktop input, UI tree, click         | Privacy & Security > Accessibility            |
| **Screen Recording**          | Desktop capture (screenshot/see)      | Privacy & Security > Screen Recording         |
| **Full Disk Access**          | File tool (access all directories)    | Privacy & Security > Full Disk Access         |
| **Automation** (general)      | AppleScript control of apps           | Privacy & Security > Automation               |

The Swift PIM helper handles permission requests natively:

```swift
func ensureCalendarAccess() {
    // macOS 14+
    eventStore.requestFullAccessToEvents { granted, _ in ... }
    guard granted else {
        print("ERROR: CALENDAR_PERMISSION_REQUIRED — grant Calendar access ...")
        exit(1)
    }
}
```

### Secret Scanning

Nebo has a separate `secret_scan.rs` module (in the agent crate) that can detect
secrets in tool outputs. The keychain tool's output could contain credentials —
the approval gate is the primary defense, but secret scanning provides defense-in-depth.

### Subprocess Safety

All subprocess execution follows these rules:
1. **No shell interpolation** — arguments passed directly to `Command`
2. **kill_on_drop(true)** — orphan processes are cleaned up
3. **Timeouts** — 30s default, configurable for slow operations
4. **String escaping** — platform-specific escaping for embedded scripts
5. **stdin piping** — `run_command_with_stdin()` avoids shell injection for body content

---

## 12. Error Handling

### Permission Denied

Each platform tool handles permission denial gracefully:

**macOS (native helper):**
```
ERROR: CALENDAR_PERMISSION_REQUIRED — grant Calendar access in System Settings > Privacy & Security > Calendars
```

**macOS (AppleScript):**
```
AppleScript error: System Events got an error: osascript is not allowed to send keystrokes.
```

**Linux (missing backend):**
```
No mail client found. Install one of:
- neomutt (recommended): sudo apt install neomutt
- mutt: sudo apt install mutt
```

**Windows (no Outlook):**
```
Mail requires Microsoft Outlook on Windows
```

### App Not Running

AppleScript `tell application "X"` will launch the target application if it's not
running. This is generally acceptable for Mail, Calendar, Contacts, Reminders, and
Music. The native Swift helper does NOT activate apps — it reads directly from the
OS frameworks' local caches.

### Timeout Handling

Organizer calendar queries can be slow (especially on first launch after reboot):
- Per-calendar: 15s AppleScript timeout within the script
- Overall: 60s (with preferences) / 180s (without)
- On timeout: suggests using `configure` to select fewer calendars
- Skipped calendars are reported in the output

### Native Helper Compilation Failure

If `swiftc` is not available (Xcode not installed) or compilation fails:

```rust
Err(e) => {
    tracing::warn!(%e, "swiftc not found, falling back to AppleScript");
    None  // caller falls through to AppleScript
}
```

This is a graceful degradation — the user gets the same functionality, just slower.

---

## 13. Linux and Windows Platform Implementations

### Linux (organizer/linux.rs)

Uses auto-detection of installed CLI tools:

```
Mail:      neomutt > mutt > s-nail > mail > sendmail (send)
           notmuch > mutt/neomutt (read)
Contacts:  khard > abook
Calendar:  khal > gcalcli > calcurse
Reminders: taskwarrior (task) > todo.sh
```

Each `detect_*()` function iterates through backends in preference order using
`which_exists()`. The first available backend is used.

### Windows (organizer/windows.rs)

Uses Outlook COM automation via PowerShell. A cached `has_outlook()` check
(with 10s timeout) detects whether Outlook is available. Reminders fall back
to Windows Task Scheduler (`Register-ScheduledTask` + `msg.exe`) when Outlook
is not installed.

Outlook folder IDs: 6=Inbox, 5=SentItems, 16=Drafts, 3=Deleted, 4=Outbox,
9=Calendar, 10=Contacts, 13=Tasks.

---

## 14. Testing

Platform tools have unit tests for:

1. **Input parsing** — OrganizerInput deserialization (to: string vs array, defaults)
2. **Date parsing** — All supported formats including natural language
3. **String escaping** — AppleScript, PowerShell escape correctness
4. **Tool metadata** — name, description, schema validation
5. **Action routing** — unknown action returns descriptive error
6. **Approval policy** — read vs write actions, auto-approve vs require-approval
7. **Resource inference** — action-based and context-based resource detection
8. **Binary detection** — `which_exists()` for installed/missing tools

Integration tests (running actual AppleScript or CLI tools) are not included in
the test suite due to CI environment constraints. Platform-specific tests are
gated with `#[cfg(target_os = "macos")]`.

Run tests:
```bash
cargo test -p nebo-tools                        # all tool tests
cargo test -p nebo-tools -- organizer           # organizer tests only
cargo test -p nebo-tools -- test_approval       # approval policy tests
```

---

## 15. Key Design Decisions

1. **Single OsTool entry point** — Reduces token count by ~80% vs 25 separate tools.
   The LLM sees one `os(resource, action)` interface instead of 25 tool schemas.

2. **Native-first with AppleScript fallback** — The Swift PIM helper is 10-100x
   faster than AppleScript for calendar/contacts/reminders (reads local SQLite cache
   directly via EventKit/CNContactStore instead of Apple Events IPC). AppleScript
   serves as universal fallback when `swiftc` is unavailable.

3. **Runtime Swift compilation** — Avoids shipping a pre-compiled binary for every
   macOS architecture. The FNV-1a hash mechanism ensures efficient recompilation
   only when the source changes.

4. **Calendar preference persistence** — Querying all 18+ calendars via AppleScript
   can take minutes on slow iCloud-synced accounts. Letting users select 3-5
   calendars via `configure` reduces query time to seconds.

5. **Deferred registration** — OsTool is deferred to save ~8-10K tokens on requests
   that do not involve OS operations. It activates when keywords match or the model
   calls `tool_search`.

6. **Per-resource approval** — Avoids "confirmation fatigue" by auto-approving safe
   operations (file reads, clipboard, search) while still gating dangerous ones
   (sending email, deleting events, keychain writes).
