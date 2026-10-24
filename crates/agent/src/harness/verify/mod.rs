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
//! - [`review`]: a round of independent reviewers, one per lens;
//! - [`findings`]: what a reviewer reports, and when the loop has stalled;
//! - [`work`]: a turn's code work, started where it happens;
//! - [`engine`]: the loop itself, the end check of a turn with code work.

pub mod checks;
pub mod engine;
pub mod findings;
pub mod lenses;
pub mod project;
pub mod review;
pub mod rules;
pub mod shadow;
pub mod work;

use std::path::{Path, PathBuf};

/// What the owner reads about a verification or one of its steps
/// ([`types::Why`]): a code the apps word, with its values.
pub use types::Why;

/// `why` in plain English, for the builder's report brief (the model reads
/// it; the owner's screen words the code itself).
pub fn english(why: &Why) -> String {
    let a = |k: &str| why.args.get(k).map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).unwrap_or_default();
    match why.code.as_str() {
        "needs_answer" => "needs the owner's answer to a question it asked".into(),
        "unreadable" => "could not read what changed".into(),
        "spend_limit" => "reached this employee's spending limit for a run".into(),
        "review_incomplete" => "a review could not finish".into(),
        "needs_decision" => format!("needs the owner's decision: {}", a("items")),
        "stopped_by_owner" => format!("stopped by the owner with {} open issue(s)", a("open")),
        "stalled" => format!("stopped making progress with {} open issue(s)", a("open")),
        "no_checks" => "nothing proved it works: the project has no build, type check or tests to run".into(),
        "live_failed" => "the live check failed".into(),
        "live_no_script" => "it could not be checked live: the app's page didn't load the check script (its entry page isn't UTF-8 text, the app isn't one of the owner's own, or the page went to another address)".into(),
        "live_unavailable" => "it could not be checked live: the bot has no browser to open the app in".into(),
        "live_ok" => "the page reloaded and checked out".into(),
        "review_clean" => "no blocking or major issues".into(),
        "review_blocking" => format!("{} blocking issue(s)", a("count")),
        "step_limit" => "the run reached its step limit before the checking finished".into(),
        "turn_ended" => "the run ended before the checking finished".into(),
        "restarted" => "Nebo restarted while it was being checked".into(),
        "kept_in_copy" => format!("its earlier work is kept in its copy at {}", a("path")),
        other => other.replace('_', " "),
    }
}

/// A commit as the owner reads it: its first ten characters.
pub fn short(commit: &str) -> String {
    commit.chars().take(10).collect()
}

/// The input fields that name a path a call works on.
pub const PATH_FIELDS: &[&str] = &["path", "file_path", "notebook_path", "cwd", "dir", "folder"];

/// The paths a call may change: its path fields (relative ones against
/// `cwd`), each of `paths[]`, and for a command the folders it runs in and
/// the paths it writes ([`command_paths`]). A program a command runs, or a
/// file it only reads, is never among them (10-23: running
/// `/opt/homebrew/bin/doctl` made a copy of the Homebrew repository).
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
        let here = input.get("cwd").and_then(|v| v.as_str()).or(cwd);
        out.extend(command_paths(command, here));
    }
    out
}

/// The folders a command moves into (`cd`, `pushd`) and the paths it
/// writes, as far as its text shows: what it removes, rewrites and makes
/// (`tools::policy::shell_effects`), what it redirects or `tee`s into, and
/// the destination of a copy, move, link or install (its last operand).
/// Relative ones are taken from `cwd`.
pub fn command_paths(command: &str, cwd: Option<&str>) -> Vec<PathBuf> {
    let resolve = |p: &str| -> PathBuf {
        let path = PathBuf::from(expand_home(p));
        match cwd {
            Some(c) if path.is_relative() => Path::new(&expand_home(c)).join(path),
            _ => path,
        }
    };
    let fx = tools::policy::shell_effects(command, cwd);
    let mut out: Vec<PathBuf> = fx
        .deletes
        .iter()
        .chain(&fx.overwrites)
        .chain(&fx.creates)
        .chain(&fx.removes_trees)
        .chain(&fx.moves)
        .filter_map(|n| n.strip_prefix("file:"))
        .map(|p| resolve(p.strip_suffix("/*").unwrap_or(p)))
        .collect();
    out.extend(tools::policy::shell_write_targets(command).iter().map(|p| resolve(p)));
    let mut here: Option<PathBuf> = cwd.map(|c| PathBuf::from(expand_home(c)));
    for sub in tools::policy::subcommands(command) {
        let words: Vec<&str> = sub.words().iter().flatten().map(String::as_str).collect();
        let Some(program) = words.first().and_then(|w| Path::new(w).file_name()).and_then(|n| n.to_str()) else { continue };
        if matches!(program, "cd" | "pushd")
            && let Some(dir) = words.get(1).filter(|d| **d != "-")
        {
            let dir = PathBuf::from(expand_home(dir));
            let next = match &here {
                Some(h) if dir.is_relative() => h.join(dir),
                _ => dir,
            };
            out.push(next.clone());
            here = Some(next);
        }
        if matches!(program, "cp" | "mv" | "ln" | "install" | "rsync")
            && let Some(dest) = words[1..].iter().rev().find(|w| !w.starts_with('-'))
        {
            out.push(resolve(dest));
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
    fn a_calls_paths_are_its_fields_its_folder_and_what_its_command_writes() {
        let paths = named_paths(
            &json!({"command": "cargo test --manifest-path /repo/Cargo.toml && OUT=/tmp/x make > /repo/log && cp a.rs /other/src/", "description": "run tests"}),
            Some("/repo/crates"),
        );
        assert_eq!(
            paths,
            vec![PathBuf::from("/repo/crates"), PathBuf::from("/repo/log"), PathBuf::from("/other/src/")]
        );
        assert_eq!(
            named_paths(&json!({"command": "cd /repo && cd web && pnpm build"}), None),
            vec![PathBuf::from("/repo"), PathBuf::from("/repo/web")]
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

    /// 10-23, live: a command that ran `/opt/homebrew/bin/doctl` made a copy
    /// of the Homebrew repository and ran in it, where there was no Cellar
    /// and no bin. Running a program, or reading a file, in a repository is
    /// never code work in it; writing there is.
    #[test]
    fn running_a_program_from_a_repository_is_not_code_work_in_it() {
        crate::test_home::with_home(|_| {
            let root = tempfile::tempdir().unwrap();
            let brew = root.path().join("homebrew");
            std::fs::create_dir_all(brew.join("bin")).unwrap();
            std::fs::create_dir_all(brew.join(".git")).unwrap();
            std::fs::write(brew.join("bin/doctl"), "#!/bin/sh\n").unwrap();
            let elsewhere = root.path().join("Letters");
            std::fs::create_dir_all(&elsewhere).unwrap();
            let doctl = brew.join("bin/doctl").display().to_string();
            let cwd = elsewhere.to_string_lossy().into_owned();
            for command in [
                format!("{doctl} apps list --format ID"),
                format!("{doctl} apps update abc --spec spec.yaml > out.json"),
                format!("cat {}/Library/Taps/README.md | head", brew.display()),
            ] {
                assert!(code_folders(&json!({ "command": command }), Some(&cwd), true, &[]).is_empty(), "{command}");
            }
            let writes = format!("echo x >> {}/Library/Formula/a.rb", brew.display());
            assert_eq!(code_folders(&json!({ "command": writes }), Some(&cwd), true, &[]), vec![project::Folder::Project(brew.clone())]);
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
