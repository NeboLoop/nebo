//! The file tools: `read_file`, `edit_file`, `write_file` (always loaded)
//! and the deferred `share_file`, `convert_file`, `checkpoint_files`,
//! `list_checkpoints`, `restore_checkpoint`, `write_plan` and `check_plan`.
//! Each is one job with a flat schema over the file handlers in
//! `file_tool.rs`; the command tools (`command_tools.rs`) share the same
//! [`Machine`], so a file changed through either is one read ledger.

use std::sync::Arc;

use serde_json::{Value, json};
use types::permissions::{CallEffects, Knowable, RuleField};

use crate::file_tool::FileTool;
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
}

impl Machine {
    pub fn new(processes: Arc<ProcessRegistry>, plugins: Option<Arc<napp::plugin::PluginStore>>) -> Self {
        let mut shell = ShellTool::new(processes);
        if let Some(ps) = plugins {
            shell = shell.with_plugin_store(ps);
        }
        Self { file: FileTool::new(), shell }
    }

    /// Each verify command gets this long; a build that needs more belongs in
    /// a background command, not a plan step.
    const PLAN_VERIFY_TIMEOUT_SECS: u64 = 120;
    /// One line of stderr per failing step in the plan document.
    const PLAN_NOTE_CHARS: usize = 160;

    /// Run every step's verify command and rewrite the checkboxes from the
    /// exit codes. The model cannot tick a box; only a passing command can.
    /// A check that verifies nothing new is reported as an error so a
    /// stalled plan never counts as progress.
    async fn check_plan(&self, ctx: &ToolContext, path: &str) -> ToolResult {
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
            // Same policy, same refusals as any command; raw mode returns
            // stdout only on success and an error carrying stderr otherwise.
            let out = self
                .shell
                .execute(
                    ctx,
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
        let write = self.file.write_document(&ctx.session_key, &path, &rewritten);
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
         - A directory, missing file or empty file returns an error instead of content.\n\
         - Don't re-read a file you just edited to check it — edit_file and write_file fail loudly if the change didn't apply."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute path of the file to read." },
                "offset": { "type": "integer", "description": "Line to start from. Only for files too large to read at once." },
                "limit": { "type": "integer", "description": "Number of lines to read. Only for files too large to read at once." }
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
            self.0.file.execute(ctx, call)
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
            self.0.file.execute(ctx, call)
        })
    }
}

// ── share_file ─────────────────────────────────────────────────────

pub struct ShareFileTool(pub Arc<Machine>);

impl DynTool for ShareFileTool {
    fn name(&self) -> &str {
        "share_file"
    }

    fn description(&self) -> String {
        "Shows the owner a file that already exists, as a download card on your reply.\n\
         - Use it for a finished deck, PDF, spreadsheet or any file already on disk.\n\
         - Files you write or convert this turn already show as cards; don't share them again or copy a file to make one."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute path of the file to show." }
            },
            "required": ["path"]
        })
    }

    fn search_hint(&self) -> &str {
        "send the owner a file download card"
    }

    fn rule_field(&self, input: &Value) -> Option<RuleField> {
        path_field(input)
    }

    fn capability(&self, _input: &Value) -> Option<&'static str> {
        Some("file")
    }

    fn activity(&self, input: &Value) -> String {
        format!("sharing {}", file_name(input))
    }

    fn outcome(&self, input: &Value) -> String {
        format!("Shared {}", file_name(input))
    }

    fn emits_image(&self, _input: &Value) -> bool {
        true
    }

    fn execute_dyn<'a>(&'a self, ctx: &'a ToolContext, input: Value) -> Fut<'a> {
        Box::pin(async move {
            let call = json!({"action": "share", "path": input.get("path").cloned().unwrap_or_default()});
            self.0.file.execute(ctx, call)
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
        "Lists the restore points saved in this conversation, newest first, with their ids and files.".to_string()
    }

    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    fn search_hint(&self) -> &str {
        "list saved file restore points"
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

    fn execute_dyn<'a>(&'a self, ctx: &'a ToolContext, _input: Value) -> Fut<'a> {
        Box::pin(async move { self.0.file.execute(ctx, json!({"action": "checkpoints"})) })
    }
}

pub struct RestoreCheckpointTool(pub Arc<Machine>);

impl DynTool for RestoreCheckpointTool {
    fn name(&self) -> &str {
        "restore_checkpoint"
    }

    fn description(&self) -> String {
        "Puts files back as they were at a restore point from checkpoint_files.\n\
         - Leave out `paths` to restore every file the checkpoint saved."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "checkpoint": { "type": "string", "description": "The checkpoint id (cp-…) from checkpoint_files or list_checkpoints." },
                "paths": { "type": "array", "items": { "type": "string" }, "description": "Only these of the checkpoint's files." }
            },
            "required": ["checkpoint"]
        })
    }

    fn search_hint(&self) -> &str {
        "undo file changes from restore point"
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

pub struct CheckPlanTool(pub Arc<Machine>);

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
        Box::pin(async move { self.0.check_plan(ctx, str_arg(&input, "path").unwrap_or("")).await })
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
        let r = CheckPlanTool(machine()).execute_dyn(&ctx(), json!({"path": plan})).await;
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
        let r = CheckPlanTool(machine()).execute_dyn(&ctx(), json!({"path": plan})).await;
        assert!(!r.is_error, "{}", r.content);
        let doc = std::fs::read_to_string(&plan).unwrap();
        assert!(doc.contains("1. ✗ bad, did not run: This git command discards work"), "{doc}");
        assert!(doc.contains("- [x] 2."), "{doc}");
    }

    // A check that verifies nothing is an error, so a stalled plan never
    // counts as progress; one newly verified step is not.
    #[tokio::test]
    async fn check_plan_sets_is_error_when_nothing_is_verified() {
        let dir = tempfile::tempdir().unwrap();
        let plan = write_plan(dir.path(), &[("fails", "false")]);
        let tool = CheckPlanTool(machine());
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
