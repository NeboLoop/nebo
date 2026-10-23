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
//! names or reaches such a path), and the command itself (`confine` runs it
//! where those paths can't be read or changed at all, and the plugins'
//! skills can't be changed).

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

/// A cloud bot's `$HOME` (`/data/home/nebo`), relative to Nebo's folder:
/// the employee's own home there, its shell profile and the tools'
/// settings it keeps. Open on a cloud bot only (`crate::cloud_bot`); on any
/// other install a `home` in Nebo's folder is Nebo's own.
const CLOUD_HOME: &str = "home";

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
        Some(Self::at(&root, &config::nebo_roots(), &session, ctx.trusted_plugin_env, crate::cloud_bot()))
    }

    /// The fence of the folder `root`, with every other folder of `roots`
    /// closed whole (`root` itself may be among them), `session` (a folder
    /// under `root`) open as the run's own, the installed plugins open
    /// when `plugins`, and the bot's home (`CLOUD_HOME`) open when `cloud`.
    pub fn at(root: &Path, roots: &[PathBuf], session: &Path, plugins: bool, cloud: bool) -> Self {
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
        if cloud {
            open.push(PathBuf::from(CLOUD_HOME));
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

    /// Where `path` sits in this Nebo's folder (relative to it, empty for
    /// the folder itself), as named or with links resolved; `None` outside.
    pub fn part_of(&self, path: &Path) -> Option<PathBuf> {
        let named = lexical(&absolute(path, None));
        let landed = resolved(&named);
        [&named, &landed].into_iter().find_map(|p| {
            [&self.root, &self.real].into_iter().find_map(|root| p.strip_prefix(root).ok().map(Path::to_path_buf))
        })
    }

    /// The folder every employee's own package is in (`user/agents`), open
    /// to it: an app's pages and sources are in its folder there.
    pub fn agents_dir(&self) -> PathBuf {
        self.root.join("user/agents")
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
    /// open to it, however it is spelled (`..`, a link, `..` after a link as
    /// the system walks it, `/tmp` for `/private/tmp`).
    pub fn closes(&self, path: &Path, cwd: Option<&Path>, access: Access) -> bool {
        let full = absolute(path, cwd);
        let named = lexical(&full);
        let landed = resolved(&named);
        let walked = physical(&full);
        [&named, &landed, &walked].into_iter().any(|p| self.closed(p, access))
    }

    /// Whether `path` is a Nebo folder or a folder above one, as named or
    /// as resolved: a second name for it (a link) reaches every file in it.
    fn above(&self, path: &Path) -> bool {
        let named = lexical(path);
        let walked = physical(path);
        self.fenced().any(|root| root.starts_with(&named) || root.starts_with(&walked))
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

    /// The first of Nebo's own files a command reaches, run from `cwd` (the
    /// process's folder when `None`), however it spells the path: the
    /// folder each of its commands runs in (`policy::located`, a move inside
    /// a subshell ending with it), the words of each (an option's value
    /// included) taken from that folder, the files its redirects name, the
    /// files its filename patterns match, and what a link it makes (`ln`,
    /// `link`) leads to. Each meets the one test (`closes`), links followed,
    /// those the command makes before included. The words can't say whether
    /// the command reads or writes, so a plugin's skill is reached freely,
    /// but a hard link to one is refused: it is a second name for the file
    /// that every fence misses. What only running the command knows (`$X`,
    /// `$(…)`) is not seen here; but a command that may stand where only
    /// running it knows (`Placed::lost`, or a `-C` folder only running it
    /// knows) names a relative path the check can't place: that is refused
    /// too ([`Reached::Unplaced`]), and an absolute path works instead.
    pub fn reached_by(&self, cmd: &str, cwd: Option<&Path>) -> Option<Reached> {
        let start = lexical(&absolute(cwd.unwrap_or(Path::new(".")), None));
        let placed = crate::policy::located(cmd, Some(&start.to_string_lossy()));
        let known = |here: &Option<PathBuf>| here.as_deref().map(lexical);
        let reach = |p: &crate::policy::Placed| p.reach.iter().map(|f| lexical(f)).collect::<Vec<_>>();
        // The links the command makes, each refused when it leads to one of
        // Nebo's own files or (a symbolic one) to a folder above one; a path
        // through one made earlier in the command goes on from where it leads.
        let links: Vec<Link> = placed.iter().flat_map(|p| links(p.sub.words(), &reach(p))).collect();
        for link in &links {
            let access = if link.symbolic { Access::Read } else { Access::Write };
            if through(&link.to, &links).iter().any(|to| self.closes(to, None, access) || (link.symbolic && self.above(to))) {
                return Some(Reached::Nebo(link.target.clone()));
            }
        }
        let reaches = |path: &Path| through(path, &links).iter().any(|p| self.closes(p, None, Access::Read));
        let from = |folders: &[PathBuf], word: &str| {
            path_words(word)
                .into_iter()
                .find(|w| folders.iter().filter_map(|f| crate::policy::reached(Some(f.as_path()), w)).any(|p| reaches(&p)))
                .map(str::to_string)
        };
        for p in &placed {
            let mut here = reach(p);
            // A command's own folder option (`tar -C dir`, `git -C dir`,
            // `make -C dir`, `--directory=dir`): its paths are taken from
            // there too.
            let (folders, unknown) = own_folders(p.sub.words());
            for f in &folders {
                let more: Vec<PathBuf> = here.iter().filter_map(|h| crate::policy::reached(Some(h), f)).map(|d| lexical(&d)).collect();
                here.extend(more);
            }
            // A redirect on a subshell around the command is taken from
            // where that subshell was entered.
            let around: Vec<PathBuf> = here.iter().cloned().chain(p.outer.iter().filter_map(known)).collect();
            if let Some(folder) = around.iter().find(|f| reaches(f)) {
                return Some(Reached::Nebo(folder.to_string_lossy().into_owned()));
            }
            // Standing where the text can't place it, a relative path is
            // nowhere the check can see.
            let lost = p.lost || unknown;
            let unplaced = |word: &str| {
                path_words(word).into_iter().find(|w| w.starts_with("~-") || (lost && relative_path(w))).map(str::to_string)
            };
            for word in p.sub.words().iter().flatten() {
                if let Some(named) = from(&here, word) {
                    return Some(Reached::Nebo(named));
                }
                if let Some(word) = unplaced(word) {
                    return Some(Reached::Unplaced(word));
                }
            }
            for target in p.sub.redirects().iter().flatten() {
                if let Some(named) = from(&around, target) {
                    return Some(Reached::Nebo(named));
                }
                if let Some(word) = unplaced(target) {
                    return Some(Reached::Unplaced(word));
                }
            }
            for pattern in p.sub.patterns() {
                if let Some(found) = around.iter().find_map(|f| self.matched(pattern, f)) {
                    return Some(Reached::Nebo(found));
                }
                if lost && !Path::new(pattern).is_absolute() && !pattern.starts_with('~') {
                    return Some(Reached::Unplaced(pattern.clone()));
                }
            }
        }
        None
    }

    /// The first of Nebo's own files the filename pattern `pattern`
    /// (`syntax::ShellCommand::patterns`), standing in `folder`, matches, or
    /// names as written when it matches nothing (the shell then passes it
    /// as it is).
    fn matched(&self, pattern: &str, folder: &Path) -> Option<String> {
        let mut budget = MAX_PATTERN_ENTRIES;
        for word in braces(pattern) {
            let anchored = anchored(&word, folder)?;
            let written = PathBuf::from(unbracket(&anchored));
            if self.closes(&written, None, Access::Read) {
                return Some(written.to_string_lossy().into_owned());
            }
            if let Some(hit) = expand(&anchored, &mut budget).into_iter().find(|p| self.closes(p, None, Access::Read)) {
                return Some(hit.to_string_lossy().into_owned());
            }
        }
        None
    }
}

/// What a command reaches that the fence keeps it from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reached {
    /// One of Nebo's own files, as the command names or reaches it.
    Nebo(String),
    /// A relative path (this word) the command names while standing where
    /// only running it knows: it could be one of Nebo's own files.
    Unplaced(String),
}

/// The folders a command's own folder options name (`-C dir`, `-Cdir`,
/// `--directory=dir`, `--directory dir`), and whether one is only known when
/// it runs.
fn own_folders(words: &[Option<String>]) -> (Vec<String>, bool) {
    let mut out = Vec::new();
    let mut unknown = false;
    let mut i = 1;
    while i < words.len() {
        let value = match words[i].as_deref() {
            Some("-C" | "--directory") => {
                i += 1;
                Some(words.get(i).cloned().flatten())
            }
            Some(w) if w.starts_with("--directory=") => Some(Some(w["--directory=".len()..].to_string())),
            Some(w) if w.starts_with("-C") && w.len() > 2 && w.is_char_boundary(2) => Some(Some(w[2..].to_string())),
            _ => None,
        };
        match value {
            Some(Some(v)) => out.push(v),
            Some(None) => unknown = true,
            None => {}
        }
        i += 1;
    }
    (out, unknown)
}

/// Whether `word` reads as a relative path the shell would take from where
/// it stands: one with a folder in it, or one starting with `.`.
fn relative_path(word: &str) -> bool {
    !word.starts_with(['/', '~', '-', '$']) && !Path::new(word).is_absolute() && (word.contains('/') || word.starts_with('.'))
}

/// A link a command makes.
struct Link {
    /// Where the link is, made absolute.
    at: PathBuf,
    /// What it leads to, made absolute.
    to: PathBuf,
    /// The target as the command writes it.
    target: String,
    /// A symbolic link, not a hard one.
    symbolic: bool,
}

/// The links the command `words` makes, standing in each of `folders`:
/// `ln` and `link`, a symbolic link's relative target taken from the folder
/// the link is made in (`-r` takes it from where the command stands, as a
/// hard link's is). The last operand may be the folder the link goes in or
/// the link itself; both are taken.
fn links(words: &[Option<String>], folders: &[PathBuf]) -> Vec<Link> {
    let mut out = Vec::new();
    let Some(words) = words.iter().map(Option::as_deref).collect::<Option<Vec<&str>>>() else { return out };
    let Some((program, args)) = words.split_first() else { return out };
    let program = Path::new(program).file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if !matches!(program, "ln" | "gln" | "link") {
        return out;
    }
    let (mut symbolic, mut relative) = (false, false);
    let mut into: Option<&str> = None;
    let mut operands: Vec<&str> = Vec::new();
    let mut args = args.iter().copied();
    let mut options = program != "link";
    while let Some(arg) = args.next() {
        match arg {
            "--" if options => options = false,
            "--symbolic" if options => symbolic = true,
            "--relative" if options => relative = true,
            "--target-directory" | "--suffix" if options => {
                let value = args.next();
                if arg == "--target-directory" {
                    into = value;
                }
            }
            a if options && a.starts_with("--") => {
                if let Some(dir) = a.strip_prefix("--target-directory=") {
                    into = Some(dir);
                }
            }
            a if options && a.starts_with('-') && a.len() > 1 => {
                for (i, c) in a.char_indices().skip(1) {
                    match c {
                        's' => symbolic = true,
                        'r' => relative = true,
                        't' | 'S' => {
                            let value = if a.len() > i + 1 { Some(&a[i + 1..]) } else { args.next() };
                            if c == 't' {
                                into = value;
                            }
                            break;
                        }
                        _ => {}
                    }
                }
            }
            a => operands.push(a),
        }
    }
    // Each place: the folder the link goes in, or the link itself.
    let (targets, places): (&[&str], Vec<(PathBuf, bool)>) = match (into, operands.as_slice()) {
        (Some(dir), all) => (all, vec![(PathBuf::from(dir), true)]),
        (None, [one]) => (std::slice::from_ref(one), vec![(PathBuf::from("."), true)]),
        (None, [targets @ .., last]) => (targets, vec![(PathBuf::from(last), true), (PathBuf::from(last), false)]),
        (None, []) => return out,
    };
    for folder in folders {
        for (place, inside) in &places {
            let Some(place) = crate::policy::reached(Some(folder.as_path()), &place.to_string_lossy()) else { continue };
            let place = lexical(&place);
            for target in targets {
                let at = if *inside {
                    place.join(Path::new(target).file_name().unwrap_or_default())
                } else {
                    place.clone()
                };
                let from = if symbolic && !relative { at.parent().map(Path::to_path_buf).unwrap_or_default() } else { folder.clone() };
                if let Some(to) = crate::policy::reached(Some(from.as_path()), target) {
                    out.push(Link { at, to, target: target.to_string(), symbolic });
                }
            }
        }
    }
    out
}

/// Every path `path` may be, through the links a command makes before it
/// reaches it: past a link, a path goes on from what the link leads to.
fn through(path: &Path, links: &[Link]) -> Vec<PathBuf> {
    let mut out = vec![path.to_path_buf()];
    let mut i = 0;
    while i < out.len() && out.len() <= links.len() * 4 {
        let current = out[i].clone();
        let parts: Vec<Component> = current.components().collect();
        for link in links {
            let mut prefix = PathBuf::new();
            for (n, part) in parts.iter().enumerate() {
                prefix.push(part.as_os_str());
                if lexical(&prefix) == link.at {
                    let mut next = link.to.clone();
                    next.extend(parts[n + 1..].iter().map(|p| p.as_os_str()));
                    if !out.contains(&next) {
                        out.push(next);
                    }
                    break;
                }
            }
        }
        i += 1;
    }
    out
}

/// The pattern `word` (one of a pattern's brace words) as an absolute
/// pattern, standing in `folder`: `~` is the home folder.
fn anchored(word: &str, folder: &Path) -> Option<String> {
    Some(match word.strip_prefix('~').filter(|rest| rest.is_empty() || rest.starts_with('/')) {
        Some(rest) => format!("{}{rest}", glob::Pattern::escape(&dirs::home_dir()?.to_string_lossy())),
        None if Path::new(word).is_absolute() => word.to_string(),
        None => format!("{}/{word}", glob::Pattern::escape(&folder.to_string_lossy())),
    })
}

/// The folders a filename pattern (`syntax::ShellCommand::patterns`)
/// standing in `folder` matches, for a `cd` to it: `None` when matching it
/// would read more than [`MAX_PATTERN_ENTRIES`] folder entries, or it
/// matches nothing (the `cd` then goes where only running it knows, or
/// fails).
pub(crate) fn folders_matching(pattern: &str, folder: &Path) -> Option<Vec<PathBuf>> {
    let mut budget = MAX_PATTERN_ENTRIES;
    let mut out = Vec::new();
    for word in braces(pattern) {
        out.extend(expand(&anchored(&word, folder)?, &mut budget).into_iter().filter(|p| p.is_dir()));
    }
    (!out.is_empty()).then_some(out)
}

/// How many folder entries one pattern may look through. A pattern that
/// needs more (`/*/*/*`) is left as written: the check never walks the
/// disk.
const MAX_PATTERN_ENTRIES: usize = 20_000;

/// The paths the absolute pattern `pattern` (`glob` syntax) matches, one
/// part at a time; nothing when matching it reads more than `budget` folder
/// entries (the budget is shared across one pattern's brace words).
fn expand(pattern: &str, budget: &mut usize) -> Vec<PathBuf> {
    let mut parts = pattern.split(|c| c == '/' || (cfg!(windows) && c == '\\'));
    let first = parts.next().unwrap_or_default();
    let mut found = vec![PathBuf::from(format!("{first}/"))];
    for part in parts.filter(|p| !p.is_empty()) {
        if !part.contains(['*', '?', '[']) {
            found.iter_mut().for_each(|p| p.push(part));
            continue;
        }
        let Ok(matcher) = glob::Pattern::new(part) else { return Vec::new() };
        // A leading `*` doesn't match a dotfile, as the shell's glob doesn't.
        let opts = glob::MatchOptions { require_literal_leading_dot: true, ..Default::default() };
        let mut next = Vec::new();
        for dir in &found {
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            for entry in entries.flatten() {
                if *budget == 0 {
                    return Vec::new();
                }
                *budget -= 1;
                if matcher.matches_with(&entry.file_name().to_string_lossy(), opts) {
                    next.push(entry.path());
                }
            }
        }
        found = next;
    }
    found.retain(|p| p.symlink_metadata().is_ok());
    found
}

/// The ways one word of a command may name a path: the word, an option's
/// value (`--file=x`, `-Ix`), and (not on Windows, where `:` follows a
/// drive) each part of a `:` list (`PATH=a:b`, `-v src:dst`).
fn path_words(word: &str) -> Vec<&str> {
    let mut out = vec![word];
    if let Some((_, value)) = word.split_once('=') {
        out.push(value);
    }
    if word.len() > 2 && word.starts_with('-') && !word.starts_with("--") && word.is_char_boundary(2) {
        out.push(&word[2..]);
    }
    if !cfg!(windows) {
        let parts: Vec<&str> = out.iter().flat_map(|w| w.split(':')).collect();
        out.extend(parts);
    }
    out.retain(|w| !w.is_empty());
    out.dedup();
    out
}

/// How many words one pattern's braces may stand for before the rest is
/// left unexpanded.
const MAX_BRACE_WORDS: usize = 1024;

/// The words a pattern's unquoted braces stand for, as the shell expands
/// them: `{a,b}` each of its parts, `{1..3}` and `{a..c}` each of the
/// sequence. A bracketed character (`[{]`) is itself.
fn braces(pattern: &str) -> Vec<String> {
    let mut out = Vec::new();
    expand_braces(pattern, &mut out);
    out
}

/// Append the words `pattern`'s braces stand for to `out`, depth-first and
/// stopping at `MAX_BRACE_WORDS`, so nesting can't take exponential time.
fn expand_braces(pattern: &str, out: &mut Vec<String>) {
    if out.len() >= MAX_BRACE_WORDS {
        return;
    }
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '[' => i = class_end(&chars, i).unwrap_or(i) + 1,
            '{' => {
                if let Some((end, parts)) = brace_parts(&chars, i) {
                    let prefix: String = chars[..i].iter().collect();
                    let suffix: String = chars[end + 1..].iter().collect();
                    for part in parts {
                        if out.len() >= MAX_BRACE_WORDS {
                            return;
                        }
                        expand_braces(&format!("{prefix}{part}{suffix}"), out);
                    }
                    return;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    out.push(pattern.to_string());
}

/// Where the bracket class opening at `chars[at]` closes: a `]` right after
/// `[` or `[!` is its own.
fn class_end(chars: &[char], at: usize) -> Option<usize> {
    let mut i = at + 1;
    if chars.get(i) == Some(&'!') {
        i += 1;
    }
    (i + 1..chars.len()).find(|&j| chars[j] == ']')
}

/// The parts the brace opening at `chars[at]` stands for, with where it
/// closes: `None` when it is no brace expansion (no `,` or `..` in it).
fn brace_parts(chars: &[char], at: usize) -> Option<(usize, Vec<String>)> {
    let mut depth = 0;
    let mut commas = Vec::new();
    let mut i = at;
    while i < chars.len() {
        match chars[i] {
            '[' => {
                i = class_end(chars, i).unwrap_or(i);
            }
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let inner: String = chars[at + 1..i].iter().collect();
                    if commas.is_empty() {
                        return sequence(&inner).map(|parts| (i, parts));
                    }
                    let mut parts = Vec::new();
                    let mut from = at + 1;
                    for &c in commas.iter().chain(std::iter::once(&i)) {
                        parts.push(chars[from..c].iter().collect());
                        from = c + 1;
                    }
                    return Some((i, parts));
                }
            }
            ',' if depth == 1 => commas.push(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// The words of a sequence brace (`1..3`, `a..c`, with an optional
/// `..step`), as the shell counts them.
fn sequence(inner: &str) -> Option<Vec<String>> {
    let parts: Vec<&str> = inner.split("..").collect();
    let (from, to, step) = match parts.as_slice() {
        [from, to] => (*from, *to, 1),
        [from, to, step] => (*from, *to, step.parse::<i64>().ok()?.unsigned_abs().max(1) as usize),
        _ => return None,
    };
    let words: Vec<String> = if let (Ok(a), Ok(b)) = (from.parse::<i64>(), to.parse::<i64>()) {
        let width = if from.starts_with('0') || to.starts_with('0') { from.len().max(to.len()) } else { 0 };
        let range: Box<dyn Iterator<Item = i64>> = if a <= b { Box::new(a..=b) } else { Box::new((b..=a).rev()) };
        range.step_by(step).take(MAX_BRACE_WORDS).map(|n| format!("{n:0width$}")).collect()
    } else {
        let (Ok(a), Ok(b)) = (from.parse::<char>(), to.parse::<char>()) else { return None };
        if !a.is_ascii_alphabetic() || !b.is_ascii_alphabetic() {
            return None;
        }
        let (a, b) = (a as u8, b as u8);
        let range: Vec<u8> = if a <= b { (a..=b).collect() } else { (b..=a).rev().collect() };
        range.into_iter().step_by(step).map(|c| (c as char).to_string()).collect()
    };
    Some(words)
}

/// A pattern as the path it names when it matches nothing: each
/// bracketed single character (`[*]`) is itself.
fn unbracket(pattern: &str) -> String {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '[' && chars.get(i + 2) == Some(&']') {
            out.push(chars[i + 1]);
            i += 3;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
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

/// `path` as the system walks it: each part that exists resolved before
/// the next is taken, so a `..` after a link leaves what the link leads to;
/// the parts that don't exist kept as written.
fn physical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => {
                out.push(other.as_os_str());
                if let Ok(real) = std::fs::canonicalize(&out) {
                    out = real;
                }
            }
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
        NeboFiles::at(root, &[], &root.join("sessions/s1"), false, false)
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
        assert!(fence.closes(&root.join("home/nebo/.zshrc"), None, Read), "a `home` in Nebo's folder is Nebo's own off a cloud bot");
        let cloud = NeboFiles::at(&root, &[], &root.join("sessions/s1"), false, true);
        assert!(!cloud.closes(&root.join("home/nebo/.zshrc"), None, Write), "a cloud bot's home is the employee's");
        assert!(cloud.closes(&root.join("settings.json"), None, Read), "the rest stays closed on a cloud bot");
    }

    /// A pattern is matched one part at a time, and one that needs more
    /// entries than `MAX_PATTERN_ENTRIES` is left as written.
    #[test]
    fn a_pattern_never_walks_the_disk() {
        let dir = tempfile::tempdir().unwrap();
        let many = dir.path().join("many");
        std::fs::create_dir_all(dir.path().join("few/a")).unwrap();
        std::fs::write(dir.path().join("few/a/x.md"), "").unwrap();
        std::fs::create_dir_all(&many).unwrap();
        for n in 0..=MAX_PATTERN_ENTRIES {
            std::fs::write(many.join(n.to_string()), "").unwrap();
        }
        let base = glob::Pattern::escape(&dir.path().to_string_lossy());
        assert_eq!(expand(&format!("{base}/few/*/*.md"), &mut MAX_PATTERN_ENTRIES.clone()), [dir.path().join("few/a/x.md")]);
        assert!(expand(&format!("{base}/few/*/none"), &mut MAX_PATTERN_ENTRIES.clone()).is_empty());
        assert!(expand(&format!("{base}/many/*"), &mut MAX_PATTERN_ENTRIES.clone()).is_empty(), "past the limit");
    }

    /// Brace expansion is bounded: deep nesting finishes at once and gives at
    /// most `MAX_BRACE_WORDS` words, and a numeric range is expanded.
    #[test]
    fn braces_are_bounded_and_ranges_expand() {
        let deep = format!("a{}", "{a,b}".repeat(40));
        let words = braces(&deep);
        assert_eq!(words.len(), MAX_BRACE_WORDS);
        assert_eq!(braces("x{1..3}y"), ["x1y", "x2y", "x3y"]);
        assert_eq!(braces("{a..c}"), ["a", "b", "c"]);
        assert_eq!(braces("p{2..1}"), ["p2", "p1"]);
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
        let workflow = NeboFiles::at(&root, &[], &root.join("sessions/s1"), true, false);
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
        let fence = NeboFiles::at(&root, &[], &root.join("sessions/s1"), true, false);
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
        let fence = NeboFiles::at(&root, &[root.clone(), other.clone(), gone.clone()], &root.join("sessions/s1"), false, false);
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
        let nested = NeboFiles::at(&inner, &[outer.clone()], &inner.join("sessions/s1"), false, false);
        assert!(!nested.closes(&inner.join("files/a.md"), None, Write), "this Nebo's workspace");
        assert!(nested.closes(&inner.join("settings.json"), None, Read));
        assert!(nested.closes(&outer.join("settings.json"), None, Read));

        // A second name for this folder.
        #[cfg(unix)]
        {
            let alias = d.path().join("alias");
            std::os::unix::fs::symlink(&root, &alias).unwrap();
            let aliased = NeboFiles::at(&root, &[alias.clone()], &root.join("sessions/s1"), false, false);
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
        let fence = NeboFiles::at(&root, &[user_home.join(&name)], &root.join("sessions/s1"), false, false);
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
        let fence = NeboFiles::at(&root, &[other.clone()], &root.join("sessions/s1"), false, false);
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
