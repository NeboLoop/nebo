use crate::origin::ToolContext;
use crate::registry::ToolResult;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// File operations: read, write, edit, glob, grep.
pub struct FileTool {
    pub on_file_read: Option<Box<dyn Fn(&str) + Send + Sync>>,
    read_cache: Mutex<HashMap<String, (SystemTime, i64, i64)>>,
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
    regex: String,
    #[serde(default)]
    glob: String,
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
            read_cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn name(&self) -> &str {
        "file"
    }

    pub fn execute(&self, _ctx: &ToolContext, input: serde_json::Value) -> ToolResult {
        let fi: FileInput = match serde_json::from_value(input) {
            Ok(v) => v,
            Err(e) => return ToolResult::error(format!("invalid input: {}", e)),
        };

        match fi.action.as_str() {
            "read" => self.handle_read(&fi),
            "write" => self.handle_write(&fi),
            "edit" => self.handle_edit(&fi),
            "glob" => self.handle_glob(&fi),
            "grep" => self.handle_grep(&fi),
            other => ToolResult::error(format!(
                "Unknown action: {} (valid: read, write, edit, glob, grep)",
                other
            )),
        }
    }

    fn handle_read(&self, input: &FileInput) -> ToolResult {
        if input.path.is_empty() {
            return ToolResult::error("Error: path is required");
        }

        if let Err(e) = validate_file_path(&input.path, "read") {
            return ToolResult::error(format!("Error: {}", e));
        }

        let path = expand_path(&input.path);
        let offset = if input.offset <= 0 { 1 } else { input.offset } as usize;
        let limit = if input.limit <= 0 { 2000 } else { input.limit } as usize;

        let metadata = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return ToolResult::error(format!("File not found: {}", path));
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

        // File read dedup: skip if same file, same range, file unchanged
        if let Ok(mtime) = metadata.modified() {
            let cache_key = path.clone();
            if let Ok(cache) = self.read_cache.lock() {
                if let Some(&(cached_mtime, cached_offset, cached_limit)) = cache.get(&cache_key) {
                    if cached_mtime == mtime
                        && cached_offset == input.offset
                        && cached_limit == input.limit
                    {
                        if let Some(ref callback) = self.on_file_read {
                            callback(&path);
                        }
                        return ToolResult::ok(format!(
                            "File unchanged since last read at line {}. Contents are already in your conversation context.",
                            offset
                        ));
                    }
                }
            }
        }

        let mut file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(e) => return ToolResult::error(format!("Error opening file: {}", e)),
        };

        // Check for binary content before attempting line-based reading
        {
            let mut sample = vec![0u8; 8192.min(metadata.len() as usize)];
            if let Ok(n) = file.read(&mut sample) {
                if n > 0 && is_binary_content(&sample[..n], n) {
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

        // Update read cache
        if let Ok(mtime) = std::fs::metadata(&path).and_then(|m| m.modified()) {
            if let Ok(mut cache) = self.read_cache.lock() {
                cache.insert(path.clone(), (mtime, input.offset, input.limit));
            }
        }

        if let Some(ref callback) = self.on_file_read {
            callback(&path);
        }

        ToolResult::ok(result)
    }

    fn handle_write(&self, input: &FileInput) -> ToolResult {
        if input.path.is_empty() {
            return ToolResult::error("Error: path is required");
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

        let path = expand_path(&input.path);

        // Create parent directories
        if let Some(parent) = Path::new(&path).parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
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
                let action = if input.append { "Appended" } else { "Wrote" };
                ToolResult::ok(format!(
                    "{} {} bytes to {}",
                    action,
                    input.content.len(),
                    path
                ))
            }
            Err(e) => ToolResult::error(format!("Error writing file: {}", e)),
        }
    }

    fn handle_edit(&self, input: &FileInput) -> ToolResult {
        if input.path.is_empty() {
            return ToolResult::error("Error: path is required");
        }
        if input.old_string.is_empty() {
            return ToolResult::error("Error: old_string is required");
        }
        if input.old_string == input.new_string {
            return ToolResult::error("Error: old_string and new_string are identical");
        }

        if let Err(e) = validate_file_path(&input.path, "edit") {
            return ToolResult::error(format!("Error: {}", e));
        }

        let path = expand_path(&input.path);

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return ToolResult::error(format!("File not found: {}", path));
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

        if input.replace_all && count > 1 {
            ToolResult::ok(format!("Replaced {} occurrences in {}", count, path))
        } else {
            ToolResult::ok(format!("Edited {}", path))
        }
    }

    fn handle_glob(&self, input: &FileInput) -> ToolResult {
        let pattern = if input.pattern.is_empty() { &input.glob } else { &input.pattern };

        // If pattern/glob are empty but path contains glob metacharacters,
        // treat path as the full glob expression (e.g. "/Users/x/Desktop/*.{png,jpg}").
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
                let p = Path::new(&expanded);
                let parent = p
                    .parent()
                    .map(|pp| pp.to_string_lossy().to_string())
                    .unwrap_or_else(|| ".".to_string());
                let file_part = p
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_else(|| expanded.clone());
                (file_part, parent)
            } else {
                return ToolResult::error("Error: pattern is required");
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
                "No files found matching \"{}\" in {}",
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

        ToolResult::ok(result)
    }

    fn handle_grep(&self, input: &FileInput) -> ToolResult {
        if input.regex.is_empty() {
            return ToolResult::error("Error: regex is required");
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
            &input.regex,
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

        if entry.file_type().is_dir() {
            continue;
        }

        let path = entry.path();
        if matcher.is_match(path) {
            matches.push(path.to_string_lossy().to_string());
        }
    }

    matches
}

fn relativize_path(path: &str, base: &str) -> String {
    Path::new(path)
        .strip_prefix(base)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string())
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

/// Expand ~ to home directory.
pub fn expand_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return format!("{}/{}", home.display(), rest);
        }
    }
    path.to_string()
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

    // ── File read dedup (same offset/limit, unchanged file) ─────────
    #[test]
    fn file_read_dedup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        fs::write(&path, "line one\nline two\n").unwrap();

        let tool = FileTool::new();
        let input = json!({"action":"read","path": path.to_str().unwrap()});

        let r1 = tool.execute(&ctx(), input.clone());
        assert!(!r1.is_error);
        assert!(r1.content.contains("line one"));

        let r2 = tool.execute(&ctx(), input);
        assert!(!r2.is_error);
        assert!(
            r2.content.contains("unchanged"),
            "expected dedup cache hit: {}",
            r2.content
        );
    }

    // ── File read dedup invalidation on modification ────────────────
    #[test]
    fn file_read_dedup_invalidation() {
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

