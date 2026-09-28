use std::path::Path;

/// Find similar files in the same directory as the missing file.
/// Returns up to 5 suggestions sorted by similarity.
pub fn find_similar_files(missing_path: &str) -> Vec<String> {
    let path = Path::new(missing_path);
    let parent = match path.parent() {
        Some(p) if p.exists() => p,
        _ => return vec![],
    };
    let target_name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n.to_lowercase(),
        None => return vec![],
    };
    let target_stem = Path::new(&target_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&target_name)
        .to_lowercase();

    let mut candidates: Vec<(String, usize)> = Vec::new();

    let entries = match std::fs::read_dir(parent) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    for entry in entries.flatten() {
        let name = match entry.file_name().into_string() {
            Ok(n) => n,
            Err(_) => continue,
        };
        if entry.path().is_dir() {
            continue;
        }

        let name_lower = name.to_lowercase();
        let stem = Path::new(&name_lower)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&name_lower)
            .to_string();

        // Score: lower is better
        let score = if name_lower == target_name {
            0 // exact case-insensitive match
        } else if stem == target_stem {
            1 // same stem, different extension
        } else if name_lower.contains(&target_stem) || target_stem.contains(&stem) {
            2 // substring match
        } else {
            let dist = levenshtein(&name_lower, &target_name);
            if dist <= 3 {
                3 + dist
            } else {
                continue;
            }
        };

        candidates.push((name, score));
    }

    candidates.sort_by_key(|(_, score)| *score);
    candidates
        .into_iter()
        .take(5)
        .map(|(name, _)| name)
        .collect()
}

/// Build a "file not found" error with suggestions.
pub fn file_not_found(path: &str) -> String {
    let similar = find_similar_files(path);
    let parent = Path::new(path).parent();
    let parent_exists = parent.is_some_and(|p| p.exists());

    let mut msg = format!("File not found: {}", path);

    if !similar.is_empty() {
        msg.push_str("\nDid you mean: ");
        msg.push_str(
            &similar
                .iter()
                .map(|s| {
                    parent
                        .map(|p| p.join(s).to_string_lossy().into_owned())
                        .unwrap_or_else(|| s.clone())
                })
                .collect::<Vec<_>>()
                .join(", "),
        );
    } else if parent_exists {
        msg.push_str(&format!(
            "\nThe directory {} exists but does not contain this file.",
            parent.unwrap().display()
        ));
    }

    msg
}

/// Build a "path not found" error for grep/glob operations.
pub fn path_not_found(path: &str) -> String {
    let p = Path::new(path);
    // Walk up to find the nearest existing ancestor
    let mut ancestor = p.parent();
    while let Some(a) = ancestor {
        if a.exists() {
            return format!(
                "Path not found: {}\nThe directory {} exists. Check the path and try again.",
                path,
                a.display()
            );
        }
        ancestor = a.parent();
    }
    format!("Path not found: {}", path)
}

/// Build a "permission denied" error with context.
pub fn permission_denied(path: &str, operation: &str) -> String {
    let mut msg = format!("Permission denied: cannot {} {}", operation, path);

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        use std::path::Path;
        let p = Path::new(path);
        if let Ok(meta) = p.metadata() {
            let mode = meta.mode();
            let owner_uid = meta.uid();
            msg.push_str(&format!(
                "\nFile permissions: {:o}, owned by uid {}.",
                mode & 0o777,
                owner_uid
            ));
        }
    }

    msg.push_str("\nDo NOT use sudo. Explain the permission issue to the user.");
    msg
}

/// The installers that work on this computer: the one wording the
/// environment tells (`Installing software`) and every "not installed"
/// result carries (`install_guidance`).
///
/// Installing needs no ask (owner, 2026-09-28: "We must be able to actually
/// install things so we can code and do other high-value things"; the
/// permission check never surfaces an install).
/// - A cloud bot (`crate::cloud_bot`): its package installer runs as root,
///   as a command of its own (`system_packages`), and what it installs is put
///   back after a restart; the language installers write to the durable
///   toolchains folder the image points them at.
/// - macOS: Homebrew installs as the user, and the language installers do.
/// - Linux (the owner's own, never root): apt and dnf need root; user-level
///   installers write to the home folder. `pip install --user` is refused by
///   current Debian and Ubuntu (PEP 668), so a Python tool goes through pipx
///   or a venv.
/// - Windows: winget's user scope and scoop need no administrator;
///   Chocolatey does, by default.
pub fn installers(cloud: bool, os: &str) -> &'static str {
    if cloud {
        return "a system package with `sudo apt-get update && sudo apt-get install -y <package>`, as a command of its \
                own, or a language's own installer (npm install -g, pip install --user, cargo install, go install); \
                both are kept after a restart";
    }
    match os {
        "macos" => "Homebrew (brew install <package>), or a language's own installer (npm install -g, pipx install or \
                    a venv for a Python tool, cargo install, go install)",
        "windows" => "winget install --scope user <id>, scoop install <package>, or a language's own installer (npm \
                      install -g, pip install --user, cargo install, go install)",
        _ => "user-level installers: pipx install or a venv (python3 -m venv) for a Python tool, npm install -g \
              --prefix ~/.local, cargo install, go install, brew install where Homebrew is set up, or a release binary \
              in ~/.local/bin",
    }
}

/// How an employee installs a missing program here, and when it tells the
/// owner instead: the one text every "not installed" result carries.
pub fn install_guidance() -> String {
    guidance(crate::cloud_bot(), std::env::consts::OS)
}

fn guidance(cloud: bool, os: &str) -> String {
    let otherwise = if cloud {
        "If no package provides it, tell the owner."
    } else {
        match os {
            "macos" => "Never sudo; if an install needs admin rights or no package provides it, tell the owner.",
            "windows" => "An installer that needs administrator rights can't run here; then, or when no package \
                          provides it, tell the owner.",
            _ => "apt and dnf need root, which Nebo never has: when only a system package provides it, or none does, \
                  tell the owner the package and the command to run.",
        }
    };
    format!("Install it and carry on with {}; then run the command again. {otherwise}", installers(cloud, os))
}

/// Build a "command not found" error with similar command suggestions.
pub fn command_not_found(cmd: &str) -> String {
    let similar = find_similar_commands(cmd);
    let mut msg = format!("Command not found: {}", cmd);
    if !similar.is_empty() {
        msg.push_str(&format!(
            ". Similar commands available: {}",
            similar.join(", ")
        ));
    }
    msg.push_str(". ");
    msg.push_str(&install_guidance());
    msg
}

/// Search PATH for commands with similar names to the missing one.
fn find_similar_commands(cmd: &str) -> Vec<String> {
    let path_var = std::env::var("PATH").unwrap_or_default();
    let cmd_lower = cmd.to_lowercase();
    let mut candidates: Vec<(String, usize)> = Vec::new();

    for dir in path_var.split(':') {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let name = match entry.file_name().into_string() {
                Ok(n) => n,
                Err(_) => continue,
            };
            let name_lower = name.to_lowercase();
            if name_lower == cmd_lower {
                continue;
            }
            let score = if name_lower.starts_with(&cmd_lower) || cmd_lower.starts_with(&name_lower)
            {
                1
            } else if name_lower.contains(&cmd_lower) || cmd_lower.contains(&name_lower) {
                2
            } else {
                let dist = levenshtein(&name_lower, &cmd_lower);
                if dist <= 2 {
                    3 + dist
                } else {
                    continue;
                }
            };
            if !candidates.iter().any(|(n, _)| n == &name) {
                candidates.push((name, score));
            }
        }
    }

    candidates.sort_by_key(|(_, score)| *score);
    candidates.into_iter().take(5).map(|(n, _)| n).collect()
}

/// Build a "no results" response (NOT an error — this is a successful empty result).
pub fn no_grep_results(pattern: &str, path: &str) -> String {
    format!(
        "No matches found for pattern '{}' in {}. This is not an error — the pattern does not appear in any files at this path. Do not retry the same search.",
        pattern, path
    )
}

/// Nothing was searched: the pattern was never tested against any file.
/// Distinct from "no matches", which the old text claimed even here.
pub fn no_files_to_search(path: &str) -> String {
    format!(
        "No files to search under {}: the file glob matched nothing there (hidden directories, node_modules, vendor, target and binary extensions are skipped). The pattern was not tested against any file, so this says nothing about whether it exists elsewhere. Search a directory that has files, or ask where the code is.",
        path
    )
}

/// Build a missing-parameter error with a complete working example.
pub fn missing_param(action: &str, param: &str, example: &str) -> String {
    format!(
        "Missing required parameter '{}' for {} action.\nExample: {}",
        param, action, example
    )
}

/// Simple Levenshtein distance for fuzzy matching.
fn levenshtein(a: &str, b: &str) -> usize {
    let a_len = a.len();
    let b_len = b.len();
    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut prev: Vec<usize> = (0..=b_len).collect();
    let mut curr = vec![0; b_len + 1];

    for (i, ca) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.chars().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr[j + 1] = (prev[j + 1] + 1)
                .min(curr[j] + 1)
                .min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b_len]
}

#[cfg(test)]
mod install_tests {
    use super::*;

    /// A cloud bot's missing program is installed with its package installer
    /// and kept; the owner's computer never hears of sudo as an option.
    #[test]
    fn the_hint_names_the_installers_that_work_here() {
        let cloud = guidance(true, "linux");
        assert!(cloud.starts_with("Install it and carry on"), "{cloud}");
        assert!(cloud.contains("`sudo apt-get update && sudo apt-get install -y <package>`, as a command of its own"), "{cloud}");
        assert!(cloud.contains("kept after a restart") && !cloud.contains("need root"), "{cloud}");
        assert_eq!(
            crate::system_packages::installer("sudo apt-get update && sudo apt-get install -y imagemagick"),
            Some(vec!["imagemagick".to_string()]),
            "the form the hint names is the form the installer lets through"
        );
        let linux = guidance(false, "linux");
        assert!(linux.contains("apt and dnf need root") && !linux.contains("sudo apt-get"), "{linux}");
        assert!(guidance(false, "macos").contains("Never sudo"));
    }
}
