use serde::Deserialize;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use crate::origin::ToolContext;
use crate::registry::ToolResult;

/// File operations: read, write, edit, glob, grep.
pub struct FileTool {
    _rg_path: Option<String>,
    pub on_file_read: Option<Box<dyn Fn(&str) + Send + Sync>>,
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
    _context: i64,
}

impl FileTool {
    pub fn new() -> Self {
        Self {
            _rg_path: find_ripgrep(),
            on_file_read: None,
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

        let file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(e) => return ToolResult::error(format!("Error opening file: {}", e)),
        };

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

        if let Some(ref callback) = self.on_file_read {
            callback(&path);
        }

        ToolResult::ok(result)
    }

    fn handle_write(&self, input: &FileInput) -> ToolResult {
        if input.path.is_empty() {
            return ToolResult::error("Error: path is required");
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
                ToolResult::ok(format!("{} {} bytes to {}", action, input.content.len(), path))
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
        if input.pattern.is_empty() {
            return ToolResult::error("Error: pattern is required");
        }

        let base_path = if input.path.is_empty() {
            ".".to_string()
        } else {
            expand_path(&input.path)
        };

        let limit = if input.limit <= 0 { 1000 } else { input.limit } as usize;

        let matches = if input.pattern.contains("**") {
            recursive_glob(&base_path, &input.pattern, limit)
        } else {
            let full_pattern = PathBuf::from(&base_path).join(&input.pattern);
            glob::glob(&full_pattern.to_string_lossy())
                .map(|paths| {
                    paths
                        .filter_map(|r| r.ok())
                        .filter(|p| p.is_file())
                        .map(|p| p.to_string_lossy().to_string())
                        .take(limit)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };

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

        if files_with_time.len() > limit {
            files_with_time.truncate(limit);
        }

        if files_with_time.is_empty() {
            return ToolResult::ok(format!(
                "No files found matching pattern: {}",
                input.pattern
            ));
        }

        let result: String = files_with_time
            .iter()
            .map(|(p, _)| p.as_str())
            .collect::<Vec<_>>()
            .join("\n");

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

        let limit = if input.limit <= 0 { 100 } else { input.limit } as usize;

        let grep = crate::grep_tool::GrepTool::new();
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
        )
    }
}

impl Default for FileTool {
    fn default() -> Self {
        Self::new()
    }
}

fn recursive_glob(base_path: &str, pattern: &str, limit: usize) -> Vec<String> {
    let parts: Vec<&str> = pattern.splitn(2, "**").collect();
    if parts.len() != 2 {
        return glob::glob(&PathBuf::from(base_path).join(pattern).to_string_lossy())
            .map(|paths| {
                paths
                    .filter_map(|r| r.ok())
                    .map(|p| p.to_string_lossy().to_string())
                    .take(limit)
                    .collect()
            })
            .unwrap_or_default();
    }

    let prefix = parts[0].trim_end_matches('/');
    let suffix = parts[1].trim_start_matches('/');

    let search_path = if prefix.is_empty() {
        PathBuf::from(base_path)
    } else {
        PathBuf::from(base_path).join(prefix)
    };

    let mut matches = Vec::new();

    let walker = walkdir::WalkDir::new(&search_path)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            if e.file_type().is_dir() {
                // Skip hidden dirs and common non-source dirs
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
        if matches.len() >= limit {
            break;
        }

        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        if entry.file_type().is_dir() {
            continue;
        }

        if !suffix.is_empty() {
            let file_name = entry.file_name().to_string_lossy();
            let matched = glob::Pattern::new(suffix)
                .map(|p| p.matches(&file_name))
                .unwrap_or(false);

            if !matched {
                // Try matching against relative path
                if let Ok(rel) = entry.path().strip_prefix(&search_path) {
                    let rel_str = rel.to_string_lossy();
                    let matched = glob::Pattern::new(suffix)
                        .map(|p| p.matches(&rel_str))
                        .unwrap_or(false);
                    if !matched {
                        continue;
                    }
                } else {
                    continue;
                }
            }
        }

        matches.push(entry.path().to_string_lossy().to_string());
    }

    matches
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

/// Validate that a file path is safe to access.
fn validate_file_path(raw_path: &str, action: &str) -> Result<(), String> {
    let expanded = expand_path(raw_path);
    let abs_path = std::path::absolute(Path::new(&expanded))
        .map_err(|e| format!("invalid path: {}", e))?;
    let abs_str = abs_path.to_string_lossy().to_string();

    // Also resolve symlinks
    let real_path = std::fs::canonicalize(&abs_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| abs_str.clone());

    for sensitive in sensitive_paths() {
        if path_matches_or_inside(&abs_str, &sensitive) || path_matches_or_inside(&real_path, &sensitive) {
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

/// Find ripgrep binary on the system.
fn find_ripgrep() -> Option<String> {
    which::which("rg").ok().map(|p| p.to_string_lossy().to_string())
}
