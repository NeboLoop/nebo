use crate::errors;
use crate::origin::ToolContext;
use crate::registry::ToolResult;
use crate::walk_bounds::{self, CutShort, WalkBounds};
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// File operations: read, write, edit, share, glob, grep.
pub struct FileTool {
    pub on_file_read: Option<Box<dyn Fn(&str) + Send + Sync>>,
    /// Per-(session, path) record of the file's mtime (ms) at the last successful read,
    /// for the read-before-edit + staleness guard. Keyed by `session_key\u{1f}path`.
    ///
    /// Metadata only — we never cache the content (the content always travels in the
    /// read's tool_result, and re-reads always return fresh content). This keeps the
    /// useful half of a read-state cache while deliberately omitting content
    /// dedup, whose "refer to the earlier read" stub turns into a blank once that earlier
    /// read is evicted by compaction.
    read_state: ReadState,
    /// LSP diagnostics source for the edit-verification chain's step 2
    /// (PRD_CODING_HARNESS P4.4). Production wires the process-global client;
    /// unit tests get `NoServers` by default and inject mocks explicitly —
    /// see `lsp::default_provider`.
    lsp: Arc<dyn crate::lsp::LspProvider>,
}

#[derive(Clone, Debug, Deserialize, Default)]
struct FileInput {
    action: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    offset: i64,
    #[serde(default)]
    limit: i64,
    #[serde(default)]
    content: String,
    #[serde(default)]
    append: bool,
    #[serde(default)]
    old_string: String,
    #[serde(default)]
    new_string: String,
    #[serde(default)]
    replace_all: bool,
    #[serde(default)]
    pattern: String,
    #[serde(default)]
    glob: String,
    /// Prior-redirect only: `dir` belongs to the search resource's schema, but
    /// models reuse it for directory listings. Folded into `path` at dispatch.
    #[serde(default)]
    dir: String,
    #[serde(default)]
    case_insensitive: bool,
    #[serde(default)]
    multiline: bool,
    #[serde(default)]
    output_mode: String,
    #[serde(default)]
    context_before: i64,
    #[serde(default)]
    context_after: i64,
    /// checkpoint: the files about to change. restore: optional subset.
    #[serde(default)]
    paths: Vec<String>,
    /// checkpoint: a short label; plan: unused.
    #[serde(default)]
    label: String,
    /// restore: the checkpoint id.
    #[serde(default)]
    checkpoint: String,
    /// plan: the plan's title.
    #[serde(default)]
    title: String,
    /// plan: [{title, verify}] — every step names a verify command.
    #[serde(default)]
    steps: Vec<serde_json::Value>,
}

/// What an empty glob says to do next.
///
/// The old sentence always offered the parent of the folder just searched,
/// and the model took the offer every time: in the gate's
/// `os-file-discovery-spiral` two hints in a row walked `/home/<bot>/` up to
/// `/home` and then to `/`, and the root walk ran until the harness cancelled
/// the run. The widening offered here never leaves the bot's own area
/// (`walk_bounds::widen_within_own_area`), so `path: "/"` is not a sentence
/// this tool can produce. At the edge of that area the next move is a
/// different tool, not a wider walk: look the name up in the index, grep the
/// contents, or ask the owner where the file lives.
fn nothing_matched(pattern: &str, base_path: &str, cwd: Option<&str>) -> String {
    let recursive = if pattern.starts_with("**/") {
        pattern.to_string()
    } else {
        format!("**/{pattern}")
    };
    let head = format!(
        "No files found matching \"{pattern}\" in {base_path}. This is not an error; nothing here matches."
    );
    let tail = "(Hidden dirs, node_modules, vendor, target, __pycache__ are not searched.)";

    match walk_bounds::widen_within_own_area(Path::new(base_path), cwd) {
        Some(wider) => format!(
            "{head} Next: widen the search one folder with os(resource: \"file\", action: \"glob\", \
             pattern: \"{recursive}\", path: \"{}\"), or search file contents with action: \"grep\". {tail}",
            wider.display()
        ),
        None => format!(
            "{head} {base_path} is the edge of this bot's own area, so there is no wider folder to \
             glob: a walk above it covers the whole machine and is stopped before it finds anything. \
             Next: look the file up by name with os(resource: \"search\", action: \"search\", \
             query: \"<name>\", dir: \"{base_path}\"), or search file contents with \
             os(resource: \"file\", action: \"grep\", pattern: \"<text>\", path: \"{base_path}\"). \
             If the file is not under this bot's area, ask the owner where it lives. {tail}"
        ),
    }
}

impl FileTool {
    pub fn new() -> Self {
        Self {
            on_file_read: None,
            read_state: Arc::new(Mutex::new(HashMap::new())),
            lsp: crate::lsp::default_provider(),
        }
    }

    pub fn name(&self) -> &str {
        "file"
    }

    pub fn execute(&self, ctx: &ToolContext, input: serde_json::Value) -> ToolResult {
        let mut fi: FileInput = match serde_json::from_value(input) {
            Ok(v) => v,
            Err(e) => return ToolResult::error(format!("invalid input: {}", e)),
        };

        // Prior-redirect: models name the listing target `dir` (ls prior).
        // `path` is the one canonical field; fold, don't reject.
        if fi.path.is_empty() && !fi.dir.is_empty() {
            fi.path = std::mem::take(&mut fi.dir);
        }
        // An isolated sub-agent's relative paths belong to its worktree, not
        // to the process cwd (which is the owner's tree).
        if let Some(cwd) = ctx.cwd.as_deref() {
            if !fi.path.is_empty() && Path::new(&fi.path).is_relative() {
                fi.path = Path::new(cwd).join(&fi.path).to_string_lossy().into_owned();
            }
            fi.paths = fi
                .paths
                .iter()
                .map(|p| {
                    if Path::new(p).is_relative() {
                        Path::new(cwd).join(p).to_string_lossy().into_owned()
                    } else {
                        p.clone()
                    }
                })
                .collect();
        }

        let session = ctx.session_key.as_str();
        match fi.action.as_str() {
            "read" => self.handle_read(ctx, &fi),
            "write" => self.handle_write(session, &fi),
            // append IS write with append: true (live 2026-09-05: the call
            // carried path and content and was told the action was unknown).
            "append" => {
                let mut appending = fi.clone();
                appending.append = true;
                self.handle_write(session, &appending)
            }
            "edit" => self.handle_edit(session, &fi),
            // Checkpoints without destructive git (P6.2): explicit, listed, reversible.
            "checkpoint" => self.handle_checkpoint(ctx, &fi),
            "checkpoints" => match crate::checkpoint::list(&ctx.session_id) {
                Ok(list) => ToolResult::ok(crate::checkpoint::render_list(&list)),
                Err(e) => ToolResult::error(e),
            },
            "restore" => self.handle_restore(ctx, &fi),
            // Plan artifacts (P6.1): a plan is a work document written through
            // the ONE write pathway, so it lands in the Work panel and is
            // versioned like any other. plan_check lives on OsTool (it needs
            // the shell).
            "plan" => self.handle_plan(session, &fi),
            "glob" => self.handle_glob(ctx, &fi),
            "grep" => self.handle_grep(&fi),
            // Hand an EXISTING file to the user as a download card. Synonyms the
            // model reaches for map to the one implementation.
            "share" | "present" | "send" => self.handle_share(&fi),
            // Prior-redirect ("ls ~/Desktop"): a directory listing IS glob with
            // its defaulted "*" pattern — route to the one implementation. Not
            // advertised in the schema; glob stays the single documented way.
            "list" | "ls" => self.handle_glob(ctx, &fi),
            "screenshot" | "capture" => ToolResult::error(format!(
                "screenshot is not a file action. To take one: os(resource: \"desktop\", action: \"screenshot\"). \
                 To find existing screenshots: os(resource: \"file\", action: \"glob\", \
                 pattern: \"*.png\", path: \"{}\").",
                if fi.path.is_empty() { "~/Desktop" } else { fi.path.as_str() }
            )),
            other => ToolResult::error(format!(
                "Unknown action: {} (valid: read, write, edit, share, glob, grep, checkpoint, checkpoints, restore, plan, plan_check)",
                other
            )),
        }
    }

    fn handle_checkpoint(&self, ctx: &ToolContext, fi: &FileInput) -> ToolResult {
        let mut paths = fi.paths.clone();
        if paths.is_empty() && !fi.path.is_empty() {
            paths.push(fi.path.clone());
        }
        if let Some(blocked) = ctx.outside_folders("checkpoint", &paths) {
            return ToolResult::error(blocked);
        }
        match crate::checkpoint::create(&ctx.session_id, &fi.label, &paths) {
            Ok(cp) => ToolResult::ok(crate::checkpoint::render_created(&cp)),
            Err(e) => ToolResult::error(e),
        }
    }

    /// Restore is fenced by the checkpoint's OWN paths (the input may name
    /// none), and it refreshes the read ledger for every file it wrote, so
    /// the next edit is not warned about a change the agent itself made.
    fn handle_restore(&self, ctx: &ToolContext, fi: &FileInput) -> ToolResult {
        let cp = match crate::checkpoint::get(&ctx.session_id, &fi.checkpoint) {
            Ok(cp) => cp,
            Err(e) => return ToolResult::error(e),
        };
        let targets = if fi.paths.is_empty() { crate::checkpoint::paths(&cp) } else { fi.paths.clone() };
        if let Some(blocked) = ctx.outside_folders("restore", &targets) {
            return ToolResult::error(blocked);
        }
        match crate::checkpoint::restore(&ctx.session_id, &fi.checkpoint, &fi.paths) {
            Ok(report) => {
                for (path, action) in &report.actions {
                    if *action == crate::checkpoint::RestoreAction::Restored {
                        self.record_read(ctx.session_key.as_str(), path);
                        crate::diagnostics_feed::clear_delivered(path);
                    }
                }
                ToolResult::ok(crate::checkpoint::render_restore(&report))
            }
            Err(e) => ToolResult::error(e),
        }
    }

    /// The agent wrote `path` itself through the shell (`tee`, `>`): refresh
    /// the read ledger so the next edit is not warned about a change it made.
    pub fn note_shell_write(&self, session: &str, path: &str) {
        self.record_read(session, path);
        crate::diagnostics_feed::clear_delivered(path);
    }

    /// Write a document through the ONE write pathway (Work panel, versions,
    /// overwrite advisory). Used by `plan_check` on OsTool, which rewrites the
    /// plan it just verified.
    pub fn write_document(&self, session: &str, path: &str, content: &str) -> ToolResult {
        let write = FileInput {
            action: "write".into(),
            path: path.to_string(),
            content: content.to_string(),
            ..Default::default()
        };
        self.handle_write(session, &write)
    }

    fn handle_plan(&self, session: &str, fi: &FileInput) -> ToolResult {
        if fi.path.is_empty() {
            return ToolResult::error(
                "plan needs `path`: where to write the plan (a .md file the owner will see in Work)",
            );
        }
        if !fi.path.ends_with(".md") {
            return ToolResult::error("plan `path` must end in .md — a plan is a markdown work document");
        }
        let steps: Vec<(String, String)> = fi
            .steps
            .iter()
            .map(|s| {
                (
                    s.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    s.get("verify").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                )
            })
            .collect();
        let content = match crate::plan::render(&fi.title, &steps) {
            Ok(c) => c,
            Err(e) => return ToolResult::error(e),
        };
        let write = FileInput {
            action: "write".into(),
            path: fi.path.clone(),
            content,
            ..Default::default()
        };
        let mut result = self.handle_write(session, &write);
        if !result.is_error {
            result.content = format!(
                "plan written to {} ({} step{}). Verify with os(resource: \"file\", action: \"plan_check\", path: \"{}\") — only a passing verify command ticks a step.\n{}",
                fi.path,
                steps.len(),
                if steps.len() == 1 { "" } else { "s" },
                fi.path,
                result.content
            );
        }
        result
    }

    fn read_state_key(session: &str, path: &str) -> String {
        format!("{session}\u{1f}{path}")
    }

    /// The session-keyed snapshot ledger, shared with whoever sweeps it for
    /// outside edits between iterations (`external_edit_notes`).
    pub fn read_state(&self) -> ReadState {
        self.read_state.clone()
    }

    /// Snapshot `path` as this session now sees it (after a read, our own
    /// write, a restore, or a noted shell write): mtime, and the content when
    /// it is small enough to diff later. A file we cannot stat is not tracked.
    fn record_read(&self, session: &str, path: &str) {
        let Some(entry) = snapshot(path) else { return };
        if let Ok(mut m) = self.read_state.lock() {
            m.insert(Self::read_state_key(session, path), entry);
        }
    }

    /// Overwrite advisory: a warning to attach to the result when the target
    /// exists and was not read this session, or changed on disk since that read.
    ///
    /// A WARNING, never a block. This used to hard-error, and a blocked model
    /// does not give up on the write — it wants the file to exist, so it routes
    /// around the guard through the shell heredoc, where it gets no staleness
    /// protection at all. The block converted a supervised write into an
    /// unsupervised one. A warning rides on the write the model was going to
    /// make anyway, and names what it may have just clobbered so it can go look.
    fn overwrite_warning(&self, session: &str, path: &str, verb: &str) -> Option<String> {
        let guard = self.read_state.lock().ok()?; // poisoned lock: no advisory
        match guard.get(&Self::read_state_key(session, path)) {
            None => {
                // Captured before the write lands: the size and mtime of what is
                // about to be replaced are the only evidence of it.
                let meta = std::fs::metadata(path).ok();
                let old_size = meta
                    .as_ref()
                    .map(|m| format!("{} bytes", m.len()))
                    .unwrap_or_else(|| "size unknown".to_string());
                let old_mtime = current_mtime_ms(path)
                    .map(fmt_ms_rfc3339)
                    .unwrap_or_else(|| "mtime unknown".to_string());
                let effect = if verb == "edit" {
                    "Only the matched text was replaced; re-read to see the rest of the file."
                } else {
                    "Its previous content is gone; re-read if it mattered."
                };
                Some(format!(
                    "Warning: {path} already existed ({old_size}, modified {old_mtime}) and this \
                     tool has no record of reading it this session (shell reads are not \
                     recorded). {effect}"
                ))
            }
            Some(entry) => {
                // Warn only when the file is demonstrably newer than the recorded
                // read. If we can't stat it, stay quiet — don't alarm on our own
                // bookkeeping.
                if let Some(cur) = current_mtime_ms(path).filter(|cur| *cur > entry.mtime_ms) {
                    // An edit replaces one string; the outside change is still on
                    // disk. Only a write replaced the whole file. Saying "overwrote"
                    // for an edit sent models re-applying changes that were there.
                    let effect = if verb == "edit" {
                        "Your edit replaced only the matched text; the other change is still there"
                    } else {
                        "Your write replaced the whole file, including that change"
                    };
                    Some(format!(
                        "Warning: {path} changed on disk (modified {} ms after your read). {effect}. Re-read before editing further.",
                        cur - entry.mtime_ms
                    ))
                } else {
                    None
                }
            }
        }
    }

    fn handle_read(&self, ctx: &ToolContext, input: &FileInput) -> ToolResult {
        let session = ctx.session_key.as_str();
        if input.path.is_empty() {
            return ToolResult::error(errors::missing_param("read", "path", "os(resource: \"file\", action: \"read\", path: \"/tmp/file.txt\")"));
        }

        // Resolve the user-supplied path through the canonical resolver:
        // tilde expansion plus a Unicode-whitespace-tolerant fallback. This
        // is the seam that handles macOS Screenshot filenames containing
        // U+202F (narrow no-break space) — the LLM types a regular space
        // and the literal lookup fails. See `types::pathres` for the
        // safety contract (exact-or-one-fuzzy-or-error).
        let path = match types::pathres::resolve(&input.path) {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(e) => return ToolResult::error(format!("Error: {}", e)),
        };

        if let Err(e) = validate_file_path(&path, "read") {
            return ToolResult::error(format!("Error: {}", e));
        }

        let offset = if input.offset <= 0 { 1 } else { input.offset } as usize;
        let limit = if input.limit <= 0 { 2000 } else { input.limit } as usize;

        let metadata = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return ToolResult::error(errors::file_not_found(&path));
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return ToolResult::error(errors::permission_denied(&path, "read"));
            }
            Err(e) => {
                return ToolResult::error(format!("Error accessing file: {}", e));
            }
        };

        if metadata.is_dir() {
            // A read of a directory is a listing; the model asked for what is
            // there, and glob with the defaulted "*" is the one implementation.
            return self.handle_glob(ctx, input);
        }

        // NOTE: A read MUST always return the file's content. We deliberately do NOT
        // suppress repeat reads with a "contents already in your context" placeholder —
        // that assumption is unverifiable (false across compaction/eviction) and the old
        // path-keyed, process-global cache gaslit the model into a retry spiral when a
        // prior read had returned nothing useful. See the #research loop incident.
        let mut file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(e) => return ToolResult::error(format!("Error opening file: {}", e)),
        };

        // Check for binary content before attempting line-based reading
        let mut utf16_text: Option<String> = None;
        {
            // Read a fixed window via a real syscall — never size this from
            // metadata.len(), which can be 0 for dataless/placeholder files (e.g.
            // iCloud "optimize storage") even when the file has content.
            let mut sample = [0u8; 8192];
            if let Ok(n) = file.read(&mut sample) {
                if n > 0 && let Some(big_endian) = detect_utf16(&sample[..n]) {
                    // UTF-16 text looks binary to the NUL scan below (every other
                    // byte is 0x00) — decode it instead of refusing to show it.
                    match std::fs::read(&path) {
                        Ok(bytes) => utf16_text = Some(decode_utf16_bytes(&bytes, big_endian)),
                        Err(e) => {
                            return ToolResult::error(format!("Error reading file: {}", e));
                        }
                    }
                } else if n > 0
                    && let Some(reason) = binary_reason(&sample[..n], n)
                {
                    // Images: return them INLINE as a viewable image (data URL) so the model
                    // actually sees the pixels. The runner renders
                    // image_url inline for multimodal providers and routes it through the vision
                    // sidecar otherwise. One canonical "read an image" path; never make the model
                    // guess the contents.
                    let mime = match std::path::Path::new(&path)
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e.to_ascii_lowercase())
                        .as_deref()
                    {
                        Some("png") => Some("image/png"),
                        Some("jpg") | Some("jpeg") => Some("image/jpeg"),
                        Some("gif") => Some("image/gif"),
                        Some("webp") => Some("image/webp"),
                        Some("bmp") => Some("image/bmp"),
                        Some("heic") => Some("image/heic"),
                        Some("tiff") | Some("tif") => Some("image/tiff"),
                        _ => None,
                    };
                    if let Some(mime) = mime {
                        use base64::Engine;
                        return match std::fs::read(&path) {
                            Ok(bytes) => {
                                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                                ToolResult::ok(format!("[image: {}]", path))
                                    .with_image_url(format!("data:{};base64,{}", mime, b64))
                            }
                            Err(e) => {
                                ToolResult::error(format!("Error reading image {}: {}", path, e))
                            }
                        };
                    }

                    // PDFs are the one binary container people actually expect a read
                    // to work on. Extract the text rather than refusing the file.
                    if sample.starts_with(b"%PDF-") {
                        return read_pdf_text(&path);
                    }

                    // Office and e-book containers: Word, PowerPoint, Excel,
                    // OpenDocument, RTF, EPUB. Refusing these is the wrong answer
                    // when the nebo-office plugin can turn them into Markdown.
                    if let Some(result) = read_office_document(&path) {
                        return result;
                    }

                    // metadata.len() is 0 for dataless/placeholder files even when
                    // a read returns bytes, so it is only quoted when non-zero.
                    let size = if metadata.len() > 0 {
                        format!("{} bytes", metadata.len())
                    } else {
                        format!("size unknown (metadata reports 0 bytes; {} bytes were read)", n)
                    };
                    return ToolResult::ok(format!(
                        "[Binary file detected: content not shown. {}; {}. To inspect the raw bytes: os(resource: \"shell\", action: \"exec\", command: \"hexdump -C '{}' | head\")]",
                        size,
                        reason,
                        path
                    ));
                }
            }
            // Seek back to start for the line-based reader
            if let Err(e) = std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(0)) {
                return ToolResult::error(format!("Error seeking file: {}", e));
            }
        }

        // UTF-16 files read from the decoded text; everything else streams from disk.
        let is_utf16 = utf16_text.is_some();
        let reader: Box<dyn BufRead> = match utf16_text {
            Some(text) => Box::new(std::io::Cursor::new(text)),
            None => Box::new(BufReader::with_capacity(1024 * 1024, file)),
        };
        // Read budget for the whole rendered result: ~25,000 tokens, the same
        // shape as Claude Code's Read. A read that overruns it is an ERROR
        // naming offset/limit, never a clipped result: the error costs ~100
        // bytes, a clipped result costs the whole budget and still leaves the
        // model without the part it wanted (Claude Code measured exactly this
        // and reverted truncation, Mar 2026).
        const FILE_READ_MAX_BYTES: usize = 100_000;
        let mut result = String::new();
        let mut line_num = 0usize;
        let mut lines_read = 0usize;
        let mut limit_truncated = false;
        let mut over_budget = false;
        // Spill artifacts (large tool results persisted under <session>/tool-results/)
        // already CONTAIN the line numbers from the read that produced them —
        // numbering them again renders "1  1  #!/usr/bin/env" and the model
        // reads it as "the file is being duplicated" (observed feeding a
        // distrust spiral in the 2026-08 replay). Our own spill files are the
        // ONE case a read renders raw.
        let numbering = std::path::Path::new(&path)
            .parent()
            .and_then(|d| d.file_name())
            .map(|n| n != "tool-results")
            .unwrap_or(true);

        let mut range_note: Option<String> = None;
        // Total line count once known (the limit path counts the remainder).
        let mut total_lines: Option<usize> = None;
        let mut lines_iter = reader.lines();
        while let Some(line_result) = lines_iter.next() {
            let line = match line_result {
                Ok(l) => l,
                Err(e) => return ToolResult::error(format!("Error reading file: {}", e)),
            };

            line_num += 1;

            if line_num < offset {
                continue;
            }

            if lines_read >= limit {
                limit_truncated = true;
                // Count the rest so the note states the real total, not a floor.
                let total = line_num + (&mut lines_iter).count();
                total_lines = Some(total);
                range_note = Some(format!(
                    "\n... (showing lines {}-{} of {}; continue with offset: {})",
                    offset,
                    line_num - 1,
                    total,
                    line_num
                ));
                break;
            }

            // A line is always shown whole (a pasted document is one long
            // line, newlines lost in the paste; clipping it mid-table left no
            // way to read the rest — Underwriter, 2026-09-09).
            if numbering {
                result.push_str(&format!("{:6}\t{}\n", line_num, line));
            } else {
                result.push_str(&line);
                result.push('\n');
            }
            lines_read += 1;

            if result.len() > FILE_READ_MAX_BYTES {
                over_budget = true;
                // Count the rest so the error states the real total.
                total_lines = Some(line_num + (&mut lines_iter).count());
                break;
            }
        }

        // `<system-reminder>` is NOT available here: it is the message-stream
        // channel (ephemeral, user-role, `steering::wrap_system_reminder`, never
        // persisted — CHAT_SYSTEM §4.2). A tool result is tool-role and IS
        // persisted, so it uses the tool-result vocabulary instead.
        //
        // What matters is that an absent result is never mistakable for the
        // file's contents, and never claims something false. The 2026-08-28
        // outage was a stub (`[os] 0 lines`) the model read as the answer.
        if result.is_empty() {
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            result = if offset > 1 {
                format!(
                    "(file has {} lines; offset {} is past the end)",
                    line_num, offset
                )
            } else if size > 0 {
                // Bytes on disk but nothing read back. Saying "empty" would be a
                // false claim the model has no way to doubt; this is a failed
                // read and is returned as one.
                return ToolResult::error(format!(
                    "(no content returned — this file is {} bytes on disk, so it is NOT empty. \
                     This is a read failure, not the file's contents. Try os(resource: \"file\", \
                     action: \"grep\", path: \"{}\", pattern: \".\") instead of repeating this read.)",
                    size, path
                ));
            } else {
                "(file is empty)".to_string()
            };
        }

        if let Some(note) = range_note {
            result.push_str(&note);
        }

        // Outline-first reads (PRD P4.1): a BLIND read (no offset/limit given)
        // that does not cover the whole file gets the file's tree-sitter
        // outline prepended, so the next read can target a symbol's line range
        // instead of paging forward blindly. Decoration of the ONE read
        // pathway — never a second one. Ranged reads and files with no
        // compiled-in grammar are untouched (absence, not noise).
        let outline = if (limit_truncated || over_budget)
            && input.offset <= 0
            && input.limit <= 0
            && !is_utf16
            && let Some(lang) = syntax::Lang::from_path(Path::new(&path))
            && let Ok(full) = std::fs::read_to_string(&path)
            && let Ok(symbols) = syntax::outline(&full, lang)
            && !symbols.is_empty()
        {
            format!(
                "[Outline ({}, {} lines total) — this read does not cover the whole file; request specific sections with offset/limit using these [start-end] line ranges.]\n{}\n\n",
                lang.name(),
                full.lines().count(),
                crate::code_tool::render_outline(&symbols, 200),
            )
        } else {
            String::new()
        };

        if over_budget {
            return ToolResult::error(format!(
                "{}File content ({} bytes rendered from line {}; the file has {} lines) exceeds the read budget of {} bytes (about 25,000 tokens). Use offset and limit to read a portion of the file, or grep for the content you need instead of reading the whole file.",
                outline,
                result.len(),
                offset,
                total_lines.unwrap_or(line_num),
                FILE_READ_MAX_BYTES
            ));
        }
        result = format!("{}{}", outline, result);

        if let Some(ref callback) = self.on_file_read {
            callback(&path);
        }

        // Record the read so a later edit/write of this path can require a prior
        // read, and so an outside change since now can be surfaced as a diff.
        self.record_read(session, &path);

        ToolResult::ok(result)
    }

    fn handle_write(&self, session: &str, input: &FileInput) -> ToolResult {
        if input.path.is_empty() {
            return ToolResult::error(errors::missing_param("write", "path", "os(resource: \"file\", action: \"write\", path: \"/tmp/file.txt\", content: \"hello\")"));
        }
        // Reject empty content (catches wrong field name like 'text' instead of 'content').
        // Append to existing file with empty content is allowed (no-op but not an error).
        if input.content.is_empty() && !input.append {
            return ToolResult::error(
                "Error: content is required for write. Use the 'content' field (not 'text' or 'data'). Example: os(resource: \"file\", action: \"write\", path: \"/tmp/f.txt\", content: \"hello\")",
            );
        }

        if let Err(e) = validate_file_path(&input.path, "write") {
            return ToolResult::error(format!("Error: {}", e));
        }

        // Office formats and PDF are binary containers — text written under these
        // extensions is ALWAYS a corrupt fake (an invented-XML .pptx shipped to a
        // user once). Redirect to the one real pathway per format.
        let ext_lower = Path::new(&input.path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        let redirect = match ext_lower.as_str() {
            "docx" | "doc" => Some("write the content as Markdown (.md), then os(resource: \"file\", action: \"convert\", path: ..., to: \"docx\")"),
            "xlsx" | "xls" => Some("write the data as CSV (.csv), then os(resource: \"file\", action: \"convert\", path: ..., to: \"xlsx\")"),
            "pdf" => Some("write the content as Markdown (.md), then os(resource: \"file\", action: \"convert\", path: ..., to: \"pdf\")"),
            "pptx" | "ppt" => Some("use the pptx skill: write a JSON spec, then run the nebo-office binary (`nebo-office pptx create spec.json -o out.pptx`)"),
            _ => None,
        };
        if let Some(how) = redirect {
            return ToolResult::error(format!(
                "Error: .{ext_lower} is a binary format — writing text to it produces a corrupt file that won't open. Instead, {how}."
            ));
        }

        let path = expand_path(&input.path);

        // Overwrite advisory, captured BEFORE the write clobbers the evidence.
        // Creating a new file, or appending, needs no advisory.
        let prior_len = if !input.append {
            std::fs::metadata(&path).ok().map(|m| m.len())
        } else {
            None
        };
        let overwrite_note = if !input.append && prior_len.is_some() {
            self.overwrite_warning(session, &path, "write")
        } else {
            None
        };

        // Create parent directories
        if let Some(parent) = Path::new(&path).parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                // EACCES: the chosen location isn't writable by Nebo (common when a
                // run picks a path outside the app's working dir). Hard-fail, but
                // tell the model exactly where it CAN write so it can retry itself.
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    let suggested = config::data_dir()
                        .map(|d| d.join("files").to_string_lossy().into_owned())
                        .unwrap_or_else(|_| "the app data directory".to_string());
                    return ToolResult::error(format!(
                        "Permission denied creating directory {} (os error 13). Nebo can't write \
                         there. Write under the app's working directory instead — \
                         {}/<name>.<ext> — it needs no permissions and uploads automatically. \
                         (You asked to write {}.)",
                        parent.display(),
                        suggested,
                        input.path
                    ));
                }
                return ToolResult::error(format!("Error creating directories: {}", e));
            }
        }

        let result = if input.append {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut f| f.write_all(input.content.as_bytes()))
        } else {
            std::fs::write(&path, &input.content).map(|_| ())
        };

        match result {
            Ok(()) => {
                // Refresh read-state to the file we just wrote, so a subsequent edit/write
                // in this session isn't wrongly flagged stale against our own write.
                self.record_read(session, &path);
                crate::diagnostics_feed::clear_delivered(&path);
                let mut msg = if input.append {
                    format!("Appended {} bytes to {}", input.content.len(), path)
                } else if let Some(old) = prior_len {
                    format!(
                        "Overwrote {} ({} to {} bytes)",
                        path,
                        old,
                        input.content.len()
                    )
                } else {
                    format!("Created {} ({} bytes)", path, input.content.len())
                };
                // Edit-verification chain step 1 (PRD P4.4): parse what is now
                // on disk. A plain write's file content IS the input content;
                // an append must be checked as the whole file, so read it back
                // (and stay silent if that read fails — no false claims).
                let appended_full = if input.append {
                    std::fs::read_to_string(&path).ok()
                } else {
                    None
                };
                let parse_src = match &appended_full {
                    Some(full) => Some(full.as_str()),
                    None if input.append => None,
                    None => Some(input.content.as_str()),
                };
                if let Some(src) = parse_src
                    && let Some(note) = syntax_note(&path, src, self.lsp.as_ref())
                {
                    msg.push('\n');
                    msg.push_str(&note);
                }
                if let Some(note) = overwrite_note {
                    msg.push_str("\n\n");
                    msg.push_str(&note);
                }
                // Raw JSX in a .html with no transpiler renders blank in every browser.
                // Redirect to the canonical pathway: write a .jsx, then convert to html
                // (Nebo's SWC engine produces a self-contained, renderable page).
                if html_has_untranspiled_jsx(&path, &input.content) {
                    msg.push_str(
                        "\n\nWARNING: this .html contains raw JSX (e.g. className=, <Component/>) \
                         with no transpiler, so it renders BLANK in a browser. To build an \
                         interactive React artifact, write the component as a .jsx file, then \
                         os(resource: \"file\", action: \"convert\", path: \"<file>.jsx\", to: \"html\"). \
                         Never put JSX or CDN-loaded React directly in a .html.",
                    );
                }
                let result = ToolResult::ok(msg);
                // Surface user-facing documents (reports/sheets/designs) as "Work"
                // artifacts so they're clickable + viewable in the Work panel. Scratch/
                // code/config writes are NOT artifacts — gate on a work-product extension.
                if is_work_document(&path) {
                    result.with_image_url(path)
                } else {
                    result
                }
            }
            Err(e) => ToolResult::error(format!("Error writing file: {}", e)),
        }
    }

    fn handle_edit(&self, session: &str, input: &FileInput) -> ToolResult {
        if input.path.is_empty() {
            return ToolResult::error(errors::missing_param("edit", "path", "os(resource: \"file\", action: \"edit\", path: \"/tmp/file.txt\", old_string: \"old\", new_string: \"new\")"));
        }
        if input.old_string.is_empty() {
            return ToolResult::error(errors::missing_param("edit", "old_string", "os(resource: \"file\", action: \"edit\", path: \"/tmp/file.txt\", old_string: \"text to find\", new_string: \"replacement\")"));
        }
        if input.old_string == input.new_string {
            return ToolResult::error("Error: old_string and new_string are identical. The edit would produce no change.");
        }

        // Same fuzzy fallback as read: edit requires the file to exist.
        let path = match types::pathres::resolve(&input.path) {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(e) => return ToolResult::error(format!("Error: {}", e)),
        };

        if let Err(e) = validate_file_path(&path, "edit") {
            return ToolResult::error(format!("Error: {}", e));
        }

        // Overwrite advisory (warn, never block — see overwrite_warning). Edit is
        // additionally self-guarding: old_string must match the CURRENT on-disk
        // content read below, so a surgical edit cannot land on text the model
        // has never seen the way a whole-file write can.
        let overwrite_note = self.overwrite_warning(session, &path, "edit");

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return ToolResult::error(errors::file_not_found(&path));
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return ToolResult::error(errors::permission_denied(&path, "edit"));
            }
            Err(e) => return ToolResult::error(format!("Error reading file: {}", e)),
        };

        // Exact match first; otherwise curly quotes read as straight ones (the
        // model types ' and " where the document has ’ and “), and the file's
        // own bytes for that span become the string to replace.
        let Some(old_string) = find_actual_string(&content, &input.old_string) else {
            return ToolResult::error(format!(
                "Error: old_string not found in file.\n\nSearched for:\n```\n{}\n```\n\nMake sure the string matches exactly, including whitespace and indentation.",
                input.old_string
            ));
        };
        let new_string = preserve_quote_style(&input.old_string, &old_string, &input.new_string);

        let count = content.matches(&old_string).count();
        if count > 1 && !input.replace_all {
            return ToolResult::error(format!(
                "Error: old_string appears {} times in file. Use replace_all=true to replace all, or make the search string more specific.",
                count
            ));
        }

        let new_content = if input.replace_all {
            content.replace(&old_string, &new_string)
        } else {
            content.replacen(&old_string, &new_string, 1)
        };

        if let Err(e) = std::fs::write(&path, &new_content) {
            return ToolResult::error(format!("Error writing file: {}", e));
        }

        // Refresh read-state to the post-edit file so a follow-up edit in this session
        // isn't wrongly flagged stale against our own edit.
        self.record_read(session, &path);
        crate::diagnostics_feed::clear_delivered(&path);

        let mut msg = if input.replace_all && count > 1 {
            format!("Replaced {} occurrences in {}", count, path)
        } else {
            // Line of the (single) match in the pre-edit content.
            let at_line = content
                .find(&old_string)
                .map(|idx| 1 + content[..idx].matches('\n').count())
                .unwrap_or(1);
            format!("Edited {}: replaced 1 occurrence at line {}", path, at_line)
        };
        // Edit-verification chain steps 1+2 (PRD P4.4): the parse verdict on
        // the post-edit content rides on the edit result, with LSP
        // diagnostics after it when a server is up.
        if let Some(note) = syntax_note(&path, &new_content, self.lsp.as_ref()) {
            msg.push('\n');
            msg.push_str(&note);
        }
        if let Some(note) = overwrite_note {
            msg.push_str("\n\n");
            msg.push_str(&note);
        }
        let result = ToolResult::ok(msg);
        // An edited work document must re-emit its artifact exactly like a write,
        // or the Work panel keeps rendering the pre-edit version — observed live:
        // the owner saw a stale document, told the agent "you didn't update it",
        // and the agent spiraled into re-writing a file it had correctly edited.
        if is_work_document(&path) {
            result.with_image_url(path)
        } else {
            result
        }
    }

    /// Hand an EXISTING file to the user as a download card.
    ///
    /// write/edit/convert only surface a file the run PRODUCES this turn (the
    /// artifact rides on that tool's `image_url`). A file that already exists —
    /// a deck a skill generated, a binary the user points at, anything the model
    /// can't re-`write` as text — had no delivery path at all: the model was left
    /// reciting `/data/files/…` paths or `cp`-ing the file to trip the freshly-
    /// produced heuristic. share is that missing path: it emits the file on the
    /// SAME `image_url` artifact channel write uses, so the chat dispatcher renders
    /// it as a card (and uploads it on a loop reply) — no re-generation, no copy.
    ///
    /// Any file type is allowed: the provider layer sniffs magic bytes and omits
    /// non-image bytes from the model payload (see `ai::image_source_to_base64`),
    /// so a `.pptx`/`.zip` path is carried as an attachment, never a bogus image.
    fn handle_share(&self, input: &FileInput) -> ToolResult {
        if input.path.is_empty() {
            return ToolResult::error(errors::missing_param(
                "share",
                "path",
                "os(resource: \"file\", action: \"share\", path: \"/data/files/deck.pptx\")",
            ));
        }

        let path = match types::pathres::resolve(&input.path) {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(e) => return ToolResult::error(format!("Error: {}", e)),
        };

        if let Err(e) = validate_file_path(&path, "share") {
            return ToolResult::error(format!("Error: {}", e));
        }

        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return ToolResult::error(errors::file_not_found(&path));
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return ToolResult::error(errors::permission_denied(&path, "share"));
            }
            Err(e) => return ToolResult::error(format!("Error accessing file: {}", e)),
        };
        if meta.is_dir() {
            return ToolResult::error(format!(
                "Error: {path} is a directory. share delivers a single file — pass the path to the file itself."
            ));
        }

        let name = std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        let size = meta.len();
        let size_str = if size >= 1024 {
            format!("{}KB", size / 1024)
        } else {
            format!("{size} bytes")
        };

        // The attachment is a fact of this reply; the model needs nothing more
        // than what was attached and how big it is.
        ToolResult::ok(format!(
            "Attached {name} ({size_str}) as a download card on this reply."
        ))
        .with_image_url(path)
    }

    fn handle_glob(&self, ctx: &ToolContext, input: &FileInput) -> ToolResult {
        // The expression goes in `pattern`. But the action is *named* "glob", so models
        // predictably pass it as `glob:` and then waste a call recovering from the error.
        // Accept `glob` as a synonym here (input tolerance — same precedent as memory
        // accepting `save` for `store`). The `glob` field is grep's file-filter for the grep
        // action; this fallback is scoped to handle_glob, so there is no collision.
        let pattern: &String = if input.pattern.is_empty() && !input.glob.is_empty() {
            &input.glob
        } else {
            &input.pattern
        };

        // If pattern/glob are empty but path contains glob metacharacters,
        // treat path as the full glob expression (e.g. "/Users/x/Desktop/*.{png,jpg}").
        let mut pattern_was_defaulted = false;
        let (resolved_pattern, base_path) = if !pattern.is_empty() {
            let bp = if input.path.is_empty() {
                std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| ".".to_string())
            } else {
                expand_path(&input.path)
            };
            (pattern.to_string(), bp)
        } else {
            let expanded = expand_path(&input.path);
            if expanded.contains('*') || expanded.contains('?') || expanded.contains('{') {
                // Split path at the first glob metacharacter boundary.
                // e.g. "fixtures/**/*.yaml" → base="fixtures", pattern="**/*.yaml"
                // e.g. "src/*.rs" → base="src", pattern="*.rs"
                let (base, pat) = split_path_at_glob(&expanded);
                (pat, base)
            } else if Path::new(&expanded).is_dir() {
                pattern_was_defaulted = true;
                ("*".to_string(), expanded)
            } else {
                // A bare path lists a directory (pattern "*"). This path is not
                // a directory, and that fact, not a missing parameter, is what
                // the caller needs: the same call succeeds on a directory.
                let what = if Path::new(&expanded).is_file() { "a file, not a directory" } else { "not a path that exists" };
                return ToolResult::error(format!(
                    "glob on {} did nothing: it is {}. A path alone lists a directory; to search by name give pattern (os(resource: \"file\", action: \"glob\", pattern: \"*.json\", path: \"<dir>\")); to read a file use action: \"read\".",
                    expanded, what
                ));
            }
        };
        let pattern = &resolved_pattern;

        let limit = if input.limit <= 0 { 100 } else { input.limit } as usize;

        let bounds = WalkBounds::for_root(Path::new(&base_path));
        let (matches, cut_short) = glob_with_globset(&base_path, pattern, limit, &bounds);

        // Sort by modification time (newest first)
        let mut files_with_time: Vec<(String, i64)> = matches
            .into_iter()
            .filter_map(|path| {
                std::fs::metadata(&path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .map(|t| {
                        (
                            path,
                            t.duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs() as i64)
                                .unwrap_or(0),
                        )
                    })
            })
            .collect();

        files_with_time.sort_by(|a, b| b.1.cmp(&a.1));

        let total_found = files_with_time.len();
        let truncated = total_found > limit;
        if truncated {
            files_with_time.truncate(limit);
        }

        if files_with_time.is_empty() {
            if let Some(cut) = cut_short {
                return ToolResult::ok(walk_bounds::took_too_long(
                    &format!("glob of \"{pattern}\""),
                    Path::new(&base_path),
                    cut,
                    "path",
                    &[],
                ));
            }
            return ToolResult::ok(nothing_matched(pattern, &base_path, ctx.cwd.as_deref()));
        }

        let display_base = &base_path;
        let paths: Vec<String> = files_with_time
            .iter()
            .map(|(p, _)| relativize_path(p, display_base))
            .collect();

        // The walk stops at limit + 1, so a truncated count is a floor, never a total.
        let mut result = if truncated {
            format!("More than {} entries match \"{}\"; showing the {} most recently modified. Narrow the pattern or raise limit.", limit, pattern, limit)
        } else {
            format!("Found {} entries matching \"{}\"", total_found, pattern)
        };
        if let Some(cut) = cut_short {
            result.push_str(&format!(
                " {}",
                walk_bounds::took_too_long(
                    &format!("glob of \"{pattern}\""),
                    Path::new(&base_path),
                    cut,
                    "path",
                    &[],
                )
            ));
        }
        result.push_str("\n\n");
        result.push_str(&paths.join("\n"));

        if pattern_was_defaulted {
            result.push_str(
                "\n\nTo filter by type, add pattern: os(action: \"glob\", pattern: \"*.json\", path: \".\")"
            );
        }

        ToolResult::ok(result)
    }

    fn handle_grep(&self, input: &FileInput) -> ToolResult {
        let pattern = &input.pattern;
        if pattern.is_empty() {
            return ToolResult::error(errors::missing_param("grep", "pattern", "os(resource: \"file\", action: \"grep\", pattern: \"TODO\", path: \".\")"));
        }

        let path = if input.path.is_empty() {
            ".".to_string()
        } else {
            expand_path(&input.path)
        };

        let limit = if input.limit <= 0 { 250 } else { input.limit } as usize;
        let offset = if input.offset <= 0 { 0 } else { input.offset } as usize;
        let output_mode = if input.output_mode.is_empty() {
            "content"
        } else {
            &input.output_mode
        };
        let ctx_before = if input.context_before <= 0 {
            0
        } else {
            input.context_before as usize
        };
        let ctx_after = if input.context_after <= 0 {
            0
        } else {
            input.context_after as usize
        };

        let grep = crate::grep_tool::GrepTool;
        grep.execute_search(
            pattern,
            &path,
            if input.glob.is_empty() {
                None
            } else {
                Some(&input.glob)
            },
            input.case_insensitive,
            input.multiline,
            limit,
            offset,
            output_mode,
            ctx_before,
            ctx_after,
        )
    }
}

impl Default for FileTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Glob using globset (supports brace expansion like *.{rs,toml}).
///
/// `bounds` is what keeps a walk from `/` from crossing the whole disk: the
/// model called `glob "**/image.png" path "/"` and this walk ran for three
/// minutes until the harness ended the run (gate 35577273218, 2026-09-21); a
/// cloud bot would walk the same way. Returns the matches (limit + 1 at most)
/// and, when the walk was stopped early, which bound stopped it.
fn glob_with_globset(
    base_path: &str,
    pattern: &str,
    limit: usize,
    bounds: &WalkBounds,
) -> (Vec<String>, Option<CutShort>) {
    let full_pattern = if pattern.contains("**") {
        // For recursive patterns, prepend base only if pattern doesn't start with /
        if Path::new(pattern).is_absolute() {
            pattern.to_string()
        } else {
            format!("{}/{}", base_path.trim_end_matches('/'), pattern)
        }
    } else {
        PathBuf::from(base_path)
            .join(pattern)
            .to_string_lossy()
            .to_string()
    };

    let matcher = match globset::GlobBuilder::new(&full_pattern)
        .literal_separator(false)
        .build()
    {
        Ok(g) => g.compile_matcher(),
        Err(_) => return (Vec::new(), None),
    };

    let is_recursive = pattern.contains("**");
    let mut matches = Vec::new();
    let mut visited = 0usize;
    let mut cut_short = None;

    let walker = walkdir::WalkDir::new(base_path)
        .follow_links(false)
        .max_depth(if is_recursive { usize::MAX } else { 1 })
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            if e.file_type().is_dir() {
                if name.starts_with('.') && name != "." {
                    return false;
                }
                if name == "node_modules"
                    || name == "vendor"
                    || name == "__pycache__"
                    || name == "target"
                {
                    return false;
                }
                // The kernel and device trees, and every foreign mount on this
                // box: a read of a directory on a virtiofs or NFS mount can
                // block with no timeout and no signal, where the deadline
                // below cannot reach it. The only fix is not going in.
                if bounds.skips(e.path()) {
                    return false;
                }
            }
            true
        });

    // Collect limit+1 so the caller can detect truncation.
    for entry in walker {
        if matches.len() > limit {
            break;
        }
        visited += 1;
        if let Some(cut) = bounds.spent(visited) {
            cut_short = Some(cut);
            break;
        }

        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        // Skip only the walk root itself (depth 0). Include both files AND
        // directories that match the pattern — `glob "*"` must list folders too.
        // Previously this `continue`d on every directory, so "list the files and
        // folders" returned files only (all subdirectories silently vanished).
        if entry.depth() == 0 {
            continue;
        }

        let path = entry.path();
        if matcher.is_match(path) {
            matches.push(path.to_string_lossy().to_string());
        }
    }

    (matches, cut_short)
}

/// Split a path at the first glob metacharacter, returning (base_dir, glob_pattern).
/// e.g. "fixtures/**/*.yaml" → ("fixtures", "**/*.yaml")
/// e.g. "src/*.rs" → ("src", "*.rs")
/// e.g. "**/*.rs" → (".", "**/*.rs")
fn split_path_at_glob(path: &str) -> (String, String) {
    let components: Vec<&str> = path.split('/').collect();
    let mut base_parts = Vec::new();
    let mut glob_parts = Vec::new();
    let mut found_glob = false;

    for component in &components {
        if !found_glob
            && !component.contains('*')
            && !component.contains('?')
            && !component.contains('{')
        {
            base_parts.push(*component);
        } else {
            found_glob = true;
            glob_parts.push(*component);
        }
    }

    let base = if base_parts.is_empty() {
        ".".to_string()
    } else {
        base_parts.join("/")
    };
    let pattern = if glob_parts.is_empty() {
        "*".to_string()
    } else {
        glob_parts.join("/")
    };
    (base, pattern)
}

fn relativize_path(path: &str, base: &str) -> String {
    Path::new(path)
        .strip_prefix(base)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string())
}

/// Extensions that count as user-facing "Work" products (reports, sheets, designs,
/// images). Code/config/scratch files are deliberately excluded so the Work panel
/// surfaces deliverables, not noise.
pub(crate) fn is_work_document(path: &str) -> bool {
    const WORK_EXTS: &[&str] = &[
        "md", "pdf", "csv", "xlsx", "xls", "docx", "doc", "pptx", "ppt", "html", "png",
        "jpg", "jpeg", "gif", "svg", "webp", "mp4", "webm", "mov", "jsx", "tsx",
    ];
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| WORK_EXTS.contains(&e.as_str()))
}

/// Detect a .html written with raw JSX but no transpiler — the agent's common
/// "React + CDN + JSX-in-a-<script>" mistake, which renders blank because browsers
/// can't parse JSX. `className=` is the tell: plain HTML uses `class=`, and Nebo's
/// SWC-compiled output uses `className:` (an object property) — only raw JSX writes
/// `className=`. We don't fire when a transpiler is present (Babel standalone) or
/// when it's Nebo's own converted shell (blob-module loader).
fn html_has_untranspiled_jsx(path: &str, content: &str) -> bool {
    let is_html = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"))
        .unwrap_or(false);
    if !is_html {
        return false;
    }
    let has_jsx = content.contains("className=") || content.contains("React.Fragment");
    let has_transpiler = content.contains("text/babel")
        || content.contains("babel/standalone")
        || content.contains("URL.createObjectURL"); // Nebo's converted blob-module shell
    has_jsx && !has_transpiler
}

/// Sensitive paths that the agent should never access.
fn sensitive_paths() -> Vec<String> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return vec![],
    };
    let home = home.to_string_lossy().to_string();

    vec![
        format!("{}/.ssh", home),
        format!("{}/.aws", home),
        format!("{}/.config/gcloud", home),
        format!("{}/.azure", home),
        format!("{}/.gnupg", home),
        format!("{}/.docker/config.json", home),
        format!("{}/.kube/config", home),
        format!("{}/.npmrc", home),
        format!("{}/.password-store", home),
        format!("{}/.bashrc", home),
        format!("{}/.bash_profile", home),
        format!("{}/.zshrc", home),
        format!("{}/.zprofile", home),
        format!("{}/.profile", home),
        "/etc/shadow".to_string(),
        "/etc/passwd".to_string(),
        "/etc/sudoers".to_string(),
    ]
}

/// Extract the text layer of a PDF.
///
/// Two honest failure modes worth distinguishing: a PDF we cannot parse, and a
/// PDF that parses fine but holds no text at all (a scan — pixels, no text
/// layer). Returning an empty string for the second would read to the model as
/// "this document is blank".
/// Containers the nebo-office `read` command turns into Markdown.
const OFFICE_READ_EXTS: &[&str] = &[
    "doc", "docx", "docm", "ppt", "pptx", "pptm", "xls", "xlsx", "xlsm", "xlsb",
    "odt", "ods", "odp", "rtf", "epub",
];

/// Read an office document as Markdown via the nebo-office binary.
///
/// Returns `None` when the file is not one of these formats or the binary is not
/// installed, so the caller falls through to its generic binary-file message.
fn read_office_document(path: &str) -> Option<ToolResult> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())?;
    if !OFFICE_READ_EXTS.contains(&ext.as_str()) {
        return None;
    }

    // The plugin exports its binary path; fall back to PATH for a dev checkout.
    let bin = std::env::var("NEBO_OFFICE_BIN").unwrap_or_else(|_| "nebo-office".to_string());
    let output = std::process::Command::new(&bin).arg("read").arg(path).output().ok()?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Some(ToolResult::error(format!(
            "Could not read {} as Markdown: {}",
            path,
            err.trim()
        )));
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        return Some(ToolResult::ok(format!(
            "[{}: parsed successfully but contains no extractable text. If it is a scan, \
             say so rather than guessing at its contents.]",
            path
        )));
    }
    Some(ToolResult::ok(text))
}

fn read_pdf_text(path: &str) -> ToolResult {
    // pdf-extract panics on some malformed files; a panic here would take the
    // whole run down, so it is caught and reported as a normal tool error.
    let extracted = std::panic::catch_unwind(|| pdf_extract::extract_text(path));

    let text = match extracted {
        Ok(Ok(text)) => text,
        Ok(Err(e)) => {
            return ToolResult::error(format!(
                "Could not extract text from PDF {}: {}. It may be encrypted or malformed.",
                path, e
            ));
        }
        Err(_) => {
            return ToolResult::error(format!(
                "Could not extract text from PDF {}: the parser failed on this file. \
                 It may be corrupt or use an unsupported encoding.",
                path
            ));
        }
    };

    if text.trim().is_empty() {
        return ToolResult::ok(format!(
            "[PDF: {} — parsed successfully but contains no text layer. It is almost certainly \
             a scan or an image-only export. The text cannot be read from the file itself; say \
             so rather than guessing at its contents.]",
            path
        ));
    }

    // Long PDFs would otherwise swallow the context window whole.
    const MAX_PDF_CHARS: usize = 100_000;
    if text.len() > MAX_PDF_CHARS {
        let cut = text
            .char_indices()
            .take_while(|(i, _)| *i < MAX_PDF_CHARS)
            .last()
            .map_or(0, |(i, c)| i + c.len_utf8());
        return ToolResult::ok(format!(
            "{}\n\n[Truncated: showing the first {} of {} bytes of {}.]",
            &text[..cut],
            cut,
            text.len(),
            path
        ));
    }

    ToolResult::ok(text)
}

/// Check if content appears to be binary by scanning for null bytes
/// and checking the ratio of non-printable characters.
/// Returns the trigger (for an honest placeholder message) when binary, `None` for text.
fn binary_reason(data: &[u8], sample_size: usize) -> Option<String> {
    let check = &data[..data.len().min(sample_size)];
    if check.iter().any(|&b| b == 0) {
        return Some("contains NUL bytes".to_string());
    }
    let non_printable = check
        .iter()
        .filter(|&&b| b < 0x20 && b != b'\n' && b != b'\r' && b != b'\t')
        .count();
    let frac = non_printable as f64 / check.len() as f64;
    if frac > 0.3 {
        return Some(format!("{:.0}% non-printable bytes", frac * 100.0));
    }
    None
}

/// Detect UTF-16 text before the binary check gets a chance to refuse it:
/// a BOM (FE FF big-endian / FF FE little-endian), or a strong alternating-NUL
/// pattern over ASCII-range bytes (how ASCII text looks when UTF-16 encoded).
/// Returns `Some(big_endian)` when detected, `None` otherwise.
fn detect_utf16(sample: &[u8]) -> Option<bool> {
    if sample.len() >= 2 {
        if sample[0] == 0xFE && sample[1] == 0xFF {
            return Some(true);
        }
        if sample[0] == 0xFF && sample[1] == 0xFE {
            return Some(false);
        }
    }
    if sample.len() >= 32 {
        let lane_nul = |start: usize| {
            let lane: Vec<u8> = sample.iter().skip(start).step_by(2).copied().collect();
            lane.iter().filter(|&&b| b == 0).count() as f64 / lane.len() as f64
        };
        let lane_ascii = |start: usize| {
            let lane: Vec<u8> = sample.iter().skip(start).step_by(2).copied().collect();
            lane.iter()
                .filter(|&&b| b == b'\n' || b == b'\r' || b == b'\t' || (0x20..0x7F).contains(&b))
                .count() as f64
                / lane.len() as f64
        };
        // Little-endian: char bytes in the even lane, NULs in the odd lane.
        if lane_nul(1) > 0.9 && lane_ascii(0) > 0.9 {
            return Some(false);
        }
        // Big-endian: NULs in the even lane, char bytes in the odd lane.
        if lane_nul(0) > 0.9 && lane_ascii(1) > 0.9 {
            return Some(true);
        }
    }
    None
}

/// Decode UTF-16 bytes (skipping any BOM) to text, lossily.
fn decode_utf16_bytes(bytes: &[u8], big_endian: bool) -> String {
    let body = if bytes.len() >= 2
        && ((bytes[0] == 0xFE && bytes[1] == 0xFF) || (bytes[0] == 0xFF && bytes[1] == 0xFE))
    {
        &bytes[2..]
    } else {
        bytes
    };
    let units: Vec<u16> = body
        .chunks_exact(2)
        .map(|c| {
            if big_endian {
                u16::from_be_bytes([c[0], c[1]])
            } else {
                u16::from_le_bytes([c[0], c[1]])
            }
        })
        .collect();
    String::from_utf16_lossy(&units)
}

/// Validate that a file path is safe to access.
fn validate_file_path(raw_path: &str, action: &str) -> Result<(), String> {
    let expanded = expand_path(raw_path);
    let abs_path =
        std::path::absolute(Path::new(&expanded)).map_err(|e| format!("invalid path: {}", e))?;
    let abs_str = abs_path.to_string_lossy().to_string();

    // Also resolve symlinks
    let real_path = std::fs::canonicalize(&abs_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| abs_str.clone());

    for sensitive in sensitive_paths() {
        if path_matches_or_inside(&abs_str, &sensitive)
            || path_matches_or_inside(&real_path, &sensitive)
        {
            return Err(format!(
                "blocked: {} access to '{}' is restricted (sensitive path). Ask the user to paste the needed value instead.",
                action, raw_path
            ));
        }
    }

    Ok(())
}

fn path_matches_or_inside(path: &str, target: &str) -> bool {
    if path == target {
        return true;
    }
    let target_with_sep = format!("{}/", target);
    path.starts_with(&target_with_sep)
}

fn straighten_quote(c: char) -> char {
    match c {
        '\u{2018}' | '\u{2019}' => '\'',
        '\u{201C}' | '\u{201D}' => '"',
        c => c,
    }
}

/// Find `needle` in `haystack`: exactly, or with curly quotes on either side
/// read as straight ones. Returns the haystack's own bytes for the span, so
/// the caller replaces what is really in the file.
fn find_actual_string(haystack: &str, needle: &str) -> Option<String> {
    if haystack.contains(needle) {
        return Some(needle.to_string());
    }
    let norm_needle: String = needle.chars().map(straighten_quote).collect();
    // Straightened haystack plus a map from each straightened byte offset back
    // to the original one (a curly quote is 3 bytes, its straight form 1).
    let mut norm = String::with_capacity(haystack.len());
    let mut back: Vec<usize> = Vec::with_capacity(haystack.len() + 1);
    for (orig, c) in haystack.char_indices() {
        let start = norm.len();
        norm.push(straighten_quote(c));
        back.extend(std::iter::repeat_n(orig, norm.len() - start));
    }
    back.push(haystack.len());
    let idx = norm.find(&norm_needle)?;
    Some(haystack[back[idx]..back[idx + norm_needle.len()]].to_string())
}

/// When old_string only matched through quote normalization (the file has
/// curly quotes, the model sent straight ones), curl the straight quotes in
/// new_string the same way so the edit keeps the document's typography. A
/// quote after whitespace, start of text, or an opening bracket opens;
/// any other closes.
fn preserve_quote_style(old_string: &str, actual_old: &str, new_string: &str) -> String {
    if old_string == actual_old || !actual_old.chars().any(|c| straighten_quote(c) != c) {
        return new_string.to_string();
    }
    let mut out = String::with_capacity(new_string.len() + 8);
    let mut prev: Option<char> = None;
    for c in new_string.chars() {
        let opening = prev.is_none_or(|p| p.is_whitespace() || matches!(p, '(' | '[' | '{'));
        out.push(match c {
            '\'' if opening => '\u{2018}',
            '\'' => '\u{2019}',
            '"' if opening => '\u{201C}',
            '"' => '\u{201D}',
            c => c,
        });
        prev = Some(c);
    }
    out
}

/// Expand `~` to the user's home directory. Tilde-only (no fuzzy
/// fallback) — call `types::pathres::resolve` directly when the file
/// must exist. Kept as a thin wrapper for legacy call sites.
/// True when the path lies under the attachment/ingestion root
/// (`<data_dir>/files/`) — files the agent pulled in, not files the owner
/// placed.
pub(crate) fn is_ingested_file(path: &str) -> bool {
    std::path::Path::new(path).starts_with(crate::checkpoint::data_dir().join("files"))
}

pub fn expand_path(path: &str) -> String {
    types::pathres::expand(path).to_string_lossy().into_owned()
}

/// Edit-verification chain, steps 1 and 2 (PRD_CODING_HARNESS P4.4): the ONE
/// note function for the ONE write/edit pathway.
///
/// Step 1 — a factual tree-sitter syntax line for a just-written file whose
/// extension maps to a compiled-in grammar. Step 2 — the LSP diagnostics line
/// for the touched file, appended AFTER the tree-sitter verdict when a
/// language server is up; any unavailability (no server installed, crashed
/// this session, or no publish within the 2s budget) appends NOTHING —
/// absence, not noise. `None` when neither step has anything to say. States
/// only — the write/edit has already landed and is NEVER blocked or rolled
/// back on a syntax error or diagnostic.
fn syntax_note(path: &str, content: &str, lsp: &dyn crate::lsp::LspProvider) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(lang) = syntax::Lang::from_path(Path::new(path))
        && let Ok(errors) = syntax::parse_check(content, lang)
    {
        if errors.is_empty() {
            parts.push(format!("syntax OK ({})", lang.name()));
        } else {
            let shown: Vec<String> = errors
                .iter()
                .take(3)
                .map(|e| format!("line {}: {}", e.line, e.message))
                .collect();
            let mut note = format!(
                "syntax: {} error{} — {}",
                errors.len(),
                if errors.len() == 1 { "" } else { "s" },
                shown.join("; ")
            );
            if errors.len() > 3 {
                note.push_str(&format!("; {} more", errors.len() - 3));
            }
            parts.push(note);
        }
    }
    if let Ok(report) = lsp.diagnostics(Path::new(path), content) {
        parts.push(crate::lsp::render_diagnostics(&report, 10));
    }
    if parts.is_empty() { None } else { Some(parts.join("\n")) }
}

/// Milliseconds since the epoch rendered as an RFC 3339 UTC timestamp (second
/// precision), or the raw value when it is out of range.
fn fmt_ms_rfc3339(ms: i64) -> String {
    match chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms) {
        Some(dt) => dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        None => format!("{} ms since epoch", ms),
    }
}

/// Current on-disk modification time of `path` in milliseconds since the epoch, or
/// `None` if it can't be determined. Used by the read-before-edit staleness guard.
fn current_mtime_ms(path: &str) -> Option<i64> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as i64)
}


/// One file as this session last saw it. `content` is `None` past
/// [`MAX_TRACKED_CONTENT_BYTES`]: the change is still reported, without lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadEntry {
    pub mtime_ms: i64,
    pub content: Option<String>,
}

/// Session-keyed file snapshots (`session\u{1f}path` → entry). Shared by the
/// file tool that fills it and the runner that sweeps it.
pub type ReadState = Arc<Mutex<HashMap<String, ReadEntry>>>;

/// Files above this size are tracked by mtime alone.
pub const MAX_TRACKED_CONTENT_BYTES: u64 = 256 * 1024;

/// Lines shown per side of an outside-edit snippet before it is elided.
pub const EDIT_SNIPPET_LINES: usize = 40;

fn snapshot(path: &str) -> Option<ReadEntry> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)?;
    let content = (meta.len() <= MAX_TRACKED_CONTENT_BYTES)
        .then(|| std::fs::read_to_string(path).ok())
        .flatten();
    Some(ReadEntry { mtime_ms, content })
}

/// Sweep `session`'s snapshots for files changed on disk by someone else (the
/// owner, a formatter, a hook) and return one reminder per changed file. Each
/// change is reported once: the snapshot is refreshed as it is reported. A
/// touch that left the bytes alone is refreshed silently.
pub fn external_edit_notes(state: &ReadState, session: &str) -> Vec<String> {
    let prefix = format!("{session}\u{1f}");
    // Snapshot-then-release: the ledger lock is never held across file I/O.
    let tracked: Vec<(String, ReadEntry)> = match state.lock() {
        Ok(m) => m
            .iter()
            .filter(|(key, _)| key.starts_with(&prefix))
            .map(|(key, entry)| (key.clone(), entry.clone()))
            .collect(),
        Err(_) => return Vec::new(), // poisoned: a panic elsewhere; no notes this pass
    };
    let mut notes = Vec::new();
    let mut refreshed: Vec<(String, ReadEntry)> = Vec::new();
    for (key, entry) in tracked {
        let path = &key[prefix.len()..];
        let Some(now) = snapshot(path) else { continue };
        if now.mtime_ms == entry.mtime_ms {
            continue;
        }
        if now.content != entry.content {
            let snippet = match (&entry.content, &now.content) {
                (Some(old), Some(new)) => edit_snippet(old, new),
                _ => "(file too large to show the changed lines)".to_string(),
            };
            let mtime = fmt_ms_rfc3339(now.mtime_ms);
            notes.push(format!(
                "Note: {path} changed on disk at {mtime} since this session last read it (not \
                 through this tool). Current content differs as shown; work from the current \
                 content:\n{snippet}"
            ));
        }
        refreshed.push((key, now));
    }
    if let Ok(mut m) = state.lock() {
        for (key, entry) in refreshed {
            m.insert(key, entry);
        }
    }
    notes
}

/// The lines that differ between two versions, numbered per side, with the
/// unchanged head and tail trimmed off. Not a minimal diff: a moved block shows
/// as the whole span between the first and last changed line, which is still
/// exactly what the model must not revert.
pub fn edit_snippet(old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let max_tail = a.len().min(b.len()) - head;
    let tail = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(max_tail)
        .take_while(|(x, y)| x == y)
        .count();
    let mut out = String::new();
    let mut side = |sign: char, lines: &[&str], first: usize| {
        for (i, line) in lines.iter().enumerate() {
            if i == EDIT_SNIPPET_LINES {
                out.push_str(&format!("{sign} ... {} more lines\n", lines.len() - i));
                break;
            }
            out.push_str(&format!("{sign}{}: {line}\n", first + i + 1));
        }
    };
    side('-', &a[head..a.len() - tail], head);
    side('+', &b[head..b.len() - tail], head);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The path a hint offers to glob next, if it offers one at all.
    fn widening_offered(hint: &str) -> Option<String> {
        let rest = &hint[hint.find("action: \"glob\"")?..];
        let start = rest.find("path: \"")? + "path: \"".len();
        let end = rest[start..].find('"')?;
        Some(rest[start..start + end].to_string())
    }

    /// The widening an empty glob offers must never leave the bot's own area.
    /// The defect (gate fixture `os-file-discovery-spiral`): the hint always
    /// named the parent of the folder just searched, the model took the offer
    /// every time, and two hints in a row walked `/home/<bot>/` up to `/home`
    /// and then to `/`, where the walk ran until the harness cancelled the
    /// run. Widening from inside home must climb to home and stop there,
    /// offering a different tool rather than a bigger walk.
    #[test]
    fn the_empty_glob_hint_never_widens_past_the_bots_own_area() {
        let _g = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let previous_home = std::env::var_os("HOME");
        // SAFETY: serialized by the crate-wide lock, and put back below.
        unsafe {
            std::env::set_var("HOME", home.path());
            std::env::set_var("NEBO_HOME", home.path().join("nebo"));
        }
        let cwd = home.path().join("work").to_string_lossy().into_owned();

        let mut at = home.path().join("Desktop").join("archive").join("2026");
        let mut offered: Vec<PathBuf> = Vec::new();
        let mut hints: Vec<String> = Vec::new();
        for _ in 0..16 {
            let hint = nothing_matched("image.png", &at.to_string_lossy(), Some(&cwd));
            hints.push(hint.clone());
            match widening_offered(&hint) {
                Some(next) => {
                    let next = PathBuf::from(next);
                    offered.push(next.clone());
                    at = next;
                }
                None => break,
            }
        }

        // SAFETY: same lock; the process gets its own home back.
        unsafe {
            match previous_home {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
        }

        for hint in &hints {
            assert!(
                !hint.contains("path: \"/\""),
                "a hint offered the whole machine: {hint}"
            );
        }
        for path in &offered {
            assert!(
                path.starts_with(home.path()),
                "a hint widened out of the bot's own area: {}",
                path.display()
            );
        }
        assert_eq!(
            offered,
            vec![
                home.path().join("Desktop").join("archive"),
                home.path().join("Desktop"),
                home.path().to_path_buf(),
            ],
            "the widening must climb to the area's root and stop there"
        );

        let edge = hints.last().expect("a hint was produced");
        assert!(
            edge.contains("edge of this bot's own area"),
            "the last hint must say why it stops: {edge}"
        );
        assert!(
            edge.contains("action: \"search\"") && edge.contains("action: \"grep\""),
            "the edge must offer a lookup by name and a content search: {edge}"
        );
        assert!(
            edge.contains("ask the owner where it lives"),
            "the edge must offer asking the owner: {edge}"
        );
    }

    /// A walk that hits its entry budget stops and says so; one that does not
    /// finishes clean. Both on the same tree, so only the budget differs.
    #[test]
    fn glob_walk_stops_at_its_entry_budget_and_says_so() {
        // tempdir names start with a dot, which the walk skips; walk a plain child.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        std::fs::create_dir(&root).unwrap();
        for d in 0..5 {
            let sub = root.join(format!("d{d}"));
            std::fs::create_dir(&sub).unwrap();
            for f in 0..5 {
                std::fs::write(sub.join(format!("f{f}.txt")), "x").unwrap();
            }
        }
        let base = root.to_string_lossy().to_string();
        let big = WalkBounds {
            skip: Vec::new(),
            max_entries: 1_000,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(60),
        };
        let (all, cut) = glob_with_globset(&base, "**/*.txt", 100, &big);
        assert_eq!((all.len(), cut), (25, None));

        let small = WalkBounds { max_entries: 8, ..big };
        let (some, cut) = glob_with_globset(&base, "**/*.txt", 100, &small);
        assert_eq!(cut, Some(CutShort::Entries(8)), "the walk must report it stopped early");
        assert!(some.len() < 25, "fewer matches than exist: {}", some.len());
    }

    /// The incident: `glob "**/image.png" path "/"` walked the whole disk for
    /// three minutes. A walk from the root must step around a foreign mount —
    /// a read inside one can block in the kernel where no deadline reaches it
    /// — and must come back when its clock runs out, saying so.
    #[test]
    fn a_glob_from_the_root_stays_out_of_a_foreign_mount_and_stops_at_its_deadline() {
        // A real directory under / stands in for the virtiofs mount that hung
        // the gate's walk, named by the mounts table this box would report.
        let mount = ["/usr", "/Library", "/opt", "/home", "/srv"]
            .iter()
            .map(Path::new)
            .find(|p| p.is_dir())
            .expect("some walkable directory exists under /");
        let table = format!(
            "/dev/vda1 / ext4 rw 0 0\nmount0 {} fuse.virtiofs rw 0 0\n",
            mount.display()
        );
        let foreign = walk_bounds::parse_foreign_mounts(&table);
        assert_eq!(foreign, vec![mount.to_path_buf()]);

        let open = WalkBounds {
            skip: Vec::new(),
            max_entries: usize::MAX,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(60),
        };
        let (listed, _) = glob_with_globset("/", "*", 10_000, &open);
        assert!(
            listed.iter().any(|p| Path::new(p) == mount),
            "a walk of / reaches {} when nothing prunes it",
            mount.display()
        );

        let pruned = WalkBounds { skip: foreign.clone(), ..open };
        let (kept, _) = glob_with_globset("/", "*", 10_000, &pruned);
        assert!(
            !kept.iter().any(|p| Path::new(p).starts_with(mount)),
            "the walk stepped into the mount it was told to skip: {kept:?}"
        );

        // And the recursive walk that hung the gate comes back at its clock.
        let budget = std::time::Duration::from_millis(500);
        let started = std::time::Instant::now();
        let timed = WalkBounds {
            skip: foreign,
            max_entries: usize::MAX,
            deadline: started + budget,
        };
        // A pattern nothing matches, so only the clock can end the walk.
        let (found, cut) = glob_with_globset("/", "**/*.nebo-no-such-extension", 100, &timed);
        let elapsed = started.elapsed();
        assert!(found.is_empty(), "found {found:?}");
        assert!(
            matches!(cut, Some(CutShort::Deadline(_))),
            "a recursive walk of / cannot finish in {budget:?}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(20),
            "the walk must return at its deadline, took {elapsed:?}"
        );

        let msg = walk_bounds::took_too_long(
            "glob of \"**/*.nebo-no-such-extension\"",
            Path::new("/"),
            cut.unwrap(),
            "path",
            &[],
        );
        assert!(msg.contains("not proof of absence"));
        assert!(msg.contains("Narrow it"));
        assert!(msg.contains("path"));
        assert!(!msg.contains('!'));
    }

    /// Spill artifacts under nebo-tool-results/ already carry line numbers
    /// from the read that produced them; reading one back must NOT number the
    /// lines again ("1  1  #!/usr..." read as file corruption by the model).
    /// Every other path keeps the numbered rendering.
    #[test]
    fn spill_artifacts_are_read_raw_everything_else_is_numbered() {
        let tmp = tempfile::tempdir().unwrap();
        let spill_dir = tmp.path().join("tool-results");
        std::fs::create_dir_all(&spill_dir).unwrap();
        let spill = spill_dir.join("abc.txt");
        std::fs::write(&spill, "     1\talready numbered\n").unwrap();
        let normal = tmp.path().join("plain.txt");
        std::fs::write(&normal, "hello\n").unwrap();

        let tool = FileTool::new();
        let spill_read = tool.execute(&ctx(), serde_json::json!({
            "action": "read", "path": spill.to_string_lossy()
        }));
        assert!(
            spill_read.content.starts_with("     1\talready numbered"),
            "spill read must be raw, got: {}",
            spill_read.content
        );
        let normal_read = tool.execute(&ctx(), serde_json::json!({
            "action": "read", "path": normal.to_string_lossy()
        }));
        assert!(
            normal_read.content.starts_with("     1\thello"),
            "normal read keeps numbering, got: {}",
            normal_read.content
        );
    }
    use crate::origin::{Origin, ToolContext};
    use serde_json::json;
    use std::fs;

    fn ctx() -> ToolContext {
        ToolContext::new(Origin::User)
    }

    /// A restore rewrites files the agent itself asked to put back; the read
    /// ledger must follow, or the very next edit is warned about its own change.
    #[test]
    fn restore_refreshes_read_state_so_the_next_edit_has_no_overwrite_warning() {
        let _g = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        // SAFETY: serialized by the crate-wide lock; checkpoints go under NEBO_HOME.
        unsafe { std::env::set_var("NEBO_HOME", home.path()) };
        let work = tempfile::tempdir().unwrap();
        let path = work.path().join("a.txt");
        std::fs::write(&path, "one\n").unwrap();
        let p = path.to_string_lossy().into_owned();
        let t = FileTool::new();
        let c = ctx();
        assert!(!t.execute(&c, serde_json::json!({"action": "read", "path": p})).is_error);
        let cp = t.execute(&c, serde_json::json!({"action": "checkpoint", "paths": [p]}));
        assert!(!cp.is_error, "{}", cp.content);
        let id = cp.content.split_whitespace().find(|w| w.starts_with("cp-")).unwrap().to_string();
        // Change it (through the tool, so the ledger sees the write), then put it back.
        assert!(!t.execute(&c, serde_json::json!({"action": "edit", "path": p, "old_string": "one", "new_string": "two"})).is_error);
        // Make sure the restore lands on a later mtime tick than the edit.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let r = t.execute(&c, serde_json::json!({"action": "restore", "checkpoint": id}));
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\n");
        let e = t.execute(&c, serde_json::json!({"action": "edit", "path": p, "old_string": "one", "new_string": "three"}));
        assert!(!e.is_error, "{}", e.content);
        assert!(!e.content.contains("Warning"), "no overwrite warning after a restore:\n{}", e.content);
        unsafe { std::env::remove_var("NEBO_HOME") };
    }

    #[test]
    fn outside_edit_is_reported_once_with_the_changed_lines() {
        let work = tempfile::tempdir().unwrap();
        let path = work.path().join("a.txt");
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
        let p = path.to_string_lossy().into_owned();
        let t = FileTool::new();
        let c = ctx();
        assert!(!t.execute(&c, json!({"action": "read", "path": p})).is_error);
        assert!(external_edit_notes(&t.read_state(), &c.session_key).is_empty(), "nothing changed yet");
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, "one\n2\nthree\n").unwrap();
        let notes = external_edit_notes(&t.read_state(), &c.session_key);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("changed on disk at 20"), "{}", notes[0]);
        assert!(notes[0].contains("work from the current content"), "{}", notes[0]);
        assert!(notes[0].contains("-2: two\n+2: 2\n"), "{}", notes[0]);
        assert!(!notes[0].contains("one"), "unchanged lines are trimmed:\n{}", notes[0]);
        assert!(external_edit_notes(&t.read_state(), &c.session_key).is_empty(), "reported once");
        // Our own edit afterwards is not an outside change.
        assert!(!t.execute(&c, json!({"action": "edit", "path": p, "old_string": "2", "new_string": "deux"})).is_error);
        assert!(external_edit_notes(&t.read_state(), &c.session_key).is_empty(), "own edits are not outside edits");
    }

    #[test]
    fn touch_that_leaves_bytes_alone_is_silent_and_other_sessions_are_not_swept() {
        let work = tempfile::tempdir().unwrap();
        let path = work.path().join("a.txt");
        std::fs::write(&path, "same\n").unwrap();
        let p = path.to_string_lossy().into_owned();
        let t = FileTool::new();
        let c = ctx();
        assert!(!t.execute(&c, json!({"action": "read", "path": p})).is_error);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, "same\n").unwrap();
        assert!(external_edit_notes(&t.read_state(), &c.session_key).is_empty(), "a touch is not an edit");
        // The sweep is gated on mtime, so land the real change on a later tick.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, "changed\n").unwrap();
        assert!(external_edit_notes(&t.read_state(), "some-other-session").is_empty());
        assert_eq!(external_edit_notes(&t.read_state(), &c.session_key).len(), 1);
    }

    #[test]
    fn edit_snippet_trims_common_head_and_tail_and_elides_long_runs() {
        assert_eq!(edit_snippet("a\nb\nc\n", "a\nB\nc\n"), "-2: b\n+2: B\n");
        assert_eq!(edit_snippet("a\n", "a\nb\n"), "+2: b\n");
        assert_eq!(edit_snippet("a\nb\n", "a\n"), "-2: b\n");
        assert_eq!(edit_snippet("x\n", "x\n"), "");
        let long: String = (0..EDIT_SNIPPET_LINES + 5).map(|i| format!("l{i}\n")).collect();
        let s = edit_snippet("", &long);
        assert!(s.contains("+ ... 5 more lines"), "{s}");
        assert_eq!(s.lines().count(), EDIT_SNIPPET_LINES + 1);
    }

    #[test]
    fn pdf_read_reports_a_pdf_failure_not_a_binary_refusal() {
        // A file claiming to be a PDF but holding garbage must route to the PDF
        // reader and come back with a PDF-specific message. The regression this
        // guards is the branch falling through to the generic binary refusal,
        // which is how PDFs became a dead end in the first place.
        let dir = std::env::temp_dir().join("nebo_pdf_route_test");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("broken.pdf");
        let mut bytes = b"%PDF-1.4\n".to_vec();
        bytes.extend_from_slice(&[0u8; 512]); // NULs — reads as binary
        fs::write(&path, &bytes).unwrap();

        let result = read_pdf_text(path.to_str().unwrap());
        let text = format!("{} {}", result.content, result.is_error);
        assert!(
            text.contains("PDF"),
            "expected a PDF-specific result, got: {text}"
        );
        assert!(
            !text.contains("Binary file detected"),
            "PDF fell through to the generic binary refusal: {text}"
        );

        fs::remove_file(&path).ok();
    }

    #[test]
    fn detects_untranspiled_jsx_html() {
        // raw JSX in a .html, no transpiler → flagged
        assert!(html_has_untranspiled_jsx(
            "dash.html",
            "<div id=root></div><script>function App(){return <div className=\"p\">hi</div>}</script>"
        ));
        // plain HTML (class=, not className=) → not flagged
        assert!(!html_has_untranspiled_jsx(
            "page.html",
            "<div class=\"p\">hi</div>"
        ));
        // Babel standalone present → transpiler handles it, not flagged
        assert!(!html_has_untranspiled_jsx(
            "ok.html",
            "<script src=\"babel/standalone\"></script><script type=\"text/babel\">const x=<div className=\"p\"/>;</script>"
        ));
        // Nebo's converted shell (blob-module loader) → not flagged
        assert!(!html_has_untranspiled_jsx(
            "conv.html",
            "<script type=module>const m=await import(URL.createObjectURL(new Blob([s])));</script> className="
        ));
        // not html → never flagged
        assert!(!html_has_untranspiled_jsx("a.jsx", "return <div className=\"p\"/>"));
    }

    /// Create a subdirectory with a non-dot name inside the tempdir.
    /// macOS tempfile dirs are dot-prefixed (.tmpXXX), which the glob walker skips.
    fn glob_dir(parent: &std::path::Path) -> PathBuf {
        let d = parent.join("testdir");
        fs::create_dir_all(&d).unwrap();
        d
    }

    // ── Glob brace expansion ────────────────────────────────────────
    #[test]
    fn glob_brace_expansion() {
        let tmp = tempfile::tempdir().unwrap();
        let base = glob_dir(tmp.path());
        fs::write(base.join("a.rs"), "").unwrap();
        fs::write(base.join("b.toml"), "").unwrap();
        fs::write(base.join("c.md"), "").unwrap();

        let tool = FileTool::new();
        let res = tool.execute(
            &ctx(),
            json!({"action":"glob","path": base.to_str().unwrap(), "pattern":"*.{rs,toml}"}),
        );

        assert!(!res.is_error, "glob failed: {}", res.content);
        assert!(res.content.contains("a.rs"), "missing a.rs");
        assert!(res.content.contains("b.toml"), "missing b.toml");
        assert!(!res.content.contains("c.md"), "c.md should be excluded");
    }

    // ── Glob lists directories too, not just files (regression) ─────
    // `glob "*"` must return BOTH files AND folders. A prior version skipped every
    // directory entry, so "list the files and folders" returned files only and all
    // subdirectories silently vanished.
    #[test]
    fn glob_includes_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let base = glob_dir(tmp.path());
        fs::write(base.join("file1.txt"), "").unwrap();
        fs::create_dir_all(base.join("subdir")).unwrap();
        fs::create_dir_all(base.join("📁 Projects")).unwrap(); // emoji dir name (Desktop case)

        let tool = FileTool::new();
        let res = tool.execute(
            &ctx(),
            json!({"action":"glob","path": base.to_str().unwrap(), "pattern":"*"}),
        );

        assert!(!res.is_error, "glob failed: {}", res.content);
        assert!(res.content.contains("file1.txt"), "missing file");
        assert!(res.content.contains("subdir"), "directory was dropped from glob results");
        assert!(res.content.contains("📁 Projects"), "emoji-named directory was dropped");
    }

    // ── "list"/"ls"/dir prior-redirects land on glob and succeed ────
    // First-call success: the ls prior (action "list", target in `dir`) must
    // execute the one glob implementation, not bounce with a correction.
    #[test]
    fn list_prior_redirects_to_glob() {
        let tmp = tempfile::tempdir().unwrap();
        let base = glob_dir(tmp.path());
        fs::write(base.join("hello.txt"), "").unwrap();

        let tool = FileTool::new();
        for (action, key) in [("list", "path"), ("ls", "path"), ("list", "dir")] {
            let res = tool.execute(&ctx(), json!({"action": action, key: base.to_str().unwrap()}));
            assert!(!res.is_error, "{action}+{key} failed: {}", res.content);
            assert!(
                res.content.contains("hello.txt"),
                "{action}+{key} did not list the directory: {}",
                res.content
            );
        }
    }

    // ── Glob path-as-pattern (no pattern field) ─────────────────────
    #[test]
    fn glob_path_as_pattern() {
        let tmp = tempfile::tempdir().unwrap();
        let base = glob_dir(tmp.path());
        fs::write(base.join("x.rs"), "").unwrap();
        fs::write(base.join("y.toml"), "").unwrap();
        fs::write(base.join("z.md"), "").unwrap();

        let tool = FileTool::new();
        let glob_path = format!("{}/*.{{rs,toml}}", base.display());
        let res = tool.execute(&ctx(), json!({"action":"glob","path": glob_path}));

        assert!(!res.is_error, "glob failed: {}", res.content);
        assert!(res.content.contains("x.rs"), "missing x.rs");
        assert!(res.content.contains("y.toml"), "missing y.toml");
        assert!(!res.content.contains("z.md"), "z.md should be excluded");
    }

    // ── Glob structured output header ───────────────────────────────
    #[test]
    fn glob_structured_output() {
        let tmp = tempfile::tempdir().unwrap();
        let base = glob_dir(tmp.path());
        fs::write(base.join("one.txt"), "").unwrap();
        fs::write(base.join("two.txt"), "").unwrap();

        let tool = FileTool::new();
        let res = tool.execute(
            &ctx(),
            json!({"action":"glob","path": base.to_str().unwrap(), "pattern":"*.txt"}),
        );

        assert!(!res.is_error);
        assert!(
            res.content.starts_with("Found 2 entries matching"),
            "unexpected header: {}",
            res.content
        );
    }

    // ── Glob default limit caps results at 100 ────────────────────
    #[test]
    fn glob_limit_truncates() {
        let tmp = tempfile::tempdir().unwrap();
        let base = glob_dir(tmp.path());
        for i in 0..150 {
            fs::write(base.join(format!("f{:04}.txt", i)), "").unwrap();
        }

        let tool = FileTool::new();
        let res = tool.execute(
            &ctx(),
            json!({"action":"glob","path": base.to_str().unwrap(), "pattern":"*.txt"}),
        );

        assert!(!res.is_error);
        // Walker collects limit+1 so caller detects truncation. The header
        // must read as a floor, never "Found 101 files".
        assert!(
            res.content.starts_with("More than 100 entries match"),
            "expected truncation notice: {}",
            res.content
        );
        // Only 100 file lines should appear (101st is used for detection only).
        let file_lines = res.content.lines().skip(2).count(); // skip header + blank line
        assert_eq!(file_lines, 100, "expected 100 file lines in output");
        // With an explicit lower limit, fewer are shown
        let res2 = tool.execute(
            &ctx(),
            json!({"action":"glob","path": base.to_str().unwrap(), "pattern":"*.txt", "limit": 10}),
        );
        assert!(!res2.is_error);
        assert!(
            res2.content.starts_with("More than 10 entries match"),
            "expected truncation with limit=10: {}",
            res2.content
        );
    }

    // ── Repeat reads always return content (no suppression) ─────────
    // A read MUST always return the file's content. We deliberately removed the old
    // path-keyed "contents unchanged" cache — it was unverifiable across compaction and
    // gaslit the model into a retry spiral (the #research read-loop incident).
    /// A file with content must NEVER read back as empty.
    ///
    /// Live 2026-08-28: `read` returned no content for a 651-line / 23KB Python
    /// file while `grep` on the same path found all 651 lines. The model was told
    /// the file was empty, and spent 15+ turns re-reading it every way it could
    /// think of. Nothing in the suite asserted this invariant, which is how it
    /// shipped. Shaped deliberately like the file that broke it.
    #[test]
    fn read_never_reports_a_non_empty_file_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grabber.py");
        let mut body = String::from("#!/usr/bin/env python3\n");
        for i in 0..650 {
            body.push_str(&format!("line_{i} = 'x' * 24  # padding to ~23KB\n"));
        }
        fs::write(&path, &body).unwrap();
        assert!(body.len() > 20_000, "fixture must match the real shape");

        let tool = FileTool::new();
        let r = tool.execute(
            &ctx(),
            json!({"action":"read","path": path.to_str().unwrap()}),
        );

        // The specific lie: claiming emptiness about a file that has content.
        assert!(
            !r.content.contains("(file is empty)"),
            "a {}-byte file must never report as empty: {}",
            body.len(),
            crate::truncate_str(&r.content, 300)
        );
        // And it must actually deliver the content, not a bland success with none.
        assert!(!r.is_error, "read must succeed: {}", r.content);
        assert!(
            r.content.contains("line_0") && r.content.contains("line_649"),
            "read must return the file's lines: {}",
            crate::truncate_str(&r.content, 300)
        );
    }

    /// The honest empty case still reads as empty.
    #[test]
    fn read_reports_a_genuinely_empty_file_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.txt");
        fs::write(&path, "").unwrap();

        let tool = FileTool::new();
        let r = tool.execute(
            &ctx(),
            json!({"action":"read","path": path.to_str().unwrap()}),
        );
        assert!(!r.is_error);
        // Tool-result vocabulary, NOT `<system-reminder>` — that tag is the
        // ephemeral message-stream channel (CHAT_SYSTEM.md §4.2) and must not
        // appear in a persisted tool result.
        assert!(
            r.content.contains("(file is empty)"),
            "a 0-byte file is genuinely empty: {}",
            r.content
        );
    }

    #[test]
    fn file_read_repeat_returns_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        fs::write(&path, "line one\nline two\n").unwrap();

        let tool = FileTool::new();
        let input = json!({"action":"read","path": path.to_str().unwrap()});

        let r1 = tool.execute(&ctx(), input.clone());
        assert!(!r1.is_error);
        assert!(r1.content.contains("line one"));

        // A second identical read returns the content again — never a placeholder.
        let r2 = tool.execute(&ctx(), input);
        assert!(!r2.is_error);
        assert!(
            r2.content.contains("line one"),
            "repeat read must return content, not a cache placeholder: {}",
            r2.content
        );
    }

    // ── Reads always reflect the current file contents ──────────────
    #[test]
    fn file_read_reflects_modification() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mut.txt");
        fs::write(&path, "version 1\n").unwrap();

        let tool = FileTool::new();
        let input = json!({"action":"read","path": path.to_str().unwrap()});

        let r1 = tool.execute(&ctx(), input.clone());
        assert!(!r1.is_error);
        assert!(r1.content.contains("version 1"));

        // Bump mtime by rewriting
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(&path, "version 2\n").unwrap();

        let r2 = tool.execute(&ctx(), input);
        assert!(!r2.is_error);
        assert!(
            r2.content.contains("version 2"),
            "expected fresh read after modification: {}",
            r2.content
        );
    }

    // ── Overwrite advisory: warn, never block ───────────────────────
    // A hard block here taught the model to route the write through a shell
    // heredoc, where it got no staleness protection at all. The write goes
    // through; the warning rides on the result.
    #[test]
    fn edit_without_prior_read_succeeds_with_warning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("e.txt");
        fs::write(&path, "alpha\n").unwrap();
        let tool = FileTool::new();
        let r = tool.execute(
            &ctx(),
            json!({"action":"edit","path": path.to_str().unwrap(),"old_string":"alpha","new_string":"beta"}),
        );
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(fs::read_to_string(&path).unwrap(), "beta\n");
        assert!(
            r.content.contains("Warning") && r.content.contains("no record of reading it"),
            "the edit lands, the advisory rides along: {}",
            r.content
        );
    }

    #[test]
    fn read_then_edit_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("e.txt");
        fs::write(&path, "alpha\n").unwrap();
        let tool = FileTool::new();
        let p = path.to_str().unwrap();
        assert!(!tool.execute(&ctx(), json!({"action":"read","path": p})).is_error);
        let r = tool.execute(
            &ctx(),
            json!({"action":"edit","path": p,"old_string":"alpha","new_string":"beta"}),
        );
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(fs::read_to_string(&path).unwrap(), "beta\n");
    }

    #[test]
    fn stale_edit_succeeds_with_warning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("e.txt");
        fs::write(&path, "alpha\n").unwrap();
        let tool = FileTool::new();
        let p = path.to_str().unwrap();
        assert!(!tool.execute(&ctx(), json!({"action":"read","path": p})).is_error);
        // External modification bumps mtime after the read.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&path, "alpha changed\n").unwrap();
        let r = tool.execute(
            &ctx(),
            json!({"action":"edit","path": p,"old_string":"alpha","new_string":"beta"}),
        );
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(fs::read_to_string(&path).unwrap(), "beta changed\n");
        assert!(r.content.contains("changed on disk"), "{}", r.content);
    }

    #[test]
    fn second_edit_after_first_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("e.txt");
        fs::write(&path, "a b a\n").unwrap();
        let tool = FileTool::new();
        let p = path.to_str().unwrap();
        assert!(!tool.execute(&ctx(), json!({"action":"read","path": p})).is_error);
        assert!(!tool
            .execute(&ctx(), json!({"action":"edit","path": p,"old_string":"b","new_string":"B"}))
            .is_error);
        // Second edit without a re-read: read-state was refreshed by the first edit.
        let r = tool.execute(
            &ctx(),
            json!({"action":"edit","path": p,"old_string":"B","new_string":"BB"}),
        );
        assert!(!r.is_error, "{}", r.content);
    }

    #[test]
    fn write_new_file_without_read_ok() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.txt");
        let tool = FileTool::new();
        let r = tool.execute(
            &ctx(),
            json!({"action":"write","path": path.to_str().unwrap(),"content":"hi\n"}),
        );
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(fs::read_to_string(&path).unwrap(), "hi\n");
    }

    #[test]
    fn overwrite_existing_without_read_succeeds_with_warning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("exists.txt");
        fs::write(&path, "old\n").unwrap();
        let tool = FileTool::new();
        let r = tool.execute(
            &ctx(),
            json!({"action":"write","path": path.to_str().unwrap(),"content":"new\n"}),
        );
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
        assert!(
            r.content.contains("Warning") && r.content.contains("no record of reading it"),
            "{}",
            r.content
        );
    }

    #[test]
    fn fresh_file_write_carries_no_warning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("brand-new.json");
        let tool = FileTool::new();
        let r = tool.execute(
            &ctx(),
            json!({"action":"write","path": path.to_str().unwrap(),"content":"{}"}),
        );
        assert!(!r.is_error, "{}", r.content);
        assert!(!r.content.contains("Warning"), "new files are clean: {}", r.content);
    }

    // ── share ────────────────────────────────────────────────────────
    #[test]
    fn share_existing_file_emits_download_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deck.pptx");
        fs::write(&path, b"PK\x03\x04 not-really-a-deck").unwrap();
        let tool = FileTool::new();
        // No prior read required — share hands over a file that already exists.
        let r = tool.execute(
            &ctx(),
            json!({"action":"share","path": path.to_str().unwrap()}),
        );
        assert!(!r.is_error, "{}", r.content);
        assert!(
            r.image_url.as_deref().is_some_and(|u| u.ends_with("deck.pptx")),
            "share must emit the file on the image_url artifact channel, got {:?}",
            r.image_url
        );
    }

    #[test]
    fn share_missing_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.pptx");
        let tool = FileTool::new();
        let r = tool.execute(
            &ctx(),
            json!({"action":"share","path": path.to_str().unwrap()}),
        );
        assert!(r.is_error);
        assert!(r.image_url.is_none());
    }

    #[test]
    fn share_directory_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let tool = FileTool::new();
        let r = tool.execute(
            &ctx(),
            json!({"action":"share","path": dir.path().to_str().unwrap()}),
        );
        assert!(r.is_error);
        assert!(r.content.contains("directory"), "{}", r.content);
    }

    // ── relativize_path ─────────────────────────────────────────────
    #[test]
    fn relativize_strips_prefix() {
        assert_eq!(relativize_path("/a/b/c.rs", "/a/b"), "c.rs");
        assert_eq!(relativize_path("/a/b/d/e.rs", "/a/b"), "d/e.rs");
    }

    #[test]
    fn relativize_returns_original_when_no_prefix() {
        assert_eq!(relativize_path("/x/y/z.rs", "/a/b"), "/x/y/z.rs");
    }

    // ── Edit-verification chain step 1: the syntax line (PRD P4.4) ──

    /// A write of valid source appends "syntax OK (<lang>)" to the result —
    /// the parse verdict rides on the ONE write pathway.
    #[test]
    fn write_valid_rust_appends_syntax_ok() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.rs");
        let tool = FileTool::new();
        let r = tool.execute(
            &ctx(),
            json!({"action":"write","path": path.to_str().unwrap(),"content":"fn main() {}\n"}),
        );
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("syntax OK (rust)"), "{}", r.content);
    }

    /// A write of broken source STATES the errors factually — and still
    /// lands. Never a block, never a rollback.
    #[test]
    fn write_broken_rust_states_errors_and_still_lands() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.rs");
        let content = "fn main() {\n    let x = ;\n}\n";
        let tool = FileTool::new();
        let r = tool.execute(
            &ctx(),
            json!({"action":"write","path": path.to_str().unwrap(),"content": content}),
        );
        assert!(!r.is_error, "syntax errors must not fail the write: {}", r.content);
        assert_eq!(fs::read_to_string(&path).unwrap(), content, "the write landed");
        assert!(r.content.contains("syntax: 1 error"), "{}", r.content);
        assert!(r.content.contains("line 2"), "{}", r.content);
    }

    /// Files with no compiled-in grammar get NO syntax line — absence, not
    /// noise.
    #[test]
    fn write_no_grammar_file_has_no_syntax_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        let tool = FileTool::new();
        let r = tool.execute(
            &ctx(),
            json!({"action":"write","path": path.to_str().unwrap(),"content":"fn nope() {}\n"}),
        );
        assert!(!r.is_error, "{}", r.content);
        assert!(!r.content.contains("syntax"), "{}", r.content);
    }

    /// An edit's result carries the parse verdict of the POST-edit content:
    /// an edit that breaks the file says so; the fix-up says OK again.
    #[test]
    fn edit_reports_syntax_verdict_of_new_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("e.rs");
        fs::write(&path, "fn main() { let x = 1; }\n").unwrap();
        let tool = FileTool::new();
        let p = path.to_str().unwrap();
        assert!(!tool.execute(&ctx(), json!({"action":"read","path": p})).is_error);
        let r = tool.execute(
            &ctx(),
            json!({"action":"edit","path": p,"old_string":"let x = 1;","new_string":"let x = ;"}),
        );
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("syntax: 1 error"), "{}", r.content);
        let r = tool.execute(
            &ctx(),
            json!({"action":"edit","path": p,"old_string":"let x = ;","new_string":"let x = 2;"}),
        );
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("syntax OK (rust)"), "{}", r.content);
    }

    // ── Edit-verification chain step 2: the LSP line (PRD P4.4) ─────

    /// Fixed-diagnostics provider standing in for a live server.
    struct MockLspDiags;

    impl crate::lsp::LspProvider for MockLspDiags {
        fn diagnostics(
            &self,
            _path: &Path,
            _text: &str,
        ) -> Result<crate::lsp::DiagReport, crate::lsp::Unavailable> {
            Ok(crate::lsp::DiagReport {
                server: "rust-analyzer".into(),
                diagnostics: vec![crate::lsp::Diag {
                    line: 1,
                    col: 4,
                    severity: crate::lsp::Severity::Warning,
                    message: "unused variable `x`".into(),
                }],
            })
        }
        fn definition(
            &self,
            path: &Path,
            _text: &str,
            _line: u32,
            _col: u32,
        ) -> Result<Vec<crate::lsp::Location>, crate::lsp::Unavailable> {
            Err(crate::lsp::Unavailable::NoServer { lang: crate::lsp::lang_label(path) })
        }
        fn references(
            &self,
            path: &Path,
            _text: &str,
            _line: u32,
            _col: u32,
        ) -> Result<Vec<crate::lsp::Location>, crate::lsp::Unavailable> {
            Err(crate::lsp::Unavailable::NoServer { lang: crate::lsp::lang_label(path) })
        }
        fn hover(
            &self,
            path: &Path,
            _text: &str,
            _line: u32,
            _col: u32,
        ) -> Result<Option<String>, crate::lsp::Unavailable> {
            Err(crate::lsp::Unavailable::NoServer { lang: crate::lsp::lang_label(path) })
        }
    }

    /// With the LSP provider unavailable (the unit-test default is
    /// lsp::NoServers), the result carries the tree-sitter verdict and NO
    /// lsp line — absence, not noise.
    #[test]
    fn write_without_lsp_server_has_no_lsp_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.rs");
        let tool = FileTool::new();
        let r = tool.execute(
            &ctx(),
            json!({"action":"write","path": path.to_str().unwrap(),"content":"fn main() {}\n"}),
        );
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("syntax OK (rust)"), "{}", r.content);
        assert!(!r.content.contains("lsp"), "no server means no lsp line: {}", r.content);
    }

    /// With a server up (mocked), the factual lsp diagnostics line rides on
    /// the write result AFTER the tree-sitter verdict.
    #[test]
    fn write_with_lsp_server_appends_diagnostics_after_syntax_verdict() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.rs");
        let mut tool = FileTool::new();
        tool.lsp = Arc::new(MockLspDiags);
        let r = tool.execute(
            &ctx(),
            json!({"action":"write","path": path.to_str().unwrap(),"content":"fn main() {}\n"}),
        );
        assert!(!r.is_error, "{}", r.content);
        let syntax_at = r.content.find("syntax OK (rust)").expect("tree-sitter verdict present");
        let lsp_at = r
            .content
            .find("lsp (rust-analyzer): 1 warning — line 1 (warning): unused variable `x`")
            .unwrap_or_else(|| panic!("lsp line present: {}", r.content));
        assert!(lsp_at > syntax_at, "lsp line comes AFTER the syntax verdict: {}", r.content);
    }

    /// The same chain rides on the edit pathway (ONE note function).
    #[test]
    fn edit_with_lsp_server_appends_diagnostics_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("e.rs");
        fs::write(&path, "fn main() { let x = 1; }\n").unwrap();
        let mut tool = FileTool::new();
        tool.lsp = Arc::new(MockLspDiags);
        let p = path.to_str().unwrap();
        assert!(!tool.execute(&ctx(), json!({"action":"read","path": p})).is_error);
        let r = tool.execute(
            &ctx(),
            json!({"action":"edit","path": p,"old_string":"let x = 1;","new_string":"let x = 2;"}),
        );
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("syntax OK (rust)"), "{}", r.content);
        assert!(
            r.content.contains("lsp (rust-analyzer): 1 warning"),
            "{}",
            r.content
        );
    }

    // ── Outline-first reads (PRD P4.1) ──────────────────────────────

    /// Write a source file long enough to trip the default 2000-line read
    /// truncation.
    fn big_rust_file(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("big.rs");
        let mut src = String::new();
        for i in 0..2100 {
            src.push_str(&format!("fn f{i}() {{}}\n"));
        }
        fs::write(&path, &src).unwrap();
        path
    }

    /// A blind read that comes back truncated prepends the tree-sitter
    /// outline with ranged-read instructions — and the outline's own cap is
    /// stated, never silent.
    #[test]
    fn truncated_read_of_source_file_prepends_outline() {
        let dir = tempfile::tempdir().unwrap();
        let path = big_rust_file(dir.path());
        let tool = FileTool::new();
        let r = tool.execute(&ctx(), json!({"action":"read","path": path.to_str().unwrap()}));
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.starts_with("[Outline (rust, 2100 lines total)"), "{}", crate::truncate_str(&r.content, 200));
        assert!(r.content.contains("fn f0  [1-1]"), "{}", crate::truncate_str(&r.content, 400));
        assert!(
            r.content.contains("… 1900 more omitted"),
            "outline cap must be stated: {}",
            crate::truncate_str(&r.content, 400)
        );
        assert!(r.content.contains("offset/limit"), "must tell the model how to read ranges");
        // The truncated content still follows — the outline decorates the ONE
        // read pathway, it does not replace the read.
        assert!(r.content.contains("showing lines 1-2000"), "{}", crate::truncate_str(&r.content, 200));
    }

    /// A read that is NOT truncated carries no outline preamble.
    #[test]
    fn untruncated_read_has_no_outline_preamble() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("small.rs");
        fs::write(&path, "fn a() {}\nfn b() {}\n").unwrap();
        let tool = FileTool::new();
        let r = tool.execute(&ctx(), json!({"action":"read","path": path.to_str().unwrap()}));
        assert!(!r.is_error);
        assert!(!r.content.contains("[Outline"), "{}", r.content);
    }

    /// An explicitly RANGED read never gets the outline — the model is
    /// already reading by range, so the preamble would be repeated noise.
    #[test]
    fn ranged_read_has_no_outline_preamble() {
        let dir = tempfile::tempdir().unwrap();
        let path = big_rust_file(dir.path());
        let tool = FileTool::new();
        let r = tool.execute(
            &ctx(),
            json!({"action":"read","path": path.to_str().unwrap(), "offset": 5}),
        );
        assert!(!r.is_error);
        assert!(!r.content.contains("[Outline"), "{}", crate::truncate_str(&r.content, 200));
        let r = tool.execute(
            &ctx(),
            json!({"action":"read","path": path.to_str().unwrap(), "limit": 10}),
        );
        assert!(!r.is_error);
        assert!(!r.content.contains("[Outline"), "{}", crate::truncate_str(&r.content, 200));
    }

    /// A limit-truncated read states the real total and the offset to continue
    /// from, never a "+" floor.
    #[test]
    fn limited_read_states_total_and_next_offset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("n.txt");
        fs::write(&path, "a\nb\nc\nd\ne\n").unwrap();
        let tool = FileTool::new();
        let r = tool.execute(
            &ctx(),
            json!({"action":"read","path": path.to_str().unwrap(), "limit": 2}),
        );
        assert!(!r.is_error, "{}", r.content);
        assert!(
            r.content.ends_with("(showing lines 1-2 of 5; continue with offset: 3)"),
            "{}",
            r.content
        );
    }

    /// An offset past the end names the file's line count, not a bound.
    #[test]
    fn offset_past_end_states_line_count() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("n.txt");
        fs::write(&path, "a\nb\nc\n").unwrap();
        let tool = FileTool::new();
        let r = tool.execute(
            &ctx(),
            json!({"action":"read","path": path.to_str().unwrap(), "offset": 10}),
        );
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(r.content, "(file has 3 lines; offset 10 is past the end)");
    }

    /// The byte cap cuts on a line boundary and the footer names the last whole
    /// line and the offset to continue from; the footer is never sliced off.
    #[test]
    fn over_budget_read_is_an_error_naming_the_file_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wide.txt");
        let mut src = String::new();
        for i in 0..1200 {
            src.push_str(&format!("{i:04} {}\n", "x".repeat(95)));
        }
        fs::write(&path, &src).unwrap();
        let tool = FileTool::new();
        let r = tool.execute(&ctx(), json!({"action":"read","path": path.to_str().unwrap()}));
        assert!(r.is_error, "{}", crate::truncate_str(&r.content, 200));
        assert!(r.content.contains("the file has 1200 lines"), "{}", r.content);
        assert!(r.content.contains("Use offset and limit"), "{}", r.content);
        assert!(r.content.len() < 600, "an error, not a clipped payload: {} bytes", r.content.len());
        // A ranged read that fits is served whole.
        let r = tool.execute(&ctx(), json!({"action":"read","path": path.to_str().unwrap(), "offset": 600, "limit": 300}));
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("   600\t0599 "), "{}", crate::truncate_str(&r.content, 200));
        assert!(r.content.contains("   899\t0898 "), "{}", crate::truncate_str(&r.content, 200));
    }

    /// A pasted document is one long line; it is shown whole.
    #[test]
    fn long_line_is_shown_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("long.txt");
        fs::write(&path, format!("{}END\n", "y".repeat(3800))).unwrap();
        let tool = FileTool::new();
        let r = tool.execute(&ctx(), json!({"action":"read","path": path.to_str().unwrap()}));
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("yyyyEND"), "the end of the line is shown");
        assert!(!r.content.contains("truncated"), "{}", crate::truncate_str(&r.content, 100));
    }

    /// A single line inside the budget is shown whole, however long.
    #[test]
    fn huge_line_within_budget_is_shown_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("huge.txt");
        fs::write(&path, format!("{}END\n", "y".repeat(60_000))).unwrap();
        let tool = FileTool::new();
        let r = tool.execute(&ctx(), json!({"action":"read","path": path.to_str().unwrap()}));
        assert!(!r.is_error, "{}", crate::truncate_str(&r.content, 100));
        assert!(r.content.contains("yyyyEND"), "{}", crate::truncate_str(&r.content, 100));
        assert!(!r.content.contains("truncated"), "{}", crate::truncate_str(&r.content, 100));
    }

    /// A straight-quoted old_string still lands on a curly-quoted document,
    /// and the replacement keeps the document's quotes.
    #[test]
    fn edit_matches_through_curly_quotes_and_keeps_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        let p = path.to_str().unwrap();
        fs::write(&path, "She said \u{201C}hello\u{201D} to Bob\u{2019}s team.\n").unwrap();
        let tool = FileTool::new();
        let r = tool.execute(&ctx(), json!({"action":"read","path": p}));
        assert!(!r.is_error);
        let r = tool.execute(&ctx(), json!({"action":"edit","path": p,
            "old_string": "said \"hello\" to Bob's", "new_string": "said \"goodbye\" to Bob's"}));
        assert!(!r.is_error, "{}", r.content);
        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(after, "She said \u{201C}goodbye\u{201D} to Bob\u{2019}s team.\n");
        // An exact match is untouched: straight quotes stay straight.
        fs::write(&path, "x = \"a\"\n").unwrap();
        let r = tool.execute(&ctx(), json!({"action":"read","path": p}));
        assert!(!r.is_error);
        let r = tool.execute(&ctx(), json!({"action":"edit","path": p, "old_string": "\"a\"", "new_string": "\"b\""}));
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(fs::read_to_string(&path).unwrap(), "x = \"b\"\n");
    }

    /// Create and overwrite are distinct observations, and an edit names the line.
    #[test]
    fn write_and_edit_results_state_what_changed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w.txt");
        let p = path.to_str().unwrap();
        let tool = FileTool::new();
        let r = tool.execute(&ctx(), json!({"action":"write","path": p,"content":"one\ntwo\n"}));
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.starts_with(&format!("Created {} (8 bytes)", p)), "{}", r.content);
        let r = tool.execute(&ctx(), json!({"action":"write","path": p,"content":"one\ntwo\nthree\n"}));
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.starts_with(&format!("Overwrote {} (8 to 14 bytes)", p)), "{}", r.content);
        let r = tool.execute(
            &ctx(),
            json!({"action":"edit","path": p,"old_string":"three","new_string":"3"}),
        );
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.starts_with(&format!("Edited {}: replaced 1 occurrence at line 3", p)), "{}", r.content);
    }

    /// Overwriting a file this session never read names what was replaced.
    #[test]
    fn unread_overwrite_warning_states_size_and_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("u.txt");
        fs::write(&path, "previous\n").unwrap();
        let p = path.to_str().unwrap();
        let tool = FileTool::new();
        let r = tool.execute(&ctx(), json!({"action":"write","path": p,"content":"new\n"}));
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("already existed (9 bytes, modified 20"), "{}", r.content);
        assert!(r.content.contains("shell reads are not recorded"), "{}", r.content);
    }
}

