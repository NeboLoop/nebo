//! Shell-command shapes the permission check and the shell tool read, and
//! the company's constitution document.

use serde::{Deserialize, Serialize};

/// One command a shell call runs, as the permission rules judge it: its
/// words with quoting removed and run-only wrappers (`nohup`, `timeout 5`,
/// `env`, …) stripped. A `None` word is only known when the call runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subcommand {
    assigns: bool,
    words: Vec<Option<String>>,
    /// An output redirect sends to a file.
    writes: bool,
    /// Each word as a filename pattern, beside `words`
    /// (`syntax::ShellCommand::globs`).
    globs: Vec<Option<String>>,
}

/// Shells whose `-c` argument is itself a script.
const SCRIPT_SHELLS: &[&str] = &["bash", "sh", "zsh", "dash", "ksh"];

/// How deep `bash -c "bash -c '…'"` / `eval` nesting is read before the
/// rest counts as unreadable.
const MAX_SCRIPT_DEPTH: usize = 8;

/// How a rule's command prefix covers one command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cover {
    No,
    Yes,
    /// Only an unknown word stands between the command and the prefix: it
    /// may be the command the rule names, or not. Never an allow's.
    Unread,
}

/// A command's second word that names a subcommand (`commit`, `run`,
/// `compose`), not a flag, a file, a path or a number — the shape kept in a
/// saved prefix, so a rule covers the action and not one file.
fn looks_like_subcommand(word: &str) -> bool {
    let mut parts = word.split('-');
    let first = parts.next().unwrap_or("");
    let lower = |p: &str| p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    first.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && lower(first)
        && parts.all(|p| !p.is_empty() && lower(p))
}

impl Subcommand {
    /// A command that can't be read: it may be anything.
    fn unknown() -> Self {
        Subcommand { assigns: false, words: vec![None], writes: false, globs: vec![None] }
    }

    /// The command's words, the program first: quoting removed,
    /// redirections left out, and `None` for one only known when it runs.
    pub fn words(&self) -> &[Option<String>] {
        &self.words
    }

    /// Whether every word of the command is known before it runs.
    pub fn readable(&self) -> bool {
        self.words.iter().all(Option::is_some)
    }

    /// How the rule prefix `pattern` — one word, two words, or an exact
    /// command — covers this command. An allow (`allow`) is strict: an
    /// unknown word or a variable prefix (`PATH=… ls`) never matches it. A
    /// deny or ask that only an unknown word stands between is `Unread`.
    pub fn covered_by(&self, pattern: &str, allow: bool) -> Cover {
        let pattern: Vec<&str> = pattern.split_whitespace().collect();
        if pattern.is_empty() || (allow && self.assigns) {
            return Cover::No;
        }
        for (i, p) in pattern.iter().enumerate() {
            match self.words.get(i) {
                Some(Some(w)) if w == p => {}
                Some(None) if !allow => return Cover::Unread,
                _ => return Cover::No,
            }
        }
        let rest = &self.words[pattern.len()..];
        if pattern.len() <= 2 || rest.is_empty() {
            Cover::Yes
        } else if !allow && rest.iter().all(Option::is_none) {
            Cover::Unread
        } else {
            Cover::No
        }
    }

    /// The prefix an "Allow always" saves for this command: the command and
    /// its subcommand (`git push`) when the
    /// second word names one, else the command exactly. `None` when no allow
    /// could ever cover it (an unknown word, a variable prefix, nothing run).
    pub fn rule_prefix(&self) -> Option<String> {
        if self.assigns || self.words.is_empty() || !self.readable() {
            return None;
        }
        let words: Vec<&str> = self.words.iter().flatten().map(String::as_str).collect();
        match words.as_slice() {
            [command, sub, ..] if looks_like_subcommand(sub) => Some(format!("{command} {sub}")),
            _ => Some(words.join(" ")),
        }
    }
}

/// Every command a shell call runs: each one of a compound command (`&&`,
/// `||`, `;`, `|`, newlines), the ones in subshells, `$(…)`, backticks and
/// redirect targets, and the scripts `bash -c` / `sh -c` / `eval` run. A
/// script that doesn't parse is one unknown command (see
/// [`Subcommand::covered_by`]); one that runs nothing is one empty command,
/// which no command rule covers.
pub fn subcommands(cmd: &str) -> Vec<Subcommand> {
    let mut out = Vec::new();
    collect_subcommands(cmd, 0, &mut out);
    if out.is_empty() {
        out.push(Subcommand { assigns: false, words: Vec::new(), writes: false, globs: Vec::new() });
    }
    out
}

fn collect_subcommands(script: &str, depth: usize, out: &mut Vec<Subcommand>) {
    let parsed = if depth < MAX_SCRIPT_DEPTH { syntax::shell_commands(script) } else { None };
    let Some(commands) = parsed else {
        out.push(Subcommand::unknown());
        return;
    };
    for command in commands {
        let mut sub = Subcommand { assigns: command.assigns, words: command.words, writes: command.writes, globs: command.globs };
        unwrap_wrappers(&mut sub);
        match inline_script(&sub.words) {
            Some(Some(inner)) => collect_subcommands(&inner, depth + 1, out),
            Some(None) => out.push(Subcommand::unknown()),
            None => {}
        }
        out.push(sub);
    }
}

/// The command a shell call's exit code comes from
/// (`syntax::shell_exit_command`), its run-only wrappers stripped
/// (`timeout 5 grep …` is grep), with the operator just before it (`&&`,
/// `||`, `|`, `;`). `None` when the script doesn't parse or ends in
/// anything but a simple command.
pub fn exit_command(cmd: &str) -> Option<(Subcommand, Option<String>)> {
    let (command, operator) = syntax::shell_exit_command(cmd)?;
    let mut sub = Subcommand { assigns: command.assigns, words: command.words, writes: command.writes, globs: command.globs };
    unwrap_wrappers(&mut sub);
    Some((sub, operator))
}

/// Strip the wrappers that only run the command after them (`nohup rm x` runs
/// `rm x`). A wrapper whose flags it can't read makes the command unknown.
fn unwrap_wrappers(sub: &mut Subcommand) {
    loop {
        let Some(Some(name)) = sub.words.first() else { return };
        let name = name.clone();
        let args = sub.words[1..].to_vec();
        let arg = |i: usize| args.get(i).cloned().flatten();
        let flag = |i: usize| arg(i).filter(|a| a.starts_with('-') && a != "--");
        let mut i = 0;
        match name.as_str() {
            "time" | "nohup" | "command" | "builtin" | "stdbuf" => {
                while flag(i).is_some() {
                    i += 1;
                }
            }
            "exec" | "timeout" | "nice" => {
                while let Some(a) = flag(i) {
                    let valued = matches!(a.as_str(), "-a" | "-k" | "-s" | "-n" | "--kill-after" | "--signal");
                    i += if valued { 2 } else { 1 };
                }
            }
            "env" => {
                while let Some(a) = flag(i).or_else(|| arg(i).filter(|a| a.contains('='))) {
                    if a.starts_with("-S") || a.starts_with("--split-string") {
                        // Its argument is itself a command line.
                        *sub = Subcommand::unknown();
                        return;
                    }
                    sub.assigns |= a.contains('=') && !a.starts_with('-');
                    i += if matches!(a.as_str(), "-u" | "-C" | "-S") { 2 } else { 1 };
                }
            }
            _ => return,
        }
        if arg(i).as_deref() == Some("--") {
            i += 1;
        }
        if i >= args.len() {
            // The wrapper alone (`time`, `env`): it is the command.
            return;
        }
        if name == "timeout" {
            let duration = |d: &str| d.trim_end_matches(['s', 'm', 'h', 'd']).parse::<f64>().is_ok();
            if !arg(i).is_some_and(|d| duration(&d)) {
                *sub = Subcommand::unknown();
                return;
            }
            i += 1;
        }
        if args[..i].iter().any(Option::is_none) {
            *sub = Subcommand::unknown();
            return;
        }
        sub.words = args[i..].to_vec();
        sub.globs = sub.globs[i + 1..].to_vec();
    }
}

/// The script a command runs from a string: `bash -c '…'` and the like, or
/// `eval …`. `Some(None)` when that script is only known when it runs.
fn inline_script(words: &[Option<String>]) -> Option<Option<String>> {
    let Some(Some(name)) = words.first() else { return None };
    if name == "eval" {
        let args = &words[1..];
        if args.is_empty() {
            return None;
        }
        return Some(args.iter().cloned().collect::<Option<Vec<_>>>().map(|a| a.join(" ")));
    }
    if !SCRIPT_SHELLS.contains(&name.as_str()) {
        return None;
    }
    let mut has_c = false;
    let mut i = 1;
    while let Some(word) = words.get(i) {
        let Some(a) = word else { return Some(None) };
        if a == "--" || !(a.starts_with('-') || a.starts_with('+')) {
            if a == "--" {
                i += 1;
            }
            break;
        }
        if !a.starts_with("--") && a[1..].contains('c') {
            has_c = true;
        }
        i += if matches!(a.as_str(), "-o" | "+o" | "--rcfile" | "--init-file") { 2 } else { 1 };
    }
    if !has_c {
        return None;
    }
    words.get(i).cloned()
}

/// Whether a shell call only reads: every command
/// it runs is fully known, sets no variable, writes no file through a
/// redirect and is read-only by [`crate::read_only_commands`]; and it doesn't
/// pair `cd` with `git` (a changed directory can carry git hooks).
pub fn is_read_only(cmd: &str) -> bool {
    let subs = subcommands(cmd);
    let name = |s: &Subcommand| s.words.first().cloned().flatten();
    let has = |n: &str| subs.iter().any(|s| name(s).as_deref() == Some(n));
    if has("cd") && has("git") {
        return false;
    }
    subs.iter().all(|s| {
        if s.assigns || s.writes || !s.readable() {
            return false;
        }
        let words: Vec<&str> = s.words.iter().flatten().map(String::as_str).collect();
        crate::read_only_commands::reads_only(&words)
    })
}

/// What a shell command removes, rewrites and brings into being, named the
/// way the employee's created ledger keeps them (`file:<path>`), so the
/// permission check's case 3 sees a shell delete the way it sees any other.
/// Each path is taken where the command reaches it, after its own `cd`s,
/// from `cwd` when the call names one; a path still relative is anchored on
/// the run's folder by the registry (`CallEffects::anchored`). What a
/// command only knows when it runs (`rm -rf "$DIR"`) is named by the
/// command itself. A program is known by its name wherever it is run from
/// (`/bin/rm` is `rm`).
///
/// A folder or a tree of files it removes is in `removes_trees`: `rm -r`
/// (`-R`, `--recursive`) and `rm -d`, `rmdir`, `find … -delete` and `find …
/// -exec rm` (each folder it starts from), `xargs rm` (what it removes
/// comes from its input, so it is named by its command), and `rsync
/// --delete` (what is in its destination). A pattern (`*`, `dir/*.o`) is
/// named by the folder it empties: `dir/*`.
pub fn shell_effects(cmd: &str, cwd: Option<&str>) -> types::permissions::CallEffects {
    use std::path::{Path, PathBuf};

    // Where the command stands: a folder (relative to the run's when not
    // absolute), or unknown after a `cd` it can't read.
    let mut here: Option<PathBuf> = Some(PathBuf::from(cwd.map(types::pathres::expand).unwrap_or_default()));
    let mut fx = types::permissions::CallEffects::unknown();
    let file = |here: &Option<PathBuf>, p: &str| -> Option<String> {
        let p = types::pathres::expand(p);
        let full = if p.is_absolute() { p } else { here.as_ref()?.join(p) };
        Some(format!("file:{}", full.display()))
    };
    let unread = |sub: &Subcommand| {
        let words: Vec<&str> = sub.words.iter().map(|w| w.as_deref().unwrap_or("…")).collect();
        format!("command:{}", words.join(" "))
    };
    // A tree a command removes: `word` itself, or what is in it when
    // `contents`, or what is in the folder a pattern names.
    let tree = |here: &Option<PathBuf>, sub: &Subcommand, word: &Option<String>, contents: bool| -> String {
        let Some(word) = word else { return unread(sub) };
        let (folder, contents) = match word.find(['*', '?', '[', '{']) {
            Some(at) => match word[..at].rfind('/') {
                Some(0) => ("/", true),
                Some(slash) => (&word[..slash], true),
                None => (".", true),
            },
            None => (word.as_str(), contents),
        };
        match file(here, folder) {
            Some(named) if contents => format!("{}", Path::new(&named).join("*").display()),
            Some(named) => named,
            None => unread(sub),
        }
    };
    // The operands of a command: its words after the options (`--` ends them).
    let operands = |args: &[Option<String>], valued: &[&str]| -> Vec<Option<String>> {
        let mut out = Vec::new();
        let mut options = true;
        let mut i = 0;
        while i < args.len() {
            match args[i].as_deref() {
                Some("--") if options => options = false,
                Some(a) if options && a.starts_with('-') && a.len() > 1 => {
                    if valued.contains(&a) {
                        i += 1;
                    }
                }
                _ => out.push(args[i].clone()),
            }
            i += 1;
        }
        out
    };

    for sub in subcommands(cmd) {
        let Some(Some(program)) = sub.words.first() else { continue };
        let program = Path::new(program).file_name().map_or(program.as_str(), |n| n.to_str().unwrap_or_default());
        let args = &sub.words[1..];
        // The words with each pattern as written, for what a removal names.
        let patterned: Vec<Option<String>> = args.iter().zip(&sub.globs[1..]).map(|(w, g)| w.clone().or_else(|| g.clone())).collect();
        let is_rm = |a: &Option<String>| a.as_deref().is_some_and(|a| Path::new(a).file_name().is_some_and(|n| n == "rm"));
        match program {
            "cd" | "pushd" => {
                here = match args.first() {
                    None => Some(types::pathres::expand("~")),
                    Some(Some(d)) if d != "-" => file(&here, d).map(|f| PathBuf::from(&f["file:".len()..])),
                    _ => None,
                };
            }
            "rm" | "rmdir" | "unlink" => {
                for operand in operands(args, &[]) {
                    match operand.and_then(|p| file(&here, &p)) {
                        Some(named) => fx.deletes.push(named),
                        None => fx.deletes.push(unread(&sub)),
                    }
                }
                let folders = program == "rmdir"
                    || (program == "rm"
                        && args.iter().flatten().take_while(|a| *a != "--").any(|a| {
                            matches!(a.as_str(), "--recursive" | "--dir")
                                || (a.starts_with('-') && !a.starts_with("--") && a.contains(['r', 'R', 'd']))
                        }));
                if folders {
                    for target in operands(&patterned, &[]) {
                        fx.removes_trees.push(tree(&here, &sub, &target, false));
                    }
                }
            }
            "find"
                if args.iter().any(|a| a.as_deref() == Some("-delete"))
                    || args.windows(2).any(|w| matches!(w[0].as_deref(), Some("-exec" | "-execdir" | "-ok" | "-okdir")) && is_rm(&w[1])) =>
            {
                let starts: Vec<Option<String>> = patterned
                    .iter()
                    .skip_while(|a| a.as_deref().is_some_and(|a| matches!(a, "-H" | "-L" | "-P") || a.starts_with("-O")))
                    .take_while(|a| a.as_deref().is_none_or(|a| !a.starts_with('-') && a != "(" && a != "!"))
                    .cloned()
                    .collect();
                let starts = if starts.is_empty() { vec![Some(".".to_string())] } else { starts };
                for start in starts {
                    fx.removes_trees.push(tree(&here, &sub, &start, false));
                }
            }
            "xargs" if args.iter().any(is_rm) => fx.removes_trees.push(unread(&sub)),
            "rsync" if args.iter().flatten().any(|a| a.starts_with("--delete")) => {
                let valued = [
                    "-e", "--rsh", "--exclude", "--include", "--filter", "-f", "--exclude-from", "--include-from",
                    "--files-from", "--log-file", "--partial-dir", "--temp-dir", "-T", "--backup-dir", "--compare-dest",
                    "--copy-dest", "--link-dest", "--chmod", "--chown", "--rsync-path", "--port", "--password-file",
                    "--timeout", "--bwlimit", "--max-size", "--min-size", "--max-delete", "--suffix", "--out-format",
                    "-M", "--remote-option",
                ];
                // The destination is the last operand; one on another
                // computer (`host:dir`) is not this one's.
                let dest = operands(&patterned, &valued).pop().flatten();
                let remote = dest.as_deref().is_some_and(|d| d.split('/').next().is_some_and(|first| first.contains(':')));
                if !remote {
                    fx.removes_trees.push(tree(&here, &sub, &dest, true));
                }
            }
            "mkdir" => {
                fx.creates.extend(operands(args, &["-m"]).iter().flatten().filter_map(|p| file(&here, p)));
            }
            "git" => {
                // `git -C <dir>` works in that folder; other global options
                // are skipped with their values.
                let mut repo = here.clone();
                let mut i = 0;
                while let Some(Some(a)) = args.get(i) {
                    match a.as_str() {
                        "-C" => {
                            repo = args.get(i + 1).cloned().flatten().and_then(|d| file(&here, &d)).map(|f| PathBuf::from(&f["file:".len()..]));
                            i += 2;
                        }
                        "-c" | "--git-dir" | "--work-tree" | "--namespace" => i += 2,
                        a if a.starts_with('-') => i += 1,
                        _ => break,
                    }
                }
                let rest = args.get(i + 1..).unwrap_or_default();
                match args.get(i).cloned().flatten().as_deref() {
                    // Commands that replace the working tree's files with
                    // another version of them.
                    Some(
                        "checkout" | "switch" | "rebase" | "merge" | "pull" | "reset" | "restore" | "cherry-pick"
                        | "revert" | "am",
                    ) => match file(&repo, ".") {
                        Some(named) => fx.overwrites.push(named),
                        None => fx.overwrites.push(unread(&sub)),
                    },
                    Some("clone") => {
                        let valued = ["-b", "--branch", "-o", "--origin", "--depth", "--reference", "--template", "-c", "--config", "--filter", "-j", "--jobs", "--separate-git-dir", "-u", "--upload-pack"];
                        let ops = operands(rest, &valued);
                        let dir = match ops.as_slice() {
                            [_, Some(dir), ..] => Some(dir.clone()),
                            [Some(url), ..] => url
                                .trim_end_matches('/')
                                .rsplit(['/', ':'])
                                .next()
                                .map(|name| name.trim_end_matches(".git").to_string()),
                            _ => None,
                        };
                        fx.creates.extend(dir.and_then(|d| file(&repo, &d)));
                    }
                    Some("worktree") if rest.first().cloned().flatten().as_deref() == Some("add") => {
                        let ops = operands(&rest[1..], &["-b", "-B", "--reason"]);
                        fx.creates.extend(ops.first().cloned().flatten().and_then(|d| file(&repo, &d)));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    fx
}

/// The first folder `cmd` removes that no command ever removes, in any
/// mode, asked or not (owner, 10-23: "can never ever be allowed"): the
/// root, every folder right under it (`/System`, `/usr`, `/bin`, `/etc`,
/// `/Library`, `/Applications`, `/Users`, …), a drive and every folder right
/// under one (`C:\Windows`, `C:\Program Files`), the home folder itself, or
/// everything in one of them (`/*`, `~/*`). `cwd` is where its relative
/// paths start.
pub fn never_removed(cmd: &str, cwd: Option<&std::path::Path>) -> Option<String> {
    let fx = shell_effects(cmd, None).anchored(cwd.unwrap_or(std::path::Path::new("")));
    // Through a link (`link/`, `link/*`), where it points is what goes.
    let through = |p: &str| {
        let (folder, contents) = match p.strip_suffix("/*") {
            Some(folder) => (folder, true),
            None => (p, false),
        };
        (contents || p.ends_with('/'))
            && std::fs::canonicalize(folder)
                .is_ok_and(|real| is_never_removed(&format!("{}{}", real.display(), if contents { "/*" } else { "" })))
    };
    fx.removes_trees.into_iter().filter_map(|t| t.strip_prefix("file:").map(str::to_string)).find(|p| is_never_removed(p) || through(p))
}

/// Whether `path` (as written, links not followed) is one no command
/// removes ([`never_removed`]).
fn is_never_removed(path: &str) -> bool {
    let mut slashed = String::new();
    for c in path.chars() {
        let c = if c == '\\' { '/' } else { c };
        if !(c == '/' && slashed.ends_with('/')) {
            slashed.push(c);
        }
    }
    // What is in a folder goes as the folder does.
    let folder = slashed.strip_suffix("/*").map(|f| if f.is_empty() { "/" } else { f }).unwrap_or(&slashed);
    let folder = if folder.len() > 1 { folder.trim_end_matches('/') } else { folder };
    let b = folder.as_bytes();
    let drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
    let under = |root: &str| folder.strip_prefix(root).is_some_and(|rest| !rest.is_empty() && !rest.contains('/'));
    let home = dirs::home_dir().map(|h| h.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_string());
    folder == "/"
        || under("/")
        || (drive && (folder.len() == 2 || under(&format!("{}/", &folder[..2]))))
        || home.is_some_and(|h| folder.eq_ignore_ascii_case(&h))
}

/// House git rules, enforced: the commands that throw away the owner's work.
/// Nebo has checkpoints (`os file checkpoint/restore`) and worktrees for
/// parallel edits, so none of these is ever the right tool. The shell refuses
/// them outright, like privilege escalation — an approval card would only
/// teach the model to ask for the wrong thing.
///
/// Every command the script runs is read by [`subcommands`]: one after
/// `&&`, `;` or `|`, in `$(…)`, inside `bash -c '…'` or `eval`, and past the
/// wrappers that only run it (`env`, `timeout 5`, `nohup`, `nice`). Only a
/// command that IS git counts: `echo git stash` and `grep "git stash"
/// notes.md` are not git.
pub fn is_destructive_git(cmd: &str) -> bool {
    subcommands(cmd).iter().any(|sub| {
        let words: Vec<&str> = sub.words.iter().map(|w| w.as_deref().unwrap_or("")).collect();
        let Some(first) = words.first() else { return false };
        if *first != "git" && !first.ends_with("/git") {
            return false;
        }
        // Skip `-C dir` / `-c k=v` style globals before the subcommand.
        let mut j = 1;
        while j < words.len() && words[j].starts_with('-') {
            j += if matches!(words[j], "-C" | "-c" | "--git-dir" | "--work-tree") { 2 } else { 1 };
        }
        let Some(verb) = words.get(j) else { return false };
        let args = &words[j + 1..];
        let has = |flag: &str| args.iter().any(|a| *a == flag);
        match *verb {
            "stash" => !args.first().is_some_and(|a| matches!(*a, "list" | "show")),
            "reset" => has("--hard") || has("--merge") || has("--keep"),
            "checkout" => args.iter().any(|a| *a == "." || *a == "--" || a.starts_with("--source")) && !has("-b"),
            // `git restore <path>` discards the working-tree change of a tracked
            // file, the same loss as `checkout -- <path>`. Only an unstage
            // (`--staged` without `--worktree`/`-W`) leaves the owner's edits alone.
            "restore" => !(has("--staged") || has("-S")) || has("--worktree") || has("-W"),
            // A dry run (`-n`/`--dry-run`) only prints what it would delete.
            "clean" => {
                !args.iter().any(|a| *a == "--dry-run" || (a.starts_with('-') && !a.starts_with("--") && a.contains('n')))
                    && args.iter().any(|a| a.starts_with('-') && (a.contains('f') || a.contains('x')))
            }
            // A `+` refspec (`git push origin +main`) forces that one ref.
            "push" => {
                has("--force")
                    || has("-f")
                    || args.iter().any(|a| a.starts_with("--force-with-lease") || (a.starts_with('+') && a.len() > 1))
            }
            "branch" => has("-D") || (has("--delete") && has("--force")),
            _ => false,
        }
    })
}

/// What a destructive git command ([`is_destructive_git`]) is answered
/// with, wherever it is met: a command, text typed into a running shell, or
/// a script a command runs.
pub const DESTRUCTIVE_GIT_REFUSAL: &str = "This git command discards work (stash, reset --hard, checkout/restore of \
     tracked files, clean -f, force push, branch -D) and is not available. \
     To be able to undo a change, take a checkpoint first with \
     checkpoint_files(paths: [...]) and put it back with restore_checkpoint. \
     For parallel edits give each helper its own copy \
     (delegate with isolation: \"worktree\"). If the owner truly wants history rewritten, \
     tell them the exact command and let them run it.";

/// The one-line risk of a destructive command, for the owner's card: the
/// first that fits, in this order (git's losses first, then removing
/// files, then databases and infrastructure). Read from each command the
/// script runs ([`subcommands`]), so a wrapped one counts; SQL is read
/// from the text, wherever it sits. `None` for anything else.
pub fn destructive_warning(cmd: &str) -> Option<types::permissions::DestructiveWarning> {
    use types::permissions::DestructiveWarning as W;
    static DROP: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)\b(DROP|TRUNCATE)\s+(TABLE|DATABASE|SCHEMA)\b").expect("valid"));
    static DELETE_ALL: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"(?i)\bDELETE\s+FROM\s+\w+[ \t]*(;|"|'|\n|$)"#).expect("valid"));
    let subs = subcommands(cmd);
    let words = |s: &Subcommand| -> Vec<String> { s.words.iter().map(|w| w.clone().unwrap_or_default()).collect() };
    // Each git command: its verb and the words after it.
    let gits: Vec<(String, Vec<String>)> = subs
        .iter()
        .filter_map(|s| {
            let w = words(s);
            let first = w.first()?;
            if first != "git" && !first.ends_with("/git") {
                return None;
            }
            let mut j = 1;
            while j < w.len() && w[j].starts_with('-') {
                j += if matches!(w[j].as_str(), "-C" | "-c" | "--git-dir" | "--work-tree") { 2 } else { 1 };
            }
            Some((w.get(j)?.clone(), w.get(j + 1..).unwrap_or_default().to_vec()))
        })
        .collect();
    let git = |verb: &str, fits: &dyn Fn(&[String]) -> bool| gits.iter().any(|(v, a)| v == verb && fits(a));
    let has = |a: &[String], f: &str| a.iter().any(|x| x == f);
    let rms: Vec<Vec<String>> =
        subs.iter().map(words).filter(|w| w.first().is_some_and(|p| p == "rm" || p.ends_with("/rm"))).collect();
    let short = |a: &str, c: char| a.starts_with('-') && !a.starts_with("--") && a.contains(c);
    let recursive = |w: &Vec<String>| w[1..].iter().any(|a| a == "--recursive" || short(a, 'r') || short(a, 'R'));
    let forced = |w: &Vec<String>| w[1..].iter().any(|a| a == "--force" || short(a, 'f'));
    let tool = |name: &str, verb: &str| subs.iter().map(words).any(|w| w.first().is_some_and(|p| p == name) && w[1..].iter().find(|a| !a.starts_with('-')).is_some_and(|a| a == verb));
    let checks: [(W, bool); 15] = [
        (W::DiscardsUncommittedChanges, git("reset", &|a| has(a, "--hard"))),
        (
            W::OverwritesRemoteHistory,
            git("push", &|a| a.iter().any(|x| x == "-f" || x == "--force" || x.starts_with("--force-with-lease") || (x.starts_with('+') && x.len() > 1))),
        ),
        (
            W::DeletesUntrackedFiles,
            git("clean", &|a| {
                !a.iter().any(|x| x == "--dry-run" || (x.starts_with('-') && !x.starts_with("--") && x.contains('n')))
                    && a.iter().any(|x| x.starts_with('-') && !x.starts_with("--") && x.contains('f'))
            }),
        ),
        (W::DiscardsWorkingTreeChanges, git("checkout", &|a| a.iter().filter(|x| *x != "--").eq(["."].iter())) || git("restore", &|a| a.iter().filter(|x| *x != "--").eq(["."].iter()))),
        (W::RemovesStashedChanges, git("stash", &|a| a.first().is_some_and(|x| x == "drop" || x == "clear"))),
        (W::ForceDeletesBranch, git("branch", &|a| has(a, "-D") || (has(a, "--delete") && has(a, "--force")))),
        (W::SkipsSafetyHooks, ["commit", "push", "merge"].iter().any(|v| git(v, &|a| has(a, "--no-verify")))),
        (W::RewritesLastCommit, git("commit", &|a| has(a, "--amend"))),
        (W::RecursivelyForceRemovesFiles, rms.iter().any(|w| recursive(w) && forced(w))),
        (W::RecursivelyRemovesFiles, rms.iter().any(recursive)),
        (W::ForceRemovesFiles, rms.iter().any(forced)),
        (W::DropsDatabaseObjects, DROP.is_match(cmd)),
        (W::DeletesAllRows, DELETE_ALL.is_match(cmd)),
        (W::DeletesKubernetesResources, tool("kubectl", "delete")),
        (W::DestroysInfrastructure, tool("terraform", "destroy")),
    ];
    checks.into_iter().find(|(_, fits)| *fits).map(|(w, _)| w)
}

/// Files a shell command writes outside the supervised edit path: `sed -i`
/// targets, `tee` targets, and `>`/`>>` redirections. `sed -i` is refused
/// (the edit action exists for that, and what the owner previews there is
/// what gets written); for the rest, this refreshes the read ledger so the
/// agent's own shell write is not later reported to it as someone else's
/// change.
pub fn shell_write_targets(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let flat = cmd.replace('\n', " ");
    for seg in flat.split(|c| c == ';' || c == '|' || c == '&') {
        let toks: Vec<&str> = seg.split_whitespace().collect();
        let mut i = 0;
        while i < toks.len() {
            let t = toks[i];
            if t == ">" || t == ">>" || t == "1>" || t == "2>" || t == "&>" {
                if let Some(target) = toks.get(i + 1) {
                    out.push(target.trim_matches(|c| c == '"' || c == '\'').to_string());
                }
                i += 2;
                continue;
            }
            if let Some(rest) = t.strip_prefix(">>").or_else(|| t.strip_prefix('>')) {
                if !rest.is_empty() && !rest.starts_with('&') {
                    out.push(rest.trim_matches(|c| c == '"' || c == '\'').to_string());
                }
            }
            if t == "tee" {
                for target in toks[i + 1..].iter().filter(|a| !a.starts_with('-')) {
                    out.push(target.trim_matches(|c| c == '"' || c == '\'').to_string());
                }
                break;
            }
            i += 1;
        }
    }
    out.retain(|p| p != "/dev/null" && !p.is_empty());
    out
}

/// `sed -i` (in-place, any suffix form) on a file. The edit action exists for
/// exactly this and keeps the read ledger honest.
pub fn is_sed_in_place(cmd: &str) -> bool {
    for seg in cmd.replace('\n', ";").split(|c| c == ';' || c == '|' || c == '&') {
        let toks: Vec<&str> = seg.split_whitespace().collect();
        let Some(first) = toks.first() else { continue };
        if *first != "sed" && !first.ends_with("/sed") {
            continue;
        }
        if toks[1..].iter().any(|t| *t == "-i" || t.starts_with("-i") && !t.starts_with("-in") || *t == "--in-place" || t.starts_with("--in-place=")) {
            return true;
        }
    }
    false
}

/// Check if a command invokes privilege escalation (sudo/doas/su) anywhere —
/// as the command itself, after a pipe/separator, or inside a substitution.
///
/// Nebo runs unattended: an interactive password prompt can never be answered
/// (it hangs until timeout), and a passwordless escalation is a silent
/// privilege grab. Neither is ever a legitimate automation step, so the shell
/// tool refuses these outright rather than gating them on approval.
pub fn is_privilege_escalation(cmd: &str) -> bool {
    // Normalize shell separators so escalators are exposed as standalone
    // tokens: `echo x | sudo tee f`, `a && sudo b`, `$(sudo id)`.
    let normalized: String = cmd
        .chars()
        .map(|c| match c {
            ';' | '|' | '&' | '(' | ')' | '`' | '\n' => ' ',
            _ => c,
        })
        .collect();
    normalized
        .split_whitespace()
        .any(|tok| matches!(tok, "sudo" | "doas" | "su"))
}

/// What a shell call reaches on the owner's own computer beyond its files:
/// his screen, microphone or camera, or his other apps
/// ([`types::permissions::OwnerReach`]), from the first command that does.
/// Each command is judged by the program it runs, so `grep screencapture`,
/// `echo osascript` and `open file.pdf` reach nothing. A call that can't be
/// read is judged by the PowerShell calls in its text.
///
/// 2026-10-04: an employee with no recording tool opened Safari with
/// `open -a`, drove it with `osascript`, and ran `screencapture -x` 90 times
/// on the owner's display to make a video, and nothing asked him.
pub fn owner_reach(cmd: &str) -> Option<types::permissions::OwnerReach> {
    let subs = subcommands(cmd);
    if let Some(reach) = subs.iter().find_map(reach_of) {
        return Some(reach);
    }
    if subs.iter().any(|s| !s.readable()) {
        return powershell_reach(cmd);
    }
    None
}

/// What one command reaches on the owner's computer (see [`owner_reach`]).
pub fn reach_of(sub: &Subcommand) -> Option<types::permissions::OwnerReach> {
    use types::permissions::OwnerReach;

    let first = sub.words.first()?.as_deref()?;
    let args = &sub.words[1..];
    let has = |w: &str| args.iter().any(|a| a.as_deref() == Some(w));
    let after = |flags: &[&str]| {
        args.iter()
            .position(|a| a.as_deref().is_some_and(|a| flags.contains(&a)))
            .map(|i| args.get(i + 1).cloned().flatten())
    };
    match program_name(first).as_str() {
        "screencapture" | "gnome-screenshot" | "scrot" | "grim" | "spectacle" | "maim" | "flameshot" | "import" => {
            Some(OwnerReach::Screen)
        }
        "imagesnap" => Some(OwnerReach::Camera),
        "rec" | "arecord" | "parecord" | "pw-record" => Some(OwnerReach::Microphone),
        // `sox -d out.wav` records from the default device; `sox in.wav -d`
        // plays to it.
        "sox" => {
            let input = args.iter().flatten().find(|a| *a == "-d" || *a == "-n" || !a.starts_with('-'));
            (input.map(String::as_str) == Some("-d")).then_some(OwnerReach::Microphone)
        }
        "ffmpeg" => ffmpeg_reach(args),
        "osascript" => osascript_reach(args),
        "open" => after(&["-a", "-b"]).map(|app| OwnerReach::App { app }),
        "cliclick" | "xdotool" | "ydotool" | "wtype" | "autohotkey" | "autohotkey64" | "autohotkeyu64" | "autohotkey32"
        | "autohotkeyu32" => Some(OwnerReach::Input),
        "powershell" | "pwsh" if !has("-File") => {
            let words: Vec<&str> = args.iter().flatten().map(String::as_str).collect();
            powershell_reach(&words.join(" "))
        }
        // A PowerShell statement read as one word.
        _ => powershell_reach(first),
    }
}

/// The prefix "Allow always" saves for a command [`reach_of`] names: the
/// program, so the next capture or the next script for an app is covered
/// whatever its file or words; `open -a` / `open -b` with its flag. `None`
/// when no allow could cover the command (a variable before it).
pub fn reach_prefix(sub: &Subcommand) -> Option<String> {
    if sub.assigns {
        return None;
    }
    let first = sub.words.first()?.as_deref()?;
    match program_name(first).as_str() {
        "open" => match sub.words.get(1) {
            Some(Some(flag)) if flag == "-a" || flag == "-b" => Some(format!("{first} {flag}")),
            _ => sub.rule_prefix(),
        },
        "powershell" | "pwsh" => sub.rule_prefix(),
        _ if powershell_reach(first).is_some() => sub.rule_prefix(),
        _ => Some(first.to_string()),
    }
}

/// A program's name as typed or by path: `/usr/sbin/screencapture` and
/// `AutoHotkey64.exe` are `screencapture` and `autohotkey64`.
fn program_name(word: &str) -> String {
    let base = word.rsplit(['/', '\\']).next().unwrap_or(word).to_ascii_lowercase();
    base.strip_suffix(".exe").map(str::to_string).unwrap_or(base)
}

/// `ffmpeg` reading from a capture device: an input format (`-f` before an
/// `-i`) that is a screen, a camera or a microphone.
fn ffmpeg_reach(args: &[Option<String>]) -> Option<types::permissions::OwnerReach> {
    use types::permissions::OwnerReach;
    let mut format: Option<&str> = None;
    for (i, a) in args.iter().enumerate() {
        match a.as_deref() {
            Some("-f") => format = args.get(i + 1).and_then(|f| f.as_deref()),
            Some("-i") => match format.take() {
                Some("avfoundation" | "x11grab" | "gdigrab" | "kmsgrab" | "ddagrab" | "fbdev") => return Some(OwnerReach::Screen),
                Some("dshow" | "v4l2" | "video4linux2") => return Some(OwnerReach::Camera),
                Some("pulse" | "alsa" | "openal" | "jack" | "oss" | "sndio") => return Some(OwnerReach::Microphone),
                _ => {}
            },
            _ => {}
        }
    }
    None
}

/// `osascript` telling an app what to do (AppleScript `tell application`,
/// JXA `Application("…")`), System Events included. A script it runs from
/// a file or its input can't be read: it is taken as one.
fn osascript_reach(args: &[Option<String>]) -> Option<types::permissions::OwnerReach> {
    use types::permissions::OwnerReach;
    let mut scripts = Vec::new();
    let mut inline = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_deref() {
            Some("-e") => {
                inline = true;
                match args.get(i + 1) {
                    Some(Some(script)) => scripts.push(script.as_str()),
                    _ => return Some(OwnerReach::App { app: None }),
                }
                i += 2;
            }
            Some("-l" | "-s") => i += 2,
            Some(a) if a.starts_with('-') => i += 1,
            _ => break,
        }
    }
    if !inline {
        return Some(OwnerReach::App { app: None });
    }
    let text = scripts.join("\n");
    static TOLD: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?i)\btell\s+app(?:lication)?\s+(?:id\s+)?"([^"]+)"|\bApplication\(\s*["']([^"']+)["']\s*\)"#)
            .expect("a valid pattern")
    });
    let mut apps = TOLD.captures_iter(&text).filter_map(|c| c.get(1).or_else(|| c.get(2))).map(|m| m.as_str().to_string());
    let first = apps.next()?;
    let other = std::iter::once(first).chain(apps).find(|a| !a.eq_ignore_ascii_case("System Events"));
    Some(match other {
        Some(app) => OwnerReach::App { app: Some(app) },
        None => OwnerReach::Input,
    })
}

/// PowerShell's own screen capture and keystrokes, which no program name
/// shows: `[…SendKeys]::SendWait(…)`, `WScript.Shell`'s `.SendKeys(…)`,
/// `Graphics.CopyFromScreen(…)`. Matched as calls, not as words in text.
fn powershell_reach(text: &str) -> Option<types::permissions::OwnerReach> {
    use types::permissions::OwnerReach;
    let lower = text.to_ascii_lowercase();
    if lower.contains(".copyfromscreen(") {
        return Some(OwnerReach::Screen);
    }
    if lower.contains("sendkeys]::send") || lower.contains(".sendkeys(") {
        return Some(OwnerReach::Input);
    }
    None
}

/// The company's own figures for unattended work, as the company layer
/// states them. Every field is optional; an absent field is no bound.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Bounds {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_amount_cents: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_day_cents: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_day_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_counterparty_day_cents: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counterparty_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freshness_secs: Option<i64>,
}

/// The company's constitution, read from the company layer: its purpose,
/// the operations reserved to the owner's own hand, the unattended figures,
/// and the owner's pages. The operations it reserves reach the permission
/// check as the company's locked ask rules.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CompanyPolicy {
    /// Operation suffixes reserved to the owner's own hand: every call asks.
    #[serde(default)]
    pub reserved: Vec<String>,
    /// Company-wide per-day figures for unattended work.
    #[serde(default)]
    pub daily: Bounds,
    #[serde(default)]
    pub purpose: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<serde_json::Value>,
}

impl CompanyPolicy {
    pub fn from_json(json: Option<&str>) -> Self {
        json.and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn is_reserved(&self, suffix: &str) -> bool {
        self.reserved.iter().any(|r| r == suffix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(cmd: &str) -> Vec<(bool, Vec<Option<String>>)> {
        subcommands(cmd).into_iter().map(|s| (s.assigns, s.words)).collect()
    }

    fn lit(ws: &[&str]) -> Vec<Option<String>> {
        ws.iter().map(|w| Some(w.to_string())).collect()
    }

    #[test]
    fn a_shell_call_splits_into_every_command_it_runs() {
        let plain = |ws: &[&[&str]]| ws.iter().map(|w| (false, lit(w))).collect::<Vec<_>>();
        let cases: &[(&str, Vec<(bool, Vec<Option<String>>)>)] = &[
            ("ls && rm -rf x", plain(&[&["ls"], &["rm", "-rf", "x"]])),
            ("ls || rm x", plain(&[&["ls"], &["rm", "x"]])),
            ("ls; rm x", plain(&[&["ls"], &["rm", "x"]])),
            ("ls | rm x", plain(&[&["ls"], &["rm", "x"]])),
            ("ls\nrm x", plain(&[&["ls"], &["rm", "x"]])),
            ("(cd a; rm b)", plain(&[&["cd", "a"], &["rm", "b"]])),
            ("echo \"a && b\"", plain(&[&["echo", "a && b"]])),
            ("echo 'a; rm b'", plain(&[&["echo", "a; rm b"]])),
            ("'r'm x", plain(&[&["rm", "x"]])),
            ("r\\m x", plain(&[&["rm", "x"]])),
            ("rm x > out.txt 2>&1", plain(&[&["rm", "x"]])),
            ("bash -c 'ls && rm -rf x'", plain(&[&["ls"], &["rm", "-rf", "x"], &["bash", "-c", "ls && rm -rf x"]])),
            ("sh -ec \"rm x\"", plain(&[&["rm", "x"], &["sh", "-ec", "rm x"]])),
            ("eval rm x", plain(&[&["rm", "x"], &["eval", "rm", "x"]])),
            ("nohup timeout -s KILL 5 nice -n 3 rm x", plain(&[&["rm", "x"]])),
            ("time", plain(&[&["time"]])),
            ("", plain(&[&[]])),
            ("FOO=1 rm x", vec![(true, lit(&["rm", "x"]))]),
            ("env A=1 rm x", vec![(true, lit(&["rm", "x"]))]),
        ];
        for (cmd, want) in cases {
            assert_eq!(&words(cmd), want, "{cmd:?}");
        }
        // Substitutions and backticks run their own commands; the outer word
        // is only known when it runs.
        assert_eq!(words("echo $(rm y)"), vec![(false, vec![Some("echo".into()), None]), (false, lit(&["rm", "y"]))]);
        assert_eq!(words("echo `rm y`"), vec![(false, vec![Some("echo".into()), None]), (false, lit(&["rm", "y"]))]);
        assert_eq!(words("ls > $(rm z)"), vec![(false, lit(&["ls"])), (false, lit(&["rm", "z"]))]);
        // What can't be read is one unknown command.
        for cmd in ["ls &&", "bash -c \"$X\"", "eval \"$X\"", "timeout -k $K 5 rm x", "env -S 'rm x'"] {
            assert_eq!(words(cmd)[0], (false, vec![None]), "{cmd:?}");
        }
        assert_eq!(words("$CMD x"), vec![(false, vec![None, Some("x".into())])]);
    }

    #[test]
    fn an_allow_is_strict_and_a_deny_reads_the_unknown_as_unread() {
        let sub = |ws: Vec<Option<String>>, assigns| Subcommand { assigns, globs: vec![None; ws.len()], words: ws, writes: false };
        let git_push = sub(lit(&["git", "push", "origin"]), false);
        assert_eq!(git_push.covered_by("git", true), Cover::Yes);
        assert_eq!(git_push.covered_by("git push", true), Cover::Yes);
        assert_eq!(git_push.covered_by("git pull", false), Cover::No);
        assert_eq!(git_push.covered_by("git push origin", true), Cover::Yes, "exact");
        assert_eq!(sub(lit(&["ls"]), true).covered_by("ls", true), Cover::No, "a variable prefix never meets an allow");
        assert_eq!(sub(lit(&["rm", "x"]), true).covered_by("rm", false), Cover::Yes);
        let dynamic = sub(vec![Some("git".into()), None], false);
        assert_eq!(dynamic.covered_by("git push", false), Cover::Unread);
        assert_eq!(dynamic.covered_by("git push", true), Cover::No);
        assert_eq!(Subcommand::unknown().covered_by("rm -rf /", false), Cover::Unread);
        assert_eq!(Subcommand::unknown().covered_by("ls", true), Cover::No);
        assert_eq!(sub(lit(&["rm", "-rf", "x"]), false).covered_by("rm", false), Cover::Yes, "known words decide");
        assert_eq!(sub(Vec::new(), false).covered_by("rm", false), Cover::No, "an empty command runs nothing");
    }

    /// The prefix "Allow always" saves: the command and its subcommand.
    #[test]
    fn a_saved_prefix_is_the_command_and_its_subcommand() {
        let prefix = |cmd: &str| subcommands(cmd).into_iter().map(|s| s.rule_prefix()).collect::<Vec<_>>();
        assert_eq!(prefix("git commit -m 'fix'"), [Some("git commit".to_string())]);
        assert_eq!(prefix("npm run build-all"), [Some("npm run".to_string())]);
        assert_eq!(prefix("ls -la"), [Some("ls -la".to_string())]);
        assert_eq!(prefix("cat file.txt"), [Some("cat file.txt".to_string())]);
        assert_eq!(prefix("chmod 755 f"), [Some("chmod 755 f".to_string())]);
        assert_eq!(prefix("ls"), [Some("ls".to_string())]);
        assert_eq!(prefix("ls && git push origin"), [Some("ls".to_string()), Some("git push".to_string())]);
        // No allow could cover these, so none is saved.
        assert_eq!(prefix("FOO=1 ls"), [None]);
        assert_eq!(prefix("$CMD x"), [None]);
        assert_eq!(prefix(""), [None]);
    }

    #[test]
    fn destructive_git_is_named_and_the_safe_forms_pass() {
        for cmd in [
            "git stash",
            "git stash push -m wip",
            "cd repo && git reset --hard HEAD~1",
            "git checkout .",
            "git checkout -- src/main.rs",
            "git restore --source=HEAD~2 src/",
            "git clean -fd",
            "git push --force origin main",
            "git push -f",
            "git -C /tmp/x branch -D feature",
            "git branch --delete --force feature",
            "git branch --force --delete feature",
            "git clean -x -f",
            "git push --force-with-lease=main",
            "a && git stash",
            "$(git stash)",
            "FOO=1 git stash",
            "/usr/bin/git stash",
            "git -c user.name=x stash",
            "git reset --merge",
            "git restore .",
            "git restore f.txt",
            "git restore --staged --worktree f.txt",
            "git restore -S -W f.txt",
            // Wrapped: another shell, a wrapper that only runs it, a forced ref.
            "bash -c 'git reset --hard'",
            "sh -c \"cd repo && git clean -fdx\"",
            "eval git stash",
            "env git push -f",
            "env GIT_DIR=.git git reset --hard",
            "timeout 5 git clean -fd",
            "nohup git stash &",
            "nice -n 5 git branch -D old",
            "git push origin +main",
            "git push origin +HEAD:main",
            "x=$(bash -c 'git checkout .')",
        ] {
            assert!(is_destructive_git(cmd), "{cmd} should be refused");
        }
        for cmd in [
            "git status",
            "git stash list",
            "git reset HEAD~1",
            "git reset --soft HEAD~1",
            "git checkout -b feature",
            "git checkout main",
            "git restore --staged src/main.rs",
            "git restore -S src/main.rs",
            "git clean -n",
            "git clean -fn",
            "git clean -nfd",
            "git clean --dry-run -f",
            "git stash show",
            "git push origin main",
            "git branch -d merged",
            "echo git stash",
            "grep \"git reset --hard\" notes.md",
            "bash -c 'git status'",
            "timeout 5 git fetch",
            "env git push origin main",
            "echo 'git push origin +main'",
        ] {
            assert!(!is_destructive_git(cmd), "{cmd} is fine");
        }
    }

    #[test]
    fn shell_write_targets_and_sed_in_place_are_recognised() {
        assert_eq!(shell_write_targets("cargo build 2>&1 | tee build.log"), vec!["build.log"]);
        assert_eq!(shell_write_targets("echo hi > out.txt && cat out.txt"), vec!["out.txt"]);
        assert_eq!(shell_write_targets("printf x >>notes.md"), vec!["notes.md"]);
        assert_eq!(shell_write_targets("cat <<EOF > a.html\n<p>hi</p>\nEOF"), vec!["a.html"]);
        assert!(shell_write_targets("ls -la > /dev/null").is_empty());
        assert!(shell_write_targets("grep -r foo . | head").is_empty());
        assert!(is_sed_in_place("sed -i 's/a/b/' f.txt"));
        assert!(is_sed_in_place("sed -i.bak 's/a/b/' f.txt"));
        assert!(is_sed_in_place("sed -i '' 's/a/b/' f.txt"));
        assert!(is_sed_in_place("/usr/bin/sed --in-place=.orig -e 's/a/b/' f.txt"));
        assert!(!is_sed_in_place("sed 's/a/b/' f.txt > g.txt"));
        assert!(!is_sed_in_place("sed -n '1,5p' f.txt"));
        assert!(!is_sed_in_place("echo sed -i"));
    }

    /// D2: a shell call is read-only when every command it runs is, each
    /// judged on its own, and none writes a file.
    #[test]
    fn read_only_shell_calls_are_recognised_per_command() {
        let reads = [
            "ls",
            "ls -la /tmp",
            "pwd && ls -la",
            "git status",
            "git log --oneline -5",
            "git diff HEAD~1 --stat",
            "git show HEAD:src/main.rs",
            "git branch -a",
            "git tag -l 'v*'",
            "git remote -v",
            "cat notes.md | grep -n todo | wc -l",
            "rg -n 'fn main' src",
            "grep -A20 -rn needle .",
            "find . -name '*.rs' -type f",
            "sed -n '1,40p' src/lib.rs",
            "sed 's/a/b/g'",
            "echo done 2>&1",
            "ls missing 2>/dev/null",
            "head -50 README.md && tail -n 5 CHANGELOG.md",
            "jq '.name' package.json",
            "date +%Y-%m-%d",
            "wc -l src/lib.rs 2>/dev/null; true",
            "ps aux",
            "sort -u names.txt",
            "xargs -n 1 echo",
            "docker ps -a",
            "timeout 5 cat /etc/hosts",
        ];
        let writes = [
            "rm notes.md",
            "ls > listing.txt",
            "echo hi >> log.txt",
            "cat a | tee b",
            "git push",
            "git branch topic",
            "git tag v1.0",
            "git reflog expire --all",
            "git remote add origin git@example.com:x.git",
            "git diff --output=patch.txt",
            "git -c core.pager=less log",
            "find . -delete",
            "find . -name x -exec rm {} ;",
            "sed -i 's/a/b/' f.txt",
            "sed 's/a/b/' f.txt",
            "sed 's/a/b/w out.txt'",
            "sed -n '1p;w out' f",
            "date 0101000026",
            "FOO=1 ls",
            "ls $HOME",
            "ls *.rs",
            "cd /tmp && git status",
            "xargs rm",
            "sort -o sorted.txt names.txt",
            "rg --pre=bash x",
            "hostname newname",
            "ps auxe",
            "tput reset",
            "uniq in.txt out.txt",
            "node script.js",
            "curl https://example.com",
            "bash -c 'ls; rm -rf x'",
            "echo $(rm x)",
            "jq -f prog.jq data.json",
            "lsof +m/tmp/x",
        ];
        for cmd in reads {
            assert!(is_read_only(cmd), "reads only: {cmd}");
        }
        for cmd in writes {
            assert!(!is_read_only(cmd), "not read-only: {cmd}");
        }
    }

    /// A folder or tree a command removes is named where it removes it,
    /// however the program is spelled; a pattern by the folder it empties;
    /// one only running it knows by its command.
    #[test]
    fn tree_removals_are_named() {
        let trees = |cmd: &str| shell_effects(cmd, Some("/w")).anchored(std::path::Path::new("/")).removes_trees;
        assert_eq!(trees("rm -rf build dist"), vec!["file:/w/build", "file:/w/dist"]);
        assert_eq!(trees("/bin/rm -R /tmp/x"), vec!["file:/tmp/x"]);
        assert_eq!(trees("rm -d empty"), vec!["file:/w/empty"]);
        assert_eq!(trees("rmdir empty"), vec!["file:/w/empty"]);
        assert_eq!(trees("cd a && rm -r ../b"), vec!["file:/w/b"]);
        assert_eq!(trees("rm -rf *"), vec!["file:/w/*"]);
        assert_eq!(trees("rm -rf out/*.o"), vec!["file:/w/out/*"]);
        assert_eq!(trees("rm -rf /*"), vec!["file:/*"]);
        assert_eq!(trees("find out -name '*.o' -delete"), vec!["file:/w/out"]);
        assert_eq!(trees("find -delete"), vec!["file:/w"]);
        assert_eq!(trees("find . -exec /bin/rm -rf {} +"), vec!["file:/w"]);
        assert_eq!(trees("ls | xargs rm"), vec!["command:xargs rm"]);
        assert_eq!(trees("ls | xargs -0 /bin/rm -f"), vec!["command:xargs -0 /bin/rm -f"]);
        assert_eq!(trees("rsync -a --delete src/ site/"), vec!["file:/w/site/*"]);
        assert_eq!(trees("rsync -a --delete-after -e ssh src/ host:site/"), Vec::<String>::new());
        assert_eq!(trees("rm -rf \"$DIR\""), vec!["command:rm -rf …"]);
        assert!(trees("rm -f a.txt").is_empty() && trees("rm --force a").is_empty() && trees("find . -name x").is_empty());
    }

    /// The root, what is right under it, a drive and what is right under
    /// one, and the home folder are never removed, nor everything in one.
    #[test]
    fn some_folders_are_never_removed() {
        let home = dirs::home_dir().unwrap().display().to_string();
        for cmd in [
            "rm -rf /", "rm -rf /*", "rm -rf //", "/bin/rm -fr /usr", "rm -r /System", "rm -rf /Applications/",
            "rm -rf /Library", "rm -rf /etc", "rm -rf /Users", "rm -rf ~", "rm -rf ~/", "rm -rf ~/*",
            "cd / && rm -rf usr", "cd ~/x && rm -rf ..", "find / -delete", "rmdir /tmp", "rsync -a --delete e/ ~/",
        ] {
            assert!(never_removed(cmd, Some(std::path::Path::new("/w"))).is_some(), "{cmd}");
        }
        assert_eq!(never_removed("rm -rf ..", Some(std::path::Path::new(&format!("{home}/x")))), Some(home.clone()));
        #[cfg(unix)]
        {
            // Through a link, where it points is what goes.
            let d = tempfile::tempdir().unwrap();
            std::os::unix::fs::symlink("/usr", d.path().join("u")).unwrap();
            let at = Some(d.path());
            assert!(never_removed("rm -rf u/", at).is_some() && never_removed("rm -rf u/*", at).is_some());
            assert_eq!(never_removed("rm -rf u", at), None, "the link itself");
        }
        for cmd in ["rm -rf /usr/local/x", "rm -rf ~/x", "rm -rf ~/x/*", "rm -rf build", "rm -f /etc", "rm -rf \"$D\""] {
            assert_eq!(never_removed(cmd, Some(std::path::Path::new("/w"))), None, "{cmd}");
        }
        // A Windows drive, as its paths are written there.
        for path in ["C:", "C:/", "c:\\", "C:\\Windows", "C:/Program Files", "D:/Data/*"] {
            assert!(is_never_removed(path), "{path}");
        }
        assert!(!is_never_removed("C:/Users/me/x") && !is_never_removed("C:/Program Files/App"));
    }

    /// The card's one-line risk follows the command, however it is
    /// wrapped; what isn't destructive has none.
    #[test]
    fn a_destructive_commands_risk_is_named() {
        use types::permissions::DestructiveWarning as W;
        assert_eq!(destructive_warning("rm -rf build"), Some(W::RecursivelyForceRemovesFiles));
        assert_eq!(destructive_warning("rm -fr build"), Some(W::RecursivelyForceRemovesFiles));
        assert_eq!(destructive_warning("rm -r build"), Some(W::RecursivelyRemovesFiles));
        assert_eq!(destructive_warning("rm -f a"), Some(W::ForceRemovesFiles));
        assert_eq!(destructive_warning("bash -c 'git reset --hard'"), Some(W::DiscardsUncommittedChanges));
        assert_eq!(destructive_warning("git push -f"), Some(W::OverwritesRemoteHistory));
        assert_eq!(destructive_warning("git clean -fd"), Some(W::DeletesUntrackedFiles));
        assert_eq!(destructive_warning("git clean -nfd"), None);
        assert_eq!(destructive_warning("git checkout -- ."), Some(W::DiscardsWorkingTreeChanges));
        assert_eq!(destructive_warning("git stash drop"), Some(W::RemovesStashedChanges));
        assert_eq!(destructive_warning("git branch -D x"), Some(W::ForceDeletesBranch));
        assert_eq!(destructive_warning("git commit --no-verify -m x"), Some(W::SkipsSafetyHooks));
        assert_eq!(destructive_warning("git commit --amend"), Some(W::RewritesLastCommit));
        assert_eq!(destructive_warning("psql -c 'DROP TABLE users'"), Some(W::DropsDatabaseObjects));
        assert_eq!(destructive_warning("sqlite3 a.db 'DELETE FROM users;'"), Some(W::DeletesAllRows));
        assert_eq!(destructive_warning("kubectl delete pod x"), Some(W::DeletesKubernetesResources));
        assert_eq!(destructive_warning("terraform destroy"), Some(W::DestroysInfrastructure));
        assert_eq!(destructive_warning("ls -la"), None);
        assert_eq!(destructive_warning("echo rm -rf x"), None);
        assert_eq!(destructive_warning("/bin/rm -rf build"), Some(W::RecursivelyForceRemovesFiles));
    }

    /// The 2026-09-29 incident, word for word: a scheduled run removed the
    /// owner's folder of repos. The shell now says what it removes, where.
    #[test]
    fn shell_effects_name_what_a_command_removes_rewrites_and_creates() {
        let fx = shell_effects("cd /Users/me/workspaces && rm -rf nebo && git clone git@github.com:acme/nebo.git", None);
        assert_eq!(fx.deletes, vec!["file:/Users/me/workspaces/nebo"]);
        assert_eq!(fx.creates, vec!["file:/Users/me/workspaces/nebo"]);
        let fx = shell_effects("kill 1; rm -rf /Users/me/workspaces/nebo", None);
        assert_eq!(fx.deletes, vec!["file:/Users/me/workspaces/nebo"]);

        // Relative paths stay relative for the registry to anchor; the call's
        // own cwd is where they start.
        assert_eq!(shell_effects("rm -r build dist", None).deletes, vec!["file:build", "file:dist"]);
        assert_eq!(shell_effects("rm -- -odd", Some("/w")).deletes, vec!["file:/w/-odd"]);
        assert_eq!(shell_effects("bash -c 'cd ~ && rmdir x'", None).deletes, vec![format!("file:{}", types::pathres::expand("~/x").display())]);

        // What only the run knows is named by its command.
        assert_eq!(shell_effects("rm -rf \"$DIR\"", None).deletes, vec!["command:rm -rf …"]);
        assert_eq!(shell_effects("cd \"$X\" && rm y", None).deletes, vec!["command:rm y"]);

        // The temp folder is a folder like any other.
        assert_eq!(shell_effects("rm -rf /tmp/build-1 /private/tmp/x", None).deletes, vec!["file:/tmp/build-1", "file:/private/tmp/x"]);

        // Git: a working tree replaced, in the folder it runs in or `-C`.
        assert_eq!(shell_effects("cd /r && git checkout main && git rebase origin/main", None).overwrites, vec!["file:/r/.", "file:/r/."]);
        assert_eq!(shell_effects("git -C /r pull", None).overwrites, vec!["file:/r/."]);
        assert!(shell_effects("git -C /r status && git commit -m x", None).overwrites.is_empty());
        assert_eq!(shell_effects("git clone --depth 1 https://github.com/acme/tool.git t2", Some("/w")).creates, vec!["file:/w/t2"]);
        assert_eq!(shell_effects("git worktree add -b fix ../wt", Some("/w/r")).creates, vec!["file:/w/r/../wt"]);
        assert_eq!(shell_effects("mkdir -p -m 755 out/a", Some("/w")).creates, vec!["file:/w/out/a"]);
        assert_eq!(shell_effects("ls -la", None), types::permissions::CallEffects::unknown());
    }

    #[test]
    fn test_is_privilege_escalation() {
        // Direct invocation
        assert!(is_privilege_escalation("sudo apt install vim"));
        assert!(is_privilege_escalation("doas pkg_add curl"));
        assert!(is_privilege_escalation("su - root"));
        // Hidden behind pipes, separators, and substitutions
        assert!(is_privilege_escalation(
            "echo \"hello\" | sudo tee /var/root/f > /dev/null"
        ));
        assert!(is_privilege_escalation("cd /tmp && sudo rm file"));
        assert!(is_privilege_escalation("ls; sudo whoami"));
        assert!(is_privilege_escalation("echo $(sudo id)"));
        assert!(is_privilege_escalation("echo `sudo id`"));
        // Not escalation: substrings and quoted words are not the sudo token
        assert!(!is_privilege_escalation("ls -la"));
        assert!(!is_privilege_escalation("echo superuser"));
        assert!(!is_privilege_escalation("visudo --check /etc/sudoers"));
        assert!(!is_privilege_escalation("git commit -m 'use sudo'"));
        assert!(!is_privilege_escalation("grep sudoers /etc/group"));
    }

    /// Watching the owner or driving his apps is matched on the program a
    /// command runs, wherever it stands in the call, and never on a word in
    /// another program's arguments.
    #[test]
    fn owner_reach_is_the_program_a_command_runs() {
        use types::permissions::OwnerReach::{self, App, Camera, Input, Microphone, Screen};
        let app = |a: &str| App { app: Some(a.to_string()) };
        let cases: &[(&str, Option<OwnerReach>)] = &[
            ("screencapture -x /tmp/a.png", Some(Screen)),
            ("/usr/sbin/screencapture -x a.png", Some(Screen)),
            ("for i in $(seq 90); do screencapture -x f$i.png; done", Some(Screen)),
            ("mkdir -p shots && cd shots && nohup screencapture -x a.png", Some(Screen)),
            ("bash -c 'screencapture -x a.png'", Some(Screen)),
            ("ffmpeg -f avfoundation -i 1:0 out.mp4", Some(Screen)),
            ("ffmpeg -f x11grab -i :0.0 out.mp4", Some(Screen)),
            ("ffmpeg -f gdigrab -i desktop out.mp4", Some(Screen)),
            ("ffmpeg -f alsa -i default out.wav", Some(Microphone)),
            ("ffmpeg -f v4l2 -i /dev/video0 out.mp4", Some(Camera)),
            ("gnome-screenshot -f a.png", Some(Screen)),
            ("scrot a.png", Some(Screen)),
            ("import -window root a.png", Some(Screen)),
            ("imagesnap a.jpg", Some(Camera)),
            ("rec out.wav", Some(Microphone)),
            ("sox -d out.wav trim 0 5", Some(Microphone)),
            ("arecord -d 5 out.wav", Some(Microphone)),
            (r#"osascript -e 'tell application "Safari" to do JavaScript "location.href = 1" in document 1'"#, Some(app("Safari"))),
            (r#"osascript -e 'tell app "Mail" to activate'"#, Some(app("Mail"))),
            (r#"osascript -l JavaScript -e 'Application("Safari").activate()'"#, Some(app("Safari"))),
            (r#"osascript -e 'tell application "System Events" to keystroke "v" using command down'"#, Some(Input)),
            ("osascript ~/drive.scpt", Some(App { app: None })),
            ("open -a Safari https://example.com", Some(app("Safari"))),
            ("open -b com.apple.Safari", Some(app("com.apple.Safari"))),
            ("cliclick c:100,200", Some(Input)),
            ("xdotool type hello", Some(Input)),
            ("AutoHotkey64.exe drive.ahk", Some(Input)),
            (r#"powershell -Command "[System.Windows.Forms.SendKeys]::SendWait('hi')""#, Some(Input)),
            (r#"powershell -Command "$g.CopyFromScreen(0, 0, 0, 0, $b.Size)""#, Some(Screen)),
            ("Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.SendKeys]::SendWait('hi')", Some(Input)),
            // Not watching or driving: the word is only text, the file
            // opens in its own app, or nothing is told to an app.
            ("grep screencapture notes.txt", None),
            ("rg -n 'osascript' src", None),
            ("echo osascript", None),
            ("echo 'tell application \"Safari\"' > notes.txt", None),
            ("open file.pdf", None),
            ("open https://example.com", None),
            (r#"osascript -e 'display notification "done"'"#, None),
            ("ffmpeg -i in.mov -f mp4 out.mp4", None),
            ("ffmpeg -i in.wav -f alsa default", None),
            ("sox in.wav -d", None),
            ("man screencapture", None),
            ("ls -la", None),
        ];
        for (cmd, want) in cases {
            assert_eq!(owner_reach(cmd).as_ref(), want.as_ref(), "{cmd}");
        }
    }

    /// "Allow always" saves the program, so the next capture or script is
    /// covered whatever its file or words, and the saved prefix covers the
    /// command it was saved for.
    #[test]
    fn a_reach_prefix_names_the_program_and_covers_its_command() {
        for (cmd, prefix) in [
            ("screencapture -x /tmp/a.png", Some("screencapture")),
            (r#"osascript -e 'tell application "Safari" to activate'"#, Some("osascript")),
            ("open -a Safari https://example.com", Some("open -a")),
            ("open -g -a Safari", Some("open -g -a Safari")),
            ("ffmpeg -f avfoundation -i 1 out.mp4", Some("ffmpeg")),
            ("X=1 screencapture a.png", None),
        ] {
            let sub = subcommands(cmd).into_iter().find(|s| reach_of(s).is_some()).expect(cmd);
            assert_eq!(reach_prefix(&sub).as_deref(), prefix, "{cmd}");
            if let Some(p) = prefix {
                assert_eq!(sub.covered_by(p, true), Cover::Yes, "{cmd}");
            }
        }
        let next = subcommands("screencapture -x /tmp/b.png").remove(0);
        assert_eq!(next.covered_by("screencapture", true), Cover::Yes, "the next capture is covered");
    }
}
