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
/// 23 resources spanning file system, shell, desktop automation, apps, settings,
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
    /// Each verify command gets this long; a build that needs more belongs in
    /// a background shell call, not a plan step.
    const PLAN_VERIFY_TIMEOUT_SECS: u64 = 120;
    /// One line of stderr per failing step in the plan document.
    const PLAN_NOTE_CHARS: usize = 160;

    /// `os(resource: "file", action: "plan_check", path: "plan.md")`: run every
    /// step's verify command and rewrite the checkboxes from the exit codes.
    /// The model cannot tick a box; only a passing command can. A check that
    /// verifies nothing new is reported as an error so a stalled plan never
    /// counts as progress.
    async fn handle_plan_check(&self, ctx: &ToolContext, input: &serde_json::Value) -> ToolResult {
        let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("");
        if path.is_empty() {
            return ToolResult::error(
                "plan_check needs `path`: the plan file written by action: \"plan\"",
            );
        }
        let path = match ctx.cwd.as_deref() {
            Some(cwd) if std::path::Path::new(path).is_relative() => {
                std::path::Path::new(cwd).join(path).to_string_lossy().into_owned()
            }
            _ => path.to_string(),
        };
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("read {path}: {e}")),
        };
        let plan = match crate::plan::parse(&content) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(e),
        };
        let dir = std::path::Path::new(&path)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".".into());
        let mut results = Vec::with_capacity(plan.steps.len());
        for step in &plan.steps {
            // Same policy, same refusals as any shell call; raw mode returns
            // stdout only on success and an error carrying stderr otherwise.
            let out = self
                .shell_tool
                .execute(
                    ctx,
                    serde_json::json!({
                        "resource": "shell", "action": "exec", "command": step.verify,
                        "cwd": dir, "timeout": Self::PLAN_VERIFY_TIMEOUT_SECS, "raw": true
                    }),
                )
                .await;
            // Raw mode reports a failure as "Command exited with code N\n<stderr>";
            // a policy refusal (destructive git) has no such header: "did not run".
            let (header, rest) = out.content.split_once('\n').unwrap_or((out.content.as_str(), ""));
            let exit = header
                .strip_prefix("Command exited with code ")
                .and_then(|c| c.trim().parse::<i32>().ok())
                .or(if out.is_error { None } else { Some(0) });
            let note = if !out.is_error {
                String::new()
            } else if exit.is_some() {
                crate::plan::first_line(rest, Self::PLAN_NOTE_CHARS)
            } else {
                crate::plan::first_line(&out.content, Self::PLAN_NOTE_CHARS)
            };
            results.push(crate::plan::StepResult { n: step.n, ok: !out.is_error, exit, note });
        }
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let (rewritten, newly) = crate::plan::apply(&content, &results, &now);
        let write = self.file_tool.write_document(&ctx.session_key, &path, &rewritten);
        if write.is_error {
            return write;
        }
        let verified = results.iter().filter(|r| r.ok).count();
        let mut summary = format!(
            "plan_check {}: {verified} of {} steps pass; {newly} newly passed on this check\n",
            path,
            results.len()
        );
        for r in &results {
            let title = plan.steps.iter().find(|s| s.n == r.n).map(|s| s.title.as_str()).unwrap_or("");
            if r.ok {
                summary.push_str(&format!("  {}. ✓ {title}\n", r.n));
            } else {
                let exit = r.exit.map(|c| format!("exit {c}")).unwrap_or_else(|| "did not run".into());
                summary.push_str(&format!("  {}. ✗ {title}, {exit}{}\n", r.n, if r.note.is_empty() { String::new() } else { format!(": {}", r.note) }));
            }
        }
        let mut result = if verified == 0 && newly == 0 {
            summary.push_str("Nothing verified. Fix the failing steps and check again; do not report the task done.");
            ToolResult::error(summary)
        } else {
            ToolResult::ok(summary)
        };
        result.payload = Some(serde_json::json!({ "newly_verified": newly, "verified": verified, "steps": results.len() }));
        result
    }

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
                return ToolResult::error(format!("Error converting: {msg}"));
            }
            Err(e) => return ToolResult::error(format!("Error converting: {e}")),
        };
        let out = src_path.with_extension(to);
        let replaced = out.exists();
        if let Err(e) = std::fs::write(&out, &bytes) {
            return ToolResult::error(format!("Error writing {}: {e}", out.display()));
        }
        let out_str = out.to_string_lossy().to_string();
        ToolResult::ok(format!(
            "Converted {src} to {out_str} ({} bytes{})",
            bytes.len(),
            if replaced { ", replacing the previous file" } else { "" }
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

    /// The redirect text for a file-management verb: names the shell command
    /// that does the job, with every path quoted for the shell, `rm -r` when
    /// the path is a directory, and no pretence that `rm` moves anything to
    /// the Trash.
    fn file_mgmt_redirect_message(input: &serde_json::Value) -> String {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let src_raw = input.get("path").and_then(|v| v.as_str()).unwrap_or("<src>");
        let dst_raw = input
            .get("destination")
            .or_else(|| input.get("to"))
            .and_then(|v| v.as_str())
            .unwrap_or("<dst>");
        let src = shell_quote(src_raw);
        let dst = shell_quote(dst_raw);
        let src_is_dir = std::path::Path::new(&crate::file_tool::expand_path(src_raw)).is_dir();
        let dir_flag = if src_is_dir { " -r" } else { "" };
        let cmd = match action {
            "copy" => format!("cp{dir_flag} {src} {dst}"),
            "delete" | "remove" | "trash" => format!("rm{dir_flag} {src}"),
            "mkdir" => format!("mkdir -p {src}"),
            "rmdir" => format!("rmdir {src}"),
            _ => format!("mv {src} {dst}"),
        };
        let trash_note = if action == "trash" {
            " Note: rm deletes permanently; it does not move the file to the Trash. If the user asked for the Trash, tell them that."
        } else {
            ""
        };
        format!(
            "The file resource has no '{action}' action. Use the shell: \
             os(resource: \"shell\", action: \"exec\", command: \"{cmd}\"){trash_note}"
        )
    }

    /// Resolve the effective resource of an os call — THE canonical chain:
    /// explicit non-empty `resource` field → [`Self::infer_resource`] from the
    /// action name → [`Self::infer_resource_from_context`] from the parameters.
    /// Every consumer (approval gate, resource permits, concurrency, capability
    /// gating, safeguards, path scoping) must use this so they all agree on
    /// which resource a call targets.
    /// The resource a call operates on, inferring it when the model omitted the
    /// field. PUBLIC because it is the ONE definition of that inference — the
    /// history summarizer (`agent::pruning`) must classify a call exactly as the
    /// executor did, or it mislabels the call and can destroy its result
    /// (2026-08-28: a bare `os {"action":"read","path":…}` was summarized as
    /// `[os] 0 lines` and the model believed the file was empty).
    pub fn resolved_resource(input: &serde_json::Value) -> &str {
        let resource = input.get("resource").and_then(|v| v.as_str()).unwrap_or("");
        if !resource.is_empty() {
            return resource;
        }
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        // Parameters settle an action that several resources share before its
        // bare name does (a `read` without a path is a mail read, not a file
        // read that then demands `path`).
        let shared = Self::infer_resource_from_shared_action(action, input);
        if !shared.is_empty() {
            return shared;
        }
        let inferred = Self::infer_resource(action);
        if inferred.is_empty() {
            Self::infer_resource_from_context(input)
        } else {
            inferred
        }
    }

    /// Actions one resource owns by name that another resource also uses,
    /// settled by the parameters the call carries. Each arm is a misroute the
    /// 2026-09-05 audit found live: a window `move` with `app` went to the
    /// mouse, a notification `send` went to Mail (and its approval gate), a
    /// stdin `write` went to the file tool, and every session verb with a
    /// `session_id` was unroutable.
    pub(crate) fn infer_resource_from_shared_action(
        action: &str,
        input: &serde_json::Value,
    ) -> &'static str {
        let has = |k: &str| {
            input
                .get(k)
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
        };
        let has_pid = input.get("pid").and_then(|v| v.as_i64()).is_some_and(|p| p > 0);
        let has_input_target = has("ref")
            || has("element_id")
            || has("element")
            || input.get("coordinate").is_some()
            || input.get("x").is_some();
        match action {
            // `account` alone is safe here: keychain uses get/find, never "read".
            "read" if input.get("path").is_none() => {
                let ctx = Self::infer_resource_from_context(input);
                if !ctx.is_empty() {
                    ctx
                } else if has("account") {
                    "mail"
                } else {
                    ""
                }
            }
            "write" if has("session_id") => "shell",
            "kill" | "info" | "list" | "status" | "poll" | "log"
                if has("session_id") || has_pid =>
            {
                "shell"
            }
            "move" if has("app") => "window",
            "click" if has("name") => "dialog",
            "click" if has("label") || has("role") => "ui",
            "click" if has("app") && !has_input_target => "ui",
            "send" if input.get("to").is_none() && (has("title") || has("message")) => {
                "notification"
            }
            _ => "",
        }
    }

    /// Infer resource from action name when resource field is omitted.
    pub(crate) fn infer_resource(action: &str) -> &str {
        match action {
            // File
            "read" | "write" | "edit" | "share" | "glob" | "grep" | "convert" | "checkpoint"
            | "checkpoints" | "restore" | "plan" | "plan_check" => "file",
            // Shell
            "exec" | "poll" | "log" => "shell",
            // Input
            "click" | "type" | "press" | "move" | "double_click" | "right_click" | "hotkey"
            | "scroll" | "drag" | "paste" => "input",
            // Capture ("capture" is what the desktop straps call a screenshot)
            "screenshot" | "see" | "capture" => "capture",
            // Settings: every setting is its own action name
            "volume" | "brightness" | "mute" | "unmute" | "wifi" | "bluetooth" | "darkmode"
            | "battery" => "settings",
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
        // Keychain: a `password` param is uniquely keychain-shaped, and
        // `service` with a keychain verb is too — models often write the full
        // arg set (service/account/password) and drop `resource`, which used
        // to cost three "Resource is required" errors before the first store.
        let has_kc_field = |key: &str| {
            input
                .get(key)
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
        };
        if has_kc_field("password")
            || ((has_kc_field("service") || has_kc_field("label"))
                && matches!(action, "get" | "find" | "add" | "store" | "delete"))
        {
            return "keychain";
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
            || input
                .get("email")
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

    /// The action a call plainly means when it names none (the agent tool's
    /// `infer_missing_action` precedent). A live run wrote
    /// os({glob: "*.md", path: ...}) and got "missing field `action`"; the
    /// shape is in the error-shape baseline, so it recurs. Anything less
    /// obvious still needs `action`.
    pub(crate) fn infer_missing_action(input: &serde_json::Value) -> Option<&'static str> {
        let obj = input.as_object()?;
        let has = |k: &str| obj.get(k).is_some_and(|v| !v.is_null() && v.as_str() != Some(""));
        if has("action") {
            return None;
        }
        if has("glob") || (has("pattern") && has("path") && !has("content")) {
            return Some("glob");
        }
        if has("command") {
            return Some("exec");
        }
        if has("path") && has("content") {
            return Some("write");
        }
        if has("path") && has("old_string") {
            return Some("edit");
        }
        if has("path") {
            return Some("read");
        }
        None
    }

    /// The actions the parse error lists when a call names none and the
    /// fields do not settle it.
    const ACTION_INDEX: &'static str = "file read/write/edit/glob/grep/share/convert/checkpoint/restore/plan; \
         shell exec/list/poll/log/write/kill/info; app launch/quit/activate/list; \
         capture screenshot/see; input click/type/press/scroll; \
         settings volume/brightness/mute/wifi/bluetooth/darkmode/battery; \
         mail unread/read/send/search; calendar today/upcoming/create; \
         reminders lists/list/create/complete; contacts search/get/create";

    /// The settings tool's call for an os settings action: the os action IS
    /// the setting name (volume, wifi, ...) and the presence of `value`
    /// decides get/set or status/toggle. `unmute` is `mute` with value false:
    /// one setting, one handler.
    pub(crate) fn settings_call(input: &serde_json::Value) -> Result<serde_json::Value, String> {
        let action = input["action"].as_str().unwrap_or("");
        let has_value = input.get("value").is_some_and(|v| !v.is_null());
        let mut call = input.clone();
        let (setting, settings_action) = match action {
            "sleep" | "lock" | "mute" => (action, "trigger"),
            "unmute" => {
                call["value"] = serde_json::json!(false);
                ("mute", "trigger")
            }
            "volume" | "brightness" => (action, if has_value { "set" } else { "get" }),
            "wifi" | "bluetooth" | "darkmode" => (action, if has_value { "toggle" } else { "status" }),
            "battery" | "info" => (action, "get"),
            other => {
                return Err(format!(
                    "Unknown setting '{other}'. Use: volume, brightness, wifi, bluetooth, battery, \
                     darkmode, sleep, lock, info, mute (value: true|false), unmute"
                ));
            }
        };
        call["resource"] = serde_json::json!(setting);
        call["action"] = serde_json::json!(settings_action);
        Ok(call)
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
         - Always pass `action`. `resource` is inferred when the action belongs to one resource (read→file, exec→shell, play→music, volume→settings) or its parameters settle it (session_id→shell, move+app→window, click+label→ui, send+title→notification); pass it for actions several resources share (create, list, search, get, delete).\n\
         - Interactive React (dashboards, charts, visualizations): write the component as a .jsx file, then convert it (action: \"convert\", to: \"html\") — Nebo transpiles it into a self-contained, renderable page. NEVER put JSX or CDN-loaded React (unpkg/esm) directly in a .html; raw JSX has no transpiler in the browser and renders blank.\n\
         - Before edit or overwrite of an EXISTING file, read it first (edit/overwrite are rejected without a prior read). A brand-new file needs no prior read.\n\
         - glob = find files by NAME pattern (*.md, src/**/*.rs); grep = match text INSIDE files by regex. Do not confuse them.\n\
         - NEVER use sudo without asking the user first; on permission denied, explain and offer alternatives.\n\n\
         Resources:\n\
         - file: read, write, edit, share, glob, grep, convert, checkpoint, checkpoints, restore, plan, plan_check — checkpoint snapshots the files you are about to change (paths: [...]) and restore puts them back (never git stash/reset); plan writes a work document whose steps each carry a verify command, and plan_check runs those commands and ticks only the steps that pass; to list a directory, glob its path (pattern defaults to *); share hands an EXISTING file to the user as a download card (a deck/PDF/binary already on disk — never recite its path or copy it to \"trigger\" a card); convert generates documents via embedded engines: .md→pdf/docx, .csv→xlsx, .jsx/.tsx→html (interactive React) (never use host binaries like wkhtmltopdf/pandoc)\n\
         - shell: exec, list (background sessions; with filter: system processes), poll, log, write (data), kill, info (session_id or pid)\n\
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
         - settings: volume, brightness, wifi, bluetooth, battery, darkmode, sleep, lock, info, mute (value: true|false), unmute\n\
         - music: play, pause, next, previous, status, search, volume, playlists, shuffle\n\
         - keychain: get, find, add (alias: store), delete (account optional — narrows the match)\n\
         - search: search (file search via OS index)\n\
         - mail: accounts, unread, read, send, search — LOCAL Apple Mail. read/search take optional account (name or address, e.g. \"sites@stadium.partners\") + mailbox; search is a SUBSTRING match on subject/sender (no Gmail operators like from:)\n\
         - contacts: search, get, create, groups\n\
         - calendar: calendars, today, upcoming, create, delete, pending, accept, decline, auto_accept, list, configure — the LOCAL Apple/Mac calendar (for Google Calendar use plugin(resource: \"gws\", ...))\n\
         - reminders: lists, list, create, complete, delete\n\n\
         Examples:\n  \
         os(resource: \"file\", action: \"read\", path: \"/path/to/file.txt\")\n  \
         os(resource: \"shell\", action: \"exec\", command: \"ls -la\")\n  \
         os(resource: \"app\", action: \"launch\", app: \"Safari\")\n  \
         os(resource: \"capture\", action: \"screenshot\")\n  \
         os(resource: \"capture\", action: \"see\", app: \"Safari\") — returns snapshot_id + element IDs\n  \
         os(resource: \"input\", action: \"click\", ref: \"B3\") — click element from snapshot (or coordinate: [x, y])\n  \
         os(resource: \"input\", action: \"type\", ref: \"T1\", text: \"hello\") — focus + type\n  \
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
        // Checkpoints and plans: declared here so the model can pass them.
        // A parameter that exists only in prose gets stripped by strict
        // providers and the model loops on "restore needs `checkpoint`".
        props.insert(
            "paths".into(),
            serde_json::json!({
                "type": "array",
                "items": { "type": "string" },
                "description": "checkpoint: the files you are about to change (absolute paths). restore: optional subset of the checkpoint's files to put back."
            }),
        );
        props.insert("label".into(), prop("string", "checkpoint: a short label, e.g. \"before rename\""));
        props.insert(
            "checkpoint".into(),
            prop("string", "restore: the checkpoint id (cp-…) from the checkpoint or checkpoints result"),
        );
        props.insert("title".into(), prop("string", "plan: the plan's title"));
        props.insert(
            "steps".into(),
            serde_json::json!({
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "what the step achieves" },
                        "verify": { "type": "string", "description": "shell command that exits 0 only when the step is done" }
                    },
                    "required": ["title", "verify"]
                },
                "description": "plan: one entry per step; every step needs a verify command"
            }),
        );
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
        props.insert("x".into(), prop("integer", "X coordinate for window move. Input actions take coordinate: [x, y] (x and y are read there too)"));
        props.insert("y".into(), prop("integer", "Y coordinate for window move"));
        props.insert(
            "coordinate".into(),
            serde_json::json!({
                "type": "array",
                "items": { "type": "integer" },
                "description": "Input click/move/type target as [x, y] on screen, when there is no element ref; drag end point"
            }),
        );
        props.insert(
            "start_coordinate".into(),
            serde_json::json!({
                "type": "array",
                "items": { "type": "integer" },
                "description": "Input drag start point as [x, y]"
            }),
        );
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
            "quality".into(),
            prop("string", "Screenshot quality: 'low' (800px JPEG), 'medium' (1280px JPEG, default), 'high' (full-res PNG)"),
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
            "ref".into(),
            prop(
                "string",
                "Input click/type/move target: the element ref from capture(action: see) (e.g. B1, T2)",
            ),
        );
        props.insert(
            "element_id".into(),
            prop("string", "Alias of ref"),
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
        props.insert("account".into(), prop("string", "Keychain account, or Mail account filter (name or address) for mail read/search"));
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
                "description": "convert: the target format, \"pdf\" (from .md/.typ), \"docx\" (from .md), \"xlsx\" (from .csv) or \"html\" (from .jsx/.tsx, interactive React); output lands next to the source. mail send: the recipient address(es)."
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
        let resource = Self::resolved_resource(input);
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
        match OsTool::resolved_resource(input) {
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
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        match OsTool::resolved_resource(input) {
            "file" => matches!(action, "read" | "list" | "glob" | "grep" | "checkpoints"),
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
            // Shorthand acceptance (first-call doctrine: fix the API, not the
            // client): a call that names no action but plainly means one is
            // normalized instead of rejected.
            let input = {
                let mut v = input;
                if let Some(action) = Self::infer_missing_action(&v) {
                    v["action"] = serde_json::json!(action);
                }
                v
            };
            let domain_input: DomainInput = match serde_json::from_value(input.clone()) {
                Ok(v) => v,
                Err(e) => {
                    let keys = input
                        .as_object()
                        .map(|o| o.keys().cloned().collect::<Vec<_>>().join(", "))
                        .unwrap_or_default();
                    return ToolResult::error(format!(
                        "Failed to parse input: {e}. Received fields: [{keys}]. Every `os` \
                         call needs an `action` (resource is inferred when omitted). Actions: \
                         {}. E.g. os(resource: \"shell\", action: \"exec\", command: \"ls -la\") or \
                         os(resource: \"file\", action: \"write\", path: \"...\", content: \"...\").",
                        Self::ACTION_INDEX
                    ));
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
                if Self::is_file_mgmt_redirect(&input) {
                    return ToolResult::error(Self::file_mgmt_redirect_message(&input));
                }
            }

            let resource = {
                let corrected = crate::domain::auto_correct_resource(
                    &domain_input,
                    &mut input,
                    RESOURCE_NAMES,
                );
                if corrected.is_empty() {
                    Self::resolved_resource(&input).to_string()
                } else {
                    corrected
                }
            };

            if resource.is_empty() {
                return ToolResult::error(format!(
                    "Could not infer a resource from action '{}'. Pass resource explicitly (file, shell, \
                     window, input, clipboard, capture, notification, ui, menu, dialog, space, shortcut, \
                     tts, dock, app, settings, music, keychain, search, mail, contacts, calendar, \
                     reminders) or use one of the documented actions.",
                    domain_input.action
                ));
            }

            // Settings VALUES models guess as resources: `os(resource:
            // "battery", action: "info")` is the natural first shape, but
            // battery/volume/brightness are ACTIONS on the settings
            // resource. Honor the guess instead of erroring.
            let resource = if matches!(resource.as_str(), "battery" | "volume" | "brightness") {
                input["action"] = serde_json::Value::String(resource.clone());
                "settings".to_string()
            } else {
                resource
            };
            input["resource"] = serde_json::Value::String(resource.clone());

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
            //
            // Exception: while the bot's on-demand desktop session is live
            // (the "computer" — Xvfb + xfce in this pod), the X11-backed
            // resources work against that display and are un-gated. Apps that
            // simply aren't in the image (Mail/Calendar/tts…) stay gated even
            // with a session up.
            if crate::server_mode() {
                let x11_backed = matches!(
                    resource.as_str(),
                    "window"
                        | "input"
                        | "clipboard"
                        | "capture"
                        | "ui"
                        | "menu"
                        | "dialog"
                        | "space"
                );
                let never_in_cloud = matches!(
                    resource.as_str(),
                    "notification"
                        | "shortcut"
                        | "tts"
                        | "dock"
                        | "mail"
                        | "contacts"
                        | "calendar"
                        | "reminders"
                );
                if never_in_cloud || (x11_backed && !crate::desktop_session::active()) {
                    return ToolResult::error(format!(
                        "os(resource: \"{resource}\") is not available in server mode — this Nebo runs in the cloud and has no screen, input devices, or desktop apps. File, shell, and web tools work normally."
                    ));
                }
            }

            match resource.as_str() {
                // File + Shell — delegate to inner tools. `convert` is handled
                // here (not in FileTool) because rendering runs on the async
                // bundled-browser engine; everything else about it is a file op.
                "file" if input["action"].as_str() == Some("convert") => {
                    self.handle_convert(&input).await
                }
                // plan_check needs the shell (each step's verify command) and
                // the file tool (the rewrite), so it lives here.
                "file" if input["action"].as_str() == Some("plan_check") => {
                    self.handle_plan_check(ctx, &input).await
                }
                "file" => self.file_tool.execute(ctx, input),
                "shell" => {
                    let command = input["command"].as_str().unwrap_or("").to_string();
                    let cwd = input["cwd"].as_str().map(str::to_string).or_else(|| ctx.cwd.clone());
                    let result = self.shell_tool.execute(ctx, input).await;
                    if !result.is_error && !command.is_empty() {
                        for target in crate::policy::shell_write_targets(&command) {
                            let path = std::path::Path::new(&target);
                            let abs = if path.is_relative() {
                                cwd.as_deref().map(|c| std::path::Path::new(c).join(path)).unwrap_or_else(|| path.to_path_buf())
                            } else {
                                path.to_path_buf()
                            };
                            self.file_tool.note_shell_write(&ctx.session_key, &abs.to_string_lossy());
                        }
                    }
                    result
                }

                // Desktop resources — delegate to DesktopTool
                "window" | "input" | "clipboard" | "capture" | "notification" | "ui" | "menu"
                | "dialog" | "space" | "shortcut" | "tts" | "dock" => {
                    self.desktop_tool.execute_dyn(ctx, input).await
                }

                // App lifecycle
                "app" => self.app_tool.execute_dyn(ctx, input).await,

                // Settings: the os action is the setting name; see settings_call.
                "settings" => match Self::settings_call(&input) {
                    Ok(settings_input) => self.settings_tool.execute_dyn(ctx, settings_input).await,
                    Err(msg) => ToolResult::error(msg),
                },

                // Music
                "music" => self.music_tool.execute_dyn(ctx, input).await,

                // Keychain
                "keychain" => self.keychain_tool.execute_dyn(ctx, input).await,

                // File search
                "search" => self.spotlight_tool.execute_dyn(ctx, input).await,

                // PIM — parse OrganizerInput and dispatch to handler functions directly
                "mail" | "contacts" | "calendar" | "reminders" => {
                    let keys = input
                        .as_object()
                        .map(|o| o.keys().cloned().collect::<Vec<_>>().join(", "))
                        .unwrap_or_default();
                    let parsed: organizer::OrganizerInput = match serde_json::from_value(input) {
                        Ok(v) => v,
                        Err(e) => {
                            return ToolResult::error(format!(
                                "Failed to parse input: {e}. Received fields: [{keys}]. Every `os` \
                                 {resource} call needs an `action`, e.g. \
                                 os(resource: \"mail\", action: \"unread\") or \
                                 os(resource: \"calendar\", action: \"today\")."
                            ));
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

                // Resources that live on OTHER tools: redirect with the exact
                // call, so a wrong-tool guess costs one corrected call, not a
                // hunt. (These are the names models actually reach for here.)
                res @ ("context" | "memory" | "session" | "task" | "profile" | "advisors") => {
                    ToolResult::error(format!(
                        "'{res}' is not an os resource — it lives on the `agent` tool. \
                         Call agent(resource: \"{res}\", action: ...) instead."
                    ))
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

/// Quote one path for a POSIX shell command line. Plain names pass through;
/// anything with spaces or shell metacharacters is single-quoted.
fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-~:@%+=,<>".contains(c));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The file-management redirect names a runnable shell command: paths
    /// with spaces are quoted, a directory gets `rm -r`, and `trash` never
    /// pretends rm moves anything to the Trash.
    #[test]
    fn file_mgmt_redirect_quotes_paths_and_handles_directories() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("my folder");
        std::fs::create_dir_all(&sub).unwrap();
        let sub_s = sub.to_string_lossy().into_owned();
        let msg = OsTool::file_mgmt_redirect_message(&serde_json::json!({
            "action": "delete", "path": sub_s
        }));
        assert!(msg.contains(&format!("rm -r '{sub_s}'")), "{msg}");
        assert!(msg.contains("has no 'delete' action"), "{msg}");

        let msg = OsTool::file_mgmt_redirect_message(&serde_json::json!({
            "action": "move", "path": "/tmp/a.txt", "destination": "/tmp/b c.txt"
        }));
        assert!(msg.contains("mv /tmp/a.txt '/tmp/b c.txt'"), "{msg}");

        let msg = OsTool::file_mgmt_redirect_message(&serde_json::json!({
            "action": "trash", "path": "/tmp/a.txt"
        }));
        assert!(msg.contains("rm /tmp/a.txt"), "{msg}");
        assert!(msg.contains("does not move the file to the Trash"), "{msg}");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    /// Every row of the 2026-09-05 misroute table (audit class C, os) plus
    /// the arms that already existed: the resource a call resolves to when
    /// it names none. One table so a dropped arm shows up as one row.
    #[test]
    fn resource_inference_table() {
        let cases: &[(serde_json::Value, &str)] = &[
            // Parameters settle a shared action name.
            (serde_json::json!({"action": "move", "app": "Safari", "x": 0, "y": 0}), "window"),
            (serde_json::json!({"action": "move", "coordinate": [10, 10]}), "input"),
            (serde_json::json!({"action": "click", "app": "Safari", "label": "OK"}), "ui"),
            (serde_json::json!({"action": "click", "role": "AXButton"}), "ui"),
            (serde_json::json!({"action": "click", "app": "Safari"}), "ui"),
            (serde_json::json!({"action": "click", "app": "Safari", "ref": "B3"}), "input"),
            (serde_json::json!({"action": "click", "name": "OK"}), "dialog"),
            (serde_json::json!({"action": "click", "x": 100, "y": 200}), "input"),
            (serde_json::json!({"action": "send", "title": "Done", "message": "Task complete"}), "notification"),
            (serde_json::json!({"action": "send", "message": "hi"}), "notification"),
            (serde_json::json!({"action": "send", "to": "a@b.c", "subject": "x"}), "mail"),
            (serde_json::json!({"action": "write", "session_id": "s1", "data": "y\n"}), "shell"),
            (serde_json::json!({"action": "write", "path": "/tmp/x", "content": "y"}), "file"),
            (serde_json::json!({"action": "kill", "session_id": "s1"}), "shell"),
            (serde_json::json!({"action": "info", "session_id": "s1"}), "shell"),
            (serde_json::json!({"action": "status", "session_id": "s1"}), "shell"),
            (serde_json::json!({"action": "list", "session_id": "s1"}), "shell"),
            (serde_json::json!({"action": "kill", "pid": 4242}), "shell"),
            (serde_json::json!({"action": "info", "pid": 4242}), "shell"),
            (serde_json::json!({"action": "poll", "session_id": "s1"}), "shell"),
            (serde_json::json!({"action": "log", "session_id": "s1"}), "shell"),
            (serde_json::json!({"action": "read", "mailbox": "INBOX"}), "mail"),
            (serde_json::json!({"action": "read", "path": "/tmp/x"}), "file"),
            // Action names that belong to one resource.
            (serde_json::json!({"action": "volume", "value": 50}), "settings"),
            (serde_json::json!({"action": "brightness"}), "settings"),
            (serde_json::json!({"action": "mute", "value": true}), "settings"),
            (serde_json::json!({"action": "unmute"}), "settings"),
            (serde_json::json!({"action": "battery"}), "settings"),
            (serde_json::json!({"action": "capture"}), "capture"),
            (serde_json::json!({"action": "screenshot"}), "capture"),
            (serde_json::json!({"action": "convert", "path": "r.md", "to": "pdf"}), "file"),
            (serde_json::json!({"action": "create", "name": "Ann", "email": "ann@x.com"}), "contacts"),
            // Shared names with nothing to settle them stay unrouted.
            (serde_json::json!({"action": "kill"}), ""),
            (serde_json::json!({"action": "list"}), ""),
            (serde_json::json!({"action": "search", "query": "x"}), ""),
            (serde_json::json!({"action": "get"}), ""),
        ];
        for (input, want) in cases {
            assert_eq!(OsTool::resolved_resource(input), *want, "{input}");
        }
    }

    /// A call that names no action but plainly means one gets it; anything
    /// less obvious keeps the parse error.
    #[test]
    fn missing_action_inference_table() {
        let cases: &[(serde_json::Value, Option<&str>)] = &[
            (serde_json::json!({"glob": "*.md", "path": "/tmp"}), Some("glob")),
            (serde_json::json!({"glob": "*.md"}), Some("glob")),
            (serde_json::json!({"pattern": "*.rs", "path": "/src"}), Some("glob")),
            (serde_json::json!({"command": "ls -la"}), Some("exec")),
            (serde_json::json!({"path": "/tmp/x", "content": "hello"}), Some("write")),
            (serde_json::json!({"path": "/tmp/x", "old_string": "a", "new_string": "b"}), Some("edit")),
            (serde_json::json!({"path": "/tmp/x"}), Some("read")),
            (serde_json::json!({"action": "grep", "glob": "*.md", "path": "/tmp"}), None),
            (serde_json::json!({"action": "", "path": "/tmp/x"}), Some("read")),
            (serde_json::json!({"pattern": "TODO"}), None),
            (serde_json::json!({"app": "Safari"}), None),
            (serde_json::json!({}), None),
        ];
        for (input, want) in cases {
            assert_eq!(OsTool::infer_missing_action(input), *want, "{input}");
        }
    }

    /// The whole shell lifecycle through `os`, the way the model reaches it:
    /// the os tool stamps resource "shell" and every session verb must still
    /// land on its handler (until 2026-09-05 each answered "exec requires
    /// command").
    #[tokio::test]
    async fn shell_session_verbs_reach_their_handlers_through_os() {
        let tool = os();
        let ctx = ToolContext::new(crate::origin::Origin::User);
        let start = tool
            .execute_dyn(
                &ctx,
                serde_json::json!({"resource": "shell", "action": "exec", "command": "sleep 5", "background": true}),
            )
            .await;
        assert!(!start.is_error, "{}", start.content);
        let id = start.content.split("**").nth(1).expect("session id between ** markers").to_string();

        let poll = tool
            .execute_dyn(&ctx, serde_json::json!({"resource": "shell", "action": "poll", "session_id": id}))
            .await;
        assert!(!poll.is_error, "{}", poll.content);
        assert!(poll.content.contains("Status: Running"), "{}", poll.content);

        let info = tool
            .execute_dyn(&ctx, serde_json::json!({"resource": "shell", "action": "info", "session_id": id}))
            .await;
        assert!(!info.is_error, "{}", info.content);
        assert!(info.content.contains("Command: `sleep 5`"), "{}", info.content);

        let log = tool
            .execute_dyn(&ctx, serde_json::json!({"action": "log", "session_id": id}))
            .await;
        assert!(!log.is_error, "{}", log.content);
        assert!(log.content.starts_with("(no output yet; still running"), "{}", log.content);

        let list = tool
            .execute_dyn(&ctx, serde_json::json!({"resource": "shell", "action": "list"}))
            .await;
        assert!(!list.is_error, "{}", list.content);
        assert!(list.content.contains(&id), "{}", list.content);

        let write = tool
            .execute_dyn(
                &ctx,
                serde_json::json!({"resource": "shell", "action": "write", "session_id": id, "data": "x\n"}),
            )
            .await;
        assert!(!write.is_error, "{}", write.content);
        assert!(write.content.starts_with("Wrote 2 bytes"), "{}", write.content);

        let kill = tool
            .execute_dyn(&ctx, serde_json::json!({"resource": "shell", "action": "kill", "session_id": id}))
            .await;
        assert!(!kill.is_error, "{}", kill.content);
        assert!(kill.content.contains("Killed session"), "{}", kill.content);

        // A kill with neither id nor pid says which of the two to pass.
        let bare = tool
            .execute_dyn(&ctx, serde_json::json!({"resource": "shell", "action": "kill"}))
            .await;
        assert!(bare.is_error);
        assert!(bare.content.contains("session_id is required"), "{}", bare.content);
        assert!(bare.content.contains("pid: <number>"), "{}", bare.content);
    }

    /// An os call with glob and path and no action runs as a glob.
    #[tokio::test]
    async fn glob_and_path_without_an_action_is_a_glob() {
        // A visible directory: tempdir names start with ".tmp" and the glob
        // walker skips hidden directories.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("alpha.md"), "# a").unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        let ctx = ToolContext::new(crate::origin::Origin::User);
        let r = os()
            .execute_dyn(&ctx, serde_json::json!({"glob": "*.md", "path": dir}))
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("alpha.md"), "{}", r.content);
        assert!(!r.content.contains("notes.txt"), "{}", r.content);

        let r = os().execute_dyn(&ctx, serde_json::json!({"app": "Safari"})).await;
        assert!(r.is_error);
        assert!(r.content.contains("missing field `action`"), "{}", r.content);
        assert!(r.content.contains("Actions: file read/write"), "{}", r.content);
    }

    /// `unmute` is `mute` with value false; the setting name becomes the
    /// settings resource and `value` picks the operation.
    #[test]
    fn settings_call_maps_the_setting_name_and_unmute() {
        let call = OsTool::settings_call(&serde_json::json!({"resource": "settings", "action": "unmute"})).unwrap();
        assert_eq!(call["resource"], "mute");
        assert_eq!(call["action"], "trigger");
        assert_eq!(call["value"], false);
        let call = OsTool::settings_call(&serde_json::json!({"action": "mute"})).unwrap();
        assert_eq!(call["resource"], "mute");
        assert!(call.get("value").is_none(), "mute alone keeps the handler's default (true)");
        let call = OsTool::settings_call(&serde_json::json!({"action": "volume", "value": 50})).unwrap();
        assert_eq!((call["resource"].as_str(), call["action"].as_str()), (Some("volume"), Some("set")));
        let call = OsTool::settings_call(&serde_json::json!({"action": "volume"})).unwrap();
        assert_eq!(call["action"], "get");
        let call = OsTool::settings_call(&serde_json::json!({"action": "wifi"})).unwrap();
        assert_eq!(call["action"], "status");
        let err = OsTool::settings_call(&serde_json::json!({"action": "loudness"})).unwrap_err();
        assert!(err.contains("Unknown setting 'loudness'"), "{err}");
        assert!(err.contains("unmute"), "{err}");
    }

    /// The os schema names the input target the handler reads (ref and
    /// coordinate), keeps element_id as its alias, declares quality, and
    /// describes `to` for both convert and mail (the second insert used to
    /// overwrite the first, leaving only the email meaning).
    #[test]
    fn os_schema_declares_input_targets_quality_and_both_meanings_of_to() {
        let schema = os().schema();
        let props = schema["properties"].as_object().expect("object schema");
        for p in ["ref", "coordinate", "start_coordinate", "quality", "element_id"] {
            assert!(props.contains_key(p), "schema is missing `{p}`");
        }
        assert_eq!(props["element_id"]["description"], "Alias of ref");
        let to = props["to"]["description"].as_str().unwrap();
        assert!(to.contains("pdf"), "{to}");
        assert!(to.contains("recipient"), "{to}");
    }

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

    /// AT-09's three "Resource is required" errors: a full keychain arg set
    /// (service/account/password) with `resource` dropped must route itself.
    #[test]
    fn keychain_shaped_args_infer_the_resource() {
        let input = serde_json::json!({
            "action": "add", "service": "myapp", "account": "me", "password": "s3cret"
        });
        assert_eq!(OsTool::resolved_resource(&input), "keychain");
        // A password alone is uniquely keychain-shaped, any verb.
        let input = serde_json::json!({"action": "store", "service": "x", "password": "p"});
        assert_eq!(OsTool::resolved_resource(&input), "keychain");
        // service + keychain verb, no password (get/find/delete legs).
        let input = serde_json::json!({"action": "delete", "service": "myapp"});
        assert_eq!(OsTool::resolved_resource(&input), "keychain");
        // A bare "delete" with a file path must NOT become keychain.
        let input = serde_json::json!({"action": "delete", "path": "/tmp/x"});
        assert_ne!(OsTool::resolved_resource(&input), "keychain");
        // An explicit resource always wins.
        let input = serde_json::json!({"resource": "file", "action": "delete", "service": "x"});
        assert_eq!(OsTool::resolved_resource(&input), "file");
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

    fn os() -> OsTool {
        OsTool::new(
            crate::policy::Policy::default(),
            Arc::new(crate::process::ProcessRegistry::new()),
        )
    }

    fn write_plan(dir: &std::path::Path, steps: &[(&str, &str)]) -> String {
        let steps: Vec<(String, String)> = steps.iter().map(|(t, v)| (t.to_string(), v.to_string())).collect();
        let doc = crate::plan::render("t", &steps).unwrap();
        let path = dir.join("PLAN.md");
        std::fs::write(&path, doc).unwrap();
        path.to_string_lossy().into_owned()
    }

    // The verify commands run in the plan's directory (relative paths in a
    // step mean "next to the plan"), through the shell's raw mode.
    #[tokio::test]
    async fn plan_check_runs_verify_in_the_plans_directory_with_raw_shell() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("marker"), "x").unwrap();
        let plan = write_plan(dir.path(), &[("marker is here", "test -f ./marker"), ("and is not elsewhere", "test -f /nonexistent/marker")]);
        let ctx = ToolContext::new(crate::origin::Origin::User);
        let r = os().execute_dyn(&ctx, serde_json::json!({"resource": "file", "action": "plan_check", "path": plan})).await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("1 of 2 steps pass; 1 newly passed"), "{}", r.content);
        let doc = std::fs::read_to_string(&plan).unwrap();
        assert!(doc.contains("- [x] 1."), "{doc}");
        assert!(doc.contains("- [ ] 2."), "{doc}");
        assert!(doc.contains("2. ✗ and is not elsewhere, exit 1"), "{doc}");
    }

    // A destructive verify command is refused like any shell call: the step
    // stays unticked and reads "did not run" with the refusal's first line.
    #[tokio::test]
    async fn plan_check_refuses_a_destructive_verify_command() {
        let dir = tempfile::tempdir().unwrap();
        let plan = write_plan(dir.path(), &[("bad", "git stash"), ("good", "true")]);
        let ctx = ToolContext::new(crate::origin::Origin::User);
        let r = os().execute_dyn(&ctx, serde_json::json!({"resource": "file", "action": "plan_check", "path": plan})).await;
        assert!(!r.is_error, "{}", r.content);
        let doc = std::fs::read_to_string(&plan).unwrap();
        assert!(doc.contains("1. ✗ bad, did not run: This git command discards work"), "{doc}");
        assert!(doc.contains("- [x] 2."), "{doc}");
    }

    // A check that verifies nothing is an error, so a stalled plan never
    // counts as progress; one newly verified step is not.
    #[tokio::test]
    async fn plan_check_sets_is_error_when_nothing_is_verified() {
        let dir = tempfile::tempdir().unwrap();
        let plan = write_plan(dir.path(), &[("fails", "false")]);
        let ctx = ToolContext::new(crate::origin::Origin::User);
        let r = os().execute_dyn(&ctx, serde_json::json!({"resource": "file", "action": "plan_check", "path": plan})).await;
        assert!(r.is_error, "{}", r.content);
        assert!(r.content.contains("Nothing verified"), "{}", r.content);
        assert_eq!(r.payload.as_ref().and_then(|p| p.get("newly_verified")).and_then(|v| v.as_u64()), Some(0));
        let sub = dir.path().join("b");
        std::fs::create_dir_all(&sub).unwrap();
        let plan2 = write_plan(&sub, &[("passes", "true")]);
        let r = os().execute_dyn(&ctx, serde_json::json!({"resource": "file", "action": "plan_check", "path": plan2})).await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(r.payload.as_ref().and_then(|p| p.get("newly_verified")).and_then(|v| v.as_u64()), Some(1));
    }

    // Every parameter the file actions read must be declared: a parameter that
    // lives only in prose is stripped by strict providers, and the model then
    // loops on "restore needs `checkpoint`" (49 calls, live, 2026-09-02).
    #[test]
    fn os_schema_declares_every_checkpoint_and_plan_parameter() {
        let schema = os().schema();
        let props = schema["properties"].as_object().expect("object schema");
        for p in ["paths", "label", "checkpoint", "title", "steps"] {
            assert!(props.contains_key(p), "schema is missing `{p}`");
        }
        assert_eq!(schema["properties"]["steps"]["items"]["required"], serde_json::json!(["title", "verify"]));
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
    fn test_resolved_resource_mail_read() {
        // "read" with mail params and no path routes to mail, not file
        let input = serde_json::json!({"action": "read", "mailbox": "INBOX", "limit": 5});
        assert_eq!(OsTool::resolved_resource(&input), "mail");
        let input = serde_json::json!({"action": "read", "account": "sites@stadium.partners"});
        assert_eq!(OsTool::resolved_resource(&input), "mail");
        // "read" with a path is still a file read
        let input = serde_json::json!({"action": "read", "path": "/tmp/x"});
        assert_eq!(OsTool::resolved_resource(&input), "file");
        // Bare "read" stays file (missing-path error is the right correction)
        let input = serde_json::json!({"action": "read"});
        assert_eq!(OsTool::resolved_resource(&input), "file");
        // Explicit resource always wins
        let input = serde_json::json!({"resource": "mail", "action": "read"});
        assert_eq!(OsTool::resolved_resource(&input), "mail");
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
