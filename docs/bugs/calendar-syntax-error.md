# Calendar Tool Syntax Error

**Date:** May 12, 2026  
**Issue:** Incorrect tool usage when querying Family calendar  
**Status:** Resolved  

---

## Error Summary

Initial attempt to query the calendar using `os(action: "calendar", ...)` resulted in error:

```
Unknown calendar action 'calendar'
```

---

## Root Cause

The error occurred because `calendar` was incorrectly specified as an **action** at the root level of the `os` tool call, when it is actually a **resource**.

### Incorrect Usage (Failed)
```bash
os(action: "calendar", ...)
```

### Correct Usage
```bash
os(resource: "calendar", action: "today")
```

Or for specific calendar filtering:
```bash
os(resource: "calendar", action: "today", calendar: "Family")
```

---

## Available `os` Resources

The `os` tool supports the following resources:
- `file` - read, write, edit, glob, grep
- `shell` - exec, list, poll, log, write, kill, info
- `window` - list, focus, minimize, maximize, resize, close, move
- `input` - click, double_click, right_click, type, press, hotkey, move, scroll, drag, paste
- `clipboard` - read, write, clear
- `capture` - screenshot, see
- `notification` - send, alert
- `ui` - tree, find, click, get_value, set_value, list_apps
- `menu` - list, menus, click, status, click_status
- `dialog` - detect, list, click, fill, dismiss
- `space` - list, switch, move_window
- `shortcut` - list, run
- `tts` - speak
- `dock` - badges, recent, is_running (macOS)
- `app` - list, launch, quit, quit_all, activate, hide, info, frontmost
- `settings` - volume, brightness, wifi, bluetooth, battery, darkmode, sleep, lock, info, mute, unmute
- `music` - play, pause, next, previous, status, search, volume, playlists, shuffle
- `keychain` - get, find, add, delete
- `search` - search (file search via OS index)
- `mail` - accounts, unread, read, send, search
- `contacts` - search, get, create, groups
- `calendar` - calendars, today, upcoming, create, delete, pending, accept, decline, auto_accept, list, configure
- `reminders` - lists, list, create, complete, delete

---

## Resolution

After identifying the schema structure through `os(info)` fallback, the correct syntax was determined:

1. **Resource parameter:** Must specify `resource: "calendar"` to access calendar functionality
2. **Action parameter:** Use specific actions like `today`, `upcoming`, `create`, `delete`, etc.
3. **Filtering:** Pass additional parameters like `calendar: "Family"` to filter results

### Working Examples

```bash
# General today's events
os(resource: "calendar", action: "today")

# Family calendar only
os(resource: "calendar", action: "today", calendar: "Family")

# Upcoming week
os(resource: "calendar", action: "upcoming", days: 7)

# Create event
os(resource: "calendar", action: "create", title: "Meeting", date: "2026-05-15 14:00")
```

---

## Prevention Guidelines

1. **Always specify resource first** - The `os` tool requires explicit resource identification
2. **Check available resources** - Use `os(resource: "info")` or review documentation if uncertain
3. **Match action to resource** - Each resource has its own set of valid actions
4. **Parameter context matters** - Parameters like `calendar: "Family"` are resource-specific filters, not top-level actions

---

## Related Issues

- Email batch processing failure (resolved with individual commands)
- GitHub build notification filtering (per user directive)
- Billing notification handling (per user directive)

---

*Document created for global AI launch preparation reference.*
