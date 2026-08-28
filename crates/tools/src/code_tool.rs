//! CodeTool — read-only code intelligence over compiled-in tree-sitter
//! grammars (`nebo-syntax`, PRD_CODING_HARNESS Pillar 2).
//!
//! Five actions: outline (symbol tree of a file), symbols (project/dir symbol
//! search by name), parse_check (syntax errors), query (structural search),
//! context (enclosing symbol + siblings for a line). READ-ONLY by contract —
//! edits stay on the ONE os file pathway, which calls `syntax::parse_check`
//! itself in its edit-verification chain.

use std::path::Path;

use crate::errors;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};
use serde_json::{Value, json};

/// Display cap for symbol/match lists. Never a silent cut — every capped list
/// ends with a stated "N more omitted" line (CODE_AUDITOR §11).
const MAX_LIST: usize = 200;

/// Files-walked cap for `symbols` — a factual note states when it is hit.
const MAX_WALK_FILES: usize = 2000;

/// Per-file size ceiling for `symbols` walks (outline-parsing a repo of
/// megabyte bundles would stall the call; skipped files are counted).
const MAX_WALK_FILE_BYTES: u64 = 1_000_000;

const SUPPORTED_LANGS: &str =
    "rust, typescript, tsx, javascript, python, go, json, yaml, toml, bash, html, css, markdown";

pub struct CodeTool;

impl CodeTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CodeTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Render an outline as indented factual lines: `kind name  [start-end]`.
/// Capped at `cap` entries with a stated omission count — never a silent cut.
/// ONE renderer, shared with the os file tool's outline-first reads.
pub(crate) fn render_outline(symbols: &[syntax::Symbol], cap: usize) -> String {
    fn walk(
        symbols: &[syntax::Symbol],
        depth: usize,
        cap: usize,
        lines: &mut Vec<String>,
        omitted: &mut usize,
    ) {
        for s in symbols {
            if lines.len() < cap {
                lines.push(format!(
                    "{}{} {}  [{}-{}]",
                    "  ".repeat(depth),
                    s.kind,
                    s.name,
                    s.start_line,
                    s.end_line
                ));
            } else {
                *omitted += 1;
            }
            walk(&s.children, depth + 1, cap, lines, omitted);
        }
    }
    let mut lines = Vec::new();
    let mut omitted = 0usize;
    walk(symbols, 0, cap, &mut lines, &mut omitted);
    if omitted > 0 {
        lines.push(format!("… {omitted} more omitted"));
    }
    lines.join("\n")
}

/// Resolve `path` to an existing readable source file and its language.
fn resolve_source_file(raw: &str, action: &str) -> Result<(String, String, syntax::Lang), ToolResult> {
    if raw.is_empty() {
        return Err(ToolResult::error(errors::missing_param(
            action,
            "path",
            &format!("code(action: \"{action}\", path: \"/path/to/file.rs\")"),
        )));
    }
    let path = match types::pathres::resolve(raw) {
        Ok(p) => p.to_string_lossy().into_owned(),
        Err(e) => return Err(ToolResult::error(format!("Error: {e}"))),
    };
    let lang = match syntax::Lang::from_path(Path::new(&path)) {
        Some(l) => l,
        None => {
            return Err(ToolResult::error(format!(
                "No compiled-in grammar for {path} — supported languages: {SUPPORTED_LANGS}. \
                 Use os(resource: \"file\", action: \"read\"/\"grep\") for other files."
            )));
        }
    };
    let source = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ToolResult::error(errors::file_not_found(&path)));
        }
        Err(e) => return Err(ToolResult::error(format!("Error reading {path}: {e}"))),
    };
    Ok((path, source, lang))
}

fn handle_outline(input: &Value) -> ToolResult {
    let raw = input.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let (path, source, lang) = match resolve_source_file(raw, "outline") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let symbols = match syntax::outline(&source, lang) {
        Ok(s) => s,
        Err(e) => return ToolResult::error(format!("Error parsing {path}: {e}")),
    };
    let total_lines = source.lines().count();
    if symbols.is_empty() {
        return ToolResult::ok(format!(
            "{path} ({}, {total_lines} lines): no outline symbols found.",
            lang.name()
        ));
    }
    ToolResult::ok(format!(
        "{path} ({}, {total_lines} lines):\n{}",
        lang.name(),
        render_outline(&symbols, MAX_LIST)
    ))
}

fn handle_parse_check(input: &Value) -> ToolResult {
    let raw = input.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let (path, source, lang) = match resolve_source_file(raw, "parse_check") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let errors = match syntax::parse_check(&source, lang) {
        Ok(e) => e,
        Err(e) => return ToolResult::error(format!("Error parsing {path}: {e}")),
    };
    if errors.is_empty() {
        return ToolResult::ok(format!("syntax OK ({}) — {path}", lang.name()));
    }
    let mut out = format!(
        "syntax: {} error{} in {path} ({}):",
        errors.len(),
        if errors.len() == 1 { "" } else { "s" },
        lang.name()
    );
    for e in &errors {
        out.push_str(&format!(
            "\n  line {}, col {}: {} — {}",
            e.line, e.col, e.message, e.excerpt
        ));
    }
    ToolResult::ok(out)
}

fn handle_query(input: &Value) -> ToolResult {
    let raw = input.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let ts_query = input.get("query").and_then(|v| v.as_str()).unwrap_or("");
    if ts_query.is_empty() {
        return ToolResult::error(errors::missing_param(
            "query",
            "query",
            "code(action: \"query\", path: \"/src/lib.rs\", query: \"(function_item name: (identifier) @name)\")",
        ));
    }
    let (path, source, lang) = match resolve_source_file(raw, "query") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let hits = match syntax::query(&source, lang, ts_query) {
        Ok(h) => h,
        Err(e) => return ToolResult::error(format!("Error: {e}")),
    };
    if hits.is_empty() {
        return ToolResult::ok(format!(
            "0 matches in {path} ({}). This is not an error — the query matched nothing.",
            lang.name()
        ));
    }
    let limit = list_limit(input);
    let mut out = format!("{} capture{} in {path} ({}):", hits.len(),
        if hits.len() == 1 { "" } else { "s" }, lang.name());
    for h in hits.iter().take(limit) {
        out.push_str(&format!("\n  line {}: @{} ({}) {}", h.line, h.capture, h.kind, h.text));
    }
    if hits.len() > limit {
        out.push_str(&format!("\n… {} more omitted", hits.len() - limit));
    }
    ToolResult::ok(out)
}

fn handle_symbols(input: &Value) -> ToolResult {
    let name = input
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let raw_dir = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    let dir = crate::file_tool::expand_path(raw_dir);
    if !Path::new(&dir).is_dir() {
        return ToolResult::error(format!(
            "Error: {dir} is not a directory. symbols searches a directory tree — \
             for one file use code(action: \"outline\", path: ...)."
        ));
    }

    let limit = list_limit(input);
    let mut rows: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut files_scanned = 0usize;
    let mut files_skipped_size = 0usize;
    let mut walk_capped = false;

    let walker = walkdir::WalkDir::new(&dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            // Never filter the walk ROOT: the user-named directory may
            // legitimately be dot-prefixed (".config", a ".tmpXXXX" tempdir) —
            // filtering it walks zero files and reports an empty result for a
            // directory that has content.
            if e.depth() == 0 {
                return true;
            }
            let n = e.file_name().to_string_lossy();
            // Same skip set as the file tool's glob walker.
            !(e.file_type().is_dir()
                && (n.starts_with('.')
                    || n == "node_modules"
                    || n == "vendor"
                    || n == "__pycache__"
                    || n == "target"))
        });

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let Some(lang) = syntax::Lang::from_path(path) else {
            continue;
        };
        if files_scanned >= MAX_WALK_FILES {
            walk_capped = true;
            break;
        }
        if entry.metadata().is_ok_and(|m| m.len() > MAX_WALK_FILE_BYTES) {
            files_skipped_size += 1;
            continue;
        }
        files_scanned += 1;
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(symbols) = syntax::outline(&source, lang) else {
            continue;
        };
        let display = path
            .strip_prefix(&dir)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_string_lossy().into_owned());
        collect_symbol_rows(&symbols, &name, &display, limit, &mut rows, &mut total);
    }

    let filter_note = if name.is_empty() {
        String::new()
    } else {
        format!(" matching \"{name}\"")
    };
    if total == 0 {
        let mut out = format!(
            "0 symbols{filter_note} in {dir} ({files_scanned} source files scanned). \
             This is not an error — nothing matched."
        );
        append_walk_notes(&mut out, walk_capped, files_skipped_size);
        return ToolResult::ok(out);
    }
    let mut out = format!(
        "{total} symbol{}{filter_note} in {dir} ({files_scanned} source files scanned):\n{}",
        if total == 1 { "" } else { "s" },
        rows.join("\n")
    );
    if total > rows.len() {
        out.push_str(&format!("\n… {} more omitted", total - rows.len()));
    }
    append_walk_notes(&mut out, walk_capped, files_skipped_size);
    ToolResult::ok(out)
}

/// Flatten one file's outline into `file:start-end  kind name` rows, filtered
/// by (case-insensitive substring) `name`. Rows past `limit` are counted in
/// `total` but not rendered — the caller states the omission.
fn collect_symbol_rows(
    symbols: &[syntax::Symbol],
    name: &str,
    display: &str,
    limit: usize,
    rows: &mut Vec<String>,
    total: &mut usize,
) {
    for s in symbols {
        if name.is_empty() || s.name.to_ascii_lowercase().contains(name) {
            *total += 1;
            if rows.len() < limit {
                rows.push(format!(
                    "{display}:{}-{}  {} {}",
                    s.start_line, s.end_line, s.kind, s.name
                ));
            }
        }
        collect_symbol_rows(&s.children, name, display, limit, rows, total);
    }
}

fn append_walk_notes(out: &mut String, walk_capped: bool, skipped_size: usize) {
    if walk_capped {
        out.push_str(&format!(
            "\n(walk stopped at {MAX_WALK_FILES} source files — narrow `path` to search the rest)"
        ));
    }
    if skipped_size > 0 {
        out.push_str(&format!(
            "\n({skipped_size} file{} over 1MB skipped)",
            if skipped_size == 1 { "" } else { "s" }
        ));
    }
}

fn handle_context(input: &Value) -> ToolResult {
    let raw = input.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let line = input.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    if line == 0 {
        return ToolResult::error(errors::missing_param(
            "context",
            "line",
            "code(action: \"context\", path: \"/src/lib.rs\", line: 120)",
        ));
    }
    let end_line = input
        .get("end_line")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(line)
        .max(line);
    let (path, source, lang) = match resolve_source_file(raw, "context") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let symbols = match syntax::outline(&source, lang) {
        Ok(s) => s,
        Err(e) => return ToolResult::error(format!("Error parsing {path}: {e}")),
    };

    // Root→leaf chain of symbols enclosing [line, end_line].
    fn find_chain<'a>(
        symbols: &'a [syntax::Symbol],
        start: usize,
        end: usize,
        chain: &mut Vec<&'a syntax::Symbol>,
    ) {
        for s in symbols {
            if s.start_line <= start && s.end_line >= end {
                chain.push(s);
                find_chain(&s.children, start, end, chain);
                return;
            }
        }
    }
    let mut chain: Vec<&syntax::Symbol> = Vec::new();
    find_chain(&symbols, line, end_line, &mut chain);

    let range_str = if end_line > line {
        format!("lines {line}-{end_line}")
    } else {
        format!("line {line}")
    };
    if chain.is_empty() {
        return ToolResult::ok(format!(
            "{range_str} in {path} ({}): no enclosing symbol. Top-level symbols:\n{}",
            lang.name(),
            render_outline(&symbols, MAX_LIST)
        ));
    }

    let enclosing = chain
        .iter()
        .rev()
        .map(|s| format!("{} {} [{}-{}]", s.kind, s.name, s.start_line, s.end_line))
        .collect::<Vec<_>>()
        .join(" < ");
    // Siblings = the other symbols at the innermost symbol's level.
    let siblings: &[syntax::Symbol] = if chain.len() >= 2 {
        &chain[chain.len() - 2].children
    } else {
        &symbols
    };
    let leaf = chain[chain.len() - 1];
    let sib_rows: Vec<String> = siblings
        .iter()
        .filter(|s| s.start_line != leaf.start_line || s.name != leaf.name)
        .take(MAX_LIST)
        .map(|s| format!("  {} {}  [{}-{}]", s.kind, s.name, s.start_line, s.end_line))
        .collect();
    let mut out = format!(
        "{range_str} in {path} ({}):\nenclosing: {enclosing}",
        lang.name()
    );
    if sib_rows.is_empty() {
        out.push_str("\nno sibling symbols at this level.");
    } else {
        out.push_str(&format!("\nsiblings:\n{}", sib_rows.join("\n")));
    }
    ToolResult::ok(out)
}

fn list_limit(input: &Value) -> usize {
    match input.get("limit").and_then(|v| v.as_u64()) {
        Some(n) if n > 0 => (n as usize).min(MAX_LIST),
        _ => MAX_LIST,
    }
}

impl DynTool for CodeTool {
    fn name(&self) -> &str {
        "code"
    }

    fn description(&self) -> String {
        "Read-only code intelligence via in-process tree-sitter (rust, typescript, tsx, javascript, python, go, json, yaml, toml, bash, html, css, markdown).\n\n\
         Rules:\n\
         - Use outline BEFORE reading a large source file blind — then read specific line ranges with os(resource: \"file\", action: \"read\", offset, limit).\n\
         - This tool never edits. Edit through os(resource: \"file\", action: \"edit\") — its result already appends a syntax check for these languages.\n\
         - Lists are capped at 200 entries; a capped list ends with an explicit \"N more omitted\" line.\n\n\
         Actions:\n\
         - outline: symbol tree of one file (functions/classes/impls/methods with line ranges)\n\
         - symbols: search a directory tree for symbols by name (case-insensitive substring; empty name lists all)\n\
         - parse_check: tree-sitter syntax errors for one file (line, col, message)\n\
         - query: structural search with a tree-sitter query (s-expression) over one file\n\
         - context: the enclosing symbol chain + sibling symbols for a line (edit anchoring)\n\n\
         Examples:\n  \
         code(action: \"outline\", path: \"/src/runner.rs\")\n  \
         code(action: \"symbols\", name: \"handle_read\", path: \"/src\")\n  \
         code(action: \"parse_check\", path: \"/src/lib.rs\")\n  \
         code(action: \"query\", path: \"/src/lib.rs\", query: \"(function_item name: (identifier) @name)\")\n  \
         code(action: \"context\", path: \"/src/lib.rs\", line: 120)"
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["outline", "symbols", "parse_check", "query", "context"],
                    "description": "outline = symbol tree of a file; symbols = search a directory for symbols by name; parse_check = syntax errors; query = tree-sitter structural search; context = enclosing symbol + siblings for a line"
                },
                "path": { "type": "string", "description": "Source file path (outline, parse_check, query, context) or directory to search (symbols; defaults to the working directory)" },
                "name": { "type": "string", "description": "Symbol name filter for symbols — case-insensitive substring; empty lists all symbols" },
                "query": { "type": "string", "description": "Tree-sitter query s-expression for the query action, e.g. (function_item name: (identifier) @name)" },
                "line": { "type": "integer", "description": "1-based line for context" },
                "end_line": { "type": "integer", "description": "Optional 1-based end line for context (defaults to line)" },
                "limit": { "type": "integer", "description": "Max list entries for symbols/query (default and cap: 200)" }
            },
            "required": ["action"]
        })
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn is_concurrent_safe(&self, _input: &Value) -> bool {
        true // every action is read-only
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
            match action {
                "outline" => handle_outline(&input),
                "symbols" => handle_symbols(&input),
                "parse_check" => handle_parse_check(&input),
                "query" => handle_query(&input),
                "context" => handle_context(&input),
                other => ToolResult::error(format!(
                    "Unknown action: {other} (valid: outline, symbols, parse_check, query, context)"
                )),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::origin::Origin;
    use serde_json::json;
    use std::fs;

    fn ctx() -> ToolContext {
        ToolContext::new(Origin::User)
    }

    async fn run(input: Value) -> ToolResult {
        CodeTool::new().execute_dyn(&ctx(), input).await
    }

    const RUST_SRC: &str = "struct Foo;\n\nimpl Foo {\n    fn bar(&self) {}\n    fn baz(&self) {}\n}\n\nfn free() {}\n";

    /// outline returns the file's symbol tree with kinds, nesting, and
    /// 1-based line ranges.
    #[tokio::test]
    async fn outline_action_lists_symbols_with_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.rs");
        fs::write(&path, RUST_SRC).unwrap();
        let r = run(json!({"action": "outline", "path": path.to_str().unwrap()})).await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("struct Foo  [1-1]"), "{}", r.content);
        assert!(r.content.contains("impl Foo  [3-6]"), "{}", r.content);
        assert!(r.content.contains("  fn bar  [4-4]"), "methods indent under impl: {}", r.content);
        assert!(r.content.contains("fn free"), "{}", r.content);
    }

    /// parse_check reports "syntax OK" for clean source and positioned
    /// errors for broken source — never silence.
    #[tokio::test]
    async fn parse_check_action_ok_and_errors() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good.rs");
        fs::write(&good, "fn main() {}\n").unwrap();
        let r = run(json!({"action": "parse_check", "path": good.to_str().unwrap()})).await;
        assert!(!r.is_error && r.content.starts_with("syntax OK (rust)"), "{}", r.content);

        let bad = dir.path().join("bad.rs");
        fs::write(&bad, "fn main() {\n    let x = ;\n}\n").unwrap();
        let r = run(json!({"action": "parse_check", "path": bad.to_str().unwrap()})).await;
        assert!(!r.is_error, "reporting errors is a successful check: {}", r.content);
        assert!(r.content.contains("syntax: 1 error"), "{}", r.content);
        assert!(r.content.contains("line 2"), "{}", r.content);
    }

    /// query runs a tree-sitter query and reports each capture with its line.
    #[tokio::test]
    async fn query_action_returns_captures() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("q.rs");
        fs::write(&path, "fn alpha() {}\nfn beta() {}\n").unwrap();
        let r = run(json!({
            "action": "query",
            "path": path.to_str().unwrap(),
            "query": "(function_item name: (identifier) @name)"
        }))
        .await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("2 captures"), "{}", r.content);
        assert!(r.content.contains("line 1: @name (identifier) alpha"), "{}", r.content);
        // An invalid query surfaces the tree-sitter error, not a false "0 matches".
        let r = run(json!({
            "action": "query",
            "path": path.to_str().unwrap(),
            "query": "(function_item"
        }))
        .await;
        assert!(r.is_error && r.content.contains("query"), "{}", r.content);
    }

    /// symbols walks a directory tree, matches by case-insensitive substring,
    /// and names the file + line range for each hit.
    #[tokio::test]
    async fn symbols_action_searches_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("a.rs"), "fn find_me() {}\nfn other() {}\n").unwrap();
        fs::write(dir.path().join("sub/b.py"), "def find_me_too():\n    pass\n").unwrap();
        fs::write(dir.path().join("c.txt"), "fn find_me_not() {}\n").unwrap();
        let r = run(json!({
            "action": "symbols",
            "name": "Find_Me",
            "path": dir.path().to_str().unwrap()
        }))
        .await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("2 symbols"), "{}", r.content);
        assert!(r.content.contains("a.rs:1-1  fn find_me"), "{}", r.content);
        assert!(r.content.contains("b.py:1-2  fn find_me_too"), "{}", r.content);
        assert!(!r.content.contains("find_me_not"), "no grammar for .txt: {}", r.content);
    }

    /// A symbol list past the cap ends with an explicit "N more omitted"
    /// line — never a silent cut (CODE_AUDITOR §11).
    #[tokio::test]
    async fn symbols_list_caps_with_stated_omission() {
        let dir = tempfile::tempdir().unwrap();
        let mut src = String::new();
        for i in 0..230 {
            src.push_str(&format!("fn f{i}() {{}}\n"));
        }
        fs::write(dir.path().join("many.rs"), &src).unwrap();
        let r = run(json!({"action": "symbols", "path": dir.path().to_str().unwrap()})).await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("230 symbols"), "{}", r.content);
        assert!(
            r.content.contains("… 30 more omitted"),
            "cap must be stated, not silent: {}",
            r.content
        );
    }

    /// context names the enclosing symbol chain and the siblings at that level.
    #[tokio::test]
    async fn context_action_reports_enclosing_and_siblings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.rs");
        fs::write(&path, RUST_SRC).unwrap();
        // Line 4 is inside fn bar, inside impl Foo.
        let r = run(json!({"action": "context", "path": path.to_str().unwrap(), "line": 4})).await;
        assert!(!r.is_error, "{}", r.content);
        assert!(
            r.content.contains("enclosing: fn bar [4-4] < impl Foo [3-6]"),
            "{}",
            r.content
        );
        assert!(r.content.contains("fn baz"), "sibling listed: {}", r.content);
        // A line outside any symbol states that and falls back to top-level.
        let r = run(json!({"action": "context", "path": path.to_str().unwrap(), "line": 2})).await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("no enclosing symbol"), "{}", r.content);
    }

    /// A file with no compiled-in grammar is an honest refusal that names the
    /// supported languages — not a guess.
    #[tokio::test]
    async fn no_grammar_extension_is_a_named_refusal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        fs::write(&path, "hello\n").unwrap();
        let r = run(json!({"action": "outline", "path": path.to_str().unwrap()})).await;
        assert!(r.is_error, "{}", r.content);
        assert!(r.content.contains("No compiled-in grammar"), "{}", r.content);
        assert!(r.content.contains("rust"), "{}", r.content);
    }

    /// Unknown actions list the valid ones.
    #[tokio::test]
    async fn unknown_action_lists_valid_actions() {
        let r = run(json!({"action": "rename"})).await;
        assert!(r.is_error);
        assert!(r.content.contains("outline, symbols, parse_check, query, context"));
    }

    /// Every action is read-only and concurrent-safe; none needs approval.
    #[test]
    fn code_tool_is_read_only_tier() {
        let tool = CodeTool::new();
        assert!(!tool.requires_approval());
        for action in ["outline", "symbols", "parse_check", "query", "context"] {
            assert!(tool.is_concurrent_safe(&json!({"action": action})), "{action}");
            assert!(!tool.requires_approval_for(&json!({"action": action})), "{action}");
        }
    }
}
