//! Verified: code work is not reported done until independent reviewers in
//! fresh contexts fail to break it and the project's own build and tests
//! pass.
//!
//! What makes work code work is where it writes, never which tools were
//! loaded: a call that changes something and names a path in a code folder
//! ([`project::code_folder`]: a project, an app's pages, one of Nebo's
//! working copies), or a call that writes an app's pages through its
//! arguments ([`writes_app_pages`]). Writing a letter, an email or a
//! research note into an ordinary folder is never code work: those turns
//! run no check and cost nothing more.
//!
//! The parts, each its own module:
//! - [`shadow`]: what changed, the whole change and each round's delta;
//! - [`project`]: which folder a change belongs to;
//! - [`lenses`]: the review lenses a change gets, and whether it is small;
//! - [`checks`]: the project's own build and tests;
//! - [`rules`]: the rules every brief carries;
//! - [`findings`]: what a reviewer reports, and when the loop has stalled.

pub mod checks;
pub mod findings;
pub mod lenses;
pub mod project;
pub mod rules;
pub mod shadow;

use std::path::{Path, PathBuf};

/// The input fields that name a path a call works on.
pub const PATH_FIELDS: &[&str] = &["path", "file_path", "notebook_path", "cwd", "dir", "folder"];

/// The paths a call names: its path fields (relative ones against `cwd`),
/// each of `paths[]`, the absolute paths in a command, and the folder a
/// command runs in.
pub fn named_paths(input: &serde_json::Value, cwd: Option<&str>) -> Vec<PathBuf> {
    let resolve = |p: &str| -> PathBuf {
        let path = PathBuf::from(expand_home(p));
        match cwd {
            Some(c) if path.is_relative() => Path::new(c).join(path),
            _ => path,
        }
    };
    let mut out: Vec<PathBuf> = Vec::new();
    for key in PATH_FIELDS {
        if let Some(p) = input
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|p| !p.trim().is_empty())
        {
            out.push(resolve(p));
        }
    }
    if let Some(paths) = input.get("paths").and_then(|v| v.as_array()) {
        out.extend(paths.iter().filter_map(|p| p.as_str()).map(resolve));
    }
    if let Some(command) = input.get("command").and_then(|v| v.as_str()) {
        if input.get("cwd").is_none()
            && let Some(c) = cwd
        {
            out.push(PathBuf::from(expand_home(c)));
        }
        for word in shlex::split(command).unwrap_or_default() {
            let word = word.trim_matches(|c| matches!(c, ';' | '&' | '|' | '(' | ')' | '>' | '<'));
            let word = word.split_once('=').map(|(_, v)| v).unwrap_or(word);
            if word.starts_with('/') || word.starts_with("~/") {
                out.push(PathBuf::from(expand_home(word)));
            }
        }
    }
    out
}

fn expand_home(p: &str) -> String {
    match (p.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest).to_string_lossy().into_owned(),
        _ => p.to_string(),
    }
}

/// Whether a call writes an app's pages through its arguments: an
/// employee created or updated with `ui` (its page files) or `app`.
pub fn writes_app_pages(input: &serde_json::Value) -> bool {
    input
        .get("ui")
        .is_some_and(|v| v.as_object().is_some_and(|o| !o.is_empty()))
        || input.get("app").is_some_and(|v| !v.is_null())
}

/// The code folders a call that changes something writes in (`changes`:
/// the call is not read-only). Empty for every other call.
pub fn code_folders(
    input: &serde_json::Value,
    cwd: Option<&str>,
    changes: bool,
    app_pages: &[PathBuf],
) -> Vec<project::Folder> {
    if !changes {
        return Vec::new();
    }
    let mut out: Vec<project::Folder> = Vec::new();
    for path in named_paths(input, cwd) {
        if let Some(folder) = project::code_folder(&path, app_pages)
            && !out.contains(&folder)
        {
            out.push(folder);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_calls_paths_are_its_fields_its_commands_paths_and_its_folder() {
        let paths = named_paths(
            &json!({"command": "cargo test --manifest-path /repo/Cargo.toml && OUT=/tmp/x make > /repo/log", "description": "run tests"}),
            Some("/repo/crates"),
        );
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/repo/crates"),
                PathBuf::from("/repo/Cargo.toml"),
                PathBuf::from("/tmp/x"),
                PathBuf::from("/repo/log")
            ]
        );
        assert_eq!(
            named_paths(&json!({"path": "src/lib.rs"}), Some("/repo")),
            vec![PathBuf::from("/repo/src/lib.rs")]
        );
    }

    #[test]
    fn a_write_into_a_project_with_any_tool_is_code_work_and_an_ordinary_folder_is_not() {
        crate::test_home::with_home(|_| {
            let root = tempfile::tempdir().unwrap();
            let proj = root.path().join("proj");
            std::fs::create_dir_all(proj.join("src")).unwrap();
            std::fs::write(proj.join("Cargo.toml"), "[package]\nname = \"p\"\n").unwrap();
            let docs = root.path().join("Letters");
            std::fs::create_dir_all(&docs).unwrap();

            let edit = json!({"path": proj.join("src/main.rs").to_string_lossy(), "old_string": "a", "new_string": "b"});
            assert_eq!(
                code_folders(&edit, None, true, &[]),
                vec![project::Folder::Project(proj.clone())],
                "edit_file on code"
            );
            let shell =
                json!({"command": format!("echo x >> {}", proj.join("src/main.rs").display())});
            assert_eq!(
                code_folders(&shell, None, true, &[]).len(),
                1,
                "run_command writing code"
            );
            assert!(
                code_folders(&edit, None, false, &[]).is_empty(),
                "a read is never code work"
            );

            let letter =
                json!({"path": docs.join("offer.docx").to_string_lossy(), "content": "Dear"});
            assert!(
                code_folders(&letter, None, true, &[]).is_empty(),
                "a letter is not code"
            );
            let email = json!({"to": "a@b.c", "subject": "Hi", "body": "Hello"});
            assert!(
                code_folders(&email, None, true, &[]).is_empty(),
                "an email is not code"
            );
        })
    }

    #[test]
    fn an_employees_page_files_in_its_arguments_are_app_page_work() {
        assert!(writes_app_pages(
            &json!({"name": "Flip", "ui": {"index.html": "<div>"}})
        ));
        assert!(writes_app_pages(
            &json!({"id": "a1", "app": {"entry": "index.html"}})
        ));
        assert!(!writes_app_pages(
            &json!({"name": "Bookkeeper", "description": "keeps books"})
        ));
        assert!(!writes_app_pages(&json!({"name": "Flip", "ui": {}})));
    }
}
