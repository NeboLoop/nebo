//! Removing a folder or a tree of files (`CallEffects::removes_trees`):
//! the owner's rule is that nothing is removed wholesale without his say,
//! in every mode, Full Access and his own chat included.
//!
//! - Inside the run's own work it goes ahead: the working copy it builds
//!   in (never the copy itself), the job's folders, the copy an isolated
//!   helper is fenced to, and what is inside a folder the employee made (a
//!   clone, a skill's data folder). A folder it made in the system's
//!   temporary folder is its own to remove whole; one it made anywhere
//!   else asks, itself (10-23: an app's folder made by hand was removed
//!   unasked).
//! - Anywhere else it asks.
//! - A critical target asks even inside the work, and no "Allow always"
//!   covers it: the root, the home folder, a folder right under the root, a
//!   drive or a folder right under one, everything in a folder (`*`,
//!   `dir/*`), and a working copy or the project it copies, themselves.
//! - A target only running the command knows (`rm -rf "$DIR"`, a `cd` the
//!   text can't follow) asks.

use std::path::{Path, PathBuf};

use types::permissions::{AskCase, Effect, RuleSource, Target};

use super::{CheckCx, RuleSet};

/// The ask a call's tree removals raise, or `None` when every one is inside
/// the run's own work (or it removes none). `decided` is the rule the call
/// met: an allow the owner gave for this very command ("Allow always", or
/// his own) covers a removal that isn't critical.
pub fn asked(cx: &CheckCx<'_>, t: &Target, rules: &RuleSet, decided: Option<(&types::permissions::Rule, Effect)>) -> Option<AskCase> {
    if t.effects.removes_trees.is_empty() {
        return None;
    }
    let work = Work::of(cx, rules, &t.effects.removes_trees);
    let mut outside: Vec<String> = Vec::new();
    let mut critical = false;
    for target in &t.effects.removes_trees {
        let Some(path) = target.strip_prefix("file:").map(Path::new) else {
            // Only running the command knows what it removes.
            outside.push(target.clone());
            continue;
        };
        if is_critical(path) || work.is_root(path) {
            critical = true;
            outside.push(target.clone());
        } else if !work.holds(path) {
            outside.push(target.clone());
        }
    }
    if outside.is_empty() {
        return None;
    }
    // Only an allow of this very command covers it: the owner's "Allow
    // always" on its card, or a command he allowed himself. An allow of
    // the shell as a whole is not his word on removing this.
    let owner_allowed = matches!(decided, Some((rule, Effect::Allow))
        if matches!(rule.source, RuleSource::AllowAlways { .. } | RuleSource::Owner)
            && matches!(rule.field, Some(types::permissions::RuleField::CommandPrefix(_))));
    if owner_allowed && !critical {
        return None;
    }
    let command = cx.input.get("command").and_then(|c| c.as_str()).unwrap_or_default();
    Some(AskCase::RemovesFolders { targets: outside, critical, warning: tools::policy::destructive_warning(command) })
}

/// The run's own work: where a removal goes ahead.
struct Work {
    /// Folders whose contents are the run's to remove.
    inside: Vec<PathBuf>,
    /// Folders the employee made: what is inside each is its own to
    /// remove, and one in the system's temporary folder is, whole.
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
        for v in live {
            let copy = PathBuf::from(&v.work_dir);
            if v.work_dir.is_empty() {
                continue;
            }
            if PathBuf::from(&v.project_root) != copy {
                inside.push(copy.clone());
                roots.push(PathBuf::from(&v.project_root));
            }
            roots.push(copy);
        }
        let made = made_by_it(cx, targets);
        Work { inside, made, roots }
    }

    /// Whether `path` is the run's to remove: inside one of its folders
    /// (never the folder itself), or a folder it made in the temporary
    /// folder, or inside one.
    fn holds(&self, path: &Path) -> bool {
        let strictly = |f: &PathBuf| !same(path, f) && within(path, f);
        self.inside.iter().any(strictly) || self.made.iter().any(|f| strictly(f) || (in_temp(f) && same(path, f)))
    }

    /// Whether `path` is a working copy or the project it copies, itself.
    fn is_root(&self, path: &Path) -> bool {
        self.roots.iter().any(|r| same(path, r))
    }
}

/// Whether `path` is in the system's temporary folder (not the folder
/// itself).
fn in_temp(path: &Path) -> bool {
    [std::env::temp_dir(), PathBuf::from("/tmp"), PathBuf::from("/private/tmp")]
        .iter()
        .any(|t| within(path, t) && !same(path, t))
}

/// The folders, among `targets` and the folders they sit in, that the
/// employee made, as its created ledger keeps them.
fn made_by_it(cx: &CheckCx<'_>, targets: &[String]) -> Vec<PathBuf> {
    let mut asked: Vec<String> = Vec::new();
    for target in targets {
        for named in super::cases::with_folders(target) {
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

/// A target no removal reaches unasked, as written (links not followed:
/// `/tmp` is caught even where it links to `/private/tmp`): everything in a
/// folder (`*`, `…/*`), the root, a drive, the home folder, or a folder
/// right under the root or a drive.
pub fn is_critical(path: &Path) -> bool {
    let text = path.to_string_lossy();
    let slashed: String = {
        let mut out = String::new();
        let mut last_slash = false;
        for c in text.chars() {
            let slash = c == '/' || c == '\\';
            if !(slash && last_slash) {
                out.push(if slash { '/' } else { c });
            }
            last_slash = slash;
        }
        out
    };
    if slashed == "*" || slashed.ends_with("/*") {
        return true;
    }
    let normal = if slashed == "/" { slashed.as_str() } else { slashed.trim_end_matches('/') };
    if normal == "/" || normal.is_empty() {
        return true;
    }
    // A drive (`C:`, `C:/`) and a folder right under one (`C:/Windows`).
    let b = normal.as_bytes();
    let on_drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
    if on_drive && (b.len() == 2 || (b[2] == b'/' && b.len() > 3 && !normal[3..].contains('/'))) {
        return true;
    }
    if let Some(home) = dirs::home_dir() {
        let home = home.to_string_lossy().replace('\\', "/");
        if normal == home.trim_end_matches('/') {
            return true;
        }
    }
    let parent = Path::new(normal).parent().map(|p| p.to_string_lossy().replace('\\', "/"));
    parent.as_deref() == Some("/")
}
