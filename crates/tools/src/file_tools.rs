//! The file tools: `read_file`, `edit_file`, `write_file`, `share_file`
//! (always loaded) and the deferred `convert_file`, `checkpoint_files`,
//! `list_checkpoints`, `restore_checkpoint`, `write_plan` and `check_plan`.
//! Each is one job with a flat schema over the file handlers in
//! `file_tool.rs`; the command tools (`command_tools.rs`) share the same
//! [`Machine`], so a file changed through either is one read ledger.

use std::sync::Arc;

use serde_json::{Value, json};
use types::permissions::{CallEffects, Knowable, RuleField};

use crate::file_tool::FileTool;
use crate::gate::{GateVerdict, PermissionGate, ResolvedCall};
use crate::origin::ToolContext;
use crate::process::ProcessRegistry;
use crate::registry::{DynTool, ToolResult};
use crate::shell_tool::ShellTool;

/// The file and shell handlers this family runs on, one of each per
/// registry: the read ledger that read-before-edit and outside-edit notes
/// depend on lives in the file handler.
pub struct Machine {
    pub file: FileTool,
    pub shell: ShellTool,
    /// The roster, for the note a web file written outside its app's
    /// served folder gets (`app_dev::misplaced_web_file_note`).
    pub store: Option<Arc<db::Store>>,
}

impl Machine {
    pub fn new(processes: Arc<ProcessRegistry>, plugins: Option<Arc<napp::plugin::PluginStore>>) -> Self {
        let mut shell = ShellTool::new(processes);
        if let Some(ps) = plugins {
            shell = shell.with_plugin_store(ps);
        }
        Self { file: FileTool::new(), shell, store: None }
    }

    pub fn with_store(mut self, store: Option<Arc<db::Store>>) -> Self {
        self.store = store;
        self
    }

    /// A workspace file's earlier versions (`workspace_history`).
    fn file_history(&self, ctx: &ToolContext, path: &str) -> ToolResult {
        let Some(store) = self.store.as_ref() else {
            return ToolResult::error("File history isn't available here.");
        };
        let files = crate::checkpoint::data_dir().join("files");
        let path = match ctx.cwd.as_deref() {
            Some(cwd) if std::path::Path::new(path).is_relative() => std::path::Path::new(cwd).join(path),
            _ => types::pathres::expand(path),
        };
        let Some(rel) = crate::workspace_history::workspace_path(&files, &path.to_string_lossy()) else {
            return ToolResult::error(format!(
                "{} is not in the workspace ({}): only its files keep earlier versions.",
                path.display(),
                files.display()
            ));
        };
        match crate::workspace_history::history(store, &rel) {
            Ok(entries) => ToolResult::ok(crate::workspace_history::describe(&rel, &entries)),
            Err(e) => ToolResult::error(e),
        }
    }

    /// Put a workspace file's earlier version back; the read ledger learns
    /// the file, so the next edit is not warned about a change it made.
    fn restore_file_history(&self, ctx: &ToolContext, id: i64) -> ToolResult {
        let Some(store) = self.store.as_ref() else {
            return ToolResult::error("File history isn't available here.");
        };
        let files = crate::checkpoint::data_dir().join("files");
        let target = match store.get_file_history(id) {
            Ok(Some(e)) => files.join(&e.path),
            Ok(None) => return ToolResult::error(format!("No earlier version {}{id} is kept.", crate::workspace_history::ID_PREFIX)),
            Err(e) => return ToolResult::error(e.to_string()),
        };
        let target = target.to_string_lossy().into_owned();
        if let Some(blocked) = ctx.outside_folders("restore", std::slice::from_ref(&target)) {
            return ToolResult::error(blocked);
        }
        match crate::workspace_history::restore(store, &files, id, None) {
            Ok(restored) => {
                self.file.note_shell_write(ctx.session_key.as_str(), &target);
                ToolResult::ok(crate::workspace_history::describe_restore(&restored))
            }
            Err(e) => ToolResult::error(e),
        }
    }

    /// A written file's result, with the app location note when the file is
    /// a web file for an app but outside the folder the app is served from.
    fn with_app_note(&self, ctx: &ToolContext, input: &Value, result: ToolResult) -> ToolResult {
        let (Some(store), false) = (self.store.as_ref(), result.is_error) else {
            return result;
        };
        let Some(path) = input.get("path").and_then(|p| p.as_str()) else {
            return result;
        };
        let path = match ctx.cwd.as_deref() {
            Some(cwd) if std::path::Path::new(path).is_relative() => std::path::Path::new(cwd).join(path),
            _ => std::path::PathBuf::from(path),
        };
        match crate::app_dev::misplaced_web_file_note(store, &path) {
            Some(note) => ToolResult { content: format!("{}\n{note}", result.content), ..result },
            None => result,
        }
    }

    /// Each verify command gets this long; a build that needs more belongs in
    /// a background command, not a plan step.
    const PLAN_VERIFY_TIMEOUT_SECS: u64 = 120;
    /// One line of stderr per failing step in the plan document.
    const PLAN_NOTE_CHARS: usize = 160;

    /// Run every step's verify command and rewrite the checkboxes from the
    /// exit codes. The model cannot tick a box; only a passing command can.
    /// Each command is a `run_command` call to the permission check first,
    /// as every shell command is (a verify command is still a command): its
    /// rules, the shell limits and the safeguards
    /// see it, and a step whose command the check refuses or parks did not
    /// run. A check that verifies nothing new is reported as an error so a
    /// stalled plan never counts as progress.
    async fn check_plan(self: &Arc<Self>, ctx: &ToolContext, gate: &dyn PermissionGate, path: &str) -> ToolResult {
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
        let run_command = crate::command_tools::RunCommandTool(self.clone());
        for step in &plan.steps {
            let call_input = json!({
                "command": step.verify,
                "description": format!("verify plan step {}: {}", step.n, step.title),
                "cwd": dir,
            });
            let call = ResolvedCall {
                tool: &run_command,
                input: &call_input,
                target: crate::registry::target_of(&run_command, &call_input),
            };
            let reach = match gate.check(ctx, &call).await {
                GateVerdict::Run { reach, .. } => reach,
                GateVerdict::Refuse(out) | GateVerdict::Parked(out) => {
                    let note = crate::plan::first_line(&out.content, Self::PLAN_NOTE_CHARS);
                    results.push(crate::plan::StepResult { n: step.n, ok: false, exit: None, note });
                    continue;
                }
            };
            // Same policy, same refusals as any command; raw mode returns
            // stdout only on success and an error carrying stderr otherwise.
            let out = self
                .shell
                .execute(
                    &ctx.confined(reach),
                    json!({
                        "action": "exec", "command": step.verify,
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
        let write = self.file.write_document(&ctx.session_key, &path, &content, &rewritten);
        if write.is_error {
            return write;
        }
        let verified = results.iter().filter(|r| r.ok).count();
        let mut summary = format!(
            "check_plan {}: {verified} of {} steps pass; {newly} newly passed on this check\n",
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
        result.payload = Some(json!({ "newly_verified": newly, "verified": verified, "steps": results.len() }));
        result
    }
}

/// Generate an office document with the embedded engines (Typst for PDF,
/// pure-Rust OOXML writers for docx/xlsx, SWC for interactive React). The
/// one document-conversion pathway: identical on every platform, never host
/// binaries (wkhtmltopdf is abandoned upstream) and never the bundled
/// browser (no layout engine).
async fn convert(path: &str, to: &str) -> ToolResult {
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
    if let Err(e) = crate::file_tool::ensure_local(&src) {
        return ToolResult::error(e);
    }
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
            ("html", "jsx") => render::jsx_to_html(&source, &file_name, render::JsxLang::Jsx, &Default::default())
                .map(|page| page.code.into_bytes())
                .map_err(|e| e.to_string()),
            ("html", "tsx") => render::jsx_to_html(&source, &file_name, render::JsxLang::Tsx, &Default::default())
                .map(|page| page.code.into_bytes())
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
    // A converted document is a work product — surface it in the Work panel.
    .with_image_url(out_str)
}

fn str_arg<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// The file a call names, as the folder a rule matches.
fn path_field(input: &Value) -> Option<RuleField> {
    str_arg(input, "path").map(|p| RuleField::Folder(crate::file_tool::expand_path(p).into()))
}

/// `file_path` is the name models also use for `path`; this family takes
/// either (param forgiveness inside the tool, never an alias across tools).
fn with_path(mut input: Value) -> Value {
    if str_arg(&input, "path").is_none()
        && let Some(obj) = input.as_object_mut()
        && let Some(p) = obj.remove("file_path")
    {
        obj.insert("path".into(), p);
    }
    input
}

/// The last component of a path, for the owner's labels.
fn file_name(input: &Value) -> String {
    let path = str_arg(input, "path").unwrap_or("");
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// A file path the call names that is an existing directory, where the tool
/// needs a file. Only absolute and `~` paths are checked here; relative ones
/// resolve against the run's folder when the handler runs.
fn names_a_directory(input: &Value) -> Option<String> {
    let path = str_arg(input, "path")?;
    let expanded = crate::file_tool::expand_path(path);
    std::path::Path::new(&expanded)
        .is_absolute()
        .then_some(())
        .filter(|_| std::path::Path::new(&expanded).is_dir())
        .map(|_| path.to_string())
}

type Fut<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>>;

// ── read_file ──────────────────────────────────────────────────────

pub struct ReadFileTool(pub Arc<Machine>);

impl DynTool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> String {
        "Reads a file from this computer.\n\
         - `path` must be absolute.\n\
         - Reads up to 2000 lines by default; for large files read only the part you need.\n\
         - Lines come back numbered from 1.\n\
         - Reads images, PDFs, Word/Excel/PowerPoint files and notebooks.\n\
         - A directory, missing or empty file is an error.\n\
         - Don't re-read a file you just edited: edit_file and write_file fail loudly if it didn't apply."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute path of the file to read." },
                "offset": { "type": "integer", "description": "Line to start from. Only for files too large to read at once." },
                "limit": { "type": "integer", "description": "Number of lines to read, with offset." },
                "question": { "type": "string", "description": "Images: what to look for." }
            },
            "required": ["path"]
        })
    }

    fn search_hint(&self) -> &str {
        "read a file's contents"
    }

    fn should_defer(&self) -> bool {
        false
    }

    fn read_only(&self, _input: &Value) -> bool {
        true
    }

    fn rule_field(&self, input: &Value) -> Option<RuleField> {
        path_field(input)
    }

    fn capability(&self, _input: &Value) -> Option<&'static str> {
        Some("file")
    }

    fn normalize_input(&self, input: Value) -> Value {
        with_path(input)
    }

    fn validate_input(&self, input: &Value) -> Result<(), String> {
        match names_a_directory(input) {
            Some(dir) => Err(format!("{dir} is a directory, not a file. List it with run_command (ls).")),
            None => Ok(()),
        }
    }

    /// A read pages itself: its footer names the offset to go on from, so a
    /// preview must never replace it.
    fn max_result_chars(&self, _input: &Value) -> Option<usize> {
        None
    }

    fn activity(&self, input: &Value) -> String {
        format!("reading {}", file_name(input))
    }

    fn outcome(&self, input: &Value) -> String {
        format!("Read {}", file_name(input))
    }

    /// A file the run pulled in (the attachment root), not one the owner placed.
    fn taint(&self, input: &Value) -> Option<types::provenance::ProvenanceClass> {
        str_arg(input, "path")
            .filter(|p| crate::file_tool::is_ingested_file(p))
            .map(|_| types::provenance::ProvenanceClass::Document)
    }

    fn clearable(&self, _input: &Value) -> bool {
        true
    }

    fn execute_dyn<'a>(&'a self, ctx: &'a ToolContext, input: Value) -> Fut<'a> {
        Box::pin(async move {
            let path = str_arg(&input, "path").unwrap_or("").to_string();
            if path.to_ascii_lowercase().ends_with(".ipynb") {
                return crate::notebook_tool::NotebookTool::new()
                    .execute_dyn(ctx, json!({"action": "read", "notebook_path": path}))
                    .await;
            }
            // A relative path resolves against the run's folder, which the
            // check before the call could not see.
            if let Some(cwd) = ctx.cwd.as_deref()
                && std::path::Path::new(&path).is_relative()
                && std::path::Path::new(cwd).join(&path).is_dir()
            {
                return ToolResult::error(format!(
                    "{path} is a directory, not a file. List it with run_command (ls)."
                ));
            }
            let mut call = json!({"action": "read", "path": path});
            for key in ["offset", "limit"] {
                if let Some(v) = input.get(key) {
                    call[key] = v.clone();
                }
            }
            self.0.file.execute(ctx, call)
        })
    }
}

// ── edit_file ──────────────────────────────────────────────────────

pub struct EditFileTool(pub Arc<Machine>);

impl DynTool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> String {
        "Replaces exact text in a file.\n\
         - Read the file with read_file in this conversation first, or the edit fails.\n\
         - `old_string` must match exactly, including indentation, and be unique — leave out the line-number prefix from read_file.\n\
         - `replace_all: true` replaces every occurrence."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute path of the file to change." },
                "old_string": { "type": "string", "description": "Exact text to replace." },
                "new_string": { "type": "string", "description": "Replacement text; must differ from old_string." },
                "replace_all": { "type": "boolean", "description": "Replace every occurrence of old_string.", "default": false }
            },
            "required": ["path", "old_string", "new_string"]
        })
    }

    fn search_hint(&self) -> &str {
        "replace exact text in a file"
    }

    fn should_defer(&self) -> bool {
        false
    }

    fn rule_field(&self, input: &Value) -> Option<RuleField> {
        path_field(input)
    }

    fn capability(&self, _input: &Value) -> Option<&'static str> {
        Some("file")
    }

    fn effects(&self, input: &Value) -> CallEffects {
        overwrites(input)
    }

    fn normalize_input(&self, input: Value) -> Value {
        with_path(input)
    }

    fn validate_input(&self, input: &Value) -> Result<(), String> {
        if input.get("old_string") == input.get("new_string") {
            return Err("No change: old_string and new_string are the same.".into());
        }
        Ok(())
    }

    fn activity(&self, input: &Value) -> String {
        format!("editing {}", file_name(input))
    }

    fn outcome(&self, input: &Value) -> String {
        format!("Edited {}", file_name(input))
    }

    fn clearable(&self, _input: &Value) -> bool {
        true
    }

    /// An edited work document re-emits its card.
    fn emits_image(&self, _input: &Value) -> bool {
        true
    }

    fn execute_dyn<'a>(&'a self, ctx: &'a ToolContext, input: Value) -> Fut<'a> {
        Box::pin(async move {
            let mut call = input;
            call["action"] = json!("edit");
            let result = self.0.file.execute(ctx, call.clone());
            self.0.with_app_note(ctx, &call, result)
        })
    }
}

/// A write or an edit brings a new file into being, or replaces the one
/// that was there, and publishes nothing. Named `file:<path>`, the way the
/// employee's created ledger keeps it.
fn overwrites(input: &Value) -> CallEffects {
    let mut effects = CallEffects { publishes: Knowable::No, ..CallEffects::default() };
    if let Some(path) = str_arg(input, "path").map(crate::file_tool::expand_path) {
        let named = format!("file:{path}");
        if std::path::Path::new(&path).exists() {
            effects.overwrites.push(named);
        } else {
            effects.creates.push(named);
        }
    }
    effects
}

// ── write_file ─────────────────────────────────────────────────────

pub struct WriteFileTool(pub Arc<Machine>);

impl DynTool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> String {
        "Writes a file, replacing it if it exists.\n\
         Use it to create a new file or fully replace one you've already read. For partial changes use edit_file.\n\
         Office and PDF files can't be written directly — write the source and use convert_file."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute path of the file to write." },
                "content": { "type": "string", "description": "The whole content of the file." }
            },
            "required": ["path", "content"]
        })
    }

    fn search_hint(&self) -> &str {
        "create or overwrite a file"
    }

    fn should_defer(&self) -> bool {
        false
    }

    fn rule_field(&self, input: &Value) -> Option<RuleField> {
        path_field(input)
    }

    fn capability(&self, _input: &Value) -> Option<&'static str> {
        Some("file")
    }

    fn effects(&self, input: &Value) -> CallEffects {
        overwrites(input)
    }

    fn normalize_input(&self, input: Value) -> Value {
        with_path(input)
    }

    fn validate_input(&self, input: &Value) -> Result<(), String> {
        match names_a_directory(input) {
            Some(dir) => Err(format!("{dir} is a directory. Include the file name in path.")),
            None => Ok(()),
        }
    }

    fn activity(&self, input: &Value) -> String {
        format!("writing {}", file_name(input))
    }

    fn outcome(&self, input: &Value) -> String {
        format!("Wrote {}", file_name(input))
    }

    fn clearable(&self, _input: &Value) -> bool {
        true
    }

    /// A written work document surfaces as a card.
    fn emits_image(&self, _input: &Value) -> bool {
        true
    }

    fn execute_dyn<'a>(&'a self, ctx: &'a ToolContext, input: Value) -> Fut<'a> {
        Box::pin(async move {
            let call = json!({
                "action": "write",
                "path": input.get("path").cloned().unwrap_or_default(),
                "content": input.get("content").cloned().unwrap_or_default(),
            });
            let result = self.0.file.execute(ctx, call);
            self.0.with_app_note(ctx, &input, result)
        })
    }
}

// ── share_file ─────────────────────────────────────────────────────

pub struct ShareFileTool(pub Arc<Machine>);

/// The files a `share_file` call names, in order and each once: `paths`,
/// the list the tool documents, else the older `path`, one path or a list.
fn share_paths(input: &Value) -> Vec<&str> {
    let named: Vec<&str> = match input.get("paths").or_else(|| input.get("path")) {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
        Some(Value::String(one)) => vec![one.as_str()],
        _ => Vec::new(),
    };
    let mut paths: Vec<&str> = Vec::new();
    for p in named.into_iter().map(str::trim).filter(|p| !p.is_empty()) {
        if !paths.contains(&p) {
            paths.push(p);
        }
    }
    paths
}

/// One named path, or the paths a string holding a JSON list of strings
/// names (`"[\"/a.jpg\",\"/b.jpg\"]"`).
fn unpack_path_list(item: Value) -> Vec<Value> {
    if let Some(text) = item.as_str().map(str::trim).filter(|t| t.starts_with('['))
        && let Ok(Value::Array(inner)) = serde_json::from_str::<Value>(text)
        && !inner.is_empty()
        && inner.iter().all(Value::is_string)
    {
        return inner;
    }
    vec![item]
}

/// What a share names, for its activity line: the file, or how many.
fn shared_names(input: &Value) -> String {
    match share_paths(input).as_slice() {
        [one] => one.rsplit('/').next().unwrap_or(one).to_string(),
        many => format!("{} files", many.len()),
    }
}

impl DynTool for ShareFileTool {
    fn name(&self) -> &str {
        "share_file"
    }

    fn description(&self) -> String {
        "Shows the owner files that already exist, each as its own download card on your reply.\n\
         - `paths` lists every file to show, in one call: {\"paths\": [\"/a.png\", \"/b.png\"]}. One file is a list of one. \
           Never one call per file.\n\
         - Use it for a finished deck, PDF, spreadsheet or any file already on disk.\n\
         - Files you write or convert this turn already show as cards; don't share them again or copy a file to make one."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Absolute paths of the files to show, all in this one call."
                }
            },
            "required": ["paths"]
        })
    }

    /// `paths` is the one documented way. The older `path` (one path or a
    /// list, and `file_path`, the name models also use) is taken into it,
    /// so old prompts and tests still share; a lone string is a list of one.
    /// A list written as one JSON string inside the list
    /// (`["[\"/a\",\"/b\"]"]`, a guess at the shape) is its paths.
    fn normalize_input(&self, mut input: Value) -> Value {
        let Some(fields) = input.as_object_mut() else {
            return input;
        };
        let mut list: Option<Vec<Value>> = None;
        for key in ["paths", "path", "file_path"] {
            match fields.remove(key) {
                Some(Value::Array(items)) => list.get_or_insert_default().extend(items.into_iter().flat_map(unpack_path_list)),
                Some(one) => list.get_or_insert_default().extend(unpack_path_list(one)),
                None => {}
            }
        }
        if let Some(list) = list {
            fields.insert("paths".into(), Value::Array(list));
        }
        input
    }

    fn search_hint(&self) -> &str {
        "send the owner a file download card"
    }

    /// Core: it is how anything reaches the owner on any device, and a
    /// first call made before its definition was sent guessed the shape.
    fn should_defer(&self) -> bool {
        false
    }

    /// The folder every named file is in: the file itself for one, and the
    /// deepest folder holding them all for several, so a folder rule covers
    /// the call only when it covers every file.
    fn rule_field(&self, input: &Value) -> Option<RuleField> {
        let mut paths = share_paths(input).into_iter().map(|p| std::path::PathBuf::from(crate::file_tool::expand_path(p)));
        let mut common = paths.next()?;
        for p in paths {
            while !p.starts_with(&common) {
                if !common.pop() {
                    break;
                }
            }
        }
        Some(RuleField::Folder(common))
    }

    fn capability(&self, _input: &Value) -> Option<&'static str> {
        Some("file")
    }

    fn validate_input(&self, input: &Value) -> Result<(), String> {
        if share_paths(input).is_empty() {
            return Err("Name the files to share in `paths`, as in {\"paths\": [\"/a.png\", \"/b.png\"]}.".to_string());
        }
        Ok(())
    }

    fn activity(&self, input: &Value) -> String {
        format!("sharing {}", shared_names(input))
    }

    fn outcome(&self, input: &Value) -> String {
        format!("Shared {}", shared_names(input))
    }

    fn emits_image(&self, _input: &Value) -> bool {
        true
    }

    /// Each file is shared on its own: one that can't be shared is named
    /// with its reason, and the others still go out, each its own card.
    fn execute_dyn<'a>(&'a self, ctx: &'a ToolContext, input: Value) -> Fut<'a> {
        Box::pin(async move {
            let paths = share_paths(&input);
            if paths.len() <= 1 {
                let path = paths.first().copied().unwrap_or_default();
                return self.0.file.execute(ctx, json!({"action": "share", "path": path}));
            }
            let mut shared = ToolResult::ok("");
            let mut lines = Vec::new();
            let mut failed = 0;
            for path in &paths {
                let one = self.0.file.execute(ctx, json!({"action": "share", "path": path}));
                match one.image_url {
                    Some(file) if !one.is_error => {
                        lines.push(one.content);
                        shared = shared.with_image_url(file);
                    }
                    _ => {
                        failed += 1;
                        let name = path.rsplit('/').next().unwrap_or(path);
                        lines.push(format!("{name} was not shared: {}", one.content.trim_start_matches("Error: ")));
                    }
                }
            }
            shared.content = lines.join("\n");
            // Nothing went out: the call failed, and says why for each file.
            shared.is_error = failed == paths.len();
            shared
        })
    }
}

// ── convert_file ───────────────────────────────────────────────────

pub struct ConvertFileTool;

impl DynTool for ConvertFileTool {
    fn name(&self) -> &str {
        "convert_file"
    }

    fn description(&self) -> String {
        "Converts a document with the built-in engines; the result lands next to the source and shows as a card.\n\
         - pdf from .md or .typ · docx from .md · xlsx from .csv · html from .jsx or .tsx (an interactive React page).\n\
         - For an interactive page, write the component as a .jsx file and convert it; raw JSX in a .html renders blank.\n\
         - Don't use pandoc, wkhtmltopdf or other host converters; they aren't installed on the owner's computer."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute path of the source file." },
                "to": { "type": "string", "enum": ["pdf", "docx", "xlsx", "html"], "description": "The format to produce." }
            },
            "required": ["path", "to"]
        })
    }

    fn search_hint(&self) -> &str {
        "convert document to pdf docx xlsx html"
    }

    fn rule_field(&self, input: &Value) -> Option<RuleField> {
        path_field(input)
    }

    fn capability(&self, _input: &Value) -> Option<&'static str> {
        Some("file")
    }

    fn activity(&self, input: &Value) -> String {
        format!("converting {}", file_name(input))
    }

    fn outcome(&self, input: &Value) -> String {
        format!("Converted {}", file_name(input))
    }

    fn emits_image(&self, _input: &Value) -> bool {
        true
    }

    fn execute_dyn<'a>(&'a self, ctx: &'a ToolContext, input: Value) -> Fut<'a> {
        Box::pin(async move {
            let path = str_arg(&input, "path").unwrap_or("");
            let path = match ctx.cwd.as_deref() {
                Some(cwd) if std::path::Path::new(path).is_relative() => {
                    std::path::Path::new(cwd).join(path).to_string_lossy().into_owned()
                }
                _ => path.to_string(),
            };
            convert(&path, str_arg(&input, "to").unwrap_or("pdf")).await
        })
    }
}

// ── checkpoints ────────────────────────────────────────────────────

pub struct CheckpointFilesTool(pub Arc<Machine>);

impl DynTool for CheckpointFilesTool {
    fn name(&self) -> &str {
        "checkpoint_files"
    }

    fn description(&self) -> String {
        "Saves a restore point for files you're about to change, so restore_checkpoint can put them back.\n\
         - Use it instead of git stash or reset to be able to undo.\n\
         - Returns the checkpoint id."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "paths": { "type": "array", "items": { "type": "string" }, "description": "Absolute paths of the files you're about to change." },
                "label": { "type": "string", "description": "A short label, e.g. \"before rename\"." }
            },
            "required": ["paths"]
        })
    }

    fn search_hint(&self) -> &str {
        "save restore point before changing files"
    }

    fn capability(&self, _input: &Value) -> Option<&'static str> {
        Some("file")
    }

    fn activity(&self, _input: &Value) -> String {
        "saving a restore point".to_string()
    }

    fn outcome(&self, _input: &Value) -> String {
        "Saved a restore point".to_string()
    }

    fn execute_dyn<'a>(&'a self, ctx: &'a ToolContext, input: Value) -> Fut<'a> {
        Box::pin(async move {
            let mut call = input;
            call["action"] = json!("checkpoint");
            self.0.file.execute(ctx, call)
        })
    }
}

pub struct ListCheckpointsTool(pub Arc<Machine>);

impl DynTool for ListCheckpointsTool {
    fn name(&self) -> &str {
        "list_checkpoints"
    }

    fn description(&self) -> String {
        "Lists the restore points saved in this conversation, newest first, with their ids and files.\n\
         - With `path`, lists that workspace file's earlier versions instead: every file in the workspace keeps \
         what it was before any change or delete (by a tool, a command or a script), with ids like fh-12."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "A workspace file whose earlier versions to list." }
            }
        })
    }

    fn search_hint(&self) -> &str {
        "list restore points or a file's earlier versions"
    }

    fn read_only(&self, _input: &Value) -> bool {
        true
    }

    fn capability(&self, _input: &Value) -> Option<&'static str> {
        Some("file")
    }

    fn activity(&self, _input: &Value) -> String {
        "listing restore points".to_string()
    }

    fn outcome(&self, _input: &Value) -> String {
        "Listed restore points".to_string()
    }

    fn execute_dyn<'a>(&'a self, ctx: &'a ToolContext, input: Value) -> Fut<'a> {
        Box::pin(async move {
            match input.get("path").and_then(Value::as_str).filter(|p| !p.trim().is_empty()) {
                Some(path) => self.0.file_history(ctx, path),
                None => self.0.file.execute(ctx, json!({"action": "checkpoints"})),
            }
        })
    }
}

pub struct RestoreCheckpointTool(pub Arc<Machine>);

impl DynTool for RestoreCheckpointTool {
    fn name(&self) -> &str {
        "restore_checkpoint"
    }

    fn description(&self) -> String {
        "Puts files back as they were at a restore point from checkpoint_files.\n\
         - Leave out `paths` to restore every file the checkpoint saved.\n\
         - An fh-… id from list_checkpoints(path) puts that workspace file's earlier version back; what it is \
         now is kept first."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "checkpoint": { "type": "string", "description": "The checkpoint id (cp-…) from checkpoint_files or list_checkpoints, or a file's earlier version (fh-…) from list_checkpoints(path)." },
                "paths": { "type": "array", "items": { "type": "string" }, "description": "Only these of the checkpoint's files." }
            },
            "required": ["checkpoint"]
        })
    }

    fn search_hint(&self) -> &str {
        "undo changes, restore a file's previous version"
    }

    fn capability(&self, _input: &Value) -> Option<&'static str> {
        Some("file")
    }

    fn activity(&self, _input: &Value) -> String {
        "restoring files".to_string()
    }

    fn outcome(&self, _input: &Value) -> String {
        "Restored files".to_string()
    }

    fn execute_dyn<'a>(&'a self, ctx: &'a ToolContext, input: Value) -> Fut<'a> {
        Box::pin(async move {
            let named = input.get("checkpoint").and_then(Value::as_str).unwrap_or_default();
            if let Some(id) = crate::workspace_history::parse_id(named) {
                return self.0.restore_file_history(ctx, id);
            }
            let mut call = input;
            call["action"] = json!("restore");
            self.0.file.execute(ctx, call)
        })
    }
}

// ── plans ──────────────────────────────────────────────────────────

pub struct WritePlanTool(pub Arc<Machine>);

impl DynTool for WritePlanTool {
    fn name(&self) -> &str {
        "write_plan"
    }

    fn description(&self) -> String {
        "Writes a step plan as a Markdown work document the owner sees, each step with a command that proves it's done.\n\
         - Every step needs `verify`: a shell command that exits 0 only when the step is done.\n\
         - Tick steps only by running check_plan; you can't tick them yourself."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute path of the plan, ending in .md." },
                "title": { "type": "string", "description": "The plan's title." },
                "steps": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": { "type": "string", "description": "What the step achieves." },
                            "verify": { "type": "string", "description": "Shell command that exits 0 only when the step is done." }
                        },
                        "required": ["title", "verify"]
                    },
                    "description": "One entry per step."
                }
            },
            "required": ["path", "steps"]
        })
    }

    fn search_hint(&self) -> &str {
        "write verified step plan document"
    }

    fn rule_field(&self, input: &Value) -> Option<RuleField> {
        path_field(input)
    }

    fn capability(&self, _input: &Value) -> Option<&'static str> {
        Some("file")
    }

    fn effects(&self, input: &Value) -> CallEffects {
        overwrites(input)
    }

    fn activity(&self, input: &Value) -> String {
        format!("writing plan {}", file_name(input))
    }

    fn outcome(&self, input: &Value) -> String {
        format!("Wrote plan {}", file_name(input))
    }

    fn emits_image(&self, _input: &Value) -> bool {
        true
    }

    fn execute_dyn<'a>(&'a self, ctx: &'a ToolContext, input: Value) -> Fut<'a> {
        Box::pin(async move {
            let mut call = input;
            call["action"] = json!("plan");
            self.0.file.execute(ctx, call)
        })
    }
}

/// The way out of Plan mode: the plan written
/// with write_plan goes to the owner on the one ask card; their approval
/// switches the employee out of Plan mode and hands the approved plan back
/// as the answer. The permission check decides when it may be called and
/// that it always asks (`permissions::plan`).
pub struct ExitPlanModeTool {
    store: Arc<db::Store>,
}

impl ExitPlanModeTool {
    pub const NAME: &'static str = "exit_plan_mode";
    /// Most of the plan the card's line carries.
    const CARD_PLAN_CHARS: usize = 2_000;

    pub fn new(store: Arc<db::Store>) -> Self {
        Self { store }
    }
}

impl DynTool for ExitPlanModeTool {
    fn name(&self) -> &str {
        Self::NAME
    }

    fn description(&self) -> String {
        "In Plan mode, once the plan is written with write_plan: sends it to the owner to approve. Approval ends Plan mode and you carry out the plan.\n\
         - Only for work that changes things. For research, looking something up or understanding a question, answer instead.\n\
         - Settle open questions with the owner first; don't ask \"is this plan okay?\" in text: this call is that question.\n\
         - If the owner declines, revise the plan and call it again."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute path of the plan written with write_plan." }
            },
            "required": ["path"]
        })
    }

    fn search_hint(&self) -> &str {
        "leave plan mode owner approves plan"
    }

    /// The owner approves the plan as it stands when they are asked: the
    /// document is read into the call once, so the card, a declined repeat
    /// and the approved answer all carry the same plan.
    fn normalize_input(&self, mut input: Value) -> Value {
        if input.get("plan").is_none()
            && let Some(path) = str_arg(&input, "path")
        {
            let plan = std::fs::read_to_string(path).unwrap_or_default();
            input["plan"] = json!(plan);
        }
        input
    }

    fn activity(&self, input: &Value) -> String {
        let plan = str_arg(input, "plan").unwrap_or("").trim();
        let plan = types::strutil::safe_prefix(plan, Self::CARD_PLAN_CHARS);
        format!("leave plan mode and carry out the plan in {}:\n{plan}", file_name(input))
    }

    fn outcome(&self, input: &Value) -> String {
        format!("The owner approved the plan in {}", file_name(input))
    }

    fn execute_dyn<'a>(&'a self, ctx: &'a ToolContext, input: Value) -> Fut<'a> {
        Box::pin(async move {
            let agent_id = ctx.grant.as_ref().map(|g| g.agent_id.clone()).unwrap_or_default();
            if let Err(e) = leave_plan_mode(&self.store, &agent_id) {
                return ToolResult::error(format!("Plan mode could not be switched off: {e}"));
            }
            let plan = str_arg(&input, "plan").unwrap_or("").trim();
            if plan.is_empty() {
                return ToolResult::ok("The owner said yes: plan mode is off. Go ahead.");
            }
            ToolResult::ok(format!(
                "The owner approved your plan. Plan mode is off: you can now carry it out.\n\n\
                 The plan is saved at {}; tick its steps with check_plan as you go.\n\n## The plan:\n{plan}",
                str_arg(&input, "path").unwrap_or("")
            ))
        })
    }
}

/// Switch employee `agent_id` out of Plan mode: to the company's mode, or
/// Automatic when the company itself plans (leaving Plan mode must never
/// land back in it). An employee with no scope of
/// its own (the main one) is in the company's mode.
fn leave_plan_mode(store: &db::Store, agent_id: &str) -> Result<(), types::NeboError> {
    use types::permissions::{Mode, Scope};
    let company = store.permission_mode(&Scope::Company)?.unwrap_or_default();
    let after = if company == Mode::Plan { Mode::Automatic } else { company };
    if agent_id.is_empty() {
        if company == Mode::Plan {
            store.set_permission_mode(&Scope::Company, after)?;
        }
        return Ok(());
    }
    let scope = Scope::Employee(agent_id.to_string());
    if store.permission_mode(&scope)?.is_some() || company == Mode::Plan {
        store.set_permission_mode(&scope, after)?;
    }
    Ok(())
}

pub struct CheckPlanTool {
    machine: Arc<Machine>,
    /// The registry's permission check: every verify command meets it.
    gate: Arc<dyn PermissionGate>,
}

impl CheckPlanTool {
    pub fn new(machine: Arc<Machine>, gate: Arc<dyn PermissionGate>) -> Self {
        Self { machine, gate }
    }
}

impl DynTool for CheckPlanTool {
    fn name(&self) -> &str {
        "check_plan"
    }

    fn description(&self) -> String {
        "Runs every step's verify command in a plan from write_plan and ticks exactly the steps that pass.\n\
         - The commands run in the plan's folder.\n\
         - A check that verifies nothing new is an error: fix the failing steps before saying the work is done."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute path of the plan written by write_plan." }
            },
            "required": ["path"]
        })
    }

    fn search_hint(&self) -> &str {
        "run plan verify commands tick steps"
    }

    fn rule_field(&self, input: &Value) -> Option<RuleField> {
        path_field(input)
    }

    fn capability(&self, _input: &Value) -> Option<&'static str> {
        Some("file")
    }

    fn activity(&self, input: &Value) -> String {
        format!("checking plan {}", file_name(input))
    }

    fn outcome(&self, input: &Value) -> String {
        format!("Checked plan {}", file_name(input))
    }

    fn emits_image(&self, _input: &Value) -> bool {
        true
    }

    fn execute_dyn<'a>(&'a self, ctx: &'a ToolContext, input: Value) -> Fut<'a> {
        Box::pin(async move {
            self.machine.check_plan(ctx, self.gate.as_ref(), str_arg(&input, "path").unwrap_or("")).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine() -> Arc<Machine> {
        Arc::new(Machine::new(Arc::new(ProcessRegistry::new()), None))
    }

    fn ctx() -> ToolContext {
        ToolContext::new(crate::origin::Origin::User)
    }

    fn write_plan(dir: &std::path::Path, steps: &[(&str, &str)]) -> String {
        let steps: Vec<(String, String)> = steps.iter().map(|(t, v)| (t.to_string(), v.to_string())).collect();
        let doc = crate::plan::render("t", &steps).unwrap();
        let path = dir.join("PLAN.md");
        std::fs::write(&path, doc).unwrap();
        path.to_string_lossy().into_owned()
    }

    /// Write, edit and read go through one ledger: the edit after the write
    /// carries no unread warning, and the read sees the edit.
    #[tokio::test]
    async fn write_edit_and_read_share_one_ledger() {
        let m = machine();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt").to_string_lossy().into_owned();
        let c = ctx();
        let w = WriteFileTool(m.clone()).execute_dyn(&c, json!({"path": path, "content": "alpha\nbeta\n"})).await;
        assert!(!w.is_error && w.content.starts_with("Created"), "{}", w.content);
        let e = EditFileTool(m.clone())
            .execute_dyn(&c, json!({"path": path, "old_string": "beta", "new_string": "gamma"}))
            .await;
        assert!(!e.is_error && !e.content.contains("WARNING"), "{}", e.content);
        let r = ReadFileTool(m).execute_dyn(&c, json!({"path": path})).await;
        assert!(!r.is_error && r.content.contains("gamma"), "{}", r.content);
    }

    /// Several files in one call's `paths`: each is its own card, in order.
    /// The older `path`, one path or a list, still works: it is taken into
    /// `paths` before the schema sees the call. Live 2026-10-03: told "send
    /// both in one go", the employee still shared one file per call.
    #[tokio::test]
    async fn a_share_of_two_files_gives_two_cards() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("cover.png");
        let b = dir.path().join("thumb.txt");
        std::fs::write(&a, b"\x89PNG\r\n\x1a\n0000").unwrap();
        std::fs::write(&b, b"thumbnail notes").unwrap();
        let (a, b) = (a.to_string_lossy().into_owned(), b.to_string_lossy().into_owned());
        let share = ShareFileTool(machine());
        assert_eq!(share.schema()["required"], json!(["paths"]));
        assert!(share.description().contains(r#"{"paths": ["/a.png", "/b.png"]}"#));

        let r = share.execute_dyn(&ctx(), json!({"paths": [a, b]})).await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(r.files().collect::<Vec<_>>(), vec![a.as_str(), b.as_str()]);
        assert!(r.content.contains("cover.png") && r.content.contains("thumb.txt"), "{}", r.content);
        assert_eq!(share.activity(&json!({"paths": [a, b]})), "sharing 2 files");

        // The old shapes: `path` as a list and as one string.
        assert_eq!(share.normalize_input(json!({"path": [a, b]})), json!({"paths": [a, b]}));
        assert_eq!(share.normalize_input(json!({"path": a})), json!({"paths": [a]}));
        assert_eq!(share.normalize_input(json!({"file_path": a})), json!({"paths": [a]}));
        // A first call's guess: the list as one JSON string in the list.
        let packed = serde_json::to_string(&[&a, &b]).unwrap();
        assert_eq!(share.normalize_input(json!({"paths": [packed]})), json!({"paths": [a, b]}));
        assert_eq!(share.normalize_input(json!({"paths": a})), json!({"paths": [a]}));
        let old = share.execute_dyn(&ctx(), json!({"path": [a, b]})).await;
        assert_eq!(old.files().collect::<Vec<_>>(), vec![a.as_str(), b.as_str()]);
        let one = share.execute_dyn(&ctx(), json!({"path": a})).await;
        assert!(!one.is_error, "{}", one.content);
        assert_eq!(one.files().collect::<Vec<_>>(), vec![a.as_str()]);
        assert_eq!(share.outcome(&json!({"path": a})), "Shared cover.png");
        assert!(share.validate_input(&json!({"paths": []})).unwrap_err().contains("`paths`"));
    }

    /// One path that can't be shared is named with its reason, and the
    /// others still go out; when none can, the call fails.
    #[tokio::test]
    async fn one_bad_path_still_shares_the_other() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("report.txt");
        std::fs::write(&good, b"the report").unwrap();
        let good = good.to_string_lossy().into_owned();
        let missing = dir.path().join("missing.png").to_string_lossy().into_owned();
        let share = ShareFileTool(machine());

        let r = share.execute_dyn(&ctx(), json!({"path": [missing, good]})).await;
        assert!(!r.is_error, "the good file went out: {}", r.content);
        assert_eq!(r.files().collect::<Vec<_>>(), vec![good.as_str()]);
        assert!(r.content.contains("missing.png was not shared") && r.content.contains("not found"), "{}", r.content);
        assert!(r.content.contains("Shared report.txt"), "{}", r.content);

        let none = share.execute_dyn(&ctx(), json!({"path": [missing, dir.path().join("gone.pdf")]})).await;
        assert!(none.is_error && none.files().next().is_none(), "{}", none.content);
        assert!(none.content.contains("missing.png") && none.content.contains("gone.pdf"), "{}", none.content);
    }

    /// A folder rule covers a share of several files only where it covers
    /// every one: the call's folder is the one holding them all.
    #[test]
    fn a_share_of_several_files_is_matched_on_the_folder_holding_them_all() {
        let share = ShareFileTool(machine());
        assert_eq!(share.rule_field(&json!({"path": "/w/a/x.png"})), Some(RuleField::Folder("/w/a/x.png".into())));
        assert_eq!(
            share.rule_field(&json!({"path": ["/w/a/x.png", "/w/a/y.png"]})),
            Some(RuleField::Folder("/w/a".into()))
        );
        assert_eq!(
            share.rule_field(&json!({"path": ["/w/a/x.png", "/w/b/c/y.png"]})),
            Some(RuleField::Folder("/w".into()))
        );
    }

    /// The checks each tool makes before it runs: a directory is not a file,
    /// and an edit that changes nothing is refused.
    #[test]
    fn a_directory_path_and_a_no_op_edit_are_refused_before_running() {
        let m = machine();
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_string_lossy().into_owned();
        let err = ReadFileTool(m.clone()).validate_input(&json!({"path": d})).unwrap_err();
        assert!(err.contains("is a directory") && err.contains("run_command"), "{err}");
        let err = WriteFileTool(m.clone()).validate_input(&json!({"path": d, "content": "x"})).unwrap_err();
        assert!(err.contains("Include the file name"), "{err}");
        let err = EditFileTool(m.clone())
            .validate_input(&json!({"path": "/tmp/x", "old_string": "a", "new_string": "a"}))
            .unwrap_err();
        assert_eq!(err, "No change: old_string and new_string are the same.");
        assert!(ReadFileTool(m).validate_input(&json!({"path": "/tmp/nebo-no-such-file"})).is_ok());
    }

    /// `file_path` is taken for `path`; `path` wins when both are given.
    #[test]
    fn file_path_is_taken_for_path() {
        let t = ReadFileTool(machine());
        assert_eq!(t.normalize_input(json!({"file_path": "/a"})), json!({"path": "/a"}));
        assert_eq!(
            t.normalize_input(json!({"path": "/a", "file_path": "/b"})),
            json!({"path": "/a", "file_path": "/b"})
        );
    }

    /// A notebook reads through the notebook handler: its cells, not JSON.
    #[tokio::test]
    async fn a_notebook_reads_as_cells() {
        let dir = tempfile::tempdir().unwrap();
        let nb = dir.path().join("n.ipynb");
        std::fs::write(
            &nb,
            json!({
                "cells": [{"cell_type": "code", "id": "c1", "metadata": {}, "source": "print(1)", "outputs": [], "execution_count": 1}],
                "metadata": {}, "nbformat": 4, "nbformat_minor": 5
            })
            .to_string(),
        )
        .unwrap();
        let r = ReadFileTool(machine()).execute_dyn(&ctx(), json!({"path": nb.to_string_lossy()})).await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("c1") && r.content.contains("print(1)"), "{}", r.content);
    }

    /// What the permission check reads: the folder a file call names, the
    /// file capability, what a write overwrites, and reads that change
    /// nothing.
    #[test]
    fn the_spec_names_the_folder_the_capability_and_the_overwrite() {
        let m = machine();
        let write = WriteFileTool(m.clone());
        let input = json!({"path": "/tmp/x.txt", "content": "y"});
        assert_eq!(write.rule_field(&input), Some(RuleField::Folder("/tmp/x.txt".into())));
        assert_eq!(write.capability(&input), Some("file"));
        // A write to a new path creates it; to an existing one replaces it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.txt").to_string_lossy().into_owned();
        let named = format!("file:{path}");
        let fx = write.effects(&json!({"path": path, "content": "y"}));
        assert_eq!((fx.creates, fx.overwrites), (vec![named.clone()], vec![]));
        std::fs::write(&path, "x").unwrap();
        let fx = write.effects(&json!({"path": path, "content": "y"}));
        assert_eq!((fx.creates, fx.overwrites), (vec![], vec![named]));
        assert!(!write.read_only(&input));
        assert!(ReadFileTool(m.clone()).read_only(&json!({"path": "/tmp/x"})));
        assert!(ListCheckpointsTool(m.clone()).read_only(&json!({})));
        assert_eq!(ReadFileTool(m).max_result_chars(&json!({"path": "/x"})), None, "a read pages itself");
    }

    // The verify commands run in the plan's directory (relative paths in a
    // step mean "next to the plan"), through the shell's raw mode.
    #[tokio::test]
    async fn check_plan_runs_verify_in_the_plans_directory_with_raw_shell() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("marker"), "x").unwrap();
        let plan = write_plan(dir.path(), &[("marker is here", "test -f ./marker"), ("and is not elsewhere", "test -f /nonexistent/marker")]);
        let r = CheckPlanTool::new(machine(), crate::gate::test_gate()).execute_dyn(&ctx(), json!({"path": plan})).await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("1 of 2 steps pass; 1 newly passed"), "{}", r.content);
        let doc = std::fs::read_to_string(&plan).unwrap();
        assert!(doc.contains("- [x] 1."), "{doc}");
        assert!(doc.contains("- [ ] 2."), "{doc}");
        assert!(doc.contains("2. ✗ and is not elsewhere, exit 1"), "{doc}");
    }

    // A destructive verify command is refused like any command: the step
    // stays unticked and reads "did not run" with the refusal's first line.
    #[tokio::test]
    async fn check_plan_refuses_a_destructive_verify_command() {
        let dir = tempfile::tempdir().unwrap();
        let plan = write_plan(dir.path(), &[("bad", "git stash"), ("good", "true")]);
        let r = CheckPlanTool::new(machine(), crate::gate::test_gate()).execute_dyn(&ctx(), json!({"path": plan})).await;
        assert!(!r.is_error, "{}", r.content);
        let doc = std::fs::read_to_string(&plan).unwrap();
        assert!(doc.contains("1. ✗ bad, did not run: This git command discards work"), "{doc}");
        assert!(doc.contains("- [x] 2."), "{doc}");
    }

    /// D11 (review 7.1): every verify command meets the permission check as
    /// a `run_command` call, so a command rule refuses it here too; the
    /// step did not run, and the others do.
    #[tokio::test]
    async fn check_plan_sends_every_verify_command_through_the_permission_check() {
        struct DenyTouch(std::sync::Mutex<Vec<(String, String)>>);
        #[async_trait::async_trait]
        impl PermissionGate for DenyTouch {
            async fn check(&self, _ctx: &ToolContext, call: &ResolvedCall<'_>) -> GateVerdict {
                let command = call.input["command"].as_str().unwrap_or("").to_string();
                self.0.lock().unwrap().push((call.target.key.clone(), command.clone()));
                if command.starts_with("touch") {
                    return GateVerdict::Refuse(ToolResult::error("'run_command' is turned off for `touch`."));
                }
                GateVerdict::Run { why: types::permissions::Why::BasicWork, reach: Default::default() }
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let plan = write_plan(dir.path(), &[("made", "touch ./made"), ("good", "true")]);
        let gate = Arc::new(DenyTouch(Default::default()));
        let r = CheckPlanTool::new(machine(), gate.clone()).execute_dyn(&ctx(), json!({"path": plan})).await;
        assert!(!r.is_error, "{}", r.content);
        assert!(!dir.path().join("made").exists(), "the refused command never ran");
        let doc = std::fs::read_to_string(&plan).unwrap();
        assert!(doc.contains("1. ✗ made, did not run: 'run_command' is turned off"), "{doc}");
        assert!(doc.contains("- [x] 2."), "{doc}");
        let seen = gate.0.lock().unwrap().clone();
        assert_eq!(
            seen,
            [("run_command".to_string(), "touch ./made".to_string()), ("run_command".to_string(), "true".to_string())],
            "each verify command is a run_command call to the check"
        );
    }

    // A check that verifies nothing is an error, so a stalled plan never
    // counts as progress; one newly verified step is not.
    #[tokio::test]
    async fn check_plan_sets_is_error_when_nothing_is_verified() {
        let dir = tempfile::tempdir().unwrap();
        let plan = write_plan(dir.path(), &[("fails", "false")]);
        let tool = CheckPlanTool::new(machine(), crate::gate::test_gate());
        let r = tool.execute_dyn(&ctx(), json!({"path": plan})).await;
        assert!(r.is_error, "{}", r.content);
        assert!(r.content.contains("Nothing verified"), "{}", r.content);
        assert_eq!(r.payload.as_ref().and_then(|p| p.get("newly_verified")).and_then(|v| v.as_u64()), Some(0));
        let sub = dir.path().join("b");
        std::fs::create_dir_all(&sub).unwrap();
        let plan2 = write_plan(&sub, &[("passes", "true")]);
        let r = tool.execute_dyn(&ctx(), json!({"path": plan2})).await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(r.payload.as_ref().and_then(|p| p.get("newly_verified")).and_then(|v| v.as_u64()), Some(1));
    }

    /// A plan written by write_plan is checked by check_plan, and the
    /// result names it.
    #[tokio::test]
    async fn write_plan_names_check_plan() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.md").to_string_lossy().into_owned();
        let r = WritePlanTool(machine())
            .execute_dyn(&ctx(), json!({"path": path, "title": "t", "steps": [{"title": "a", "verify": "true"}]}))
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("check_plan"), "{}", r.content);
    }
}
