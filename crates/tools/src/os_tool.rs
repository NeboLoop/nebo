use std::sync::Arc;

use crate::app_tool::AppTool;
use crate::desktop_tool::DesktopTool;
use crate::domain::DomainInput;
use crate::file_tool::FileTool;
use crate::keychain_tool::KeychainTool;
use crate::music_tool::MusicTool;
use crate::organizer;
use crate::origin::ToolContext;
use crate::policy::Policy;
use crate::process::ProcessRegistry;
use crate::registry::{DynTool, ResourceKind, ToolResult};
use crate::settings_tool::SettingsTool;
use crate::shell_tool::ShellTool;
use crate::spotlight_tool::SpotlightTool;

/// Unified OS tool — all local machine operations under one namespace.
///
/// 25 resources spanning file system, shell, desktop automation, apps, settings,
/// media, credentials, search, and personal information management.
/// Per-resource approval: safe resources auto-approve, sensitive ones require confirmation.
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

/// Organizer actions that modify data and require user approval.
const ORGANIZER_WRITE_ACTIONS: &[&str] =
    &["send", "create", "delete", "complete", "accept", "decline"];

/// Resources that auto-approve (no user confirmation needed).
const AUTO_APPROVE_RESOURCES: &[&str] = &[
    "file",
    "shell",
    "clipboard",
    "capture",
    "search",
    "notification",
    "tts",
    "dock",
];

impl OsTool {
    pub fn new(policy: Policy, process_registry: Arc<ProcessRegistry>) -> Self {
        Self {
            file_tool: FileTool::new(),
            shell_tool: ShellTool::new(policy, process_registry),
            desktop_tool: DesktopTool::new(),
            app_tool: AppTool::new(),
            settings_tool: SettingsTool::new(),
            music_tool: MusicTool::new(),
            keychain_tool: KeychainTool::new(),
            spotlight_tool: SpotlightTool::new(),
            store: None,
        }
    }

    pub fn with_plugin_store(mut self, ps: Arc<napp::plugin::PluginStore>) -> Self {
        self.shell_tool = self.shell_tool.with_plugin_store(ps);
        self
    }

    /// `os(resource: "file", action: "convert", path: "report.md", to: "pdf")` —
    /// generate office documents with the embedded engines (Typst for PDF,
    /// pure-Rust OOXML writers for docx/xlsx). The one document-conversion
    /// pathway: identical on every platform — never host binaries (wkhtmltopdf
    /// is abandoned upstream) and never the bundled browser (no layout engine).
    async fn handle_convert(&self, input: &serde_json::Value) -> ToolResult {
        let path = input["path"].as_str().unwrap_or("");
        let to = input["to"].as_str().unwrap_or("pdf");
        if path.is_empty() {
            return ToolResult::error(
                "Error: path is required. Example: os(resource: \"file\", action: \"convert\", path: \"/path/report.md\", to: \"pdf\")",
            );
        }
        let src = crate::file_tool::expand_path(path);
        let src_path = std::path::Path::new(&src);
        if !src_path.exists() {
            return ToolResult::error(format!("Error: source file not found: {src}"));
        }
        let ext = src_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let source = match std::fs::read_to_string(src_path) {
            Ok(s) => s,
            Err(e) => return ToolResult::error(format!("Error reading {src}: {e}")),
        };
        // Rendering is CPU-bound — keep it off the async runtime threads.
        let to_owned = to.to_string();
        let file_name = src_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "component".into());
        let rendered = tokio::task::spawn_blocking(move || {
            match (to_owned.as_str(), ext.as_str()) {
                ("pdf", "md" | "markdown" | "txt") => {
                    render::markdown_to_pdf(&source).map_err(|e| e.to_string())
                }
                ("pdf", "typ") => render::typst_to_pdf(&source).map_err(|e| e.to_string()),
                ("docx", "md" | "markdown" | "txt") => {
                    render::markdown_to_docx(&source).map_err(|e| e.to_string())
                }
                ("xlsx", "csv") => render::csv_to_xlsx(&source).map_err(|e| e.to_string()),
                ("html", "jsx") => render::jsx_to_html(&source, &file_name, render::JsxLang::Jsx)
                    .map(String::into_bytes)
                    .map_err(|e| e.to_string()),
                ("html", "tsx") => render::jsx_to_html(&source, &file_name, render::JsxLang::Tsx)
                    .map(String::into_bytes)
                    .map_err(|e| e.to_string()),
                ("pdf", other) => Err(format!(
                    "pdf converts from .md or .typ (got .{other}). Write the document as Markdown first."
                )),
                ("docx", other) => Err(format!(
                    "docx converts from .md (got .{other}). Write the document as Markdown first."
                )),
                ("xlsx", other) => Err(format!(
                    "xlsx converts from .csv (got .{other}). Write the data as CSV first."
                )),
                ("html", other) => Err(format!(
                    "html converts from .jsx or .tsx (got .{other}). Write the interactive component as a single-file .jsx first."
                )),
                (other, _) => Err(format!(
                    "unsupported target format '{other}' (supported: pdf from .md/.typ, docx from .md, xlsx from .csv, html from .jsx/.tsx)."
                )),
            }
        })
        .await;
        let bytes = match rendered {
            Ok(Ok(b)) => b,
            Ok(Err(msg)) => {
                return ToolResult::error(format!(
                    "Error converting: {msg}. The source document still renders in the Work panel."
                ));
            }
            Err(e) => return ToolResult::error(format!("Error converting: {e}")),
        };
        let out = src_path.with_extension(to);
        if let Err(e) = std::fs::write(&out, &bytes) {
            return ToolResult::error(format!("Error writing {}: {e}", out.display()));
        }
        let out_str = out.to_string_lossy().to_string();
        ToolResult::ok(format!(
            "Converted {src} to {out_str} ({} bytes)",
            bytes.len()
        ))
        // PDF is a user-facing work product — surface it in the Work panel.
        .with_image_url(out_str)
    }

    pub fn with_store(mut self, store: Arc<db::Store>) -> Self {
        self.store = Some(store);
        self
    }

    /// True when the call is a file-management verb (move/copy/rename/delete/
    /// mkdir) shaped like a file op (has `path`, no explicit resource) rather
    /// than a mouse `move`. The file tool has no such actions (they go through
    /// the shell), so these are redirected to a shell
    /// correction — and the permission gate must NOT treat them as desktop
    /// control. One detection, shared by `execute` (the redirect) and
    /// `capabilities::gating_capability` (skip the wrong-capability ask).
    pub(crate) fn is_file_mgmt_redirect(input: &serde_json::Value) -> bool {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let has_explicit_resource = input
            .get("resource")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty());
        let has_path = input.get("path").and_then(|v| v.as_str()).is_some();
        let has_dest = input
            .get("destination")
            .or_else(|| input.get("to"))
            .and_then(|v| v.as_str())
            .is_some();
        let file_mgmt_verb = matches!(
            action,
            "move" | "copy" | "rename" | "delete" | "remove" | "mkdir" | "rmdir" | "trash"
        );
        !has_explicit_resource && file_mgmt_verb && has_path && (has_dest || action != "move")
    }

    /// Infer resource from action name when resource field is omitted.
    pub(crate) fn infer_resource(action: &str) -> &str {
        match action {
            // File
            "read" | "write" | "edit" | "glob" | "grep" | "convert" => "file",
            // Shell
            "exec" | "poll" | "log" => "shell",
            // Input
            "click" | "type" | "press" | "move" | "double_click" | "right_click" | "hotkey"
            | "scroll" | "drag" | "paste" => "input",
            // Capture
            "screenshot" | "see" => "capture",
            // Music
            "play" | "pause" | "next" | "previous" | "shuffle" | "playlists" => "music",
            // App
            "launch" | "quit" | "quit_all" | "activate" | "hide" | "frontmost" => "app",
            // TTS
            "speak" => "tts",
            // Organizer inferences
            "accounts" | "unread" | "send" => "mail",
            "today" | "upcoming" | "calendars" | "configure" | "pending" | "accept" | "decline"
            | "auto_accept" => "calendar",
            "groups" => "contacts",
            "lists" | "complete" => "reminders",
            _ => "",
        }
    }

    /// Infer resource from parameter context when action-based inference fails
    /// (e.g. "create" is shared across calendar, contacts, reminders).
    pub(crate) fn infer_resource_from_context(input: &serde_json::Value) -> &'static str {
        // File: "list"/"ls" with a dir/path target is a directory listing
        // (a strong model prior — routed to file, which handles it via glob).
        // Bare "list" with no target stays ambiguous (window, app, shell, ...).
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        if matches!(action, "list" | "ls")
            && (input
                .get("dir")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
                || input
                    .get("path")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| !s.is_empty()))
        {
            return "file";
        }
        // Calendar: date, calendar, end_date, location, or days present
        if input
            .get("date")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty())
            || input
                .get("calendar")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
            || input
                .get("end_date")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
            || input.get("days").is_some()
        {
            return "calendar";
        }
        // Reminders: list, due_date, or priority present
        if input
            .get("list")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty())
            || input
                .get("due_date")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
            || input.get("priority").is_some()
        {
            return "reminders";
        }
        // Contacts: email, phone, or company present
        if input
            .get("phone")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty())
            || input
                .get("company")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
        {
            return "contacts";
        }
        // Mail: to, cc, subject, or mailbox present
        if input.get("to").is_some()
            || input
                .get("subject")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
            || input
                .get("mailbox")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
        {
            return "mail";
        }
        ""
    }

    pub fn file_tool(&self) -> &FileTool {
        &self.file_tool
    }

    pub fn shell_tool(&self) -> &ShellTool {
        &self.shell_tool
    }
}

impl DynTool for OsTool {
    fn name(&self) -> &str {
        "os"
    }

    fn description(&self) -> String {
        "Local machine operations — files, shell, apps, desktop automation, settings, media, credentials, search, PIM.\n\n\
         Rules:\n\
         - ALWAYS call this tool for file/system facts — NEVER answer from memory or training data. To read a file, call os(resource: \"file\", action: \"read\"); do NOT claim a file is missing or report its contents without calling first.\n\
         - Prefer file actions over shell: use file read NOT shell cat, file grep NOT shell grep, file glob NOT shell find.\n\
         - Always pass `action`. `resource` is usually inferred from the action (read→file, exec→shell, play→music); pass it ONLY to disambiguate actions shared across resources (e.g. create, list).\n\
         - Interactive React (dashboards, charts, visualizations): write the component as a .jsx file, then convert it (action: \"convert\", to: \"html\") — Nebo transpiles it into a self-contained, renderable page. NEVER put JSX or CDN-loaded React (unpkg/esm) directly in a .html; raw JSX has no transpiler in the browser and renders blank.\n\
         - Before edit or overwrite of an EXISTING file, read it first (edit/overwrite are rejected without a prior read). A brand-new file needs no prior read.\n\
         - glob = find files by NAME pattern (*.md, src/**/*.rs); grep = match text INSIDE files by regex. Do not confuse them.\n\
         - NEVER use sudo without asking the user first; on permission denied, explain and offer alternatives.\n\n\
         Resources:\n\
         - file: read, write, edit, glob, grep, convert — to list a directory, glob its path (pattern defaults to *); convert generates documents via embedded engines: .md→pdf/docx, .csv→xlsx, .jsx/.tsx→html (interactive React) (never use host binaries like wkhtmltopdf/pandoc)\n\
         - shell: exec, list, poll, log, write, kill, info\n\
         - window: list, focus, minimize, maximize, resize, close, move\n\
         - input: click, double_click, right_click, type, press, hotkey, move, scroll, drag, paste\n\
         - clipboard: read, write, clear\n\
         - capture: screenshot, see\n\
         - notification: send, alert\n\
         - ui: tree, find, click, get_value, set_value, list_apps\n\
         - menu: list, menus, click, status, click_status\n\
         - dialog: detect, list, click, fill, dismiss\n\
         - space: list, switch, move_window\n\
         - shortcut: list, run\n\
         - tts: speak\n\
         - dock: badges, recent, is_running (macOS)\n\
         - app: list, launch, quit, quit_all, activate, hide, info, frontmost\n\
         - settings: volume, brightness, wifi, bluetooth, battery, darkmode, sleep, lock, info, mute (value: true|false)\n\
         - music: play, pause, next, previous, status, search, volume, playlists, shuffle\n\
         - keychain: get, find, add, delete\n\
         - search: search (file search via OS index)\n\
         - mail: accounts, unread, read, send, search\n\
         - contacts: search, get, create, groups\n\
         - calendar: calendars, today, upcoming, create, delete, pending, accept, decline, auto_accept, list, configure — the LOCAL Apple/Mac calendar (for Google Calendar use plugin(resource: \"gws\", ...))\n\
         - reminders: lists, list, create, complete, delete\n\n\
         Examples:\n  \
         os(resource: \"file\", action: \"read\", path: \"/path/to/file.txt\")\n  \
         os(resource: \"shell\", action: \"exec\", command: \"ls -la\")\n  \
         os(resource: \"app\", action: \"launch\", app: \"Safari\")\n  \
         os(resource: \"capture\", action: \"screenshot\")\n  \
         os(resource: \"capture\", action: \"see\", app: \"Safari\") — returns snapshot_id + element IDs\n  \
         os(resource: \"input\", action: \"click\", element_id: \"B3\") — click element from snapshot\n  \
         os(resource: \"input\", action: \"type\", element_id: \"T1\", text: \"hello\") — focus + type\n  \
         os(resource: \"music\", action: \"play\")\n  \
         os(resource: \"keychain\", action: \"get\", service: \"myapp\", account: \"user@example.com\")\n  \
         os(resource: \"mail\", action: \"unread\")"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        // Built programmatically to avoid serde_json::json! recursion limit
        let mut props = serde_json::Map::new();

        let prop = |t: &str, d: &str| -> serde_json::Value {
            serde_json::json!({"type": t, "description": d})
        };

        props.insert(
            "resource".into(),
            serde_json::json!({
                "type": "string",
                "description": "Optional. The resource category — usually inferred from the action (read→file, exec→shell). Specify it only to disambiguate actions shared across resources (e.g. create, list).",
                "enum": [
                    "file", "shell",
                    "window", "input", "clipboard", "capture", "notification",
                    "ui", "menu", "dialog", "space", "shortcut", "tts", "dock",
                    "app", "settings", "music", "keychain", "search",
                    "mail", "contacts", "calendar", "reminders"
                ]
            }),
        );
        props.insert(
            "action".into(),
            prop("string", "The operation to perform on the selected resource (e.g. resource: \"file\" → action: \"read\"; resource: \"calendar\" → action: \"today\"). Never put a resource name here."),
        );
        // File
        props.insert("path".into(), prop("string", "File or directory path"));
        props.insert("content".into(), prop("string", "REQUIRED for write. The file content to write. Must use this exact field name — not 'text' or 'data'."));
        props.insert("pattern".into(), prop("string", "Pattern to match: filename glob (for glob action) or regex (for grep action)"));
        props.insert("to".into(), prop("string", "Target format for convert: \"pdf\" (from .md/.typ), \"docx\" (from .md), \"xlsx\" (from .csv), \"html\" (from .jsx/.tsx — interactive React component). Output lands next to the source."));
        props.insert(
            "old_string".into(),
            prop("string", "String to find (for edit)"),
        );
        props.insert(
            "new_string".into(),
            prop("string", "Replacement string (for edit)"),
        );
        props.insert(
            "replace_all".into(),
            prop("boolean", "Replace all occurrences"),
        );
        props.insert("offset".into(), prop("integer", "Line offset for reading"));
        props.insert(
            "limit".into(),
            prop("integer", "Max lines/results to return"),
        );
        props.insert("append".into(), prop("boolean", "Append to file"));
        // "pattern" is already registered above (used by both glob and grep)
        // "regex" kept on FileInput for backward compat but removed from schema
        props.insert(
            "case_insensitive".into(),
            prop("boolean", "Case-insensitive search"),
        );
        props.insert(
            "glob".into(),
            prop("string", "File filter pattern for grep"),
        );
        props.insert(
            "output_mode".into(),
            serde_json::json!({
                "type": "string",
                "description": "Grep result format: 'content' (matching lines with context, default), 'files' (file paths only), 'count' (match counts per file)",
                "enum": ["content", "files", "count"]
            }),
        );
        props.insert(
            "context_before".into(),
            prop("integer", "Lines to show before each grep match (like grep -B)"),
        );
        props.insert(
            "context_after".into(),
            prop("integer", "Lines to show after each grep match (like grep -A)"),
        );
        // Shell
        props.insert("command".into(), prop("string", "Shell command to execute"));
        props.insert(
            "timeout".into(),
            prop("integer", "Command timeout in seconds"),
        );
        props.insert("session_id".into(), prop("string", "Background session ID"));
        props.insert("pid".into(), prop("integer", "Process ID"));
        props.insert(
            "signal".into(),
            prop("string", "Signal: SIGTERM, SIGKILL, SIGINT"),
        );
        props.insert(
            "background".into(),
            prop("boolean", "Run command in background"),
        );
        props.insert(
            "cwd".into(),
            prop("string", "Working directory to run the command in"),
        );
        props.insert(
            "data".into(),
            prop("string", "stdin to write to a background session (shell write)"),
        );
        props.insert(
            "filter".into(),
            prop("string", "Substring filter for shell process/session list"),
        );
        // Desktop
        props.insert("app".into(), prop("string", "Application name"));
        props.insert(
            "title".into(),
            prop("string", "Window or notification title"),
        );
        props.insert("message".into(), prop("string", "Notification message"));
        props.insert("text".into(), prop("string", "Text to type/write/speak"));
        props.insert("key".into(), prop("string", "Key to press"));
        props.insert("keys".into(), prop("string", "Key combination for hotkey"));
        props.insert("x".into(), prop("integer", "X coordinate"));
        props.insert("y".into(), prop("integer", "Y coordinate"));
        props.insert("x2".into(), prop("integer", "End X coordinate (drag)"));
        props.insert("y2".into(), prop("integer", "End Y coordinate (drag)"));
        props.insert("dx".into(), prop("integer", "Scroll delta X"));
        props.insert("dy".into(), prop("integer", "Scroll delta Y"));
        props.insert("width".into(), prop("integer", "Width for resize/move"));
        props.insert("height".into(), prop("integer", "Height for resize/move"));
        props.insert(
            "region".into(),
            prop("string", "Screenshot region: 'x,y,w,h'"),
        );
        props.insert(
            "name".into(),
            prop("string", "Name for shortcut/menu/contact/reminder"),
        );
        props.insert("value".into(), prop("string", "Value to set"));
        props.insert("role".into(), prop("string", "UI element role filter"));
        props.insert("label".into(), prop("string", "UI element label"));
        props.insert("index".into(), prop("integer", "Index for space/menu"));
        props.insert("voice".into(), prop("string", "TTS voice name"));
        props.insert("rate".into(), prop("integer", "TTS speaking rate"));
        // Snapshot (see → click flow)
        props.insert(
            "element_id".into(),
            prop(
                "string",
                "Element ID from a snapshot (e.g. B1, T2). Use capture(action: see) first",
            ),
        );
        props.insert(
            "snapshot_id".into(),
            prop("string", "Snapshot ID from a previous see action"),
        );
        props.insert(
            "max_elements".into(),
            prop("integer", "Max elements returned by see (default: 100)"),
        );
        // Keychain
        props.insert("service".into(), prop("string", "Keychain service name"));
        props.insert("account".into(), prop("string", "Keychain account"));
        props.insert("password".into(), prop("string", "Password to store"));
        // Search
        props.insert("query".into(), prop("string", "Search query"));
        props.insert("dir".into(), prop("string", "Directory to search within"));
        // Organizer
        props.insert("email".into(), prop("string", "Email address"));
        props.insert("subject".into(), prop("string", "Email subject"));
        props.insert("body".into(), prop("string", "Email/event body"));
        props.insert(
            "to".into(),
            serde_json::json!({
                "oneOf": [
                    { "type": "string" },
                    { "type": "array", "items": { "type": "string" } }
                ],
                "description": "Email recipient(s)"
            }),
        );
        props.insert(
            "cc".into(),
            serde_json::json!({
                "type": "array",
                "items": { "type": "string" },
                "description": "CC recipient(s)"
            }),
        );
        props.insert(
            "mailbox".into(),
            prop("string", "Mailbox name (e.g. 'INBOX', 'Sent')"),
        );
        props.insert("calendar".into(), prop("string", "Calendar name"));
        props.insert(
            "date".into(),
            prop("string", "Start date (e.g. '2025-03-15 10:00', 'tomorrow')"),
        );
        props.insert(
            "end_date".into(),
            prop("string", "End date (defaults to start + 1 hour)"),
        );
        props.insert("location".into(), prop("string", "Event location"));
        props.insert(
            "days".into(),
            prop("integer", "Number of days to look ahead (default: 7)"),
        );
        props.insert("list".into(), prop("string", "Reminder list name"));
        props.insert(
            "due_date".into(),
            prop(
                "string",
                "Due date (e.g. '2025-03-15', 'tomorrow', 'in 3 days')",
            ),
        );
        props.insert(
            "priority".into(),
            prop("integer", "Priority: 1-3=high, 4-6=medium, 7-9=low"),
        );
        props.insert("phone".into(), prop("string", "Contact phone number"));
        props.insert(
            "company".into(),
            prop("string", "Contact company/organization"),
        );
        props.insert("notes".into(), prop("string", "Notes or description"));

        serde_json::json!({
            "type": "object",
            "properties": serde_json::Value::Object(props),
            "required": ["action"]
        })
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn requires_approval_for(&self, input: &serde_json::Value) -> bool {
        let resource = input.get("resource").and_then(|v| v.as_str()).unwrap_or("");
        // If no resource, try to infer it from action, then from context
        let resource = if resource.is_empty() {
            let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
            let inferred = Self::infer_resource(action);
            if inferred.is_empty() {
                Self::infer_resource_from_context(input)
            } else {
                inferred
            }
        } else {
            resource
        };
        // Organizer resources: only write actions need approval
        match resource {
            "mail" | "contacts" | "calendar" | "reminders" => {
                let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
                ORGANIZER_WRITE_ACTIONS.contains(&action)
            }
            _ => !AUTO_APPROVE_RESOURCES.contains(&resource),
        }
    }

    fn resource_permit(&self, input: &serde_json::Value) -> Option<ResourceKind> {
        let resource = input.get("resource").and_then(|v| v.as_str()).unwrap_or("");
        let resource = if resource.is_empty() {
            let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
            let inferred = OsTool::infer_resource(action);
            if inferred.is_empty() {
                OsTool::infer_resource_from_context(input)
            } else {
                inferred
            }
        } else {
            resource
        };
        match resource {
            // Physical screen resources — one mouse, one keyboard, one display
            "window" | "input" | "ui" | "menu" | "dialog" | "space" | "shortcut" => {
                Some(ResourceKind::Screen)
            }
            // Parallelizable: capture, app, clipboard, notification, tts, dock, file,
            // shell, settings, music, keychain, search, mail, contacts, calendar, reminders
            _ => None,
        }
    }

    fn is_concurrent_safe(&self, input: &serde_json::Value) -> bool {
        let resource = input.get("resource").and_then(|v| v.as_str()).unwrap_or("");
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let resource = if resource.is_empty() {
            let inferred = OsTool::infer_resource(action);
            if inferred.is_empty() {
                OsTool::infer_resource_from_context(input)
            } else {
                inferred
            }
        } else {
            resource
        };
        match resource {
            "file" => matches!(action, "read" | "list" | "glob" | "grep"),
            "search" => true,
            "capture" => matches!(action, "screenshot" | "see"),
            _ => false,
        }
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let domain_input: DomainInput = match serde_json::from_value(input.clone()) {
                Ok(v) => v,
                Err(e) => {
                    return ToolResult::error(format!("Failed to parse input: {}", e));
                }
            };

            const RESOURCE_NAMES: &[&str] = &[
                "file", "shell", "window", "input", "clipboard", "capture", "notification",
                "ui", "menu", "dialog", "space", "shortcut", "tts", "dock",
                "app", "settings", "music", "keychain", "search",
                "mail", "contacts", "calendar", "reminders",
            ];

            let mut input = input;

            // File-management verbs (move/copy/rename/delete/mkdir) with file-shaped
            // args are file operations, NOT a mouse "move" — but action-name inference
            // resolves bare "move" to the desktop "input" resource, which then gated on
            // the wrong (Desktop) capability and surfaced a misleading "need Desktop".
            // The file tool has no move/copy/delete (those go through the shell),
            // so steer the agent to shell `mv`/`cp`/`rm` instead of
            // misrouting. Disambiguated by file args: a real mouse move never carries
            // `path` + `destination`.
            {
                let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
                if Self::is_file_mgmt_redirect(&input) {
                    let src = input.get("path").and_then(|v| v.as_str()).unwrap_or("<src>");
                    let dst = input
                        .get("destination")
                        .or_else(|| input.get("to"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("<dst>");
                    let cmd = match action {
                        "copy" => format!("cp {src} {dst}"),
                        "delete" | "remove" | "trash" => format!("rm {src}"),
                        "mkdir" => format!("mkdir -p {src}"),
                        "rmdir" => format!("rmdir {src}"),
                        _ => format!("mv {src} {dst}"),
                    };
                    return ToolResult::error(format!(
                        "The file tool only reads/writes/edits files — it has no '{action}'. \
                         To move, copy, rename, or delete files, run the shell command directly: \
                         os(resource: \"shell\", action: \"exec\", command: \"{cmd}\")"
                    ));
                }
            }

            let resource = {
                let corrected = crate::domain::auto_correct_resource(
                    &domain_input,
                    &mut input,
                    RESOURCE_NAMES,
                );
                if corrected.is_empty() {
                    let inferred = Self::infer_resource(&domain_input.action);
                    if inferred.is_empty() {
                        Self::infer_resource_from_context(&input).to_string()
                    } else {
                        inferred.to_string()
                    }
                } else {
                    corrected
                }
            };

            if resource.is_empty() {
                return ToolResult::error(
                    "Resource is required. Available: file, shell, window, input, clipboard, capture, \
                     notification, ui, menu, dialog, space, shortcut, tts, dock, app, settings, music, \
                     keychain, search, mail, contacts, calendar, reminders",
                );
            }

            // Ensure resource is present in input for downstream tools
            if !input
                .get("resource")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
            {
                input["resource"] = serde_json::Value::String(resource.clone());
            }

            // Desktop-bound resources have no counterpart in a cloud deploy —
            // no screen, input devices, or Mail/Calendar apps. Refuse with a
            // reason the model can act on, instead of letting the platform
            // layer fail deep inside with a cryptic xdotool/Evolution error.
            // file/shell/web/search all work normally here, so only these are
            // gated.
            if crate::server_mode()
                && matches!(
                    resource.as_str(),
                    "window"
                        | "input"
                        | "clipboard"
                        | "capture"
                        | "notification"
                        | "ui"
                        | "menu"
                        | "dialog"
                        | "space"
                        | "shortcut"
                        | "tts"
                        | "dock"
                        | "mail"
                        | "contacts"
                        | "calendar"
                        | "reminders"
                )
            {
                return ToolResult::error(format!(
                    "os(resource: \"{resource}\") is not available in server mode — this Nebo runs in the cloud and has no screen, input devices, or desktop apps. File, shell, and web tools work normally."
                ));
            }

            match resource.as_str() {
                // File + Shell — delegate to inner tools. `convert` is handled
                // here (not in FileTool) because rendering runs on the async
                // bundled-browser engine; everything else about it is a file op.
                "file" if input["action"].as_str() == Some("convert") => {
                    self.handle_convert(&input).await
                }
                "file" => self.file_tool.execute(ctx, input),
                "shell" => self.shell_tool.execute(ctx, input).await,

                // Desktop resources — delegate to DesktopTool
                "window" | "input" | "clipboard" | "capture" | "notification" | "ui" | "menu"
                | "dialog" | "space" | "shortcut" | "tts" | "dock" => {
                    self.desktop_tool.execute_dyn(ctx, input).await
                }

                // App lifecycle
                "app" => self.app_tool.execute_dyn(ctx, input).await,

                // Settings — OsTool action = setting name, value determines operation
                "settings" => {
                    let action = input["action"].as_str().unwrap_or("");
                    let has_value = input
                        .get("value")
                        .and_then(|v| if v.is_null() { None } else { Some(v) })
                        .is_some();
                    let mut settings_input = input.clone();

                    // The OsTool action IS the setting name (volume, wifi, etc.)
                    // Infer the SettingsTool action from the setting type + context
                    let settings_action = match action {
                        "sleep" | "lock" | "mute" => "trigger",
                        "volume" | "brightness" => {
                            if has_value {
                                "set"
                            } else {
                                "get"
                            }
                        }
                        "wifi" | "bluetooth" | "darkmode" => {
                            if has_value {
                                "toggle"
                            } else {
                                "status"
                            }
                        }
                        "battery" | "info" => "get",
                        other => {
                            return ToolResult::error(format!(
                                "Unknown setting '{}'. Use: volume, brightness, wifi, bluetooth, battery, darkmode, sleep, lock, info, mute (value: true|false)",
                                other
                            ));
                        }
                    };
                    settings_input["resource"] = serde_json::Value::String(action.to_string());
                    settings_input["action"] =
                        serde_json::Value::String(settings_action.to_string());
                    self.settings_tool.execute_dyn(ctx, settings_input).await
                }

                // Music
                "music" => self.music_tool.execute_dyn(ctx, input).await,

                // Keychain
                "keychain" => self.keychain_tool.execute_dyn(ctx, input).await,

                // File search
                "search" => self.spotlight_tool.execute_dyn(ctx, input).await,

                // PIM — parse OrganizerInput and dispatch to handler functions directly
                "mail" | "contacts" | "calendar" | "reminders" => {
                    let parsed: organizer::OrganizerInput = match serde_json::from_value(input) {
                        Ok(v) => v,
                        Err(e) => {
                            return ToolResult::error(format!("Failed to parse input: {}", e));
                        }
                    };
                    match resource.as_str() {
                        "mail" => organizer::handle_mail(&parsed.action, &parsed).await,
                        "contacts" => organizer::handle_contacts(&parsed.action, &parsed).await,
                        "calendar" => {
                            organizer::handle_calendar(
                                &parsed.action,
                                &parsed,
                                ctx,
                                self.store.as_ref(),
                            )
                            .await
                        }
                        "reminders" => organizer::handle_reminders(&parsed.action, &parsed).await,
                        _ => unreachable!(),
                    }
                }

                other => ToolResult::error(format!(
                    "Unknown resource '{}'. Available: file, shell, window, input, clipboard, capture, \
                     notification, ui, menu, dialog, space, shortcut, tts, dock, app, settings, music, \
                     keychain, search, mail, contacts, calendar, reminders",
                    other
                )),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_resource() {
        assert_eq!(OsTool::infer_resource("read"), "file");
        assert_eq!(OsTool::infer_resource("exec"), "shell");
        assert_eq!(OsTool::infer_resource("click"), "input");
        assert_eq!(OsTool::infer_resource("screenshot"), "capture");
        assert_eq!(OsTool::infer_resource("play"), "music");
        assert_eq!(OsTool::infer_resource("launch"), "app");
        assert_eq!(OsTool::infer_resource("speak"), "tts");
        assert_eq!(OsTool::infer_resource("unread"), "mail");
        assert_eq!(OsTool::infer_resource("today"), "calendar");
        assert_eq!(OsTool::infer_resource("unknown_action"), "");
    }

    #[test]
    fn test_infer_resource_from_context_list_with_target() {
        // "list" with a dir/path target is a directory listing → file
        let input = serde_json::json!({"action": "list", "dir": "~/Desktop"});
        assert_eq!(OsTool::infer_resource_from_context(&input), "file");
        let input = serde_json::json!({"action": "ls", "path": "/tmp"});
        assert_eq!(OsTool::infer_resource_from_context(&input), "file");
        // Bare "list" stays ambiguous (window, app, shell, ...)
        let input = serde_json::json!({"action": "list"});
        assert_eq!(OsTool::infer_resource_from_context(&input), "");
        // "list" with a reminders list name still routes to reminders
        let input = serde_json::json!({"action": "list", "list": "Groceries"});
        assert_eq!(OsTool::infer_resource_from_context(&input), "reminders");
    }

    #[test]
    fn test_approval_map() {
        let tool = OsTool::new(
            crate::policy::Policy::default(),
            Arc::new(crate::process::ProcessRegistry::new()),
        );

        // Auto-approve resources
        for resource in AUTO_APPROVE_RESOURCES {
            let input = serde_json::json!({"resource": resource, "action": "test"});
            assert!(
                !tool.requires_approval_for(&input),
                "{} should auto-approve",
                resource
            );
        }

        // Requires-approval resources (non-organizer)
        let sensitive = [
            "input", "window", "ui", "menu", "dialog", "app", "settings", "music", "keychain",
            "space", "shortcut",
        ];
        for resource in &sensitive {
            let input = serde_json::json!({"resource": resource, "action": "test"});
            assert!(
                tool.requires_approval_for(&input),
                "{} should require approval",
                resource
            );
        }
    }

    #[test]
    fn test_infer_resource_approval() {
        let tool = OsTool::new(
            crate::policy::Policy::default(),
            Arc::new(crate::process::ProcessRegistry::new()),
        );
        // read → file → auto-approve
        let input = serde_json::json!({"action": "read", "path": "/tmp/test"});
        assert!(!tool.requires_approval_for(&input));

        // click → input → requires approval
        let input = serde_json::json!({"action": "click", "x": 100, "y": 200});
        assert!(tool.requires_approval_for(&input));
    }

    #[test]
    fn test_organizer_read_actions_auto_approve() {
        let tool = OsTool::new(
            crate::policy::Policy::default(),
            Arc::new(crate::process::ProcessRegistry::new()),
        );
        let read_actions = [
            ("mail", "unread"),
            ("mail", "accounts"),
            ("mail", "read"),
            ("mail", "search"),
            ("contacts", "search"),
            ("contacts", "get"),
            ("contacts", "groups"),
            ("calendar", "today"),
            ("calendar", "upcoming"),
            ("calendar", "calendars"),
            ("calendar", "list"),
            ("calendar", "configure"),
            ("reminders", "lists"),
            ("reminders", "list"),
        ];
        for (resource, action) in &read_actions {
            let input = serde_json::json!({"resource": resource, "action": action});
            assert!(
                !tool.requires_approval_for(&input),
                "os(resource: \"{}\", action: \"{}\") should auto-approve",
                resource,
                action
            );
        }
    }

    #[test]
    fn test_organizer_write_actions_require_approval() {
        let tool = OsTool::new(
            crate::policy::Policy::default(),
            Arc::new(crate::process::ProcessRegistry::new()),
        );
        let write_actions = [
            ("mail", "send"),
            ("contacts", "create"),
            ("calendar", "create"),
            ("reminders", "create"),
            ("reminders", "complete"),
            ("reminders", "delete"),
        ];
        for (resource, action) in &write_actions {
            let input = serde_json::json!({"resource": resource, "action": action});
            assert!(
                tool.requires_approval_for(&input),
                "os(resource: \"{}\", action: \"{}\") should require approval",
                resource,
                action
            );
        }
    }

    #[test]
    fn test_infer_resource_from_context() {
        // Calendar: date param present → infer "calendar"
        assert_eq!(
            OsTool::infer_resource_from_context(
                &serde_json::json!({"action": "create", "date": "2025-06-15"})
            ),
            "calendar"
        );
        // Reminders: due_date present → infer "reminders"
        assert_eq!(
            OsTool::infer_resource_from_context(
                &serde_json::json!({"action": "create", "due_date": "tomorrow"})
            ),
            "reminders"
        );
        // Contacts: phone present → infer "contacts"
        assert_eq!(
            OsTool::infer_resource_from_context(
                &serde_json::json!({"action": "create", "phone": "555-1234"})
            ),
            "contacts"
        );
        // Mail: to present → infer "mail"
        assert_eq!(
            OsTool::infer_resource_from_context(
                &serde_json::json!({"action": "send", "to": "user@example.com"})
            ),
            "mail"
        );
        // No context → empty
        assert_eq!(
            OsTool::infer_resource_from_context(&serde_json::json!({"action": "create"})),
            ""
        );
    }

    #[test]
    fn test_infer_configure() {
        assert_eq!(OsTool::infer_resource("configure"), "calendar");
    }

    #[test]
    fn test_resource_as_action_autocorrect() {
        // When LLM puts resource name as action (e.g. os(action: "calendar")),
        // requires_approval_for should still resolve correctly via inference
        let tool = OsTool::new(
            crate::policy::Policy::default(),
            Arc::new(crate::process::ProcessRegistry::new()),
        );
        // "calendar" as action → infer_resource returns "" → infer_from_context → ""
        // But in execute_dyn, RESOURCE_NAMES check catches it
        let input = serde_json::json!({"action": "calendar"});
        // Should not panic at minimum
        let _ = tool.requires_approval_for(&input);
    }

    #[test]
    fn test_schema_requires_resource() {
        let tool = OsTool::new(
            crate::policy::Policy::default(),
            Arc::new(crate::process::ProcessRegistry::new()),
        );
        let schema = tool.schema();
        let required = schema["required"].as_array().unwrap();
        let required_strs: Vec<&str> = required.iter().map(|v| v.as_str().unwrap()).collect();
        // One precise calling convention: `action` is the only required field.
        // `resource` is optional — inferred from the action (read→file), passed
        // only to disambiguate shared actions (create, list). See infer_resource.
        assert!(required_strs.contains(&"action"), "schema must require 'action'");
        assert!(
            !required_strs.contains(&"resource"),
            "resource must NOT be required — it is inferred from action"
        );
    }

    #[test]
    fn test_schema_has_grep_fields() {
        let tool = OsTool::new(
            crate::policy::Policy::default(),
            Arc::new(crate::process::ProcessRegistry::new()),
        );
        let schema = tool.schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("output_mode"), "schema missing output_mode");
        assert!(props.contains_key("context_before"), "schema missing context_before");
        assert!(props.contains_key("context_after"), "schema missing context_after");
    }
}
