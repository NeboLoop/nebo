//! Nebo's own files: the ONE answer to "may an employee reach this path?"
//! for everything under Nebo's folders: this Nebo's (`config::data_dir()`)
//! and every other one this computer may have (`config::nebo_roots()`).
//!
//! An employee works in some of this Nebo's folder: its workspace, the
//! copies its helpers work in, the company layers, the skills and employee
//! packages it runs, and its own conversation's saved results. It reads, and
//! never changes, the skills the installed plugins ship: `use_skill` hands it
//! their folders. Everything else there is Nebo's own and never an
//! employee's: the settings file holding the server's secret, the logs, the
//! database, the plugins' programs, accounts and data, the other
//! conversations' files. On 2026-09-26 an employee asked to fix a mail
//! sign-in `cat` the settings file and put the server's secret into its
//! context; others read the logs looking for answers.
//!
//! Every other Nebo folder on this computer is closed whole: the
//! platform-native one when this Nebo is relocated (`NEBO_HOME`), the
//! pre-v5 one and `~/.nebo`. Another install's settings are its own, never
//! this one's employees' (2026-09-27: a run whose `NEBO_HOME` was fenced
//! read a second Nebo's `settings.json` under `~/.local/share/nebo`).
//!
//! Three doors apply this one fence: the file tools (the safeguard refuses a
//! path it closes), the text of a command (the safeguard refuses one that
//! names such a path), and the command itself (`confine` runs it where those
//! paths can't be read or changed at all, and the plugins' skills can't be
//! changed).

use std::path::{Component, Path, PathBuf};

/// The parts of Nebo's folder an employee works in, relative to it. The
/// run's own session folder (its saved tool results and checkpoints) is
/// added per run.
const OPEN: &[&str] = &[
    // The workspace: the documents it writes and the files it is sent.
    "files",
    // The project copies its isolated helpers work in.
    "worktrees",
    // Deep research runs and their reports.
    "research",
    // The company and industry layers, always editable.
    "packs",
    // Skills and employee packages: a skill's body names its own folder and
    // its data folder for the commands it runs, and an employee package
    // carries skills of its own.
    "learned",
    "nebo/skills",
    "user/skills",
    "appdata/skills",
    "nebo/agents",
    "user/agents",
    "appdata/agents",
    // A cloud bot's user-level installs (`npm install -g`, `pip install
    // --user`, `cargo install`, `go install`): the image points them here
    // so they persist with the bot's state.
    "toolchains",
];

/// Where the installed plugins' packages live, relative to Nebo's folder.
/// The folder of each skill a plugin ships is read by every run and changed
/// by none: `use_skill` hands the model that folder ("This skill's files are
/// in: …") and the skill's body names the files in it. A skill's folder is
/// found as the skill loader finds it: from the plugin's own folder down,
/// the first folder holding a `SKILL.md` (`napp::reader::walk_for_marker`).
/// The rest of a package, the plugin's program above all, stays closed: a
/// plugin runs through its tool, with its accounts.
const PLUGIN_PACKAGES: &[&str] = &["nebo/plugins", "user/plugins"];

/// The file that makes a folder a skill's, as the skill loader looks for it
/// (any case).
const SKILL_MARKER: &str = "SKILL.md";

/// The installed plugins, their programs and their data: open to a
/// workflow's command step alone (`ToolContext::trusted_plugin_env`), which
/// runs them with their auth env as the owner's own step. Their accounts
/// (`nebo/plugin-profiles`) stay closed to it as to every command.
const PLUGINS: &[&str] = &["nebo/plugins", "user/plugins", "appdata/plugins"];

/// What a path is reached for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Read: the installed plugins' skills are open to it.
    Read,
    /// Created, changed or removed.
    Write,
}

/// Nebo's folders, closed but for the parts one run works in.
#[derive(Debug, Clone)]
pub struct NeboFiles {
    /// This Nebo's folder, as named.
    root: PathBuf,
    /// This Nebo's folder with every link resolved: what the operating
    /// system checks, and what a path through a link lands on.
    real: PathBuf,
    /// Every other Nebo folder on this computer, closed whole, as named and
    /// resolved.
    others: Vec<(PathBuf, PathBuf)>,
    /// The open parts of this Nebo's folder, relative to it.
    open: Vec<PathBuf>,
}

impl NeboFiles {
    /// The fence for the run `ctx` belongs to, or `None` when Nebo's folder
    /// can't be found. Every other Nebo folder this computer may have is
    /// closed with it. A workflow's command step also works in the installed
    /// plugins (`PLUGINS`).
    pub fn of(ctx: &crate::origin::ToolContext) -> Option<Self> {
        let root = config::data_dir().ok()?;
        let session = crate::checkpoint::session_dir(&ctx.session_id);
        Some(Self::at(&root, &config::nebo_roots(), &session, ctx.trusted_plugin_env))
    }

    /// The fence of the folder `root`, with every other folder of `roots`
    /// closed whole (`root` itself may be among them), `session` (a folder
    /// under `root`) open as the run's own, and the installed plugins open
    /// when `plugins`.
    pub fn at(root: &Path, roots: &[PathBuf], session: &Path, plugins: bool) -> Self {
        let root = lexical(&absolute(root, None));
        let real = resolved(&root);
        let mut others: Vec<(PathBuf, PathBuf)> = Vec::new();
        for other in roots {
            let named = lexical(&absolute(other, None));
            let landed = resolved(&named);
            if named != root && landed != real && !others.iter().any(|(_, r)| *r == landed) {
                others.push((named, landed));
            }
        }
        let mut open: Vec<PathBuf> = OPEN.iter().map(PathBuf::from).collect();
        if plugins {
            open.extend(PLUGINS.iter().map(PathBuf::from));
        }
        if let Ok(rel) = lexical(&absolute(session, None)).strip_prefix(&root)
            && rel.components().count() >= 2
        {
            open.push(rel.to_path_buf());
        }
        Self { root, real, others, open }
    }

    /// Every Nebo folder, with its links resolved: this one's first.
    pub fn real_roots(&self) -> Vec<PathBuf> {
        std::iter::once(self.real.clone()).chain(self.others.iter().map(|(_, real)| real.clone())).collect()
    }

    /// The open parts, as real paths.
    pub fn real_open(&self) -> Vec<PathBuf> {
        self.open.iter().map(|rel| self.real.join(rel)).collect()
    }

    /// The folders of the skills the installed plugins ship, as real paths:
    /// open to read, never to change (those in an open part excepted).
    /// Found as the skill loader finds them, from each plugin's own folder.
    pub fn real_skills(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for packages in PLUGIN_PACKAGES {
            let Ok(plugins) = std::fs::read_dir(self.real.join(packages)) else { continue };
            for plugin in plugins.flatten() {
                if plugin.file_type().is_ok_and(|t| t.is_dir()) {
                    napp::reader::walk_for_marker(&plugin.path(), SKILL_MARKER, &mut |dir| out.push(dir.to_path_buf()));
                }
            }
        }
        let open = self.real_open();
        out.retain(|dir| !open.iter().any(|o| dir.starts_with(o)));
        out
    }

    /// The workspace, where the employee's own documents go: the owner's
    /// (`config::workspace_dir`) for this Nebo's root, `files` under any other.
    pub fn workspace(&self) -> PathBuf {
        match config::data_dir() {
            Ok(live) if live == self.root => config::workspace_dir().unwrap_or_else(|_| self.root.join("files")),
            _ => self.root.join("files"),
        }
    }

    /// Whether `path` (relative ones against `cwd`), reached for `access`,
    /// is one of Nebo's own files: inside a Nebo folder, outside every part
    /// open to it, however it is spelled (`..`, a link, `/tmp` for
    /// `/private/tmp`).
    pub fn closes(&self, path: &Path, cwd: Option<&Path>, access: Access) -> bool {
        let named = lexical(&absolute(path, cwd));
        let landed = resolved(&named);
        [&named, &landed].into_iter().any(|p| self.closed(p, access))
    }

    fn closed(&self, path: &Path, access: Access) -> bool {
        self.fenced().any(|root| path.starts_with(root)) && !self.opens(path, access)
    }

    /// Every Nebo folder, as named and as resolved.
    fn fenced(&self) -> impl Iterator<Item = &PathBuf> {
        [&self.root, &self.real].into_iter().chain(self.others.iter().flat_map(|(named, real)| [named, real]))
    }

    /// Whether `path` is in an open part of this Nebo's folder or, read, in
    /// a skill an installed plugin ships.
    fn opens(&self, path: &Path, access: Access) -> bool {
        [&self.root, &self.real].into_iter().any(|root| {
            let Ok(rel) = path.strip_prefix(root) else { return false };
            self.open.iter().any(|o| rel.starts_with(o)) || (access == Access::Read && in_plugin_skill(root, rel))
        })
    }

    /// Every entry of every Nebo folder that is closed, as real paths, each
    /// the top-most one: the parents of the open parts and of the plugins'
    /// skills are entered, not closed whole (the skills are kept unchanged
    /// by the caller, from `real_skills`). Links are left out: one that leads
    /// back into a folder is closed at what it leads to.
    pub fn closed_entries(&self) -> Vec<PathBuf> {
        let reached: Vec<PathBuf> = self.real_open().into_iter().chain(self.real_skills()).collect();
        let mut out = Vec::new();
        for root in self.real_roots() {
            if reached.iter().any(|r| r.starts_with(&root)) {
                walk(&root, &reached, &mut out);
            } else if root.exists() {
                out.push(root);
            }
        }
        out.sort();
        out.dedup();
        let all = out.clone();
        out.retain(|p| !all.iter().any(|q| q != p && p.starts_with(q)));
        out
    }

    /// The ways a command's text names a Nebo folder, each with the folder
    /// it means: as named, as resolved, through `~`, `$HOME` and (Windows)
    /// the variables the folders sit under, and with spaces escaped.
    fn spellings(&self) -> Vec<(String, PathBuf)> {
        let variables = variables();
        let mut out: Vec<(String, PathBuf)> = Vec::new();
        for root in self.fenced() {
            let s = root.to_string_lossy().into_owned();
            let mut forms = vec![s.clone()];
            for (var, value) in &variables {
                if let Some(rest) = s.strip_prefix(value.to_string_lossy().as_ref())
                    && (rest.is_empty() || rest.starts_with(['/', '\\']))
                {
                    forms.push(format!("{var}{rest}"));
                }
            }
            for f in forms {
                if ESCAPES && f.contains(' ') {
                    out.push((f.replace(' ', "\\ "), root.clone()));
                }
                out.push((f, root.clone()));
            }
        }
        out.sort();
        out.dedup();
        // Longest first, so the escaped form is found before its prefix.
        out.sort_by_key(|(s, _)| std::cmp::Reverse(s.len()));
        out
    }

    /// The first of Nebo's own files a command's text names, if any. A
    /// folder's name may hold spaces (`Application Support`), so a path is
    /// the spelling found and what follows it. The text can't say whether
    /// the command reads or writes, so a plugin's skill is named freely: the
    /// command itself can't change it (`confine`).
    pub fn named_in(&self, text: &str, cwd: Option<&Path>) -> Option<String> {
        // macOS's and Windows' usual filesystems don't mind a name's case,
        // so neither does the check there. ASCII folding keeps every offset.
        let fold = |s: &str| if cfg!(any(target_os = "macos", windows)) { s.to_ascii_lowercase() } else { s.to_string() };
        let folded = fold(text);
        for (spelling, meant) in self.spellings() {
            for (at, _) in folded.match_indices(fold(&spelling).as_str()) {
                let end = at + spelling.len();
                let rest = path_token(&text[end..]);
                let path = PathBuf::from(format!("{}{rest}", meant.to_string_lossy()));
                if self.closes(&path, cwd, Access::Read) {
                    let written = &text[at..end];
                    return Some(format!("{}{rest}", if ESCAPES { written.replace("\\ ", " ") } else { written.to_string() }));
                }
            }
        }
        None
    }
}

/// Every entry under `dir` that is closed, top-most first: an entry in a
/// part `reached` is left alone, a folder above one is entered.
fn walk(dir: &Path, reached: &[PathBuf], out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        if reached.iter().any(|r| path.starts_with(r)) {
            continue;
        }
        if kind.is_dir() && reached.iter().any(|r| r.starts_with(&path)) {
            walk(&path, reached, out);
        } else {
            out.push(path);
        }
    }
}

/// Whether `rel`, a path in the Nebo folder `root`, is in the folder of a
/// skill an installed plugin ships: at or under the first folder, from the
/// plugin's own folder down, that holds a `SKILL.md`, as the skill loader
/// walks it.
fn in_plugin_skill(root: &Path, rel: &Path) -> bool {
    PLUGIN_PACKAGES.iter().any(|packages| {
        let Ok(inner) = rel.strip_prefix(packages) else { return false };
        let mut dir = root.join(packages);
        inner.components().any(|part| {
            dir.push(part);
            holds_skill(&dir)
        })
    })
}

/// Whether the folder `dir` holds a skill's `SKILL.md` (any case).
fn holds_skill(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| entries.flatten().any(|e| e.file_name().eq_ignore_ascii_case(SKILL_MARKER)))
}

/// The variables a command names a folder through, with their values.
fn variables() -> Vec<(&'static str, PathBuf)> {
    let mut out = Vec::new();
    if let Some(home) = dirs::home_dir() {
        let names: &[&'static str] = if cfg!(windows) { &["~", "$HOME", "%USERPROFILE%", "$env:USERPROFILE"] } else { &["~", "$HOME", "${HOME}"] };
        out.extend(names.iter().map(|n| (*n, home.clone())));
    }
    if cfg!(windows)
        && let Some(appdata) = dirs::data_dir()
    {
        out.extend(["%APPDATA%", "$env:APPDATA"].map(|n| (n, appdata.clone())));
    }
    out
}

/// Whether a command's shell escapes a character with `\\`: not on
/// Windows, where it separates a path's parts.
const ESCAPES: bool = !cfg!(windows);

/// The rest of a path in a command's text: up to the first unescaped
/// space, quote or shell operator, escapes removed.
fn path_token(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' if ESCAPES => match chars.next() {
                Some(next) => out.push(next),
                None => break,
            },
            c if c.is_whitespace() => break,
            '"' | '\'' | '`' | ';' | '|' | '&' | '<' | '>' | '(' | ')' | '$' | '*' | '?' | '[' | '{' => break,
            c => out.push(c),
        }
    }
    out
}

/// `path` made absolute: `~` expanded, a relative path taken from `cwd`
/// (or the process's folder).
fn absolute(path: &Path, cwd: Option<&Path>) -> PathBuf {
    let expanded = types::pathres::expand(&path.to_string_lossy());
    if expanded.is_absolute() {
        return expanded;
    }
    match cwd {
        Some(cwd) => absolute(cwd, None).join(expanded),
        None => std::path::absolute(&expanded).unwrap_or(expanded),
    }
}

/// `path` with `.` and `..` worked out from the words alone.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `path` with every link in the part that exists resolved, and the rest
/// kept as written.
fn resolved(path: &Path) -> PathBuf {
    let mut existing = path.to_path_buf();
    let mut rest = Vec::new();
    loop {
        if let Ok(real) = std::fs::canonicalize(&existing) {
            let mut out = real;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (existing.file_name().map(|n| n.to_os_string()), existing.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Access::{Read, Write};
    use super::*;

    /// A Nebo folder with a plugin installed: one whose skills sit under a
    /// version folder, one whose skill has no version folder and a
    /// lower-case marker.
    fn home() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("nebo-home");
        for d in [
            "files",
            "logs",
            "data",
            "sessions/s1/tool-results",
            "sessions/s2",
            "nebo/skills/x",
            "nebo/plugin-profiles/ledger",
            "appdata/plugins/ledger",
            "worktrees/w",
            "user/plugins/ledger/0.1.0/skills/ledger-bills/references",
            "nebo/plugins/books/skills/books-shared",
        ] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        for (f, body) in [
            ("settings.json", "{\"accessSecret\": \"s\"}"),
            ("logs/nebo.log", "log"),
            ("nebo/plugin-profiles/ledger/creds.json", "{\"refresh\": \"r\"}"),
            ("user/plugins/ledger/0.1.0/ledger", "#!/bin/sh"),
            ("user/plugins/ledger/0.1.0/plugin.json", "{}"),
            ("user/plugins/ledger/0.1.0/skills/ledger-bills/SKILL.md", "---\nname: ledger-bills\n---\n"),
            ("user/plugins/ledger/0.1.0/skills/ledger-bills/references/rates.md", "rates"),
            ("nebo/plugins/books/skills/books-shared/skill.md", "---\nname: books-shared\n---\n"),
        ] {
            std::fs::write(root.join(f), body).unwrap();
        }
        (dir, root)
    }

    fn fence(root: &Path) -> NeboFiles {
        NeboFiles::at(root, &[], &root.join("sessions/s1"), false)
    }

    #[test]
    fn the_folder_is_closed_but_for_the_parts_an_employee_works_in() {
        let (_d, root) = home();
        let fence = fence(&root);
        for closed in ["settings.json", "logs/nebo.log", "data/nebo.db", "sessions/s2/x", "nebo/plugin-profiles/a", "appdata/plugins/p/db", "bot_id", ""] {
            assert!(fence.closes(&root.join(closed), None, Read), "{closed} must be closed");
        }
        for open in ["files/report.md", "files", "worktrees/w/src/main.rs", "sessions/s1/tool-results/r.txt", "nebo/skills/x/SKILL.md", "packs/acme/COMPANY.md"] {
            assert!(!fence.closes(&root.join(open), None, Write), "{open} must be open");
        }
        assert!(!fence.closes(Path::new("/etc/hosts"), None, Write), "outside the folder is not the fence's");
    }

    /// `use_skill` hands the model a plugin skill's folder ("This skill's
    /// files are in: …"): it and everything in it read, nothing in it
    /// changed. The plugin's program, its manifest, its accounts and its data
    /// stay closed; a marker planted elsewhere opens nothing.
    #[test]
    fn the_skills_a_plugin_ships_are_read_and_never_changed() {
        let (_d, root) = home();
        let fence = fence(&root);
        let skill = "user/plugins/ledger/0.1.0/skills/ledger-bills";
        for read in [
            skill.to_string(),
            format!("{skill}/SKILL.md"),
            format!("{skill}/references/rates.md"),
            format!("{skill}/references/not-yet.md"),
            "nebo/plugins/books/skills/books-shared/skill.md".to_string(),
        ] {
            assert!(!fence.closes(&root.join(&read), None, Read), "{read} must be readable");
            assert!(fence.closes(&root.join(&read), None, Write), "{read} must never be changed");
        }
        for closed in [
            "user/plugins/ledger/0.1.0/ledger",
            "user/plugins/ledger/0.1.0/plugin.json",
            "user/plugins/ledger/0.1.0/skills",
            "user/plugins",
            "nebo/plugin-profiles/ledger/creds.json",
            "appdata/plugins/ledger/state.db",
        ] {
            assert!(fence.closes(&root.join(closed), None, Read), "{closed} must be closed");
        }
        std::fs::write(root.join("sessions/s2/SKILL.md"), "planted").unwrap();
        assert!(fence.closes(&root.join("sessions/s2/SKILL.md"), None, Read), "a marker outside a plugin opens nothing");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("settings.json"), root.join(skill).join("link")).unwrap();
            assert!(fence.closes(&root.join(skill).join("link"), None, Read), "a link in a skill to a closed file");
        }
        let workflow = NeboFiles::at(&root, &[], &root.join("sessions/s1"), true);
        assert!(!workflow.closes(&root.join(skill).join("SKILL.md"), None, Write), "a workflow step works in the plugins");
        let real = std::fs::canonicalize(&root).unwrap();
        let mut skills = fence.real_skills();
        skills.sort();
        assert_eq!(skills, [real.join("nebo/plugins/books/skills/books-shared"), real.join(skill)]);
        assert!(workflow.real_skills().is_empty(), "open to a workflow step, nothing is read-only");
    }

    #[test]
    fn a_workflow_step_also_works_in_the_installed_plugins_and_nothing_more() {
        let (_d, root) = home();
        let fence = NeboFiles::at(&root, &[], &root.join("sessions/s1"), true);
        for open in ["nebo/plugins/odoo/1.0.0/odoo", "user/plugins/mine/bin", "appdata/plugins/odoo/cache.db", "files/a.md"] {
            assert!(!fence.closes(&root.join(open), None, Write), "{open} must be open to a workflow step");
        }
        for closed in ["settings.json", "logs/nebo.log", "data/nebo.db", "nebo/plugin-profiles/gws/creds.json", "sessions/s2/x"] {
            assert!(fence.closes(&root.join(closed), None, Read), "{closed} must stay closed to a workflow step");
        }
        let model = self::fence(&root);
        assert!(model.closes(&root.join("appdata/plugins/odoo/cache.db"), None, Read), "a model's command never reaches plugin data");
    }

    /// Another Nebo on this computer (the platform-native folder while this
    /// one runs on `NEBO_HOME`, the pre-v5 one, `~/.nebo`) is closed whole,
    /// its workspace included. This Nebo's own parts stay open, even when
    /// its folder sits inside another's, and a second name for this folder
    /// is this folder.
    #[test]
    fn every_other_nebo_folder_is_closed_whole() {
        let (d, root) = home();
        let other = d.path().join("local-share-nebo");
        std::fs::create_dir_all(other.join("files")).unwrap();
        std::fs::write(other.join("settings.json"), "{\"accessSecret\": \"theirs\"}").unwrap();
        let gone = d.path().join("dot-nebo");
        let fence = NeboFiles::at(&root, &[root.clone(), other.clone(), gone.clone()], &root.join("sessions/s1"), false);
        for closed in [other.clone(), other.join("settings.json"), other.join("files/a.md"), gone.join("settings.json")] {
            assert!(fence.closes(&closed, None, Read), "{} must be closed", closed.display());
        }
        assert!(!fence.closes(&root.join("files/a.md"), None, Write));
        let o = other.to_string_lossy();
        assert!(fence.named_in(&format!("cat {o}/settings.json"), None).is_some());
        assert!(fence.named_in(&format!("find {o} -name '*.json'"), None).is_some());

        // This Nebo's folder inside another's.
        let outer = d.path().join("outer");
        let inner = outer.join("instance");
        std::fs::create_dir_all(inner.join("files")).unwrap();
        std::fs::write(outer.join("settings.json"), "{}").unwrap();
        let nested = NeboFiles::at(&inner, &[outer.clone()], &inner.join("sessions/s1"), false);
        assert!(!nested.closes(&inner.join("files/a.md"), None, Write), "this Nebo's workspace");
        assert!(nested.closes(&inner.join("settings.json"), None, Read));
        assert!(nested.closes(&outer.join("settings.json"), None, Read));

        // A second name for this folder.
        #[cfg(unix)]
        {
            let alias = d.path().join("alias");
            std::os::unix::fs::symlink(&root, &alias).unwrap();
            let aliased = NeboFiles::at(&root, &[alias.clone()], &root.join("sessions/s1"), false);
            assert!(!aliased.closes(&alias.join("files/a.md"), None, Write), "its workspace by the other name");
            assert!(aliased.closes(&alias.join("settings.json"), None, Read));
        }
    }

    /// A command names another Nebo's folder through `$HOME` or `~` as
    /// often as by its full path.
    #[test]
    fn a_command_naming_a_folder_through_home_is_found() {
        let Some(user_home) = dirs::home_dir() else { return };
        let (_d, root) = home();
        let name = format!(".nebo-fence-proof-{}", std::process::id());
        let fence = NeboFiles::at(&root, &[user_home.join(&name)], &root.join("sessions/s1"), false);
        for text in [
            format!("cat $HOME/{name}/settings.json"),
            format!("cat \"${{HOME}}/{name}/logs/nebo.log\""),
            format!("grep -r secret ~/{name}"),
        ] {
            assert!(fence.named_in(&text, None).is_some(), "{text}");
        }
        assert!(fence.named_in(&format!("cat ~/{name}-notes.txt"), None).is_none(), "a sibling that only starts alike");
    }

    #[test]
    fn a_path_spelled_around_the_fence_is_still_closed() {
        let (_d, root) = home();
        let fence = fence(&root);
        assert!(fence.closes(&root.join("files/../settings.json"), None, Read), "..");
        assert!(fence.closes(Path::new("../settings.json"), Some(&root.join("files")), Read), "relative to the run's folder");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("settings.json"), root.join("files/link")).unwrap();
            assert!(fence.closes(&root.join("files/link"), None, Read), "a link in the workspace to a closed file");
        }
        let real = std::fs::canonicalize(&root).unwrap();
        assert!(fence.closes(&real.join("logs/nebo.log"), None, Read), "the resolved spelling");
    }

    #[test]
    fn closed_entries_are_the_topmost_closed_ones() {
        let (d, root) = home();
        let other = d.path().join("other-nebo");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("settings.json"), "{}").unwrap();
        let fence = NeboFiles::at(&root, &[other.clone()], &root.join("sessions/s1"), false);
        let real = std::fs::canonicalize(&root).unwrap();
        let mut got: Vec<String> = fence
            .closed_entries()
            .iter()
            .filter_map(|p| p.strip_prefix(&real).ok())
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        got.sort();
        assert_eq!(
            got,
            [
                "appdata/plugins",
                "data",
                "logs",
                "nebo/plugin-profiles",
                "sessions/s2",
                "settings.json",
                "user/plugins/ledger/0.1.0/ledger",
                "user/plugins/ledger/0.1.0/plugin.json",
            ]
        );
        assert!(fence.closed_entries().contains(&std::fs::canonicalize(&other).unwrap()), "another Nebo's folder, whole");
    }

    #[test]
    fn a_command_naming_a_closed_file_is_found() {
        let (_d, root) = home();
        let fence = fence(&root);
        let r = root.to_string_lossy();
        assert!(fence.named_in(&format!("cat {r}/settings.json | head"), None).is_some());
        assert!(fence.named_in(&format!("grep -r gmail \"{r}/logs\""), None).is_some());
        assert!(fence.named_in(&format!("find {r} -type f"), None).is_some(), "the folder itself");
        assert!(fence.named_in(&format!("ls {r}/files/ && cat {r}/files/a.md"), None).is_none());
        assert!(fence.named_in(&format!("cat {r}/user/plugins/ledger/0.1.0/skills/ledger-bills/references/rates.md"), None).is_none(), "a plugin's skill");
        assert!(fence.named_in(&format!("{r}/user/plugins/ledger/0.1.0/ledger bills list"), None).is_some(), "a plugin's program");
        assert!(fence.named_in("cat /etc/hosts", None).is_none());
        let spaced = root.join("Application Support");
        std::fs::create_dir_all(spaced.join("files")).unwrap();
        let fence = self::fence(&spaced);
        let s = spaced.to_string_lossy();
        assert!(fence.named_in(&format!("cat {}/settings.json", s.replace(' ', "\\ ")), None).is_some(), "escaped spaces");
        assert!(fence.named_in(&format!("cat \"{s}/files/a.md\""), None).is_none());
    }
}
