//! Removing a folder or a tree of files (`CallEffects::removes_trees`):
//! one rule, in every mode, Full Access and the owner's own chat included.
//!
//! - Inside the run's own work it goes ahead: inside the working copy this
//!   conversation builds in, inside the job's folders and the copy an
//!   isolated helper is fenced to, and a folder the employee made, whole or
//!   in part.
//! - Anywhere else it asks. Nobody attending, its card goes to the owner's
//!   Inbox, the step is refused and the run goes on.
//! - A working copy or the project it copies, itself (or everything in
//!   it), asks every time: no "Allow always" covers it.
//! - A target only running the command knows (`rm -rf "$DIR"`) asks.
//!
//! The folders no command ever removes (the root and what is right under
//! it, a drive, the home folder) are refused before this, by the safeguard
//! (`tools::policy::never_removed`).

use std::path::{Path, PathBuf};

use types::permissions::{AskCase, Effect, Rule, RuleField, RuleSource, Target};

use super::{CheckCx, RuleSet};

/// The ask a call's tree removals raise, or `None` when every one is inside
/// the run's own work (or it removes none). `decided` is the rule the call
/// met: an allow the owner gave for this command ("Allow always" on its
/// card, or a command rule of his own) covers a removal outside the work,
/// never one of the work itself.
pub fn asked(cx: &CheckCx<'_>, t: &Target, rules: &RuleSet, decided: Option<(&Rule, Effect)>) -> Option<AskCase> {
    if t.effects.removes_trees.is_empty() {
        return None;
    }
    let work = Work::of(cx, rules, &t.effects.removes_trees);
    let mut outside: Vec<String> = Vec::new();
    let mut whole_work = false;
    for target in &t.effects.removes_trees {
        let Some(path) = target.strip_prefix("file:").map(Path::new) else {
            // Only running the command knows what it removes.
            outside.push(target.clone());
            continue;
        };
        // `folder/*`: what is in the folder.
        let (folder, contents) = match path.file_name() {
            Some(name) if name == "*" => (path.parent().unwrap_or(path), true),
            _ => (path, false),
        };
        if work.is_root(folder) {
            whole_work = true;
            outside.push(target.clone());
        } else if !work.holds(folder, contents) {
            outside.push(target.clone());
        }
    }
    if outside.is_empty() {
        return None;
    }
    let owner_allowed = matches!(decided, Some((rule, Effect::Allow))
        if matches!(rule.source, RuleSource::AllowAlways { .. } | RuleSource::Owner)
            && matches!(rule.field, Some(RuleField::CommandPrefix(_))));
    if owner_allowed && !whole_work {
        return None;
    }
    Some(AskCase::RemovesFolders { targets: outside, whole_work })
}

/// The run's own work: where a removal goes ahead.
struct Work {
    /// Folders whose contents are the run's to remove.
    inside: Vec<PathBuf>,
    /// Folders the employee made: its own to remove, whole or in part.
    made: Vec<PathBuf>,
    /// Working copies and the projects they copy: never removed unasked.
    roots: Vec<PathBuf>,
}

impl Work {
    fn of(cx: &CheckCx<'_>, rules: &RuleSet, targets: &[String]) -> Self {
        let mut inside = rules.folders();
        inside.extend(cx.grant.fence.iter().flatten().cloned());
        let mut roots: Vec<PathBuf> = cx.grant.fence.iter().flatten().cloned().collect();
        // The copies this conversation's checked work is built in.
        let live = cx
            .store
            .list_verifications(&db::VerificationQuery { session_key: Some(&cx.ctx.session_key), live: true, ..Default::default() })
            .unwrap_or_default();
        for v in live.into_iter().filter(|v| !v.work_dir.is_empty()) {
            let copy = PathBuf::from(&v.work_dir);
            if PathBuf::from(&v.project_root) != copy {
                inside.push(copy.clone());
                roots.push(PathBuf::from(&v.project_root));
            }
            roots.push(copy);
        }
        Work { inside, made: made_by_it(cx, targets), roots }
    }

    /// Whether `folder` (or what is in it, `contents`) is the run's to
    /// remove: inside one of its folders, or a folder it made.
    fn holds(&self, folder: &Path, contents: bool) -> bool {
        self.inside.iter().any(|f| within(folder, f) && (contents || !same(folder, f)))
            || self.made.iter().any(|f| within(folder, f))
    }

    /// Whether `path` is a working copy or the project it copies, itself.
    fn is_root(&self, path: &Path) -> bool {
        self.roots.iter().any(|r| same(path, r))
    }
}

/// The folders, among `targets` and the folders they sit in, that the
/// employee made, as its created ledger keeps them.
fn made_by_it(cx: &CheckCx<'_>, targets: &[String]) -> Vec<PathBuf> {
    let mut asked: Vec<String> = Vec::new();
    for target in targets {
        let target = target.strip_suffix("/*").unwrap_or(target);
        for named in std::iter::once(target.to_string()).chain(super::cases::with_folders(target)) {
            if named.starts_with("file:") && !asked.contains(&named) {
                asked.push(named);
            }
        }
    }
    cx.store
        .created_by(&cx.grant.agent_id, &asked)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|n| n.strip_prefix("file:").map(PathBuf::from))
        .collect()
}

/// Whether `path` is `folder` or inside it, as named or with links resolved.
fn within(path: &Path, folder: &Path) -> bool {
    path.starts_with(folder) || resolved(path).starts_with(resolved(folder))
}

/// Whether two paths name one folder, as named or with links resolved.
fn same(a: &Path, b: &Path) -> bool {
    a == b || resolved(a) == resolved(b)
}

/// `path` with links resolved as far as it exists.
fn resolved(path: &Path) -> PathBuf {
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    let mut at = path.to_path_buf();
    loop {
        if let Ok(real) = std::fs::canonicalize(&at) {
            return rest.iter().rev().fold(real, |p, part| p.join(part));
        }
        match (at.file_name().map(|f| f.to_os_string()), at.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                at = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}
