//! Removing a folder or a tree of files (`CallEffects::removes_trees`):
//! one rule, in every mode, Full Access and the owner's own chat included.
//!
//! - Inside the run's own work it goes ahead: inside the project this
//!   conversation's checked work is done in (or a helper's copy of it),
//!   inside the job's folders and the copy an isolated helper is fenced to,
//!   and a folder the employee made, whole or in part.
//! - Anywhere else it asks. Nobody attending, its card goes to the owner's
//!   Inbox, the step is refused and the run goes on.
//! - That project or a copy of it, itself (or everything in it), asks
//!   every time: no "Allow always" covers it. So does a protected folder
//!   anywhere in it ([`PROTECTED_DIRS`]: its git folder, Nebo's own, an
//!   editor's or an agent's settings), however deep.
//! - A removal in a project (or a copy of one) of a folder that isn't
//!   there when the call is checked asks when the same command can put
//!   something there first (`mkdir`, a move, copy or link into it): what it
//!   will hold then (a `.git` moved or linked in) can't be known. Otherwise
//!   it removes nothing, and goes ahead (`rm -rf dist && pnpm build`).
//! - Moving or renaming a protected folder away (`CallEffects::moves`:
//!   `mv .git junk/`) asks every time, with no "Allow always", as removing
//!   it does. A move only running the command knows (`mv "$X" y`) asks.
//!
//! What goes ahead inside a project is gone for good when git ignores it:
//! the change's snapshot (`verify::shadow`) holds only what the project
//! doesn't ignore.
//! - A target only running the command knows (`rm -rf "$DIR"`) asks.
//!
//! The folders no command ever removes (the root and what is right under
//! it, a drive, the home folder) are refused before this, by the safeguard
//! (`tools::policy::never_removed`).

use std::path::{Path, PathBuf};

use types::permissions::{AskCase, Effect, Rule, RuleField, RuleSource, Target};

use super::{CheckCx, RuleSet};

/// The folders inside a project no removal reaches unasked, and no "Allow
/// always" covers: the repository itself, Nebo's working files, and the
/// editors' and agents' own settings.
pub const PROTECTED_DIRS: &[&str] = &[".git", ".nebo", ".vscode", ".idea", ".claude"];

/// The most entries a removal's tree is walked through looking for a
/// protected folder in it; a tree bigger than that asks.
const MAX_WALK: usize = 50_000;

/// Whether `name` is one of [`PROTECTED_DIRS`], whatever its case: macOS's
/// and Windows' usual filesystems take `.GIT` for `.git`.
fn protected(name: &std::ffi::OsStr) -> bool {
    PROTECTED_DIRS.iter().any(|p| name.to_string_lossy().eq_ignore_ascii_case(p))
}

/// Whether the tree at `dir` holds a protected folder (a nested
/// repository's `.git`, an editor's settings), walked without following
/// links and stopping at the first. In a repository, what git ignores
/// (`node_modules`, `target`, as the project's own ignore rules say) is not
/// walked: it is the project's own to clean, as the owner ruled for what is
/// in a project (10-23: "Do what Claude does here"). `true` too when the
/// walk passes [`MAX_WALK`] entries, or can't be read: nothing then vouches
/// for it.
fn holds_protected(dir: &Path) -> bool {
    let ignored = match ignored_in(dir) {
        Ignored::Whole => return false,
        Ignored::Some(paths) => paths,
        Ignored::NoRepository => Vec::new(),
    };
    let mut stack = vec![dir.to_path_buf()];
    let mut seen = 0usize;
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { return d != dir || dir.exists() };
        for entry in entries {
            let Ok(entry) = entry else { return true };
            let path = entry.path();
            if ignored.contains(&path) {
                continue;
            }
            seen += 1;
            if seen > MAX_WALK || protected(&entry.file_name()) {
                return true;
            }
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(path);
            }
        }
    }
    false
}

/// What git ignores of the tree at `dir`.
enum Ignored {
    /// `dir` itself is ignored.
    Whole,
    /// The ignored files and folders in it, as git lists them (a folder
    /// whole).
    Some(Vec<PathBuf>),
    /// It is in no repository (or git can't say): nothing is skipped.
    NoRepository,
}

/// Asked of git once for a decision: whether `dir` is ignored, and else
/// which files and folders in it are (`git ls-files --others --ignored
/// --exclude-standard --directory`), by the repository's own rules.
fn ignored_in(dir: &Path) -> Ignored {
    let git = |args: &[&str]| {
        command::new::<std::process::Command>("git", command::Console::Hidden)
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .ok()
    };
    match git(&["rev-parse", "--is-inside-work-tree"]) {
        Some(out) if out.status.success() => {}
        _ => return Ignored::NoRepository,
    }
    let whole = match (dir.parent(), dir.file_name()) {
        (Some(parent), Some(name)) => command::new::<std::process::Command>("git", command::Console::Hidden)
            .arg("-C")
            .arg(parent)
            .args(["check-ignore", "-q", "--"])
            .arg(name)
            .env("GIT_TERMINAL_PROMPT", "0")
            .status()
            .is_ok_and(|s| s.success()),
        _ => false,
    };
    if whole {
        return Ignored::Whole;
    }
    match git(&["ls-files", "-z", "--others", "--ignored", "--exclude-standard", "--directory", "."]) {
        Some(out) if out.status.success() => Ignored::Some(
            out.stdout
                .split(|b| *b == 0)
                .filter(|p| !p.is_empty())
                .map(|p| dir.join(String::from_utf8_lossy(p).trim_end_matches('/')))
                .collect(),
        ),
        _ => Ignored::NoRepository,
    }
}

/// The ask a call's tree removals raise, or `None` when every one is inside
/// the run's own work (or it removes none). `decided` is the rule the call
/// met: an allow the owner gave for this command ("Allow always" on its
/// card, or a command rule of his own) covers a removal outside the work,
/// never one of the work itself.
pub fn asked(cx: &CheckCx<'_>, t: &Target, rules: &RuleSet, decided: Option<(&Rule, Effect)>) -> Option<AskCase> {
    if t.effects.removes_trees.is_empty() && t.effects.moves.is_empty() {
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
        // `folder/*`: what is in the folder. Through a link, what it points
        // to is removed when the target is written `link/` or `link/*`; the
        // link alone otherwise.
        let (folder, contents, follows) = match path.file_name() {
            Some(name) if name == "*" => (path.parent().unwrap_or(path), true, true),
            _ => (path, false, target.ends_with('/')),
        };
        if work.is_root(folder) || work.guarded(folder, follows) {
            whole_work = true;
            outside.push(target.clone());
        } else if work.unknowable(folder, &t.effects) || !work.holds(folder, contents, follows) {
            outside.push(target.clone());
        }
    }
    let removing = !outside.is_empty();
    for target in &t.effects.moves {
        let Some(path) = target.strip_prefix("file:").map(Path::new) else {
            outside.push(target.clone());
            continue;
        };
        let (folder, contents) = match path.file_name() {
            Some(name) if name == "*" => (path.parent().unwrap_or(path), true),
            _ => (path, false),
        };
        if work.moves_protected(folder, contents) {
            whole_work = true;
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
    let command = cx.input.get("command").and_then(|c| c.as_str()).unwrap_or_default();
    Some(AskCase::RemovesFolders {
        targets: outside,
        whole_work,
        moves: !removing,
        warning: tools::policy::destructive_warning(command),
    })
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
        let live = cx
            .store
            .list_verifications(&db::VerificationQuery { session_key: Some(&cx.ctx.session_key), live: true, ..Default::default() })
            .unwrap_or_default();
        // The folder this conversation's checked work is done in: the
        // project itself, or a helper's copy of it.
        for v in live.into_iter().filter(|v| !v.work_dir.is_empty()) {
            let work_dir = PathBuf::from(&v.work_dir);
            inside.push(work_dir.clone());
            roots.push(PathBuf::from(&v.project_root));
            roots.push(work_dir);
        }
        Work { inside, made: made_by_it(cx, targets), roots }
    }

    /// Whether `folder` (or what is in it, `contents`) is the run's to
    /// remove: inside one of its folders, or a folder it made. Where it
    /// really is decides: a link inside the work to a folder outside it is
    /// outside, when the removal goes through it (`follows`).
    fn holds(&self, folder: &Path, contents: bool, follows: bool) -> bool {
        let real = if follows { resolved(folder) } else { placed(folder) };
        self.inside.iter().any(|f| real.starts_with(resolved(f)) && (contents || !same(folder, f)))
            || self.made.iter().any(|f| real.starts_with(resolved(f)))
    }

    /// Whether `folder` is, is inside, or holds a protected folder
    /// ([`PROTECTED_DIRS`], whatever its case): counted below the work folder
    /// that holds it (a helper's copy sits in the project's `.nebo`, and what
    /// is in it is its own), or along its whole path when no work folder
    /// does; and anywhere in the tree it removes ([`holds_protected`]).
    fn guarded(&self, folder: &Path, follows: bool) -> bool {
        let real = if follows { resolved(folder) } else { placed(folder) };
        self.within_protected(&real) || (real.is_dir() && holds_protected(&real))
    }

    /// Whether moving `folder` (or what is in it, `contents`) away moves a
    /// protected folder: it is or is inside one, or, for what is in it, one
    /// is right in it. A link moves as the link.
    fn moves_protected(&self, folder: &Path, contents: bool) -> bool {
        let real = placed(folder);
        self.within_protected(&real)
            || (contents && std::fs::read_dir(&real).is_ok_and(|es| es.flatten().any(|e| protected(&e.file_name()))))
    }

    /// Whether the real path `real` is or is inside a protected folder,
    /// counted below the work folder that holds it, or along its whole path
    /// when none does.
    fn within_protected(&self, real: &Path) -> bool {
        let below = self
            .inside
            .iter()
            .chain(&self.made)
            .chain(&self.roots)
            .map(|f| resolved(f))
            .filter(|f| real.starts_with(f))
            .max_by_key(|f| f.components().count())
            .and_then(|f| real.strip_prefix(&f).ok().map(Path::to_path_buf))
            .unwrap_or_else(|| real.to_path_buf());
        below.components().any(|c| protected(c.as_os_str()))
    }

    /// Whether `folder` is in a project or a copy of one, isn't there yet,
    /// and the call can put something there first (`fx`: a folder it makes,
    /// a move, copy or link into it, or above it, or one only running the
    /// command can tell): what it will hold then can't be known.
    fn unknowable(&self, folder: &Path, fx: &types::permissions::CallEffects) -> bool {
        if std::fs::symlink_metadata(folder).is_ok() || !self.roots.iter().any(|r| placed(folder).starts_with(resolved(r))) {
            return false;
        }
        fx.creates.iter().chain(&fx.moves).chain(&fx.places).any(|named| match named.strip_prefix("file:") {
            Some(p) => {
                let p = Path::new(p.strip_suffix("/*").unwrap_or(p));
                p.starts_with(folder) || folder.starts_with(p)
            }
            None => true,
        })
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

/// Whether two paths name one folder, as named or with links resolved:
/// either is enough to count as the folder itself, which only ever asks.
fn same(a: &Path, b: &Path) -> bool {
    a == b || resolved(a) == resolved(b)
}

/// Where `path` itself really is: its folder with links resolved, and its
/// own name as written (a link there is the link, not what it points to).
fn placed(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => resolved(parent).join(name),
        _ => resolved(path),
    }
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
