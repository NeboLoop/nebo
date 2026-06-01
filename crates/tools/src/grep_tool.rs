use std::io;
use std::path::Path;

use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::Lossy;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};

use crate::errors;
use crate::registry::ToolResult;

// ── Dangerous path deny list ───────────────────────────────────────

const DANGEROUS_PATHS: &[&str] = &[
    "/",
    "/usr",
    "/var",
    "/etc",
    "/System",
    "/Library",
    "/Applications",
    "/bin",
    "/sbin",
    "/opt",
];

fn is_dangerous_path(path: &str) -> bool {
    if let Ok(abs) = std::path::absolute(Path::new(path)) {
        let abs_str = abs.to_string_lossy();
        DANGEROUS_PATHS.iter().any(|d| abs_str.as_ref() == *d)
    } else {
        false
    }
}

// ── GrepTool ───────────────────────────────────────────────────────

/// GrepTool searches for patterns in files using the grep-searcher crate.
pub struct GrepTool;

impl GrepTool {
    pub fn new() -> Self {
        Self
    }

    #[allow(clippy::too_many_arguments)]
    pub fn execute_search(
        &self,
        pattern: &str,
        path: &str,
        file_glob: Option<&str>,
        case_insensitive: bool,
        limit: usize,
        offset: usize,
        output_mode: &str,
        context_before: usize,
        context_after: usize,
    ) -> ToolResult {
        if is_dangerous_path(path) {
            return ToolResult::error(format!(
                "Error: Cannot search '{}' - path is too broad. Please specify a more specific directory.",
                path
            ));
        }

        let limit = if limit == 0 { 250 } else { limit };

        let matcher = match RegexMatcherBuilder::new()
            .case_insensitive(case_insensitive)
            .build(pattern)
        {
            Ok(m) => m,
            Err(e) => return ToolResult::error(format!("Invalid regex pattern: {}", e)),
        };

        let p = Path::new(path);
        let files = if p.is_dir() {
            find_files(path, file_glob)
        } else if p.is_file() {
            vec![path.to_string()]
        } else {
            return ToolResult::error(errors::path_not_found(path));
        };

        if files.is_empty() {
            return ToolResult::ok(errors::no_grep_results(pattern, path));
        }

        // Resolve the search base for relative path display.
        let search_base = if p.is_dir() {
            std::path::absolute(p)
                .unwrap_or_else(|_| p.to_path_buf())
        } else {
            std::path::absolute(p.parent().unwrap_or(Path::new(".")))
                .unwrap_or_else(|_| p.parent().unwrap_or(Path::new(".")).to_path_buf())
        };

        match output_mode {
            "files" => self.search_files_mode(&matcher, &files, &search_base, pattern, limit, offset),
            "count" => self.search_count_mode(&matcher, &files, &search_base, pattern, limit, offset),
            _ => self.search_content_mode(
                &matcher,
                &files,
                &search_base,
                pattern,
                limit,
                offset,
                context_before,
                context_after,
            ),
        }
    }

    // ── content mode ───────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    fn search_content_mode(
        &self,
        matcher: &grep_regex::RegexMatcher,
        files: &[String],
        search_base: &Path,
        pattern: &str,
        limit: usize,
        offset: usize,
        context_before: usize,
        context_after: usize,
    ) -> ToolResult {
        let mut searcher = SearcherBuilder::new()
            .before_context(context_before)
            .after_context(context_after)
            .line_number(true)
            .binary_detection(BinaryDetection::quit(0))
            .build();

        let mut all_lines: Vec<String> = Vec::new();
        let mut match_count: usize = 0;
        let mut file_count: usize = 0;
        // Track how many raw match lines (not context) we have collected so
        // we can stop early once we have enough after applying offset.
        let collected_enough = |mc: usize| mc >= offset + limit;

        for file_path in files {
            if collected_enough(match_count) {
                break;
            }

            let rel = relativize(file_path, search_base);

            let mut file_lines: Vec<ContentLine> = Vec::new();
            let mut file_had_match = false;

            let mut sink = ContentSink {
                lines: &mut file_lines,
                rel_path: &rel,
            };

            // Errors (permission denied, binary quit, etc.) are silently skipped.
            let _ = searcher.search_path(matcher, file_path, &mut sink);

            if file_lines.iter().any(|l| l.is_match) {
                file_had_match = true;
            }

            if file_had_match {
                file_count += 1;
                // Count actual match lines in this file.
                let file_matches: usize = file_lines.iter().filter(|l| l.is_match).count();
                match_count += file_matches;

                for cl in &file_lines {
                    all_lines.push(cl.formatted.clone());
                }
            }
        }

        if match_count == 0 {
            return ToolResult::ok(errors::no_grep_results(pattern, &search_base.display().to_string()));
        }

        // Apply offset/limit on the formatted output lines. We need to
        // figure out which *match* lines correspond to offset..offset+limit
        // and include their surrounding context.
        let paginated = paginate_content_lines(&all_lines, offset, limit);
        let shown = paginated.len();
        let truncated = match_count > offset + limit;

        let mut result = format!("Found {} matches in {} files", match_count, file_count);
        if shown < match_count {
            result.push_str(&format!(" (showing {})", shown));
        }
        result.push_str("\n\n");
        result.push_str(&paginated.join("\n"));

        if truncated {
            result.push_str(&format!(
                "\n(Results truncated. Use offset={} to see more.)",
                offset + limit
            ));
        }

        ToolResult::ok(result)
    }

    // ── files mode ─────────────────────────────────────────────────

    fn search_files_mode(
        &self,
        matcher: &grep_regex::RegexMatcher,
        files: &[String],
        search_base: &Path,
        pattern: &str,
        limit: usize,
        offset: usize,
    ) -> ToolResult {
        let mut searcher = SearcherBuilder::new()
            .line_number(true)
            .binary_detection(BinaryDetection::quit(0))
            .build();

        // Collect (path, mtime) for files that have at least one match.
        let mut matched_files: Vec<(String, std::time::SystemTime)> = Vec::new();

        for file_path in files {
            let mut has_match = false;
            let _ = searcher.search_path(
                matcher,
                file_path,
                Lossy(|_line_num, _line| {
                    has_match = true;
                    // Stop after first match — we only need to know the file matches.
                    Ok(false)
                }),
            );

            if has_match {
                let mtime = std::fs::metadata(file_path)
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                matched_files.push((file_path.clone(), mtime));
            }
        }

        if matched_files.is_empty() {
            return ToolResult::ok(errors::no_grep_results(pattern, &search_base.display().to_string()));
        }

        // Sort by mtime descending (newest first).
        matched_files.sort_by(|a, b| b.1.cmp(&a.1));

        let total = matched_files.len();
        let after_offset: Vec<_> = matched_files.into_iter().skip(offset).collect();
        let page: Vec<_> = after_offset.iter().take(limit).collect();
        let truncated = after_offset.len() > limit;

        let mut result = format!("Found {} files matching \"{}\"\n\n", total, pattern);
        for (fp, _) in &page {
            result.push_str(&relativize(fp, search_base));
            result.push('\n');
        }

        if truncated {
            result.push_str(&format!(
                "(Results truncated. Use offset={} to see more.)",
                offset + limit
            ));
        }

        ToolResult::ok(result.trim_end())
    }

    // ── count mode ─────────────────────────────────────────────────

    fn search_count_mode(
        &self,
        matcher: &grep_regex::RegexMatcher,
        files: &[String],
        search_base: &Path,
        pattern: &str,
        limit: usize,
        offset: usize,
    ) -> ToolResult {
        let mut searcher = SearcherBuilder::new()
            .line_number(true)
            .binary_detection(BinaryDetection::quit(0))
            .build();

        let mut counts: Vec<(String, usize)> = Vec::new();
        let mut total_matches: usize = 0;

        for file_path in files {
            let mut count: usize = 0;
            let _ = searcher.search_path(
                matcher,
                file_path,
                Lossy(|_line_num, _line| {
                    count += 1;
                    Ok(true)
                }),
            );

            if count > 0 {
                total_matches += count;
                let rel = relativize(file_path, search_base);
                counts.push((rel, count));
            }
        }

        if counts.is_empty() {
            return ToolResult::ok(errors::no_grep_results(pattern, &search_base.display().to_string()));
        }

        let total_files = counts.len();
        let after_offset: Vec<_> = counts.into_iter().skip(offset).collect();
        let page: Vec<_> = after_offset.iter().take(limit).collect();
        let truncated = after_offset.len() > limit;

        let mut result = format!(
            "Found {} matches across {} files\n\n",
            total_matches, total_files
        );
        for (rel, c) in &page {
            result.push_str(&format!("{}: {}\n", rel, c));
        }

        if truncated {
            result.push_str(&format!(
                "(Results truncated. Use offset={} to see more.)",
                offset + limit
            ));
        }

        ToolResult::ok(result.trim_end())
    }
}

impl Default for GrepTool {
    fn default() -> Self {
        Self::new()
    }
}

// ── Content sink ───────────────────────────────────────────────────

/// A single output line from a content-mode search: either a match line
/// or a context line (before/after) or a context-break separator.
struct ContentLine {
    formatted: String,
    is_match: bool,
}

struct ContentSink<'a> {
    lines: &'a mut Vec<ContentLine>,
    rel_path: &'a str,
}

impl Sink for ContentSink<'_> {
    type Error = io::Error;

    fn matched(
        &mut self,
        _searcher: &Searcher,
        mat: &SinkMatch<'_>,
    ) -> Result<bool, Self::Error> {
        let line_num = mat.line_number().unwrap_or(0);
        let text = String::from_utf8_lossy(mat.bytes());
        let text = text.trim_end_matches('\n').trim_end_matches('\r');
        let content = if text.len() > 500 {
            format!("{}...", crate::truncate_str(text, 500))
        } else {
            text.to_string()
        };
        self.lines.push(ContentLine {
            formatted: format!("{}:{}:{}", self.rel_path, line_num, content),
            is_match: true,
        });
        Ok(true)
    }

    fn context(
        &mut self,
        _searcher: &Searcher,
        ctx: &SinkContext<'_>,
    ) -> Result<bool, Self::Error> {
        let line_num = ctx.line_number().unwrap_or(0);
        let text = String::from_utf8_lossy(ctx.bytes());
        let text = text.trim_end_matches('\n').trim_end_matches('\r');
        let content = if text.len() > 500 {
            format!("{}...", crate::truncate_str(text, 500))
        } else {
            text.to_string()
        };
        self.lines.push(ContentLine {
            formatted: format!("{}:{}-{}", self.rel_path, line_num, content),
            is_match: false,
        });
        Ok(true)
    }

    fn context_break(&mut self, _searcher: &Searcher) -> Result<bool, Self::Error> {
        self.lines.push(ContentLine {
            formatted: "--".to_string(),
            is_match: false,
        });
        Ok(true)
    }
}

// ── Helpers ────────────────────────────────────────────────────────

/// Paginate content lines (match + context) by match-line offset/limit.
/// Returns the slice of formatted lines that belong to the matches in
/// [offset .. offset+limit].
fn paginate_content_lines(lines: &[String], offset: usize, limit: usize) -> Vec<String> {
    if offset == 0 && limit >= lines.len() {
        return lines.to_vec();
    }

    // We need to identify which output lines are "match" lines
    // (contain `:digits:` after the path, NOT `-digits-` context lines)
    // and keep context lines that are adjacent to included match lines.
    //
    // Simple approach: walk lines, track match index, include lines
    // whose nearest match is within [offset, offset+limit).
    let mut match_idx: usize = 0;
    let mut result = Vec::new();

    for line in lines {
        // Context break or context lines don't increment match_idx.
        let is_match_line = !line.starts_with("--") && !is_context_line(line);
        if is_match_line {
            if match_idx >= offset && match_idx < offset + limit {
                result.push(line.clone());
            }
            match_idx += 1;
        } else if match_idx > offset && match_idx <= offset + limit {
            // Include context that follows an included match.
            result.push(line.clone());
        } else if match_idx >= offset && match_idx < offset + limit {
            // Include context that precedes an included match (before_context).
            result.push(line.clone());
        }
    }

    result
}

/// Heuristic: context lines use `-` as the separator between line number and
/// content (e.g. `file:42-content`), while match lines use `:` (e.g.
/// `file:42:content`).
fn is_context_line(line: &str) -> bool {
    // Look for the pattern `path:number-` which indicates a context line.
    // Match lines have `path:number:`.
    if let Some(first_colon) = line.find(':') {
        let rest = &line[first_colon + 1..];
        // Find the end of the digits.
        let digit_end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(0);
        if digit_end > 0 {
            let after_digits = rest.as_bytes().get(digit_end);
            return after_digits == Some(&b'-');
        }
    }
    false
}

fn relativize(abs_path: &str, search_base: &Path) -> String {
    Path::new(abs_path)
        .strip_prefix(search_base)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| abs_path.to_string())
}

fn find_files(dir: &str, file_glob: Option<&str>) -> Vec<String> {
    let mut files = Vec::new();
    let binary_exts = [
        ".exe", ".bin", ".so", ".dylib", ".png", ".jpg", ".gif", ".ico", ".zip", ".tar", ".gz",
    ];

    let glob_matcher = file_glob.and_then(|g| {
        globset::GlobBuilder::new(g)
            .literal_separator(false)
            .build()
            .ok()
            .map(|glob| glob.compile_matcher())
    });

    let walker = walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            if e.file_type().is_dir() {
                if name.starts_with('.') && name != "." {
                    return false;
                }
                if name == "node_modules" || name == "vendor" || name == "__pycache__" {
                    return false;
                }
            }
            true
        });

    for entry in walker {
        if files.len() >= 10000 {
            break;
        }

        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        if entry.file_type().is_dir() {
            continue;
        }

        let path_str = entry.path().to_string_lossy().to_string();

        // Skip binary files
        if let Some(ext) = entry.path().extension() {
            let ext_str = format!(".{}", ext.to_string_lossy());
            if binary_exts.contains(&ext_str.as_str()) {
                continue;
            }
        }

        // Check glob filter
        if let Some(ref gm) = glob_matcher {
            let file_name = entry.file_name().to_string_lossy();
            if !gm.is_match(file_name.as_ref()) {
                continue;
            }
        }

        files.push(path_str);
    }

    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Create a non-hidden subdirectory inside the temp dir so that
    /// `find_files`'s walker doesn't skip it (macOS tempdir names
    /// start with `.tmp` which triggers the hidden-dir filter).
    fn test_dir() -> (tempfile::TempDir, std::path::PathBuf) {
        let td = tempdir().unwrap();
        let work = td.path().join("work");
        fs::create_dir(&work).unwrap();
        (td, work)
    }

    fn search(
        tool: &GrepTool,
        pattern: &str,
        path: &str,
        file_glob: Option<&str>,
        case_insensitive: bool,
        limit: usize,
        offset: usize,
        output_mode: &str,
        ctx_before: usize,
        ctx_after: usize,
    ) -> ToolResult {
        tool.execute_search(pattern, path, file_glob, case_insensitive, limit, offset, output_mode, ctx_before, ctx_after)
    }

    #[test]
    fn test_basic_search() {
        let (_td, work) = test_dir();
        fs::write(work.join("hello.txt"), "alpha\nbeta\ngamma\nalpha again\n").unwrap();

        let tool = GrepTool::new();
        let res = search(&tool, "alpha", work.to_str().unwrap(), None, false, 0, 0, "content", 0, 0);

        assert!(!res.is_error);
        assert!(res.content.contains("alpha"));
        assert!(res.content.contains("alpha again"));
        assert!(res.content.contains("Found 2 matches"));
    }

    #[test]
    fn test_brace_expansion_in_file_glob() {
        let (_td, work) = test_dir();
        fs::write(work.join("lib.rs"), "needle in rust\n").unwrap();
        fs::write(work.join("config.toml"), "needle in toml\n").unwrap();
        fs::write(work.join("readme.md"), "needle in markdown\n").unwrap();

        let tool = GrepTool::new();
        let res = search(&tool, "needle", work.to_str().unwrap(), Some("*.{rs,toml}"), false, 0, 0, "content", 0, 0);

        assert!(!res.is_error);
        assert!(res.content.contains("lib.rs"));
        assert!(res.content.contains("config.toml"));
        assert!(!res.content.contains("readme.md"));
    }

    #[test]
    fn test_case_insensitive() {
        let (_td, work) = test_dir();
        fs::write(work.join("mixed.txt"), "Hello\nhello\nHELLO\nworld\n").unwrap();

        let tool = GrepTool::new();
        let res = search(&tool, "hello", work.to_str().unwrap(), None, true, 0, 0, "content", 0, 0);

        assert!(!res.is_error);
        assert!(res.content.contains("Found 3 matches"));
    }

    #[test]
    fn test_files_mode() {
        let (_td, work) = test_dir();
        fs::write(work.join("a.txt"), "findme\n").unwrap();
        fs::write(work.join("b.txt"), "findme\n").unwrap();
        fs::write(work.join("c.txt"), "nothing\n").unwrap();

        let tool = GrepTool::new();
        let res = search(&tool, "findme", work.to_str().unwrap(), None, false, 0, 0, "files", 0, 0);

        assert!(!res.is_error);
        assert!(res.content.contains("a.txt"));
        assert!(res.content.contains("b.txt"));
        assert!(!res.content.contains("c.txt"));
        assert!(res.content.contains("Found 2 files"));
    }

    #[test]
    fn test_count_mode() {
        let (_td, work) = test_dir();
        fs::write(work.join("a.txt"), "x\nx\nx\n").unwrap();
        fs::write(work.join("b.txt"), "x\ny\n").unwrap();

        let tool = GrepTool::new();
        let res = search(&tool, "x", work.to_str().unwrap(), None, false, 0, 0, "count", 0, 0);

        assert!(!res.is_error);
        assert!(res.content.contains("4 matches"));
        assert!(res.content.contains("2 files"));
        assert!(res.content.contains(": 3"));
        assert!(res.content.contains(": 1"));
    }

    #[test]
    fn test_context_lines() {
        let (_td, work) = test_dir();
        fs::write(work.join("ctx.txt"), "line1\nline2\nTARGET\nline4\nline5\n").unwrap();

        let tool = GrepTool::new();
        let res = search(&tool, "TARGET", work.to_str().unwrap(), None, false, 0, 0, "content", 1, 1);

        assert!(!res.is_error);
        assert!(res.content.contains("TARGET"));
        assert!(res.content.contains("line2"));
        assert!(res.content.contains("line4"));
    }

    #[test]
    fn test_offset_pagination() {
        let (_td, work) = test_dir();
        let content: String = (0..20).map(|i| format!("match_{}\n", i)).collect();
        fs::write(work.join("many.txt"), &content).unwrap();

        let tool = GrepTool::new();
        let res1 = search(&tool, "match_", work.to_str().unwrap(), None, false, 5, 0, "content", 0, 0);
        assert!(!res1.is_error);
        assert!(res1.content.contains("showing 5"));

        let res2 = search(&tool, "match_", work.to_str().unwrap(), None, false, 5, 5, "content", 0, 0);
        assert!(!res2.is_error);
        assert!(res2.content.contains("showing 5"));
    }

    #[test]
    fn test_dangerous_path_blocked() {
        let tool = GrepTool::new();
        let res = search(&tool, "anything", "/", None, false, 0, 0, "content", 0, 0);

        assert!(res.is_error);
        assert!(res.content.contains("too broad"));
    }

    #[test]
    fn test_no_matches() {
        let (_td, work) = test_dir();
        fs::write(work.join("file.txt"), "hello world\n").unwrap();

        let tool = GrepTool::new();
        let res = search(&tool, "zzz_nonexistent_zzz", work.to_str().unwrap(), None, false, 0, 0, "content", 0, 0);

        assert!(!res.is_error);
        assert!(res.content.contains("No matches found"));
    }

    #[test]
    fn test_invalid_regex() {
        let (_td, work) = test_dir();
        fs::write(work.join("file.txt"), "hello\n").unwrap();

        let tool = GrepTool::new();
        let res = search(&tool, "[invalid(", work.to_str().unwrap(), None, false, 0, 0, "content", 0, 0);

        assert!(res.is_error);
        assert!(res.content.contains("Invalid regex"));
    }

    #[test]
    fn test_relativize_paths() {
        let (_td, work) = test_dir();
        let sub = work.join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("deep.txt"), "target_word\n").unwrap();

        let tool = GrepTool::new();
        let res = search(&tool, "target_word", work.to_str().unwrap(), None, false, 0, 0, "content", 0, 0);

        assert!(!res.is_error);
        assert!(res.content.contains("sub/deep.txt"));
    }

    #[test]
    fn test_is_context_line() {
        assert!(!is_context_line("file.rs:42:fn main()"));
        assert!(is_context_line("file.rs:42-    let x = 1;"));
        assert!(!is_context_line("--"));
        assert!(!is_context_line("no_line_number"));
    }
}
