use std::io::BufRead;
use std::path::Path;
use std::process::Command;

use crate::registry::ToolResult;

/// GrepTool searches for patterns in files using ripgrep or pure Rust fallback.
pub struct GrepTool {
    rg_path: Option<String>,
}

impl GrepTool {
    pub fn new() -> Self {
        Self {
            rg_path: which::which("rg")
                .ok()
                .map(|p| p.to_string_lossy().to_string()),
        }
    }

    pub fn execute_search(
        &self,
        pattern: &str,
        path: &str,
        file_glob: Option<&str>,
        case_insensitive: bool,
        limit: usize,
    ) -> ToolResult {
        // Block dangerous root paths
        if let Ok(abs) = std::path::absolute(Path::new(path)) {
            let abs_str = abs.to_string_lossy();
            let dangerous = [
                "/", "/usr", "/var", "/etc", "/System", "/Library",
                "/Applications", "/bin", "/sbin", "/opt",
            ];
            for d in &dangerous {
                if abs_str.as_ref() == *d {
                    return ToolResult::error(format!(
                        "Error: Cannot search '{}' - path is too broad. Please specify a more specific directory.",
                        path
                    ));
                }
            }
        }

        if let Some(ref rg) = self.rg_path {
            self.execute_with_ripgrep(rg, pattern, path, file_glob, case_insensitive, limit)
        } else {
            self.execute_with_rust(pattern, path, file_glob, case_insensitive, limit)
        }
    }

    fn execute_with_ripgrep(
        &self,
        rg_path: &str,
        pattern: &str,
        path: &str,
        file_glob: Option<&str>,
        case_insensitive: bool,
        limit: usize,
    ) -> ToolResult {
        let mut args = vec![
            "--line-number".to_string(),
            "--no-heading".to_string(),
            "--color=never".to_string(),
            format!("--max-count={}", limit),
        ];

        if case_insensitive {
            args.push("-i".to_string());
        }

        if let Some(g) = file_glob {
            args.push("--glob".to_string());
            args.push(g.to_string());
        }

        args.push(pattern.to_string());
        args.push(path.to_string());

        let mut rg_cmd = Command::new(rg_path);
        rg_cmd.args(&args);
        crate::process::hide_window_std(&mut rg_cmd);
        let output = match rg_cmd.output() {
            Ok(o) => o,
            Err(e) => return ToolResult::error(format!("Error running ripgrep: {}", e)),
        };

        // rg returns exit code 1 when no matches found
        if !output.status.success() && output.status.code() == Some(1) {
            return ToolResult::ok(format!("No matches found for pattern: {}", pattern));
        }

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.is_empty() {
                return ToolResult::error(format!("Error: {}", stderr.trim()));
            }
            return ToolResult::error(format!(
                "Error running search: exit code {:?}",
                output.status.code()
            ));
        }

        let result = String::from_utf8_lossy(&output.stdout);
        let result = result.trim();

        if result.is_empty() {
            return ToolResult::ok(format!("No matches found for pattern: {}", pattern));
        }

        // Truncate if too many lines
        let lines: Vec<&str> = result.lines().collect();
        if lines.len() > limit {
            let truncated = lines[..limit].join("\n");
            return ToolResult::ok(format!(
                "{}\n... (showing first {} matches)",
                truncated, limit
            ));
        }

        ToolResult::ok(result)
    }

    fn execute_with_rust(
        &self,
        pattern: &str,
        path: &str,
        file_glob: Option<&str>,
        case_insensitive: bool,
        limit: usize,
    ) -> ToolResult {
        let regex_pattern = if case_insensitive {
            format!("(?i){}", pattern)
        } else {
            pattern.to_string()
        };

        let re = match regex::Regex::new(&regex_pattern) {
            Ok(r) => r,
            Err(e) => return ToolResult::error(format!("Invalid regex pattern: {}", e)),
        };

        let p = Path::new(path);
        let files = if p.is_dir() {
            find_files(path, file_glob)
        } else if p.is_file() {
            vec![path.to_string()]
        } else {
            return ToolResult::error(format!("Error: {} not found", path));
        };

        let mut matches = Vec::new();

        for file_path in &files {
            if matches.len() >= limit {
                break;
            }

            let remaining = limit - matches.len();
            if let Ok(file_matches) = search_file(file_path, &re, remaining) {
                matches.extend(file_matches);
            }
        }

        if matches.is_empty() {
            return ToolResult::ok(format!("No matches found for pattern: {}", pattern));
        }

        let mut result = String::new();
        for m in &matches {
            result.push_str(&format!("{}:{}:{}\n", m.file, m.line, m.content));
        }

        if matches.len() >= limit {
            result.push_str(&format!("\n... (showing first {} matches)", limit));
        }

        ToolResult::ok(result.trim())
    }
}

impl Default for GrepTool {
    fn default() -> Self {
        Self::new()
    }
}

struct GrepMatch {
    file: String,
    line: usize,
    content: String,
}

fn find_files(dir: &str, file_glob: Option<&str>) -> Vec<String> {
    let mut files = Vec::new();
    let binary_exts = [
        ".exe", ".bin", ".so", ".dylib", ".png", ".jpg", ".gif", ".ico",
        ".zip", ".tar", ".gz",
    ];

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
        if let Some(g) = file_glob {
            let file_name = entry.file_name().to_string_lossy();
            let matched = glob::Pattern::new(g)
                .map(|p| p.matches(&file_name))
                .unwrap_or(false);
            if !matched {
                continue;
            }
        }

        files.push(path_str);
    }

    files
}

fn search_file(
    path: &str,
    re: &regex::Regex,
    max_matches: usize,
) -> Result<Vec<GrepMatch>, std::io::Error> {
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let mut matches = Vec::new();

    for (line_num, line_result) in reader.lines().enumerate() {
        if matches.len() >= max_matches {
            break;
        }

        let line = line_result?;
        if re.is_match(&line) {
            let content = if line.len() > 500 {
                format!("{}...", &line[..500])
            } else {
                line
            };

            matches.push(GrepMatch {
                file: path.to_string(),
                line: line_num + 1,
                content,
            });
        }
    }

    Ok(matches)
}
