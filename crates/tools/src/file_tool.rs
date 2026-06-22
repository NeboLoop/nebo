use crate::errors;
use crate::origin::ToolContext;
use crate::registry::ToolResult;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// File operations: read, write, edit, glob, grep.
pub struct FileTool {
    pub on_file_read: Option<Box<dyn Fn(&str) + Send + Sync>>,
    /// Per-(session, path) record of the file's mtime (ms) at the last successful read,
    /// for the read-before-edit + staleness guard. Keyed by `session_key\u{1f}path`.
    ///
    /// Metadata only — we never cache the content (the content always travels in the
    /// read's tool_result, and re-reads always return fresh content). This mirrors the
    /// useful half of Claude Code's readFileState while deliberately omitting its content
    /// dedup, whose "refer to the earlier read" stub turns into a blank once that earlier
    /// read is evicted by compaction.
    read_state: Arc<Mutex<HashMap<String, i64>>>,
}

#[derive(Debug, Deserialize)]
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
    output_mode: String,
    #[serde(default)]
    context_before: i64,
    #[serde(default)]
    context_after: i64,
}

impl FileTool {
    pub fn new() -> Self {
        Self {
            on_file_read: None,
            read_state: Arc::new(Mutex::new(HashMap::new())),
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

        let session = ctx.session_key.as_str();
        match fi.action.as_str() {
            "read" => self.handle_read(session, &fi),
            "write" => self.handle_write(session, &fi),
            "edit" => self.handle_edit(session, &fi),
            "glob" => self.handle_glob(&fi),
            "grep" => self.handle_grep(&fi),
            // Prior-redirect ("ls ~/Desktop"): a directory listing IS glob with
            // its defaulted "*" pattern — route to the one implementation. Not
            // advertised in the schema; glob stays the single documented way.
            "list" | "ls" => self.handle_glob(&fi),
            other => ToolResult::error(format!(
                "Unknown action: {} (valid: read, write, edit, glob, grep)",
                other
            )),
        }
    }

    fn read_state_key(session: &str, path: &str) -> String {
        format!("{session}\u{1f}{path}")
    }

    /// Record that `path` was read in `session` at the file's current mtime.
    fn record_read(&self, session: &str, path: &str, mtime_ms: i64) {
        if let Ok(mut m) = self.read_state.lock() {
            m.insert(Self::read_state_key(session, path), mtime_ms);
        }
    }

    /// Guard for edit/overwrite: the file must have been read in this session, and
    /// must not have changed on disk since that read. Returns Err(message) otherwise.
    fn check_editable(&self, session: &str, path: &str, verb: &str) -> Result<(), String> {
        let guard = match self.read_state.lock() {
            Ok(g) => g,
            Err(_) => return Ok(()), // poisoned lock: don't block the edit on our bookkeeping
        };
        match guard.get(&Self::read_state_key(session, path)) {
            None => Err(format!(
                "File has not been read yet: {path}\n\
                 Read it first so your {verb} is based on its current contents:\n\
                 os(resource: \"file\", action: \"read\", path: \"{path}\")"
            )),
            Some(&read_mtime) => {
                // Only reject when the file is demonstrably newer than our recorded read.
                // If we can't stat it, stay lenient (don't block on our own bookkeeping).
                if let Some(cur) = current_mtime_ms(path) {
                    if cur > read_mtime {
                        return Err(format!(
                            "File has been modified since you last read it (by the user, a \
                             linter, or another process): {path}\n\
                             Read it again before this {verb} so you don't overwrite those changes."
                        ));
                    }
                }
                Ok(())
            }
        }
    }

    fn handle_read(&self, session: &str, input: &FileInput) -> ToolResult {
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
            return ToolResult::error(format!(
                "Path is a directory: {}\nUse glob action to list directory contents",
                path
            ));
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
        {
            // Read a fixed window via a real syscall — never size this from
            // metadata.len(), which can be 0 for dataless/placeholder files (e.g.
            // iCloud "optimize storage") even when the file has content.
            let mut sample = [0u8; 8192];
            if let Ok(n) = file.read(&mut sample) {
                if n > 0 && is_binary_content(&sample[..n], n) {
                    // Images: return them INLINE as a viewable image (data URL) so the model
                    // actually sees the pixels — like Claude Code's Read. The runner renders
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
                    return ToolResult::ok("[Binary file detected — content not shown]");
                }
            }
            // Seek back to start for the line-based reader
            if let Err(e) = std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(0)) {
                return ToolResult::error(format!("Error seeking file: {}", e));
            }
        }

        let reader = BufReader::with_capacity(1024 * 1024, file);
        let mut result = String::new();
        let mut line_num = 0usize;
        let mut lines_read = 0usize;

        for line_result in reader.lines() {
            let line = match line_result {
                Ok(l) => l,
                Err(e) => return ToolResult::error(format!("Error reading file: {}", e)),
            };

            line_num += 1;

            if line_num < offset {
                continue;
            }

            if lines_read >= limit {
                result.push_str(&format!(
                    "\n... (showing lines {}-{} of {}+)",
                    offset,
                    line_num - 1,
                    line_num
                ));
                break;
            }

            let display_line = if line.len() > 2000 {
                format!("{}...", crate::truncate_str(&line, 2000))
            } else {
                line
            };

            result.push_str(&format!("{:6}\t{}\n", line_num, display_line));
            lines_read += 1;
        }

        if result.is_empty() {
            if offset > 1 {
                result = format!("(file has fewer than {} lines)", offset);
            } else {
                result = "(file is empty)".to_string();
            }
        }

        // Cap total result size to prevent huge files from blowing up context
        const FILE_READ_MAX_CHARS: usize = 50_000;
        if result.len() > FILE_READ_MAX_CHARS {
            let total_len = result.len();
            let truncated = crate::truncate_str(&result, FILE_READ_MAX_CHARS);
            result = format!(
                "{}\n\n[Output truncated: {} total chars, showing first {}. Use offset/limit params to read specific sections.]",
                truncated, total_len, FILE_READ_MAX_CHARS
            );
        }

        if let Some(ref callback) = self.on_file_read {
            callback(&path);
        }

        // Record the read so a later Edit/Write of this path can require a prior read
        // and detect on-disk changes since now. Metadata only — content is never cached.
        let mtime_ms = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        self.record_read(session, &path, mtime_ms);

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

        // Overwriting an existing file requires a prior read (and that it hasn't changed
        // since), so we don't silently clobber edits made by the user or another process.
        // Creating a new file, or appending, does not need a prior read.
        if !input.append && Path::new(&path).exists() {
            if let Err(msg) = self.check_editable(session, &path, "write") {
                return ToolResult::error(msg);
            }
        }

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
                if let Some(m) = current_mtime_ms(&path) {
                    self.record_read(session, &path, m);
                }
                let action = if input.append { "Appended" } else { "Wrote" };
                let mut msg = format!("{} {} bytes to {}", action, input.content.len(), path);
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

        // Edit requires a prior read of the current contents, and that the file hasn't
        // changed on disk since that read (read-before-edit + staleness guard).
        if let Err(msg) = self.check_editable(session, &path, "edit") {
            return ToolResult::error(msg);
        }

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

        if !content.contains(&input.old_string) {
            return ToolResult::error(format!(
                "Error: old_string not found in file.\n\nSearched for:\n```\n{}\n```\n\nMake sure the string matches exactly, including whitespace and indentation.",
                input.old_string
            ));
        }

        let count = content.matches(&input.old_string).count();
        if count > 1 && !input.replace_all {
            return ToolResult::error(format!(
                "Error: old_string appears {} times in file. Use replace_all=true to replace all, or make the search string more specific.",
                count
            ));
        }

        let new_content = if input.replace_all {
            content.replace(&input.old_string, &input.new_string)
        } else {
            content.replacen(&input.old_string, &input.new_string, 1)
        };

        if let Err(e) = std::fs::write(&path, &new_content) {
            return ToolResult::error(format!("Error writing file: {}", e));
        }

        // Refresh read-state to the post-edit file so a follow-up edit in this session
        // isn't wrongly flagged stale against our own edit.
        if let Some(m) = current_mtime_ms(&path) {
            self.record_read(session, &path, m);
        }

        if input.replace_all && count > 1 {
            ToolResult::ok(format!("Replaced {} occurrences in {}", count, path))
        } else {
            ToolResult::ok(format!("Edited {}", path))
        }
    }

    fn handle_glob(&self, input: &FileInput) -> ToolResult {
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
                return ToolResult::error(errors::missing_param("glob", "pattern", "os(resource: \"file\", action: \"glob\", pattern: \"*.rs\", path: \".\")"));
            }
        };
        let pattern = &resolved_pattern;

        let limit = if input.limit <= 0 { 100 } else { input.limit } as usize;

        let matches = glob_with_globset(&base_path, pattern, limit);

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
            return ToolResult::ok(format!(
                "No files found matching \"{}\" in {}. This is not an error — no files match this pattern in this directory. Try a different pattern or path.",
                pattern, base_path
            ));
        }

        let display_base = &base_path;
        let paths: Vec<String> = files_with_time
            .iter()
            .map(|(p, _)| relativize_path(p, display_base))
            .collect();

        let mut result = format!("Found {} files matching \"{}\"", total_found, pattern);
        if truncated {
            result.push_str(&format!(" (showing first {}, results truncated)", limit));
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
fn glob_with_globset(base_path: &str, pattern: &str, limit: usize) -> Vec<String> {
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
        Err(_) => return Vec::new(),
    };

    let is_recursive = pattern.contains("**");
    let mut matches = Vec::new();

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
            }
            true
        });

    // Collect limit+1 so the caller can detect truncation.
    for entry in walker {
        if matches.len() > limit {
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

    matches
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
fn is_work_document(path: &str) -> bool {
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

/// Check if content appears to be binary by scanning for null bytes
/// and checking the ratio of non-printable characters.
fn is_binary_content(data: &[u8], sample_size: usize) -> bool {
    let check = &data[..data.len().min(sample_size)];
    if check.iter().any(|&b| b == 0) {
        return true;
    }
    let non_printable = check
        .iter()
        .filter(|&&b| b < 0x20 && b != b'\n' && b != b'\r' && b != b'\t')
        .count();
    non_printable as f64 / check.len() as f64 > 0.3
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
                "blocked: {} access to {:?} is restricted (sensitive path)",
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

/// Expand `~` to the user's home directory. Tilde-only (no fuzzy
/// fallback) — call `types::pathres::resolve` directly when the file
/// must exist. Kept as a thin wrapper for legacy call sites.
pub fn expand_path(path: &str) -> String {
    types::pathres::expand(path).to_string_lossy().into_owned()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::origin::{Origin, ToolContext};
    use serde_json::json;
    use std::fs;

    fn ctx() -> ToolContext {
        ToolContext::new(Origin::User)
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
            res.content.starts_with("Found 2 files matching"),
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
        // Walker collects limit+1 so caller detects truncation.
        // Default limit=100, so 150 files → "Found 101 files" + truncation notice.
        assert!(
            res.content.contains("results truncated"),
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
            res2.content.contains("results truncated"),
            "expected truncation with limit=10: {}",
            res2.content
        );
    }

    // ── Repeat reads always return content (no suppression) ─────────
    // A read MUST always return the file's content. We deliberately removed the old
    // path-keyed "contents unchanged" cache — it was unverifiable across compaction and
    // gaslit the model into a retry spiral (the #research read-loop incident).
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

    // ── Read-before-edit + staleness guard ──────────────────────────
    #[test]
    fn edit_requires_prior_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("e.txt");
        fs::write(&path, "alpha\n").unwrap();
        let tool = FileTool::new();
        let r = tool.execute(
            &ctx(),
            json!({"action":"edit","path": path.to_str().unwrap(),"old_string":"alpha","new_string":"beta"}),
        );
        assert!(r.is_error);
        assert!(r.content.contains("has not been read yet"), "{}", r.content);
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
    fn edit_rejected_when_modified_since_read() {
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
        assert!(r.is_error);
        assert!(r.content.contains("modified since"), "{}", r.content);
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
    fn overwrite_existing_without_read_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("exists.txt");
        fs::write(&path, "old\n").unwrap();
        let tool = FileTool::new();
        let r = tool.execute(
            &ctx(),
            json!({"action":"write","path": path.to_str().unwrap(),"content":"new\n"}),
        );
        assert!(r.is_error);
        assert!(r.content.contains("has not been read yet"), "{}", r.content);
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
}

