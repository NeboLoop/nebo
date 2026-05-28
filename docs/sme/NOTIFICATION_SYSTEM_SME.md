# Notification System SME

> Deep-dive reference for Nebo's notification infrastructure. Covers OS-level native
> notifications, in-app persistent notifications, WebSocket real-time push, user
> preferences, and cross-system integration points.

---

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [Notification Layers](#notification-layers)
3. [Native OS Notifications (nebo-notify crate)](#native-os-notifications)
4. [In-App Persistent Notifications](#in-app-persistent-notifications)
5. [WebSocket Real-Time Push](#websocket-real-time-push)
6. [Frontend Notification Store](#frontend-notification-store)
7. [NotificationBell Component](#notificationbell-component)
8. [Message Tool Integration](#message-tool-integration)
9. [Cross-System Trigger Points](#cross-system-trigger-points)
10. [User Preferences](#user-preferences)
11. [Database Schema](#database-schema)
12. [REST API](#rest-api)
13. [Do Not Disturb Detection](#do-not-disturb-detection)
14. [Alert Dialogs (Modal Notifications)](#alert-dialogs)
15. [Input Sanitization and Security](#input-sanitization-and-security)
16. [Notification Lifecycle and Cleanup](#notification-lifecycle-and-cleanup)
17. [Platform-Specific Implementation Details](#platform-specific-implementation-details)
18. [Error Handling Strategy](#error-handling-strategy)
19. [Current Limitations and Future Work](#current-limitations-and-future-work)
20. [Key Files Reference](#key-files-reference)

---

## Architecture Overview

Nebo has a **three-layer notification architecture**: native OS notifications for
system-tray-level alerts, persistent in-app notifications stored in SQLite, and
real-time WebSocket push to the frontend. These layers operate independently but are
often triggered together from the same call site.

```
+----------------------------------------------------------------------+
|                        TRIGGER SOURCES                                |
|                                                                       |
|  Scheduler      Workflow Manager     Agent Worker     Message Tool    |
|  (cron jobs)    (run failures)       (heartbeat,      (owner notify,  |
|                                       watch errors)    system send)   |
+------+----------+------+------------+------+---------+------+---------+
       |                 |                   |                |
       v                 v                   v                v
+----------------------------------------------------------------------+
|                    NOTIFICATION DISPATCH                               |
|                                                                       |
|  +----------------------------+  +--------------------------------+   |
|  | notify_crate::send()       |  | store.create_notification()    |   |
|  | (native OS, fire-and-      |  | (SQLite persistence)           |   |
|  |  forget, currently NO-OP)  |  |                                |   |
|  +----------------------------+  +------+-------------------------+   |
|                                         |                             |
|                                         v                             |
|                              +----------------------+                 |
|                              | hub.broadcast(       |                 |
|                              |  "notification_      |                 |
|                              |   created", payload) |                 |
|                              +----------+-----------+                 |
+----------------------------------------------------------------------+
                                          |
                                          | WebSocket
                                          v
+----------------------------------------------------------------------+
|                      FRONTEND (SvelteKit)                             |
|                                                                       |
|  +---------------------------+     +------------------------------+   |
|  | WebSocket Listener        |     | notifications store          |   |
|  | on("notification_created")|---->| (Svelte writable)            |   |
|  | on("notification")        |     |                              |   |
|  +---------------------------+     +------+-----------------------+   |
|                                           |                           |
|                                           v                           |
|                                    +---------------+                  |
|                                    | Notification  |                  |
|                                    | Bell (UI)     |                  |
|                                    +---------------+                  |
+----------------------------------------------------------------------+
```

---

## Notification Layers

### Layer 1: Native OS Notifications

Handled by the `nebo-notify` crate (`crates/notify/`). Fires platform-native toast
notifications via shell commands. **Currently disabled** — the public `send()` function
is a no-op because OS notifications lack deep-linking (clicking them does nothing
useful). Platform-specific `send_platform()` functions remain implemented but are gated
behind `#[allow(dead_code)]`, awaiting tauri-plugin-notification integration.

### Layer 2: In-App Persistent Notifications

Stored in the `notifications` SQLite table. These survive restarts and provide a
history the user can browse, mark as read, and delete. Each notification has a `type`,
`title`, `body`, optional `action_url` (for deep-linking to a specific page), and
`read_at` tracking.

### Layer 3: WebSocket Real-Time Push

After persisting a notification, the server broadcasts a `notification_created` event
via `ClientHub`. The frontend listener receives it and prepends it to the in-memory
store without an API round-trip. A separate legacy `notification` event also exists for
generic push notifications that may not be persisted.

---

## Native OS Notifications

### Crate: `nebo-notify`

**File:** `crates/notify/src/lib.rs`
**Cargo.toml dependency alias:** `notify_crate` (to avoid collision with the `notify`
file-watcher crate)

```rust
// Workspace Cargo.toml
notify_crate = { path = "crates/notify", package = "nebo-notify" }
```

### Public API

```rust
/// Send a native OS notification. Falls back silently if unavailable.
/// Currently disabled (no-op) — awaiting tauri-plugin-notification.
pub fn send(_title: &str, _body: &str) {
    // TODO: replace with tauri-plugin-notification for deep-linked notifications
}
```

The function signature accepts `title` and `body`. It is intentionally a no-op. When
re-enabled, it will call the platform-specific `send_platform()` implementation.

### Platform Implementations

All platform functions share the same signature:

```rust
fn send_platform(title: &str, body: &str) -> Result<(), String>
```

#### macOS

Uses `osascript` to invoke AppleScript's `display notification`:

```rust
#[cfg(target_os = "macos")]
fn send_platform(title: &str, body: &str) -> Result<(), String> {
    let script = format!("display notification \"{}\" with title \"{}\"", body, title);
    Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| e.to_string())?;
    Ok(())
}
```

This uses macOS Notification Center via AppleScript. Limitations: no custom icons, no
sound control, no action buttons, no deep-linking. The notification appears as coming
from "osascript" rather than "Nebo" in the notification center.

#### Linux

Uses `notify-send` from the `libnotify` package:

```rust
#[cfg(target_os = "linux")]
fn send_platform(title: &str, body: &str) -> Result<(), String> {
    Command::new("notify-send")
        .args([title, body])
        .output()
        .map_err(|e| e.to_string())?;
    Ok(())
}
```

Requires `libnotify-bin` (Ubuntu/Debian) or equivalent. Uses the freedesktop
notification specification. Supports GNOME, KDE, XFCE notification daemons.

#### Windows

Uses PowerShell to access the Windows Runtime `ToastNotificationManager`:

```rust
#[cfg(target_os = "windows")]
fn send_platform(title: &str, body: &str) -> Result<(), String> {
    let ps = format!(
        r#"
[Windows.UI.Notifications.ToastNotificationManager, ...] > $null
$template = ...GetTemplateContent(ToastTemplateType::ToastText02)
$textNodes = $template.GetElementsByTagName('text')
$textNodes.Item(0).AppendChild($template.CreateTextNode('{}')) > $null
$textNodes.Item(1).AppendChild($template.CreateTextNode('{}')) > $null
$toast = [Windows.UI.Notifications.ToastNotification]::new($template)
[..ToastNotificationManager]::CreateToastNotifier('Nebo').Show($toast)
"#,
        title, body
    );
    Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .output()
        .map_err(|e| e.to_string())?;
    Ok(())
}
```

Uses the WinRT `ToastText02` template (two-line text toast). Registers as the "Nebo"
app ID. The `-NoProfile -NonInteractive` flags ensure fast, non-blocking execution.

#### Unsupported Platforms

Returns `Ok(())` silently — no error, no notification.

### Input Sanitization

The `sanitize()` function protects against shell injection in notification text:

```rust
fn sanitize(s: &str) -> String {
    let s = s.replace('\'', "\u{2019}"); // straight quote -> curly quote
    let s = s.replace('\\', "");          // strip backslashes
    let s = s.replace('"', "\u{201C}");   // straight double quote -> curly
    if s.len() > 256 {
        // Truncate at valid UTF-8 boundary
        let mut end = 256;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    } else {
        s
    }
}
```

Key behaviors:
- Replaces straight quotes with curly Unicode equivalents (prevents shell escape)
- Strips backslashes entirely
- Truncates at 256 bytes (char-boundary-safe) with "..." suffix
- Currently not called from `send()` because it is a no-op, but will be used when
  re-enabled

---

## In-App Persistent Notifications

### Database Model

**File:** `crates/db/src/models.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub id: String,
    pub user_id: String,
    #[serde(rename = "type")]
    pub notification_type: String,
    pub title: String,
    pub body: Option<String>,
    pub action_url: Option<String>,
    pub icon: Option<String>,
    pub read_at: Option<i64>,
    pub created_at: i64,
}
```

### Notification Types

The `notification_type` field is a free-form string. Observed types in the codebase:

| Type      | Source                        | Description                                    |
|-----------|-------------------------------|------------------------------------------------|
| `"info"`  | `message_tool` notify/send    | Informational notification from agent           |
| `"error"` | `workflow_manager` failures   | Workflow run failure with deep-link to run page |
| `"warning"` | `agent_worker` auth errors  | Plugin authentication required                  |

The frontend maps these to color-coded badges:

| Type      | Frontend Color   |
|-----------|------------------|
| `agent`   | `bg-success`     |
| `system`  | `bg-info`        |
| `warning` | `bg-warning`     |
| `error`   | `bg-error`       |

### Store Query Methods

**File:** `crates/db/src/queries/notifications.rs`

```
Store::create_notification(id, user_id, type, title, body, action_url, icon)
    -> Result<Notification>

Store::create_notification_if_not_exists(id, user_id, type, title, body, action_url, icon)
    -> Result<()>
    // INSERT OR IGNORE — deduplicates by primary key

Store::get_notification(id, user_id) -> Result<Option<Notification>>

Store::list_user_notifications(user_id, page_size, page_offset) -> Result<Vec<Notification>>
    // Ordered by created_at DESC

Store::list_unread_notifications(user_id, page_size) -> Result<Vec<Notification>>
    // WHERE read_at IS NULL

Store::count_unread_notifications(user_id) -> Result<i64>

Store::mark_notification_read(id, user_id) -> Result<()>
    // SET read_at = now

Store::mark_all_notifications_read(user_id) -> Result<()>
    // WHERE read_at IS NULL

Store::delete_notification(id, user_id) -> Result<()>

Store::delete_old_notifications(before: i64) -> Result<()>
    // WHERE created_at < before (epoch seconds)
```

The `create_notification_if_not_exists` method uses `INSERT OR IGNORE` keyed on the
notification `id` (PRIMARY KEY). This is used by the agent worker for plugin auth
notifications to avoid spamming repeated "needs authentication" alerts — the ID is
deterministic: `"auth-required:{agent_id}:{plugin_slug}"`.

---

## WebSocket Real-Time Push

### Server-Side Broadcasting

When a notification is created that should appear in real time, the server calls:

```rust
hub.broadcast(
    "notification_created",
    serde_json::json!({
        "id": notif_id,
        "type": "error",
        "title": title,
        "body": body,
        "actionUrl": action_url,
    }),
);
```

This sends the notification payload to all connected WebSocket clients. The
`ClientHub` is defined in `crates/server/src/handlers/ws.rs`.

### Two WebSocket Event Types

The frontend listens to two distinct notification events:

```
+---------------------+      +------------------------+
| "notification"      |      | "notification_created" |
| (legacy/generic)    |      | (persistent, from DB)  |
+---------------------+      +------------------------+
        |                              |
        v                              v
  Inline push to                pushNotification()
  notifications store           (from notifications
  + toast via addToast()         store module)
```

1. **`notification`** — Generic push event. Constructs a notification object inline,
   generates a synthetic ID (`ws-{timestamp}`), and triggers a toast via `addToast()`.
   Used for transient notifications that may not be persisted.

2. **`notification_created`** — Structured event matching the DB `Notification` model.
   Calls `pushNotification()` which maps the server payload to the frontend
   `Notification` interface and prepends it to the store. No toast is triggered
   (the bell badge count update is the signal).

### Frontend Listener Registration

**File:** `app/src/lib/websocket/listeners.ts`

```typescript
// Bootstrap existing notifications on connect
loadNotifications();

// Legacy generic notification
ws.on('notification', (data: any) => {
    const n = {
        id: data.id || `ws-${Date.now()}`,
        type: data.type || 'system',
        title: data.title || '',
        message: data.message || data.body || '',
        time: 'just now',
        read: false,
        link: data.link || data.actionUrl || undefined,
    };
    notifications.update(list => [n, ...list]);
    addToast(n.title || n.message, n.type === 'error' ? 'error' : 'info');
});

// Persistent notification created
ws.on('notification_created', (data: any) => {
    pushNotification(data);
});
```

On initial WebSocket connection, `loadNotifications()` fetches existing notifications
from the REST API to hydrate the store. Subsequent notifications arrive via WebSocket
push only — no polling.

---

## Frontend Notification Store

**File:** `app/src/lib/stores/notifications.ts`

### Types

```typescript
type NotificationType = 'agent' | 'system' | 'warning' | 'error';

interface Notification {
    id: string;
    type: NotificationType;
    title: string;
    message: string;
    time: string;       // Human-readable relative time string
    read: boolean;
    link?: string;      // Deep-link URL (e.g., "/agent-id/runs/run-id")
}
```

### Stores

```typescript
const notifications = writable<Notification[]>([]);
const unreadCount = derived(notifications, ($n) => $n.filter(n => !n.read).length);
```

`notifications` is the source of truth for the UI. `unreadCount` is a derived store
that powers the badge counter on the notification bell.

### Key Functions

```
loadNotifications()          Fetch from GET /api/v1/notifications, map to frontend
                             model, set store. Called once on WS connect. Guarded
                             by `loaded` flag to prevent duplicate fetches.

pushNotification(data)       Map server payload to Notification, prepend to store.
                             Pure push — no API call. Used by WS listener.

markAsRead(id)               Set read=true locally, fire-and-forget PUT to API.

markAllRead()                Set all read=true locally, fire-and-forget PUT to API.

removeNotification(id)       Remove from local store, fire-and-forget DELETE to API.
```

All mutation functions follow an **optimistic update** pattern: the local store is
updated immediately, and the API call is fire-and-forget with `.catch(() => {})`. If
the API call fails, the UI stays updated until the next page load.

### Relative Time Formatting

```typescript
function formatRelativeTime(isoDate: string | number): string
```

Converts timestamps to human-readable strings: "just now", "5 minutes ago",
"2 hours ago", "3 days ago". Used for the `time` field in the notification list.

---

## NotificationBell Component

**File:** `app/src/lib/components/NotificationBell.svelte`

A dropdown bell icon in the top navigation. Shows:

- Bell icon with unread count badge (capped at "9+")
- Dropdown panel (280px wide, max 320px tall scrollable)
- "Mark all read" button when unread count > 0
- Per-notification: colored type dot, title, time, message preview, dismiss button
- Click navigates to `notif.link` via `goto()` and marks as read

### Visual Type Mapping

```
agent   -> bg-success  (green dot)
system  -> bg-info     (blue dot)
warning -> bg-warning  (amber dot)
error   -> bg-error    (red dot)
```

### Interaction Flow

```
User clicks bell
  -> Dropdown opens
  -> Notification list rendered from $notifications store

User clicks notification row
  -> markAsRead(notif.id)  [optimistic local + fire-and-forget API]
  -> goto(notif.link)      [SvelteKit navigation]
  -> Dropdown closes

User clicks X on notification
  -> removeNotification(notif.id)  [optimistic local + fire-and-forget API]
  -> e.stopPropagation() prevents row click

User clicks "Mark all read"
  -> markAllRead()  [optimistic local + fire-and-forget API]
```

---

## Message Tool Integration

**File:** `crates/tools/src/message_tool.rs`

The `MessageTool` is one of the 10 STRAP domain tools. It handles outbound delivery to
the owner. Notification-related resources and actions:

### Resource: `owner`

```
message(resource: "owner", action: "notify", text: "Task complete!")
```

Flow:
1. Validates `text` is non-empty
2. Gets or creates a companion chat for the user
3. Inserts an "assistant" role message into that chat
4. Calls `notify_crate::send("Nebo", text)` for OS notification (currently no-op)
5. Returns "Notified owner: {text}"

### Resource: `notify`

```
message(resource: "notify", action: "send", title: "Alert", text: "Something happened")
message(resource: "notify", action: "alert", title: "Warning", text: "...")
message(resource: "notify", action: "dnd_status")
```

#### Action: `send`

1. Validates `text` is non-empty
2. Creates a persistent notification in SQLite via `store.create_notification()`
   with type `"info"`
3. Calls `notify_crate::send(title, text)` for OS notification (currently no-op)
4. Returns "Notification sent: {text}"

#### Action: `alert`

Displays a **modal dialog** rather than a passive notification. Platform-specific:
- macOS: `display alert` via osascript (blocks until dismissed)
- Linux: `notify-send --urgency=critical` (requires libnotify)
- Windows: `System.Windows.MessageBox::Show` via PowerShell

#### Action: `dnd_status`

Checks the OS Do Not Disturb status. Returns JSON with `dnd_enabled: bool`. See
the [Do Not Disturb Detection](#do-not-disturb-detection) section below.

### Resource Inference

The tool auto-corrects missing or misplaced `resource` fields. If the resource is
empty, it infers from the action:

```rust
fn infer_resource(&self, action: &str) -> &str {
    match action {
        "notify" => "owner",
        "alert" | "dnd_status" => "notify",
        "conversations" | "read" | "search" => "sms",
        _ => "",
    }
}
```

---

## Cross-System Trigger Points

### Where `notify_crate::send()` Is Called

All of these are currently no-ops but document the intended trigger points:

```
+-----------------------------------+---------------------------------------------+
| Call Site                         | Title / Body Pattern                        |
+-----------------------------------+---------------------------------------------+
| scheduler.rs (cron success)       | "Nebo" / "{job.name} completed"             |
| scheduler.rs (cron failure)       | "Nebo" / "{job.name} failed: {err}"         |
| lib.rs (agent space inbound msg)  | "Agent space: {name}" / "{preview}"         |
| lib.rs (NeboAI inbound msg)     | "Message from {from}" / "{preview}"         |
| agent_worker.rs (heartbeat fire)  | "Nebo" / "Heartbeat: {binding_name}"        |
| agent_worker.rs (heartbeat fail)  | "Nebo" / "{binding_name} failed: {err}"     |
| agent_worker.rs (watch fail)      | "Nebo" / "{binding_name} failed: {err}"     |
| message_tool.rs (owner notify)    | "Nebo" / "{text}"                           |
| message_tool.rs (notify send)     | "{title}" / "{text}"                        |
+-----------------------------------+---------------------------------------------+
```

### Where Persistent Notifications Are Created

```
+-----------------------------------+---------------------------------------------+
| Call Site                         | Type / ID Pattern                           |
+-----------------------------------+---------------------------------------------+
| workflow_manager.rs               | "error" / "wf-fail:{run_id}"               |
|   notify_workflow_failure()       | action_url: "/{agent_id}/runs/{run_id}"     |
|                                   |                                             |
| agent_worker.rs                   | "warning" / "auth-required:{agent}:{plugin}"|
|   plugin auth error               | action_url: "/settings/plugins"             |
|                                   |                                             |
| message_tool.rs                   | "info" / UUID                               |
|   notify.send action              | action_url: None                            |
+-----------------------------------+---------------------------------------------+
```

### Notification Deduplication Strategy

```
notify_workflow_failure():
  ID = "wf-fail:{run_id}"
  Uses create_notification() -- each run_id is unique, so no collision

agent_worker plugin auth:
  ID = "auth-required:{agent_id}:{plugin_slug}"
  Uses create_notification_if_not_exists() -- INSERT OR IGNORE
  Prevents duplicate "needs auth" notifications for the same agent+plugin pair
  Cleaned up when plugin re-authenticates (delete_notification called in plugins.rs)

message_tool notify.send:
  ID = UUID (always unique)
  Uses create_notification()
```

---

## User Preferences

### Database Fields

**Table:** `user_preferences`
**Migration:** `0003_notifications.sql`

```sql
ALTER TABLE user_preferences
    ADD COLUMN inapp_notifications INTEGER NOT NULL DEFAULT 1;
```

The `UserPreference` model includes:

```rust
pub struct UserPreference {
    pub user_id: String,
    pub email_notifications: i64,    // 0 or 1
    pub marketing_emails: i64,       // 0 or 1
    pub timezone: String,
    pub language: String,
    pub theme: String,
    pub updated_at: i64,
    pub inapp_notifications: i64,    // 0 or 1
}
```

### Preference Update

**File:** `crates/db/src/queries/user_profile.rs`

```rust
pub fn update_user_preferences(
    &self,
    theme: Option<&str>,
    language: Option<&str>,
    timezone: Option<&str>,
    email_notifications: Option<bool>,
    inapp_notifications: Option<bool>,
) -> Result<(), NeboError>
```

**Note:** The `inapp_notifications` and `email_notifications` preference columns exist
in the database and can be toggled via the user settings handler, but the notification
creation code does **not currently check these preferences** before creating or
broadcasting notifications. This is a gap — the preferences are stored but not enforced.

---

## Database Schema

**Migration:** `crates/db/migrations/0003_notifications.sql`

```sql
CREATE TABLE IF NOT EXISTS notifications (
    id          TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    type        TEXT NOT NULL,      -- info, error, warning, system, etc.
    title       TEXT NOT NULL,
    body        TEXT,
    action_url  TEXT,               -- deep-link path (e.g., "/agent-id/runs/run-id")
    icon        TEXT,               -- unused currently
    read_at     INTEGER,            -- epoch seconds, NULL = unread
    created_at  INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

CREATE INDEX idx_notifications_user         ON notifications(user_id);
CREATE INDEX idx_notifications_user_unread  ON notifications(user_id, read_at);
CREATE INDEX idx_notifications_created      ON notifications(created_at DESC);
```

### Index Strategy

- `idx_notifications_user` — filter by user (multi-user support)
- `idx_notifications_user_unread` — efficient unread count queries
- `idx_notifications_created` — ordered listing (newest first)

### Foreign Key Cascade

`user_id` references `users(id)` with `ON DELETE CASCADE`. Deleting a user
automatically removes all their notifications.

---

## REST API

**Route file:** `crates/server/src/routes/notifications.rs`
**Handler file:** `crates/server/src/handlers/notification.rs`

All routes are under `/api/v1/` and protected by optional JWT (falls back to local
user in desktop mode).

```
GET    /api/v1/notifications              -> list_notifications
       Returns: { "notifications": [...] }
       Params: page_size=50, page_offset=0 (hardcoded)

PUT    /api/v1/notifications/{id}/read    -> mark_read
       Returns: { "success": true }

PUT    /api/v1/notifications/read-all     -> mark_all_read
       Returns: { "success": true }

DELETE /api/v1/notifications/{id}         -> delete_notification
       Returns: { "success": true }

GET    /api/v1/notifications/unread-count -> unread_count
       Returns: { "count": N }
```

### User Identity Resolution

All handlers use a shared `user_id()` helper:

```rust
fn user_id(claims: Option<axum::Extension<AuthClaims>>, state: &AppState) -> String {
    claims
        .map(|c| c.0.user_id)
        .or_else(|| state.store.ensure_local_user_id().ok())
        .unwrap_or_default()
}
```

In desktop mode (no JWT), falls back to `ensure_local_user_id()` which returns the
single local user. In cloud mode, extracts from JWT claims.

---

## Do Not Disturb Detection

**File:** `crates/tools/src/message_tool.rs` (`handle_dnd_status`)

The agent can query the OS DND status before deciding whether to notify. Accessible
via `message(resource: "notify", action: "dnd_status")`.

### macOS

Two-stage detection:

1. **Focus Modes (macOS 12+):** Reads
   `com.apple.controlcenter NSStatusItem Visible FocusModes` via `defaults read`.
   Value "1" means a Focus Mode is active.

2. **Legacy DND (macOS 11 and earlier):** Falls back to reading
   `com.apple.ncprefs dnd_prefs`. Checks for `dndDisplayLock = 1` or
   `dndMirrored = 1`.

```rust
// Returns JSON:
{ "dnd_enabled": true|false, "raw": "<system output>" }
```

### Linux

Queries the freedesktop D-Bus notifications interface:

```bash
dbus-send --session --print-reply \
    --dest=org.freedesktop.Notifications \
    /org/freedesktop/Notifications \
    org.freedesktop.DBus.Properties.Get \
    string:org.freedesktop.Notifications \
    string:DoNotDisturb
```

Falls back to `{ "dnd_enabled": false, "note": "Could not query D-Bus" }` if
D-Bus is unavailable.

### Windows

Reads the registry key for Focus Assist (quiet hours):

```
HKCU\Software\Microsoft\Windows\CurrentVersion\CloudStore\Store\
    DefaultAccount\Current\default$windows.data.notifications.
    quiethourssettings\windows.data.notifications.quiethourssettings
```

---

## Alert Dialogs

**File:** `crates/tools/src/message_tool.rs` (`handle_alert`)

Modal alerts block until dismissed. They are for urgent, attention-demanding situations.

### Platform Implementations

```
+----------+------------------------------------------------------------+
| Platform | Implementation                                             |
+----------+------------------------------------------------------------+
| macOS    | osascript: display alert "{title}" message "{text}"        |
|          | Produces a native macOS alert dialog with OK button         |
+----------+------------------------------------------------------------+
| Linux    | notify-send --urgency=critical {title} {text}              |
|          | Critical urgency may bypass DND depending on DE             |
+----------+------------------------------------------------------------+
| Windows  | PowerShell: [System.Windows.MessageBox]::Show("{text}",     |
|          |   "{title}")                                               |
|          | Produces a WPF MessageBox dialog                           |
+----------+------------------------------------------------------------+
```

Input escaping:
- macOS: double quotes are backslash-escaped
- Windows: double quotes are backtick-escaped (PowerShell convention)

---

## Input Sanitization and Security

### Shell Injection Prevention

The `sanitize()` function in `nebo-notify` converts potentially dangerous characters:

```
'  ->  \u{2019}  (right single quotation mark)
\  ->  (removed)
"  ->  \u{201C}  (left double quotation mark)
```

Combined with the 256-byte truncation, this prevents:
- Shell command injection via crafted notification text
- Buffer overflow via extremely long strings
- Quote-based escapes in osascript/PowerShell contexts

### SQL Injection in SMS Queries

The `message_tool.rs` SMS functions use direct string interpolation into SQLite queries
against `chat.db` (read-only Messages.app database). Basic escaping is applied:

```rust
let escaped_phone = phone.replace('\'', "''");  // SQL single-quote escape
let escaped_query = query_text.replace('\'', "''");
```

This is limited but acceptable because:
1. The target database is read-only (Messages.app `chat.db`)
2. The queries are SELECT-only
3. The sqlite3 CLI is used in read mode

---

## Notification Lifecycle and Cleanup

### Creation

Notifications are created with `strftime('%s', 'now')` as the `created_at` timestamp
(epoch seconds). The `read_at` field starts as NULL.

### Read Tracking

When marked as read, `read_at` is set to the current epoch second via
`strftime('%s', 'now')`. Once set, it is never cleared (no "mark as unread" feature).

### Deletion

Individual notifications can be deleted by the user via the REST API or by system
code (e.g., `delete_notification` is called in `plugins.rs` to clean up auth-required
notifications after successful plugin authentication).

### Bulk Cleanup

`delete_old_notifications(before: i64)` removes all notifications with
`created_at < before`. This method exists in the Store but is **not currently called**
from any scheduler or cleanup task. It is available for future use (e.g., a periodic
cleanup job to remove notifications older than 30 days).

### Deduplication on Auth Notifications

Auth-required notifications use `create_notification_if_not_exists()` with a
deterministic ID (`"auth-required:{agent_id}:{plugin_slug}"`). This prevents duplicate
notifications when the agent worker's watch loop repeatedly encounters the same auth
error. When the plugin is re-authenticated, the notification is explicitly deleted:

```rust
// In plugins.rs after successful auth:
let notif_id = format!("auth-required:{}:{}", agent.id, slug);
let _ = store.delete_notification(&notif_id, "");
```

---

## Platform-Specific Implementation Details

### macOS Specifics

| Feature            | Implementation                              |
|--------------------|---------------------------------------------|
| Toast notification | `osascript -e 'display notification ...'`   |
| Alert dialog       | `osascript -e 'display alert ...'`          |
| DND detection      | `defaults read com.apple.controlcenter`     |
| SMS send           | `osascript` -> Messages.app `send` command  |
| SMS read           | `sqlite3` -> `~/Library/Messages/chat.db`   |

The SMS integration is macOS-only and requires Full Disk Access permissions to read
the Messages database.

### Linux Specifics

| Feature            | Implementation                              |
|--------------------|---------------------------------------------|
| Toast notification | `notify-send {title} {body}`                |
| Alert dialog       | `notify-send --urgency=critical`            |
| DND detection      | D-Bus `org.freedesktop.Notifications`       |
| SMS                | Not supported                               |

The `which_exists()` helper checks for `notify-send` on PATH before attempting to
use it.

### Windows Specifics

| Feature            | Implementation                              |
|--------------------|---------------------------------------------|
| Toast notification | PowerShell WinRT ToastNotificationManager    |
| Alert dialog       | PowerShell WPF MessageBox                   |
| DND detection      | Registry read (Focus Assist / Quiet Hours)  |
| SMS                | Not supported                               |

All Windows implementations use `powershell -NoProfile -NonInteractive` for
minimal startup overhead.

---

## Error Handling Strategy

### Native Notifications (nebo-notify)

- `send()` is a no-op — cannot fail
- `send_platform()` returns `Result<(), String>` — errors are string-formatted
- Errors are silently swallowed (fire-and-forget philosophy)

### In-App Notifications (DB)

- `create_notification()` returns `Result<Notification, NeboError>`
- Callers log warnings on failure but do not propagate errors upstream:
  ```rust
  if let Err(e) = store.create_notification(...) {
      warn!("failed to create notification: {}", e);
  }
  ```
- The principle: notification failure should never block the primary operation
  (workflow execution, agent response, etc.)

### REST API Handlers

- Return `HandlerResult<serde_json::Value>` (Axum JSON response)
- Errors are mapped via `to_error_response` to appropriate HTTP status codes
- `unread_count` uses `.unwrap_or(0)` — returns 0 on any DB error

### Message Tool

- Returns `ToolResult::error(message)` on failures
- The agent sees the error and can retry or inform the user
- OS notification failures are silently swallowed (no-op currently)

---

## Current Limitations and Future Work

### Disabled OS Notifications

The `notify_crate::send()` function is intentionally a no-op. The comment explains:

> Currently disabled — OS notifications are not deep-linked so clicking them does
> nothing useful. Re-enable once tauri-plugin-notification is wired up with action
> handling.

**Future plan:** Replace shell-command-based notifications with
`tauri-plugin-notification` which supports:
- Deep-link actions (clicking notification opens specific page)
- Custom icons and sounds
- Action buttons
- Notification grouping/threading

### User Preference Enforcement Gap

The `inapp_notifications` preference exists in the database but notification creation
code does not check it. When set to `0`, notifications should be suppressed, but they
currently are not.

### No Badge/Sound System

There is no sound playback on notification arrival. The bell icon shows a count badge
but there is no system tray badge integration. The Tauri system tray does not currently
reflect unread notification count.

### No Notification Categories/Channels

All notifications are flat — there is no category system that would let users
selectively mute workflow failures while keeping agent messages. The `type` field
could serve as a channel identifier for future per-category preferences.

### Hardcoded Pagination

The `list_notifications` handler uses hardcoded `page_size=50, page_offset=0`.
There are no query parameters for pagination.

### No Bulk Cleanup Schedule

`delete_old_notifications()` exists but is never called. Old notifications accumulate
indefinitely in the database.

---

## Key Files Reference

```
crates/notify/
  Cargo.toml                         Crate manifest (nebo-notify)
  src/lib.rs                         OS notification dispatch + sanitization

crates/db/
  migrations/0003_notifications.sql  Schema: notifications table + inapp_notifications pref
  src/models.rs                      Notification struct (line ~302)
  src/queries/notifications.rs       All notification CRUD queries
  src/queries/user_profile.rs        inapp_notifications preference read/write

crates/server/
  src/routes/notifications.rs        Axum route definitions (5 endpoints)
  src/handlers/notification.rs       Handler implementations (list, mark, delete, count)
  src/scheduler.rs                   Cron job completion/failure -> notify_crate::send()
  src/workflow_manager.rs            Workflow failure -> DB notification + WS broadcast
  src/lib.rs                         Inbound message handling -> notify_crate::send()

crates/tools/
  src/message_tool.rs                MessageTool: owner/notify/sms resources, DND, alerts

crates/agent/
  src/agent_worker.rs                Heartbeat/watch triggers + plugin auth notifications

app/src/lib/
  stores/notifications.ts            Frontend notification store (writable + derived)
  components/NotificationBell.svelte Bell dropdown UI component
  websocket/listeners.ts             WS event handlers: notification + notification_created
  api/nebo.ts                        Generated API client (CRUD functions)
```
