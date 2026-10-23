//! Nebo's secrets: the ONE answer to "may an employee reach this path?"
//! for Nebo's folders, this Nebo's (`config::data_dir()`) and every other
//! one this computer may have (`config::nebo_roots()`).
//!
//! An employee never reads or changes Nebo's secrets ([`PROTECTED`]): the
//! settings file holding the server's secret, Nebo's keys and sign-in
//! tokens, the database, the plugins' accounts, a linked runtime's keys and
//! settings, the browser profile holding the owner's signed-in sessions,
//! the logs (they can hold what a tool call carried, a token included), and
//! the other conversations' saved results (a Confidential chat stays its
//! own; the run's own session folder is its to use). Everything else in
//! Nebo's folder is normal to read and write. On 2026-09-26 an employee
//! asked to fix a mail sign-in `cat` the settings file and put the server's
//! secret into its context.
//!
//! The same list holds in every Nebo folder on this computer: another
//! install's secrets are its own (2026-09-27: a run whose `NEBO_HOME` was
//! fenced read a second Nebo's `settings.json` under `~/.local/share/nebo`).
//!
//! Three doors apply this one list: the file tools (the safeguard refuses a
//! path it closes), the text of a command (the safeguard refuses one that
//! names such a path), and the command itself (`confine` runs it where those
//! paths can't be read or changed at all).

use std::path::{Component, Path, PathBuf};

/// Nebo's secrets, relative to a Nebo folder: the one list every door
/// closes.
pub const PROTECTED: &[&str] = &[
    // The server's secret and the owner's sign-in.
    "settings.json",
    "neboai_token.cache",
    // Nebo's own keys: its local API, its MCP door, its browser extension.
    ".install-key",
    ".mcp-key",
    ".extension-secret",
    // The database and its backups.
    "data",
    // What tool calls carried, tokens included.
    "logs",
    // Every conversation's saved results, Confidential ones included; the
    // run's own is opened (`NeboFiles::at`).
    "sessions",
    // The plugins' accounts.
    "nebo/plugin-profiles",
    // A linked runtime's device key, its settings (with its keys) and logs.
    "link/oal",
    "link/agents.json",
    "link/logs",
    // Nebo's browser profile: the owner's signed-in sessions.
    "chromium-own-pages",
];

/// Nebo's secrets in every Nebo folder on this computer.
#[derive(Debug, Clone)]
pub struct NeboFiles {
    /// Each Nebo folder, as named and with every link resolved (what the
    /// operating system checks): this Nebo's first.
    roots: Vec<(PathBuf, PathBuf)>,
    /// The run's own session folder, relative to this Nebo's: its own
    /// saved results, open to it.
    own: Option<PathBuf>,
}

impl NeboFiles {
    /// The list for the run `ctx` belongs to, or `None` when Nebo's folder
    /// can't be found.
    pub fn of(ctx: &crate::origin::ToolContext) -> Option<Self> {
        let root = config::data_dir().ok()?;
        let session = crate::checkpoint::session_dir(&ctx.session_id);
        Some(Self::at(&root, &config::nebo_roots(), &session))
    }

    /// The secrets of the folder `root` and of every other folder of
    /// `roots` (`root` itself may be among them), with `session` (a folder
    /// under `root`) open as the run's own.
    pub fn at(root: &Path, roots: &[PathBuf], session: &Path) -> Self {
        let named = lexical(&absolute(root, None));
        let real = resolved(&named);
        let own = lexical(&absolute(session, None))
            .strip_prefix(&named)
            .ok()
            .filter(|rel| rel.starts_with("sessions") && rel.components().count() >= 2)
            .map(Path::to_path_buf);
        let mut all = vec![(named, real)];
        for other in roots {
            let named = lexical(&absolute(other, None));
            let landed = resolved(&named);
            if !all.iter().any(|(n, r)| *n == named || *r == landed) {
                all.push((named, landed));
            }
        }
        Self { roots: all, own }
    }

    /// The workspace, where the employee's own documents go: the owner's
    /// (`config::workspace_dir`) for this Nebo's folder, `files` under any
    /// other.
    pub fn workspace(&self) -> PathBuf {
        let root = &self.roots[0].0;
        match config::data_dir() {
            Ok(live) if live == *root => config::workspace_dir().unwrap_or_else(|_| root.join("files")),
            _ => root.join("files"),
        }
    }

    /// Whether `path` (relative ones against `cwd`) is one of Nebo's
    /// secrets, however it is spelled (`..`, a link, `/tmp` for
    /// `/private/tmp`).
    pub fn closes(&self, path: &Path, cwd: Option<&Path>) -> bool {
        let named = lexical(&absolute(path, cwd));
        let landed = resolved(&named);
        [&named, &landed].into_iter().any(|p| self.secret(p))
    }

    fn secret(&self, path: &Path) -> bool {
        self.roots.iter().enumerate().any(|(i, (named, real))| {
            [named, real].into_iter().any(|root| {
                let Ok(rel) = path.strip_prefix(root) else { return false };
                let own = i == 0 && self.own.as_ref().is_some_and(|own| rel.starts_with(own));
                !own && PROTECTED.iter().any(|p| rel.starts_with(p))
            })
        })
    }

    /// Every secret, in every Nebo folder, as a real path, whether it
    /// exists or not; and the run's own session folder, which is open.
    pub fn real_protected(&self) -> (Vec<PathBuf>, Option<PathBuf>) {
        let closed = self.roots.iter().flat_map(|(_, real)| PROTECTED.iter().map(move |p| real.join(p))).collect();
        (closed, self.own.as_ref().map(|own| self.roots[0].1.join(own)))
    }

    /// Every secret that exists, as a real path, the run's own session
    /// folder left out (each other session folder named instead).
    pub fn closed_entries(&self) -> Vec<PathBuf> {
        let (closed, own) = self.real_protected();
        let mut out = Vec::new();
        for path in closed {
            if !path.exists() {
                continue;
            }
            match &own {
                Some(own) if own.starts_with(&path) => {
                    for entry in std::fs::read_dir(&path).into_iter().flatten().flatten() {
                        if entry.path() != *own {
                            out.push(entry.path());
                        }
                    }
                }
                _ => out.push(path),
            }
        }
        out
    }

    /// The ways a command's text names a Nebo folder, each with the folder
    /// it means: as named, as resolved, through `~`, `$HOME` and (Windows)
    /// the variables the folders sit under, and with spaces escaped.
    fn spellings(&self) -> Vec<(String, PathBuf)> {
        let variables = variables();
        let mut out: Vec<(String, PathBuf)> = Vec::new();
        for root in self.roots.iter().flat_map(|(named, real)| [named, real]) {
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

    /// The first of Nebo's secrets a command's text names, if any. A
    /// folder's name may hold spaces (`Application Support`), so a path is
    /// the spelling found and what follows it.
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
                if self.closes(&path, cwd) {
                    let written = &text[at..end];
                    return Some(format!("{}{rest}", if ESCAPES { written.replace("\\ ", " ") } else { written.to_string() }));
                }
            }
        }
        None
    }
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
    use super::*;

    /// A Nebo folder holding every kind of thing an employee meets there.
    fn home() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("nebo-home");
        for d in ["files", "logs", "data", "sessions/s1", "sessions/s2", "nebo/plugin-profiles/p", "nebo/plugins/p/1.0.0/skills/s", "user/agents/Scores/ui", "user/sdk", "verify/v-1", "worktrees/site/v-1", "link/oal", "chromium-own-pages"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        for (f, body) in [("settings.json", "{\"accessSecret\": \"s\"}"), (".install-key", "k"), (".mcp-key", "k"), (".extension-secret", "k"), ("neboai_token.cache", "t"), ("logs/nebo.log", "log"), ("data/nebo.db", "db"), ("sessions/s2/r.txt", "theirs"), ("bot_id", "b")] {
            std::fs::write(root.join(f), body).unwrap();
        }
        (dir, root)
    }

    fn fence(root: &Path) -> NeboFiles {
        NeboFiles::at(root, &[], &root.join("sessions/s1"))
    }

    /// Only Nebo's secrets are closed: settings, keys, tokens, the database,
    /// the logs, other conversations' results, the plugins' accounts, a
    /// linked runtime's keys, the browser profile. Everything else in Nebo's
    /// folder, and the run's own session folder, is normal to use.
    #[test]
    fn only_nebos_secrets_are_closed() {
        let (_d, root) = home();
        let f = fence(&root);
        for closed in ["settings.json", ".install-key", ".mcp-key", ".extension-secret", "neboai_token.cache", "data/nebo.db", "data", "logs/nebo.log", "sessions/s2/r.txt", "sessions", "nebo/plugin-profiles/p/x", "link/oal/device.key", "link/agents.json", "link/logs/a.log", "chromium-own-pages/Default/Cookies"] {
            assert!(f.closes(&root.join(closed), None), "{closed} is a secret");
        }
        for open in ["", "user", "user/sdk/nebo.global.js", "user/agents/Scores/ui/index.html", "files/a.md", "nebo/plugins/p/1.0.0/skills/s/SKILL.md", "nebo/plugins/p/bin", "verify/v-1/x", "worktrees/site/v-1/src/a.ts", "sessions/s1/tool-results/r.txt", "bot_id", "models.yaml", "research/r.md"] {
            assert!(!f.closes(&root.join(open), None), "{open:?} is normal to use");
        }
        assert!(!f.closes(Path::new("/etc/hosts"), None), "outside Nebo's folder is not this list's");
    }

    /// Another Nebo on this computer keeps its secrets too, and its other
    /// files are as open as this one's.
    #[test]
    fn every_nebo_folder_keeps_its_secrets() {
        let (_d, root) = home();
        let (_o, other) = home();
        let f = NeboFiles::at(&root, &[root.clone(), other.clone()], &root.join("sessions/s1"));
        assert!(f.closes(&other.join("settings.json"), None));
        assert!(f.closes(&other.join("sessions/s1/r.txt"), None), "the run's own session is this Nebo's, not another's");
        assert!(!f.closes(&other.join("files/a.md"), None));
    }

    /// However a secret is spelled: `..`, a link to it or to a folder above
    /// it, a relative path from a folder inside.
    #[test]
    fn a_secret_spelled_around_the_list_is_still_closed() {
        let (d, root) = home();
        let f = fence(&root);
        assert!(f.closes(&root.join("files/../settings.json"), None));
        assert!(f.closes(Path::new("../settings.json"), Some(&root.join("files"))));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&root, d.path().join("alias")).unwrap();
            assert!(f.closes(&d.path().join("alias/data/nebo.db"), None));
            std::os::unix::fs::symlink(root.join("settings.json"), d.path().join("s.json")).unwrap();
            assert!(f.closes(&d.path().join("s.json"), None));
        }
    }

    /// The secrets that exist, for the sandbox: the run's own session
    /// folder stays out, each other one is named.
    #[test]
    fn closed_entries_name_each_secret_but_the_runs_own_session() {
        let (_d, root) = home();
        let real = std::fs::canonicalize(&root).unwrap();
        let entries = fence(&root).closed_entries();
        assert!(entries.contains(&real.join("settings.json")) && entries.contains(&real.join("data")));
        assert!(entries.contains(&real.join("sessions/s2")), "{entries:?}");
        assert!(!entries.iter().any(|e| e.starts_with(real.join("sessions/s1"))), "the run's own is open");
        assert!(!entries.contains(&real.join("sessions")));
        assert!(!entries.iter().any(|e| e.starts_with(real.join("user"))), "the rest of the folder is open");
    }

    /// A command's text that names a secret, through `~` too.
    #[test]
    fn a_command_naming_a_secret_is_found() {
        let (_d, root) = home();
        let f = fence(&root);
        let r = root.display();
        assert_eq!(f.named_in(&format!("cat {r}/settings.json"), None), Some(format!("{r}/settings.json")));
        assert_eq!(f.named_in(&format!("sqlite3 {r}/data/nebo.db .dump"), None), Some(format!("{r}/data/nebo.db")));
        assert_eq!(f.named_in(&format!("ls {r}/user/agents && cat {r}/user/sdk/x.js"), None), None, "the rest is normal");
        assert_eq!(f.named_in(&format!("ls {r}"), None), None, "listing the folder itself is fine");
    }
}
