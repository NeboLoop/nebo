//! Nebo's own files: the ONE answer to "may an employee reach this path?"
//! for everything under Nebo's folder (`config::data_dir()`).
//!
//! An employee works in some of that folder: its workspace, the copies its
//! helpers work in, the company layers, the skills and employee packages it
//! runs, and its own conversation's saved results. Everything else there is
//! Nebo's own and never an employee's: the settings file holding the
//! server's secret, the logs, the database, the plugins' accounts and data,
//! the other conversations' files. On 2026-09-26 an employee asked to fix a
//! mail sign-in `cat` the settings file and put the server's secret into its
//! context; others read the logs looking for answers.
//!
//! Three doors apply this one fence: the file tools (the safeguard refuses a
//! path it closes), the text of a command (the safeguard refuses one that
//! names such a path), and the command itself (`confine` runs it where those
//! paths can't be read or changed at all).

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
];

/// Nebo's folder, closed but for the parts one run works in.
#[derive(Debug, Clone)]
pub struct NeboFiles {
    /// The folder as named.
    root: PathBuf,
    /// The folder with every link resolved: what the operating system
    /// checks, and what a path through a link lands on.
    real: PathBuf,
    /// The open parts, relative to the folder.
    open: Vec<PathBuf>,
}

impl NeboFiles {
    /// The fence for a run of session `session_id`, or `None` when Nebo's
    /// folder can't be found.
    pub fn of(session_id: &str) -> Option<Self> {
        let root = config::data_dir().ok()?;
        let session = crate::checkpoint::session_dir(session_id);
        Some(Self::at(&root, &session))
    }

    /// The fence of the folder `root`, with `session` (a folder under it)
    /// open as the run's own.
    pub fn at(root: &Path, session: &Path) -> Self {
        let root = lexical(&absolute(root, None));
        let real = resolved(&root);
        let mut open: Vec<PathBuf> = OPEN.iter().map(PathBuf::from).collect();
        if let Ok(rel) = lexical(&absolute(session, None)).strip_prefix(&root)
            && rel.components().count() >= 2
        {
            open.push(rel.to_path_buf());
        }
        Self { root, real, open }
    }

    /// Nebo's folder, with its links resolved.
    pub fn real_root(&self) -> &Path {
        &self.real
    }

    /// The open parts, as real paths.
    pub fn real_open(&self) -> Vec<PathBuf> {
        self.open.iter().map(|rel| self.real.join(rel)).collect()
    }

    /// The workspace, where the employee's own documents go.
    pub fn workspace(&self) -> PathBuf {
        self.root.join("files")
    }

    /// Whether `path` (relative ones against `cwd`) is one of Nebo's own
    /// files: inside the folder, outside every open part, however it is
    /// spelled (`..`, a link, `/tmp` for `/private/tmp`).
    pub fn closes(&self, path: &Path, cwd: Option<&Path>) -> bool {
        let named = lexical(&absolute(path, cwd));
        let landed = resolved(&named);
        [&named, &landed].into_iter().any(|p| self.closed(p))
    }

    fn closed(&self, path: &Path) -> bool {
        [&self.root, &self.real].into_iter().any(|root| match path.strip_prefix(root) {
            Ok(rel) => !self.open.iter().any(|o| rel.starts_with(o)),
            Err(_) => false,
        })
    }

    /// Every entry of the folder that is closed, as real paths, each the
    /// top-most one: the open parts' parents are entered, not closed whole.
    /// Links are left out: one that leads back into the folder is closed at
    /// what it leads to.
    pub fn closed_entries(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        self.walk(&self.real, Path::new(""), &mut out);
        out
    }

    fn walk(&self, dir: &Path, rel: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_symlink() {
                continue;
            }
            let rel = rel.join(entry.file_name());
            if self.open.iter().any(|o| rel.starts_with(o)) {
                continue;
            }
            if kind.is_dir() && self.open.iter().any(|o| o.starts_with(&rel)) {
                self.walk(&entry.path(), &rel, out);
            } else {
                out.push(entry.path());
            }
        }
    }

    /// The ways a command's text names the folder: as named, as resolved,
    /// from `~`, and with its spaces escaped.
    pub fn spellings(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for root in [&self.root, &self.real] {
            let s = root.to_string_lossy().into_owned();
            let mut forms = vec![s.clone()];
            if let Some(home) = dirs::home_dir()
                && let Some(rest) = s.strip_prefix(home.to_string_lossy().as_ref())
            {
                forms.push(format!("~{rest}"));
            }
            for f in forms {
                if f.contains(' ') {
                    out.push(f.replace(' ', "\\ "));
                }
                out.push(f);
            }
        }
        out.sort();
        out.dedup();
        // Longest first, so the escaped form is found before its prefix.
        out.sort_by_key(|s| std::cmp::Reverse(s.len()));
        out
    }

    /// The first of Nebo's own files a command's text names, if any. The
    /// folder's name may hold spaces (`Application Support`), so a path is
    /// the spelling found and what follows it.
    pub fn named_in(&self, text: &str, cwd: Option<&Path>) -> Option<String> {
        for spelling in self.spellings() {
            for (at, _) in text.match_indices(spelling.as_str()) {
                let named = format!("{}{}", spelling.replace("\\ ", " "), path_token(&text[at + spelling.len()..]));
                if self.closes(Path::new(&expand_home(&named)), cwd) {
                    return Some(named);
                }
            }
        }
        None
    }
}

/// The rest of a path in a command's text: up to the first unescaped
/// space, quote or shell operator, escapes removed.
fn path_token(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
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

fn expand_home(path: &str) -> String {
    match (path.strip_prefix('~'), dirs::home_dir()) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => {
            format!("{}{rest}", home.to_string_lossy())
        }
        _ => path.to_string(),
    }
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

    fn home() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("nebo-home");
        for d in ["files", "logs", "data", "sessions/s1/tool-results", "sessions/s2", "nebo/skills/x", "nebo/plugin-profiles", "appdata/plugins", "worktrees/w"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("settings.json"), "{\"accessSecret\": \"s\"}").unwrap();
        std::fs::write(root.join("logs/nebo.log"), "log").unwrap();
        (dir, root)
    }

    #[test]
    fn the_folder_is_closed_but_for_the_parts_an_employee_works_in() {
        let (_d, root) = home();
        let fence = NeboFiles::at(&root, &root.join("sessions/s1"));
        for closed in ["settings.json", "logs/nebo.log", "data/nebo.db", "sessions/s2/x", "nebo/plugin-profiles/a", "appdata/plugins/p/db", "bot_id", ""] {
            assert!(fence.closes(&root.join(closed), None), "{closed} must be closed");
        }
        for open in ["files/report.md", "files", "worktrees/w/src/main.rs", "sessions/s1/tool-results/r.txt", "nebo/skills/x/SKILL.md", "packs/acme/COMPANY.md"] {
            assert!(!fence.closes(&root.join(open), None), "{open} must be open");
        }
        assert!(!fence.closes(Path::new("/etc/hosts"), None), "outside the folder is not the fence's");
    }

    #[test]
    fn a_path_spelled_around_the_fence_is_still_closed() {
        let (_d, root) = home();
        let fence = NeboFiles::at(&root, &root.join("sessions/s1"));
        assert!(fence.closes(&root.join("files/../settings.json"), None), "..");
        assert!(fence.closes(Path::new("../settings.json"), Some(&root.join("files"))), "relative to the run's folder");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("settings.json"), root.join("files/link")).unwrap();
            assert!(fence.closes(&root.join("files/link"), None), "a link in the workspace to a closed file");
        }
        let real = std::fs::canonicalize(&root).unwrap();
        assert!(fence.closes(&real.join("logs/nebo.log"), None), "the resolved spelling");
    }

    #[test]
    fn closed_entries_are_the_topmost_closed_ones() {
        let (_d, root) = home();
        let fence = NeboFiles::at(&root, &root.join("sessions/s1"));
        let real = std::fs::canonicalize(&root).unwrap();
        let mut got: Vec<String> = fence
            .closed_entries()
            .iter()
            .map(|p| p.strip_prefix(&real).unwrap().to_string_lossy().into_owned())
            .collect();
        got.sort();
        assert_eq!(got, ["appdata/plugins", "data", "logs", "nebo/plugin-profiles", "sessions/s2", "settings.json"]);
    }

    #[test]
    fn a_command_naming_a_closed_file_is_found() {
        let (_d, root) = home();
        let fence = NeboFiles::at(&root, &root.join("sessions/s1"));
        let r = root.to_string_lossy();
        assert!(fence.named_in(&format!("cat {r}/settings.json | head"), None).is_some());
        assert!(fence.named_in(&format!("grep -r gmail \"{r}/logs\""), None).is_some());
        assert!(fence.named_in(&format!("find {r} -type f"), None).is_some(), "the folder itself");
        assert!(fence.named_in(&format!("ls {r}/files/ && cat {r}/files/a.md"), None).is_none());
        assert!(fence.named_in("cat /etc/hosts", None).is_none());
        let spaced = root.join("Application Support");
        std::fs::create_dir_all(spaced.join("files")).unwrap();
        let fence = NeboFiles::at(&spaced, &spaced.join("sessions/s1"));
        let s = spaced.to_string_lossy();
        assert!(fence.named_in(&format!("cat {}/settings.json", s.replace(' ', "\\ ")), None).is_some(), "escaped spaces");
        assert!(fence.named_in(&format!("cat \"{s}/files/a.md\""), None).is_none());
    }
}
