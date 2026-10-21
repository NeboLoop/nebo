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
//! - [`work`]: a turn's code work, moved into isolated copies;
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

/// What a landing came to, as the owner reads it.
pub fn landing_why(outcome: &crate::worktree::MergeOutcome) -> Why {
    use crate::worktree::{Busy, MergeOutcome};
    match outcome {
        MergeOutcome::NoChanges => Why::new("nothing_to_land"),
        MergeOutcome::Merged { commit, .. } => Why::new("landed").with("commit", short(commit)),
        MergeOutcome::Conflict { files, .. } => Why::new("conflicts").with("files", files.join(", ")),
        MergeOutcome::OwnerEdits { files, .. } => Why::new("owner_edits").with("files", files.join(", ")),
        MergeOutcome::OwnerBusy { why, .. } => match why {
            Busy::Detached | Busy::MadeDetached => Why::new("busy_no_branch"),
            Busy::OtherBranch { now, made_from } => {
                Why::new("busy_branch").with("now", now.as_str()).with("from", made_from.as_str())
            }
            Busy::InProgress(op) => Why::new("busy_in_progress").with("op", op.replace('_', "-")),
        },
        MergeOutcome::HookFailed { .. } => Why::new("hook_failed"),
        MergeOutcome::RetestFailed { .. } => Why::new("retest_failed"),
        MergeOutcome::NotAttempted { .. } => Why::new("not_attempted"),
        MergeOutcome::Unreviewed { files, .. } => Why::new("unreviewed").with("files", files.join(", ")),
        MergeOutcome::NeedsReview { files, .. } => Why::new("needs_review").with("files", files.join(", ")),
        MergeOutcome::Failed { .. } => Why::new("land_failed"),
    }
    }

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
        "no_checks" => "no tests or build could run".into(),
        "landed_unreviewed" => "landed without passing review, at the owner's choice".into(),
        "edits_in_way" => "not landed yet: the owner's edits are still in the way".into(),
        "kept_own_edits" => "not landed: the owner kept their own edits; the change waits in its copy".into(),
        "live_failed" => "the live check failed".into(),
        "live_no_script" => "it could not be checked live: the app's page didn't load the check script (its entry page isn't UTF-8 text, the app isn't one of the owner's own, or the page went to another address)".into(),
        "live_unavailable" => "it could not be checked live: the bot has no browser to open the app in".into(),
        "live_ok" => "the page reloaded and checked out".into(),
        "review_clean" => "no blocking or major issues".into(),
        "review_blocking" => format!("{} blocking issue(s)", a("count")),
        "up_to_date" => "already up to date with the project".into(),
        "caught_up" => format!("caught up to {}", a("commit")),
        "conflicts" => format!("the owner's changes and this one meet in {}", a("files")),
        "landed" => format!("landed as {}", a("commit")),
        "nothing_to_land" => "nothing to land".into(),
        "owner_edits" => format!("the owner's own edits in {} are in the way", a("files")),
        "busy_no_branch" => "not landed yet: the project is on no branch".into(),
        "busy_branch" => format!("not landed yet: the project is on branch {}, and the copy was made from {}", a("now"), a("from")),
        "busy_in_progress" => format!("not landed yet: a {} is in progress in the project", a("op")),
        "hook_failed" => "the project's commit hooks refused it".into(),
        "retest_failed" => "the checks failed after catching up with the project".into(),
        "not_attempted" => "not tried, because an earlier piece did not land".into(),
        "land_failed" => "it could not land; nothing of the owner's changed".into(),
        "step_limit" => "the run reached its step limit before the checking finished".into(),
        "turn_ended" => "the run ended before the checking finished".into(),
        "restarted" => "Nebo restarted while it was being checked".into(),
        "unreviewed" => format!("the copy holds changes nobody reviewed: {}", a("files")),
        "needs_review" => format!("what would land differs from what was reviewed in {}; reviewed again first", a("files")),
        "changed_after_review" => "the copy changed after it was reviewed; reviewed again first".into(),
        "waiting_to_land" => "waiting for another landing on this project to finish".into(),
        other => other.replace('_', " "),
    }
}

/// A commit as the owner reads it: its first ten characters.
pub fn short(commit: &str) -> String {
    commit.chars().take(10).collect()
}

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
