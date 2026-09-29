//! Which shell commands only read, judged on the words of one command as the
//! shell splitter reads them ([`crate::policy::subcommands`]): a command is
//! read-only when its name and every flag are on the allowlist, or it is one
//! of the commands that read whatever their arguments.
//!
//! The words come from a real shell grammar with quoting removed, and a word
//! only known when the command runs (`$X`, `$(…)`, a glob) never reaches
//! here, so no `$`, brace or backtick checks are needed: nothing unexpanded
//! can slip through.

/// What a flag takes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Arg {
    None,
    Number,
    String,
    Char,
    /// The literal `{}` (xargs `-I`).
    Braces,
    /// The literal `EOF` (xargs `-E`).
    Eof,
}

/// A command and the flags it may be given while only reading.
struct Config {
    /// The command's words (`git diff`).
    command: &'static str,
    /// The tool stops reading flags at `--`.
    double_dash: bool,
    flags: &'static [(&'static str, Arg)],
}

/// The read-only commands and the flags each may take, git, docker, ripgrep
/// and pyright included, the longest command first (so `git stash list`
/// matches before `git`).
const ALLOWLIST: &[Config] = &[
    Config { command: "git stash list", double_dash: true, flags: &[("--oneline", Arg::None), ("--graph", Arg::None), ("--decorate", Arg::None), ("--no-decorate", Arg::None), ("--date", Arg::String), ("--relative-date", Arg::None), ("--all", Arg::None), ("--branches", Arg::None), ("--tags", Arg::None), ("--remotes", Arg::None), ("--max-count", Arg::Number), ("-n", Arg::Number)] },
    Config { command: "git config --get", double_dash: true, flags: &[("--local", Arg::None), ("--global", Arg::None), ("--system", Arg::None), ("--worktree", Arg::None), ("--default", Arg::String), ("--type", Arg::String), ("--bool", Arg::None), ("--int", Arg::None), ("--bool-or-int", Arg::None), ("--path", Arg::None), ("--expiry-date", Arg::None), ("-z", Arg::None), ("--null", Arg::None), ("--name-only", Arg::None), ("--show-origin", Arg::None), ("--show-scope", Arg::None)] },
    Config { command: "git remote show", double_dash: true, flags: &[("-n", Arg::None)] },
    Config { command: "git stash show", double_dash: true, flags: &[("--stat", Arg::None), ("--numstat", Arg::None), ("--shortstat", Arg::None), ("--name-only", Arg::None), ("--name-status", Arg::None), ("--color", Arg::None), ("--no-color", Arg::None), ("--patch", Arg::None), ("-p", Arg::None), ("--no-patch", Arg::None), ("--no-ext-diff", Arg::None), ("-s", Arg::None), ("--word-diff", Arg::None), ("--word-diff-regex", Arg::String), ("--diff-filter", Arg::String), ("--abbrev", Arg::Number)] },
    Config { command: "git worktree list", double_dash: true, flags: &[("--porcelain", Arg::None), ("-v", Arg::None), ("--verbose", Arg::None), ("--expire", Arg::String)] },
    Config { command: "git diff", double_dash: true, flags: &[("--stat", Arg::None), ("--numstat", Arg::None), ("--shortstat", Arg::None), ("--name-only", Arg::None), ("--name-status", Arg::None), ("--color", Arg::None), ("--no-color", Arg::None), ("--dirstat", Arg::None), ("--summary", Arg::None), ("--patch-with-stat", Arg::None), ("--word-diff", Arg::None), ("--word-diff-regex", Arg::String), ("--color-words", Arg::None), ("--no-renames", Arg::None), ("--no-ext-diff", Arg::None), ("--check", Arg::None), ("--ws-error-highlight", Arg::String), ("--full-index", Arg::None), ("--binary", Arg::None), ("--abbrev", Arg::Number), ("--break-rewrites", Arg::None), ("--find-renames", Arg::None), ("--find-copies", Arg::None), ("--find-copies-harder", Arg::None), ("--irreversible-delete", Arg::None), ("--diff-algorithm", Arg::String), ("--histogram", Arg::None), ("--patience", Arg::None), ("--minimal", Arg::None), ("--ignore-space-at-eol", Arg::None), ("--ignore-space-change", Arg::None), ("--ignore-all-space", Arg::None), ("--ignore-blank-lines", Arg::None), ("--inter-hunk-context", Arg::Number), ("--function-context", Arg::None), ("--exit-code", Arg::None), ("--quiet", Arg::None), ("--cached", Arg::None), ("--staged", Arg::None), ("--pickaxe-regex", Arg::None), ("--pickaxe-all", Arg::None), ("--no-index", Arg::None), ("--relative", Arg::String), ("--diff-filter", Arg::String), ("-p", Arg::None), ("-u", Arg::None), ("-s", Arg::None), ("-M", Arg::None), ("-C", Arg::None), ("-B", Arg::None), ("-D", Arg::None), ("-l", Arg::None), ("-S", Arg::String), ("-G", Arg::String), ("-O", Arg::String), ("-R", Arg::None)] },
    Config { command: "git log", double_dash: true, flags: &[("--oneline", Arg::None), ("--graph", Arg::None), ("--decorate", Arg::None), ("--no-decorate", Arg::None), ("--date", Arg::String), ("--relative-date", Arg::None), ("--all", Arg::None), ("--branches", Arg::None), ("--tags", Arg::None), ("--remotes", Arg::None), ("--since", Arg::String), ("--after", Arg::String), ("--until", Arg::String), ("--before", Arg::String), ("--max-count", Arg::Number), ("-n", Arg::Number), ("--stat", Arg::None), ("--numstat", Arg::None), ("--shortstat", Arg::None), ("--name-only", Arg::None), ("--name-status", Arg::None), ("--color", Arg::None), ("--no-color", Arg::None), ("--patch", Arg::None), ("-p", Arg::None), ("--no-patch", Arg::None), ("--no-ext-diff", Arg::None), ("-s", Arg::None), ("--author", Arg::String), ("--committer", Arg::String), ("--grep", Arg::String), ("--abbrev-commit", Arg::None), ("--full-history", Arg::None), ("--dense", Arg::None), ("--sparse", Arg::None), ("--simplify-merges", Arg::None), ("--ancestry-path", Arg::None), ("--source", Arg::None), ("--first-parent", Arg::None), ("--merges", Arg::None), ("--no-merges", Arg::None), ("--reverse", Arg::None), ("--walk-reflogs", Arg::None), ("--skip", Arg::Number), ("--max-age", Arg::Number), ("--min-age", Arg::Number), ("--no-min-parents", Arg::None), ("--no-max-parents", Arg::None), ("--follow", Arg::None), ("--no-walk", Arg::None), ("--left-right", Arg::None), ("--cherry-mark", Arg::None), ("--cherry-pick", Arg::None), ("--boundary", Arg::None), ("--topo-order", Arg::None), ("--date-order", Arg::None), ("--author-date-order", Arg::None), ("--pretty", Arg::String), ("--format", Arg::String), ("--diff-filter", Arg::String), ("-S", Arg::String), ("-G", Arg::String), ("--pickaxe-regex", Arg::None), ("--pickaxe-all", Arg::None)] },
    Config { command: "git show", double_dash: true, flags: &[("--oneline", Arg::None), ("--graph", Arg::None), ("--decorate", Arg::None), ("--no-decorate", Arg::None), ("--date", Arg::String), ("--relative-date", Arg::None), ("--stat", Arg::None), ("--numstat", Arg::None), ("--shortstat", Arg::None), ("--name-only", Arg::None), ("--name-status", Arg::None), ("--color", Arg::None), ("--no-color", Arg::None), ("--patch", Arg::None), ("-p", Arg::None), ("--no-patch", Arg::None), ("--no-ext-diff", Arg::None), ("-s", Arg::None), ("--abbrev-commit", Arg::None), ("--word-diff", Arg::None), ("--word-diff-regex", Arg::String), ("--color-words", Arg::None), ("--pretty", Arg::String), ("--format", Arg::String), ("--first-parent", Arg::None), ("--raw", Arg::None), ("--diff-filter", Arg::String), ("-m", Arg::None), ("--quiet", Arg::None)] },
    Config { command: "git shortlog", double_dash: true, flags: &[("--all", Arg::None), ("--branches", Arg::None), ("--tags", Arg::None), ("--remotes", Arg::None), ("--since", Arg::String), ("--after", Arg::String), ("--until", Arg::String), ("--before", Arg::String), ("-s", Arg::None), ("--summary", Arg::None), ("-n", Arg::None), ("--numbered", Arg::None), ("-e", Arg::None), ("--email", Arg::None), ("-c", Arg::None), ("--committer", Arg::None), ("--group", Arg::String), ("--format", Arg::String), ("--no-merges", Arg::None), ("--author", Arg::String)] },
    Config { command: "git reflog", double_dash: true, flags: &[("--oneline", Arg::None), ("--graph", Arg::None), ("--decorate", Arg::None), ("--no-decorate", Arg::None), ("--date", Arg::String), ("--relative-date", Arg::None), ("--all", Arg::None), ("--branches", Arg::None), ("--tags", Arg::None), ("--remotes", Arg::None), ("--since", Arg::String), ("--after", Arg::String), ("--until", Arg::String), ("--before", Arg::String), ("--max-count", Arg::Number), ("-n", Arg::Number), ("--author", Arg::String), ("--committer", Arg::String), ("--grep", Arg::String)] },
    Config { command: "git ls-remote", double_dash: true, flags: &[("--branches", Arg::None), ("-b", Arg::None), ("--tags", Arg::None), ("-t", Arg::None), ("--heads", Arg::None), ("-h", Arg::None), ("--refs", Arg::None), ("--quiet", Arg::None), ("-q", Arg::None), ("--exit-code", Arg::None), ("--get-url", Arg::None), ("--symref", Arg::None), ("--sort", Arg::String)] },
    Config { command: "git status", double_dash: true, flags: &[("--short", Arg::None), ("-s", Arg::None), ("--branch", Arg::None), ("-b", Arg::None), ("--porcelain", Arg::None), ("--long", Arg::None), ("--verbose", Arg::None), ("-v", Arg::None), ("--untracked-files", Arg::String), ("-u", Arg::String), ("--ignored", Arg::None), ("--ignore-submodules", Arg::String), ("--column", Arg::None), ("--no-column", Arg::None), ("--ahead-behind", Arg::None), ("--no-ahead-behind", Arg::None), ("--renames", Arg::None), ("--no-renames", Arg::None), ("--find-renames", Arg::String), ("-M", Arg::String)] },
    Config { command: "git blame", double_dash: true, flags: &[("--color", Arg::None), ("--no-color", Arg::None), ("-L", Arg::String), ("--porcelain", Arg::None), ("-p", Arg::None), ("--line-porcelain", Arg::None), ("--incremental", Arg::None), ("--root", Arg::None), ("--show-stats", Arg::None), ("--show-name", Arg::None), ("--show-number", Arg::None), ("-n", Arg::None), ("--show-email", Arg::None), ("-e", Arg::None), ("-f", Arg::None), ("--date", Arg::String), ("-w", Arg::None), ("--ignore-rev", Arg::String), ("--ignore-revs-file", Arg::String), ("-M", Arg::None), ("-C", Arg::None), ("--score-debug", Arg::None), ("--abbrev", Arg::Number), ("-s", Arg::None), ("-l", Arg::None), ("-t", Arg::None)] },
    Config { command: "git ls-files", double_dash: true, flags: &[("--cached", Arg::None), ("-c", Arg::None), ("--deleted", Arg::None), ("-d", Arg::None), ("--modified", Arg::None), ("-m", Arg::None), ("--others", Arg::None), ("-o", Arg::None), ("--ignored", Arg::None), ("-i", Arg::None), ("--stage", Arg::None), ("-s", Arg::None), ("--killed", Arg::None), ("-k", Arg::None), ("--unmerged", Arg::None), ("-u", Arg::None), ("--directory", Arg::None), ("--no-empty-directory", Arg::None), ("--eol", Arg::None), ("--full-name", Arg::None), ("--abbrev", Arg::Number), ("--debug", Arg::None), ("-z", Arg::None), ("-t", Arg::None), ("-v", Arg::None), ("-f", Arg::None), ("--exclude", Arg::String), ("-x", Arg::String), ("--exclude-from", Arg::String), ("-X", Arg::String), ("--exclude-per-directory", Arg::String), ("--exclude-standard", Arg::None), ("--error-unmatch", Arg::None), ("--recurse-submodules", Arg::None)] },
    Config { command: "git remote", double_dash: true, flags: &[("-v", Arg::None), ("--verbose", Arg::None)] },
    Config { command: "git merge-base", double_dash: true, flags: &[("--is-ancestor", Arg::None), ("--fork-point", Arg::None), ("--octopus", Arg::None), ("--independent", Arg::None), ("--all", Arg::None)] },
    Config { command: "git rev-parse", double_dash: true, flags: &[("--verify", Arg::None), ("--short", Arg::String), ("--abbrev-ref", Arg::None), ("--symbolic", Arg::None), ("--symbolic-full-name", Arg::None), ("--show-toplevel", Arg::None), ("--show-cdup", Arg::None), ("--show-prefix", Arg::None), ("--git-dir", Arg::None), ("--git-common-dir", Arg::None), ("--absolute-git-dir", Arg::None), ("--show-superproject-working-tree", Arg::None), ("--is-inside-work-tree", Arg::None), ("--is-inside-git-dir", Arg::None), ("--is-bare-repository", Arg::None), ("--is-shallow-repository", Arg::None), ("--is-shallow-update", Arg::None), ("--path-prefix", Arg::None)] },
    Config { command: "git rev-list", double_dash: true, flags: &[("--all", Arg::None), ("--branches", Arg::None), ("--tags", Arg::None), ("--remotes", Arg::None), ("--since", Arg::String), ("--after", Arg::String), ("--until", Arg::String), ("--before", Arg::String), ("--max-count", Arg::Number), ("-n", Arg::Number), ("--author", Arg::String), ("--committer", Arg::String), ("--grep", Arg::String), ("--count", Arg::None), ("--reverse", Arg::None), ("--first-parent", Arg::None), ("--ancestry-path", Arg::None), ("--merges", Arg::None), ("--no-merges", Arg::None), ("--min-parents", Arg::Number), ("--max-parents", Arg::Number), ("--no-min-parents", Arg::None), ("--no-max-parents", Arg::None), ("--skip", Arg::Number), ("--max-age", Arg::Number), ("--min-age", Arg::Number), ("--walk-reflogs", Arg::None), ("--oneline", Arg::None), ("--abbrev-commit", Arg::None), ("--pretty", Arg::String), ("--format", Arg::String), ("--abbrev", Arg::Number), ("--full-history", Arg::None), ("--dense", Arg::None), ("--sparse", Arg::None), ("--source", Arg::None), ("--graph", Arg::None)] },
    Config { command: "git describe", double_dash: true, flags: &[("--tags", Arg::None), ("--match", Arg::String), ("--exclude", Arg::String), ("--long", Arg::None), ("--abbrev", Arg::Number), ("--always", Arg::None), ("--contains", Arg::None), ("--first-match", Arg::None), ("--exact-match", Arg::None), ("--candidates", Arg::Number), ("--dirty", Arg::None), ("--broken", Arg::None)] },
    Config { command: "git cat-file", double_dash: true, flags: &[("-t", Arg::None), ("-s", Arg::None), ("-p", Arg::None), ("-e", Arg::None), ("--batch-check", Arg::None), ("--allow-undetermined-type", Arg::None)] },
    Config { command: "git for-each-ref", double_dash: true, flags: &[("--format", Arg::String), ("--sort", Arg::String), ("--count", Arg::Number), ("--contains", Arg::String), ("--no-contains", Arg::String), ("--merged", Arg::String), ("--no-merged", Arg::String), ("--points-at", Arg::String)] },
    Config { command: "git grep", double_dash: true, flags: &[("-e", Arg::String), ("-E", Arg::None), ("--extended-regexp", Arg::None), ("-G", Arg::None), ("--basic-regexp", Arg::None), ("-F", Arg::None), ("--fixed-strings", Arg::None), ("-P", Arg::None), ("--perl-regexp", Arg::None), ("-i", Arg::None), ("--ignore-case", Arg::None), ("-v", Arg::None), ("--invert-match", Arg::None), ("-w", Arg::None), ("--word-regexp", Arg::None), ("-n", Arg::None), ("--line-number", Arg::None), ("-c", Arg::None), ("--count", Arg::None), ("-l", Arg::None), ("--files-with-matches", Arg::None), ("-L", Arg::None), ("--files-without-match", Arg::None), ("-h", Arg::None), ("-H", Arg::None), ("--heading", Arg::None), ("--break", Arg::None), ("--full-name", Arg::None), ("--color", Arg::None), ("--no-color", Arg::None), ("-o", Arg::None), ("--only-matching", Arg::None), ("-A", Arg::Number), ("--after-context", Arg::Number), ("-B", Arg::Number), ("--before-context", Arg::Number), ("-C", Arg::Number), ("--context", Arg::Number), ("--and", Arg::None), ("--or", Arg::None), ("--not", Arg::None), ("--max-depth", Arg::Number), ("--untracked", Arg::None), ("--no-index", Arg::None), ("--recurse-submodules", Arg::None), ("--cached", Arg::None), ("--threads", Arg::Number), ("-q", Arg::None), ("--quiet", Arg::None)] },
    Config { command: "git tag", double_dash: true, flags: &[("-l", Arg::None), ("--list", Arg::None), ("-n", Arg::Number), ("--contains", Arg::String), ("--no-contains", Arg::String), ("--merged", Arg::String), ("--no-merged", Arg::String), ("--sort", Arg::String), ("--format", Arg::String), ("--points-at", Arg::String), ("--column", Arg::None), ("--no-column", Arg::None), ("-i", Arg::None), ("--ignore-case", Arg::None)] },
    Config { command: "git branch", double_dash: true, flags: &[("-l", Arg::None), ("--list", Arg::None), ("-a", Arg::None), ("--all", Arg::None), ("-r", Arg::None), ("--remotes", Arg::None), ("-v", Arg::None), ("-vv", Arg::None), ("--verbose", Arg::None), ("--color", Arg::None), ("--no-color", Arg::None), ("--column", Arg::None), ("--no-column", Arg::None), ("--abbrev", Arg::Number), ("--no-abbrev", Arg::None), ("--contains", Arg::String), ("--no-contains", Arg::String), ("--merged", Arg::None), ("--no-merged", Arg::None), ("--points-at", Arg::String), ("--sort", Arg::String), ("--show-current", Arg::None), ("-i", Arg::None), ("--ignore-case", Arg::None)] },
    Config { command: "docker logs", double_dash: true, flags: &[("--follow", Arg::None), ("-f", Arg::None), ("--tail", Arg::String), ("-n", Arg::String), ("--timestamps", Arg::None), ("-t", Arg::None), ("--since", Arg::String), ("--until", Arg::String), ("--details", Arg::None)] },
    Config { command: "docker inspect", double_dash: true, flags: &[("--format", Arg::String), ("-f", Arg::String), ("--type", Arg::String), ("--size", Arg::None), ("-s", Arg::None)] },
    Config { command: "xargs", double_dash: true, flags: &[("-I", Arg::Braces), ("-n", Arg::Number), ("-P", Arg::Number), ("-L", Arg::Number), ("-s", Arg::Number), ("-E", Arg::Eof), ("-0", Arg::None), ("-t", Arg::None), ("-r", Arg::None), ("-x", Arg::None), ("-d", Arg::Char)] },
    Config { command: "file", double_dash: true, flags: &[("--brief", Arg::None), ("-b", Arg::None), ("--mime", Arg::None), ("-i", Arg::None), ("--mime-type", Arg::None), ("--mime-encoding", Arg::None), ("--apple", Arg::None), ("--check-encoding", Arg::None), ("-c", Arg::None), ("--exclude", Arg::String), ("--exclude-quiet", Arg::String), ("--print0", Arg::None), ("-0", Arg::None), ("-f", Arg::String), ("-F", Arg::String), ("--separator", Arg::String), ("--help", Arg::None), ("--version", Arg::None), ("-v", Arg::None), ("--no-dereference", Arg::None), ("-h", Arg::None), ("--dereference", Arg::None), ("-L", Arg::None), ("--magic-file", Arg::String), ("-m", Arg::String), ("--keep-going", Arg::None), ("-k", Arg::None), ("--list", Arg::None), ("-l", Arg::None), ("--no-buffer", Arg::None), ("-n", Arg::None), ("--preserve-date", Arg::None), ("-p", Arg::None), ("--raw", Arg::None), ("-r", Arg::None), ("-s", Arg::None), ("--special-files", Arg::None), ("--uncompress", Arg::None), ("-z", Arg::None)] },
    Config { command: "sed", double_dash: true, flags: &[("--expression", Arg::String), ("-e", Arg::String), ("--quiet", Arg::None), ("--silent", Arg::None), ("-n", Arg::None), ("--regexp-extended", Arg::None), ("-r", Arg::None), ("--posix", Arg::None), ("-E", Arg::None), ("--line-length", Arg::Number), ("-l", Arg::Number), ("--zero-terminated", Arg::None), ("-z", Arg::None), ("--separate", Arg::None), ("-s", Arg::None), ("--unbuffered", Arg::None), ("-u", Arg::None), ("--debug", Arg::None), ("--help", Arg::None), ("--version", Arg::None)] },
    Config { command: "sort", double_dash: true, flags: &[("--ignore-leading-blanks", Arg::None), ("-b", Arg::None), ("--dictionary-order", Arg::None), ("-d", Arg::None), ("--ignore-case", Arg::None), ("-f", Arg::None), ("--general-numeric-sort", Arg::None), ("-g", Arg::None), ("--human-numeric-sort", Arg::None), ("-h", Arg::None), ("--ignore-nonprinting", Arg::None), ("-i", Arg::None), ("--month-sort", Arg::None), ("-M", Arg::None), ("--numeric-sort", Arg::None), ("-n", Arg::None), ("--random-sort", Arg::None), ("-R", Arg::None), ("--reverse", Arg::None), ("-r", Arg::None), ("--sort", Arg::String), ("--stable", Arg::None), ("-s", Arg::None), ("--unique", Arg::None), ("-u", Arg::None), ("--version-sort", Arg::None), ("-V", Arg::None), ("--zero-terminated", Arg::None), ("-z", Arg::None), ("--key", Arg::String), ("-k", Arg::String), ("--field-separator", Arg::String), ("-t", Arg::String), ("--check", Arg::None), ("-c", Arg::None), ("--check-char-order", Arg::None), ("-C", Arg::None), ("--merge", Arg::None), ("-m", Arg::None), ("--buffer-size", Arg::String), ("-S", Arg::String), ("--parallel", Arg::Number), ("--batch-size", Arg::Number), ("--help", Arg::None), ("--version", Arg::None)] },
    Config { command: "man", double_dash: true, flags: &[("-a", Arg::None), ("--all", Arg::None), ("-d", Arg::None), ("-f", Arg::None), ("--whatis", Arg::None), ("-h", Arg::None), ("-k", Arg::None), ("--apropos", Arg::None), ("-l", Arg::String), ("-w", Arg::None), ("-S", Arg::String), ("-s", Arg::String)] },
    Config { command: "help", double_dash: true, flags: &[("-d", Arg::None), ("-m", Arg::None), ("-s", Arg::None)] },
    Config { command: "netstat", double_dash: true, flags: &[("-a", Arg::None), ("-L", Arg::None), ("-l", Arg::None), ("-n", Arg::None), ("-f", Arg::String), ("-g", Arg::None), ("-i", Arg::None), ("-I", Arg::String), ("-s", Arg::None), ("-r", Arg::None), ("-m", Arg::None), ("-v", Arg::None)] },
    Config { command: "ps", double_dash: true, flags: &[("-e", Arg::None), ("-A", Arg::None), ("-a", Arg::None), ("-d", Arg::None), ("-N", Arg::None), ("--deselect", Arg::None), ("-f", Arg::None), ("-F", Arg::None), ("-l", Arg::None), ("-j", Arg::None), ("-y", Arg::None), ("-w", Arg::None), ("-ww", Arg::None), ("--width", Arg::Number), ("-c", Arg::None), ("-H", Arg::None), ("--forest", Arg::None), ("--headers", Arg::None), ("--no-headers", Arg::None), ("-n", Arg::String), ("--sort", Arg::String), ("-L", Arg::None), ("-T", Arg::None), ("-m", Arg::None), ("-C", Arg::String), ("-G", Arg::String), ("-g", Arg::String), ("-p", Arg::String), ("--pid", Arg::String), ("-q", Arg::String), ("--quick-pid", Arg::String), ("-s", Arg::String), ("--sid", Arg::String), ("-t", Arg::String), ("--tty", Arg::String), ("-U", Arg::String), ("-u", Arg::String), ("--user", Arg::String), ("--help", Arg::None), ("--info", Arg::None), ("-V", Arg::None), ("--version", Arg::None)] },
    Config { command: "base64", double_dash: false, flags: &[("-d", Arg::None), ("-D", Arg::None), ("--decode", Arg::None), ("-b", Arg::Number), ("--break", Arg::Number), ("-w", Arg::Number), ("--wrap", Arg::Number), ("-i", Arg::String), ("--input", Arg::String), ("--ignore-garbage", Arg::None), ("-h", Arg::None), ("--help", Arg::None), ("--version", Arg::None)] },
    Config { command: "grep", double_dash: true, flags: &[("-e", Arg::String), ("--regexp", Arg::String), ("-f", Arg::String), ("--file", Arg::String), ("-F", Arg::None), ("--fixed-strings", Arg::None), ("-G", Arg::None), ("--basic-regexp", Arg::None), ("-E", Arg::None), ("--extended-regexp", Arg::None), ("-P", Arg::None), ("--perl-regexp", Arg::None), ("-i", Arg::None), ("--ignore-case", Arg::None), ("--no-ignore-case", Arg::None), ("-v", Arg::None), ("--invert-match", Arg::None), ("-w", Arg::None), ("--word-regexp", Arg::None), ("-x", Arg::None), ("--line-regexp", Arg::None), ("-c", Arg::None), ("--count", Arg::None), ("--color", Arg::String), ("--colour", Arg::String), ("-L", Arg::None), ("--files-without-match", Arg::None), ("-l", Arg::None), ("--files-with-matches", Arg::None), ("-m", Arg::Number), ("--max-count", Arg::Number), ("-o", Arg::None), ("--only-matching", Arg::None), ("-q", Arg::None), ("--quiet", Arg::None), ("--silent", Arg::None), ("-s", Arg::None), ("--no-messages", Arg::None), ("-b", Arg::None), ("--byte-offset", Arg::None), ("-H", Arg::None), ("--with-filename", Arg::None), ("-h", Arg::None), ("--no-filename", Arg::None), ("--label", Arg::String), ("-n", Arg::None), ("--line-number", Arg::None), ("-T", Arg::None), ("--initial-tab", Arg::None), ("-u", Arg::None), ("--unix-byte-offsets", Arg::None), ("-Z", Arg::None), ("--null", Arg::None), ("-z", Arg::None), ("--null-data", Arg::None), ("-A", Arg::Number), ("--after-context", Arg::Number), ("-B", Arg::Number), ("--before-context", Arg::Number), ("-C", Arg::Number), ("--context", Arg::Number), ("--group-separator", Arg::String), ("--no-group-separator", Arg::None), ("-a", Arg::None), ("--text", Arg::None), ("--binary-files", Arg::String), ("-D", Arg::String), ("--devices", Arg::String), ("-d", Arg::String), ("--directories", Arg::String), ("--exclude", Arg::String), ("--exclude-from", Arg::String), ("--exclude-dir", Arg::String), ("--include", Arg::String), ("-r", Arg::None), ("--recursive", Arg::None), ("-R", Arg::None), ("--dereference-recursive", Arg::None), ("--line-buffered", Arg::None), ("-U", Arg::None), ("--binary", Arg::None), ("--help", Arg::None), ("-V", Arg::None), ("--version", Arg::None)] },
    Config { command: "rg", double_dash: true, flags: &[("-e", Arg::String), ("--regexp", Arg::String), ("-f", Arg::String), ("-i", Arg::None), ("--ignore-case", Arg::None), ("-S", Arg::None), ("--smart-case", Arg::None), ("-F", Arg::None), ("--fixed-strings", Arg::None), ("-w", Arg::None), ("--word-regexp", Arg::None), ("-v", Arg::None), ("--invert-match", Arg::None), ("-c", Arg::None), ("--count", Arg::None), ("-l", Arg::None), ("--files-with-matches", Arg::None), ("--files-without-match", Arg::None), ("-n", Arg::None), ("--line-number", Arg::None), ("-o", Arg::None), ("--only-matching", Arg::None), ("-A", Arg::Number), ("--after-context", Arg::Number), ("-B", Arg::Number), ("--before-context", Arg::Number), ("-C", Arg::Number), ("--context", Arg::Number), ("-H", Arg::None), ("-h", Arg::None), ("--heading", Arg::None), ("--no-heading", Arg::None), ("-q", Arg::None), ("--quiet", Arg::None), ("--column", Arg::None), ("-g", Arg::String), ("--glob", Arg::String), ("-t", Arg::String), ("--type", Arg::String), ("-T", Arg::String), ("--type-not", Arg::String), ("--type-list", Arg::None), ("--hidden", Arg::None), ("--no-ignore", Arg::None), ("-u", Arg::None), ("-m", Arg::Number), ("--max-count", Arg::Number), ("-d", Arg::Number), ("--max-depth", Arg::Number), ("-a", Arg::None), ("--text", Arg::None), ("-z", Arg::None), ("-L", Arg::None), ("--follow", Arg::None), ("--color", Arg::String), ("--json", Arg::None), ("--stats", Arg::None), ("--help", Arg::None), ("--version", Arg::None), ("--debug", Arg::None), ("--", Arg::None)] },
    Config { command: "sha256sum", double_dash: true, flags: &[("-b", Arg::None), ("--binary", Arg::None), ("-t", Arg::None), ("--text", Arg::None), ("-c", Arg::None), ("--check", Arg::None), ("--ignore-missing", Arg::None), ("--quiet", Arg::None), ("--status", Arg::None), ("--strict", Arg::None), ("-w", Arg::None), ("--warn", Arg::None), ("--tag", Arg::None), ("-z", Arg::None), ("--zero", Arg::None), ("--help", Arg::None), ("--version", Arg::None)] },
    Config { command: "sha1sum", double_dash: true, flags: &[("-b", Arg::None), ("--binary", Arg::None), ("-t", Arg::None), ("--text", Arg::None), ("-c", Arg::None), ("--check", Arg::None), ("--ignore-missing", Arg::None), ("--quiet", Arg::None), ("--status", Arg::None), ("--strict", Arg::None), ("-w", Arg::None), ("--warn", Arg::None), ("--tag", Arg::None), ("-z", Arg::None), ("--zero", Arg::None), ("--help", Arg::None), ("--version", Arg::None)] },
    Config { command: "md5sum", double_dash: true, flags: &[("-b", Arg::None), ("--binary", Arg::None), ("-t", Arg::None), ("--text", Arg::None), ("-c", Arg::None), ("--check", Arg::None), ("--ignore-missing", Arg::None), ("--quiet", Arg::None), ("--status", Arg::None), ("--strict", Arg::None), ("-w", Arg::None), ("--warn", Arg::None), ("--tag", Arg::None), ("-z", Arg::None), ("--zero", Arg::None), ("--help", Arg::None), ("--version", Arg::None)] },
    Config { command: "tree", double_dash: true, flags: &[("-a", Arg::None), ("-d", Arg::None), ("-l", Arg::None), ("-f", Arg::None), ("-x", Arg::None), ("-L", Arg::Number), ("-P", Arg::String), ("-I", Arg::String), ("--gitignore", Arg::None), ("--gitfile", Arg::String), ("--ignore-case", Arg::None), ("--matchdirs", Arg::None), ("--metafirst", Arg::None), ("--prune", Arg::None), ("--info", Arg::None), ("--infofile", Arg::String), ("--noreport", Arg::None), ("--charset", Arg::String), ("--filelimit", Arg::Number), ("-q", Arg::None), ("-N", Arg::None), ("-Q", Arg::None), ("-p", Arg::None), ("-u", Arg::None), ("-g", Arg::None), ("-s", Arg::None), ("-h", Arg::None), ("--si", Arg::None), ("--du", Arg::None), ("-D", Arg::None), ("--timefmt", Arg::String), ("-F", Arg::None), ("--inodes", Arg::None), ("--device", Arg::None), ("-v", Arg::None), ("-t", Arg::None), ("-c", Arg::None), ("-U", Arg::None), ("-r", Arg::None), ("--dirsfirst", Arg::None), ("--filesfirst", Arg::None), ("--sort", Arg::String), ("-i", Arg::None), ("-A", Arg::None), ("-S", Arg::None), ("-n", Arg::None), ("-C", Arg::None), ("-X", Arg::None), ("-J", Arg::None), ("-H", Arg::String), ("--nolinks", Arg::None), ("--hintro", Arg::String), ("--houtro", Arg::String), ("-T", Arg::String), ("--hyperlink", Arg::None), ("--scheme", Arg::String), ("--authority", Arg::String), ("--fromfile", Arg::None), ("--fromtabfile", Arg::None), ("--fflinks", Arg::None), ("--help", Arg::None), ("--version", Arg::None)] },
    Config { command: "date", double_dash: true, flags: &[("-d", Arg::String), ("--date", Arg::String), ("-r", Arg::String), ("--reference", Arg::String), ("-u", Arg::None), ("--utc", Arg::None), ("--universal", Arg::None), ("-I", Arg::None), ("--iso-8601", Arg::String), ("-R", Arg::None), ("--rfc-email", Arg::None), ("--rfc-3339", Arg::String), ("--debug", Arg::None), ("--help", Arg::None), ("--version", Arg::None)] },
    Config { command: "hostname", double_dash: true, flags: &[("-f", Arg::None), ("--fqdn", Arg::None), ("--long", Arg::None), ("-s", Arg::None), ("--short", Arg::None), ("-i", Arg::None), ("--ip-address", Arg::None), ("-I", Arg::None), ("--all-ip-addresses", Arg::None), ("-a", Arg::None), ("--alias", Arg::None), ("-d", Arg::None), ("--domain", Arg::None), ("-A", Arg::None), ("--all-fqdns", Arg::None), ("-v", Arg::None), ("--verbose", Arg::None), ("-h", Arg::None), ("--help", Arg::None), ("-V", Arg::None), ("--version", Arg::None)] },
    Config { command: "info", double_dash: true, flags: &[("-f", Arg::String), ("--file", Arg::String), ("-d", Arg::String), ("--directory", Arg::String), ("-n", Arg::String), ("--node", Arg::String), ("-a", Arg::None), ("--all", Arg::None), ("-k", Arg::String), ("--apropos", Arg::String), ("-w", Arg::None), ("--where", Arg::None), ("--location", Arg::None), ("--show-options", Arg::None), ("--vi-keys", Arg::None), ("--subnodes", Arg::None), ("-h", Arg::None), ("--help", Arg::None), ("--usage", Arg::None), ("--version", Arg::None)] },
    Config { command: "lsof", double_dash: true, flags: &[("-?", Arg::None), ("-h", Arg::None), ("-v", Arg::None), ("-a", Arg::None), ("-b", Arg::None), ("-C", Arg::None), ("-l", Arg::None), ("-n", Arg::None), ("-N", Arg::None), ("-O", Arg::None), ("-P", Arg::None), ("-Q", Arg::None), ("-R", Arg::None), ("-t", Arg::None), ("-U", Arg::None), ("-V", Arg::None), ("-X", Arg::None), ("-H", Arg::None), ("-E", Arg::None), ("-F", Arg::None), ("-g", Arg::None), ("-i", Arg::None), ("-K", Arg::None), ("-L", Arg::None), ("-o", Arg::None), ("-r", Arg::None), ("-s", Arg::None), ("-S", Arg::None), ("-T", Arg::None), ("-x", Arg::None), ("-A", Arg::String), ("-c", Arg::String), ("-d", Arg::String), ("-e", Arg::String), ("-k", Arg::String), ("-p", Arg::String), ("-u", Arg::String)] },
    Config { command: "pgrep", double_dash: true, flags: &[("-d", Arg::String), ("--delimiter", Arg::String), ("-l", Arg::None), ("--list-name", Arg::None), ("-a", Arg::None), ("--list-full", Arg::None), ("-v", Arg::None), ("--inverse", Arg::None), ("-w", Arg::None), ("--lightweight", Arg::None), ("-c", Arg::None), ("--count", Arg::None), ("-f", Arg::None), ("--full", Arg::None), ("-g", Arg::String), ("--pgroup", Arg::String), ("-G", Arg::String), ("--group", Arg::String), ("-i", Arg::None), ("--ignore-case", Arg::None), ("-n", Arg::None), ("--newest", Arg::None), ("-o", Arg::None), ("--oldest", Arg::None), ("-O", Arg::String), ("--older", Arg::String), ("-P", Arg::String), ("--parent", Arg::String), ("-s", Arg::String), ("--session", Arg::String), ("-t", Arg::String), ("--terminal", Arg::String), ("-u", Arg::String), ("--euid", Arg::String), ("-U", Arg::String), ("--uid", Arg::String), ("-x", Arg::None), ("--exact", Arg::None), ("-F", Arg::String), ("--pidfile", Arg::String), ("-L", Arg::None), ("--logpidfile", Arg::None), ("-r", Arg::String), ("--runstates", Arg::String), ("--ns", Arg::String), ("--nslist", Arg::String), ("--help", Arg::None), ("-V", Arg::None), ("--version", Arg::None)] },
    Config { command: "tput", double_dash: true, flags: &[("-T", Arg::String), ("-V", Arg::None), ("-x", Arg::None)] },
    Config { command: "ss", double_dash: true, flags: &[("-h", Arg::None), ("--help", Arg::None), ("-V", Arg::None), ("--version", Arg::None), ("-n", Arg::None), ("--numeric", Arg::None), ("-r", Arg::None), ("--resolve", Arg::None), ("-a", Arg::None), ("--all", Arg::None), ("-l", Arg::None), ("--listening", Arg::None), ("-o", Arg::None), ("--options", Arg::None), ("-e", Arg::None), ("--extended", Arg::None), ("-m", Arg::None), ("--memory", Arg::None), ("-p", Arg::None), ("--processes", Arg::None), ("-i", Arg::None), ("--info", Arg::None), ("-s", Arg::None), ("--summary", Arg::None), ("-4", Arg::None), ("--ipv4", Arg::None), ("-6", Arg::None), ("--ipv6", Arg::None), ("-0", Arg::None), ("--packet", Arg::None), ("-t", Arg::None), ("--tcp", Arg::None), ("-M", Arg::None), ("--mptcp", Arg::None), ("-S", Arg::None), ("--sctp", Arg::None), ("-u", Arg::None), ("--udp", Arg::None), ("-d", Arg::None), ("--dccp", Arg::None), ("-w", Arg::None), ("--raw", Arg::None), ("-x", Arg::None), ("--unix", Arg::None), ("--tipc", Arg::None), ("--vsock", Arg::None), ("-f", Arg::String), ("--family", Arg::String), ("-A", Arg::String), ("--query", Arg::String), ("--socket", Arg::String), ("-Z", Arg::None), ("--context", Arg::None), ("-z", Arg::None), ("--contexts", Arg::None), ("-b", Arg::None), ("--bpf", Arg::None), ("-E", Arg::None), ("--events", Arg::None), ("-H", Arg::None), ("--no-header", Arg::None), ("-O", Arg::None), ("--oneline", Arg::None), ("--tipcinfo", Arg::None), ("--tos", Arg::None), ("--cgroup", Arg::None), ("--inet-sockopt", Arg::None)] },
    Config { command: "fd", double_dash: true, flags: &[("-h", Arg::None), ("--help", Arg::None), ("-V", Arg::None), ("--version", Arg::None), ("-H", Arg::None), ("--hidden", Arg::None), ("-I", Arg::None), ("--no-ignore", Arg::None), ("--no-ignore-vcs", Arg::None), ("--no-ignore-parent", Arg::None), ("-s", Arg::None), ("--case-sensitive", Arg::None), ("-i", Arg::None), ("--ignore-case", Arg::None), ("-g", Arg::None), ("--glob", Arg::None), ("--regex", Arg::None), ("-F", Arg::None), ("--fixed-strings", Arg::None), ("-a", Arg::None), ("--absolute-path", Arg::None), ("-L", Arg::None), ("--follow", Arg::None), ("-p", Arg::None), ("--full-path", Arg::None), ("-0", Arg::None), ("--print0", Arg::None), ("-d", Arg::Number), ("--max-depth", Arg::Number), ("--min-depth", Arg::Number), ("--exact-depth", Arg::Number), ("-t", Arg::String), ("--type", Arg::String), ("-e", Arg::String), ("--extension", Arg::String), ("-S", Arg::String), ("--size", Arg::String), ("--changed-within", Arg::String), ("--changed-before", Arg::String), ("-o", Arg::String), ("--owner", Arg::String), ("-E", Arg::String), ("--exclude", Arg::String), ("--ignore-file", Arg::String), ("-c", Arg::String), ("--color", Arg::String), ("-j", Arg::Number), ("--threads", Arg::Number), ("--max-buffer-time", Arg::String), ("--max-results", Arg::Number), ("-1", Arg::None), ("-q", Arg::None), ("--quiet", Arg::None), ("--show-errors", Arg::None), ("--strip-cwd-prefix", Arg::None), ("--one-file-system", Arg::None), ("--prune", Arg::None), ("--search-path", Arg::String), ("--base-directory", Arg::String), ("--path-separator", Arg::String), ("--batch-size", Arg::Number), ("--no-require-git", Arg::None), ("--hyperlink", Arg::String), ("--and", Arg::String), ("--format", Arg::String)] },
    Config { command: "fdfind", double_dash: true, flags: &[("-h", Arg::None), ("--help", Arg::None), ("-V", Arg::None), ("--version", Arg::None), ("-H", Arg::None), ("--hidden", Arg::None), ("-I", Arg::None), ("--no-ignore", Arg::None), ("--no-ignore-vcs", Arg::None), ("--no-ignore-parent", Arg::None), ("-s", Arg::None), ("--case-sensitive", Arg::None), ("-i", Arg::None), ("--ignore-case", Arg::None), ("-g", Arg::None), ("--glob", Arg::None), ("--regex", Arg::None), ("-F", Arg::None), ("--fixed-strings", Arg::None), ("-a", Arg::None), ("--absolute-path", Arg::None), ("-L", Arg::None), ("--follow", Arg::None), ("-p", Arg::None), ("--full-path", Arg::None), ("-0", Arg::None), ("--print0", Arg::None), ("-d", Arg::Number), ("--max-depth", Arg::Number), ("--min-depth", Arg::Number), ("--exact-depth", Arg::Number), ("-t", Arg::String), ("--type", Arg::String), ("-e", Arg::String), ("--extension", Arg::String), ("-S", Arg::String), ("--size", Arg::String), ("--changed-within", Arg::String), ("--changed-before", Arg::String), ("-o", Arg::String), ("--owner", Arg::String), ("-E", Arg::String), ("--exclude", Arg::String), ("--ignore-file", Arg::String), ("-c", Arg::String), ("--color", Arg::String), ("-j", Arg::Number), ("--threads", Arg::Number), ("--max-buffer-time", Arg::String), ("--max-results", Arg::Number), ("-1", Arg::None), ("-q", Arg::None), ("--quiet", Arg::None), ("--show-errors", Arg::None), ("--strip-cwd-prefix", Arg::None), ("--one-file-system", Arg::None), ("--prune", Arg::None), ("--search-path", Arg::String), ("--base-directory", Arg::String), ("--path-separator", Arg::String), ("--batch-size", Arg::Number), ("--no-require-git", Arg::None), ("--hyperlink", Arg::String), ("--and", Arg::String), ("--format", Arg::String)] },
    Config { command: "pyright", double_dash: false, flags: &[("--outputjson", Arg::None), ("--project", Arg::String), ("-p", Arg::String), ("--pythonversion", Arg::String), ("--pythonplatform", Arg::String), ("--typeshedpath", Arg::String), ("--venvpath", Arg::String), ("--level", Arg::String), ("--stats", Arg::None), ("--verbose", Arg::None), ("--version", Arg::None), ("--dependencies", Arg::None), ("--warnings", Arg::None)] },
];

/// Commands that read whatever their arguments (`READONLY_COMMANDS`).
const READS: &[&str] = &[
    "cal", "uptime", "cat", "head", "tail", "wc", "stat", "strings", "hexdump", "od", "nl", "id", "uname", "free",
    "df", "du", "locale", "groups", "nproc", "basename", "dirname", "realpath", "cut", "paste", "tr", "column",
    "tac", "rev", "fold", "expand", "unexpand", "fmt", "comm", "cmp", "numfmt", "readlink", "diff", "true", "false",
    "sleep", "which", "type", "expr", "test", "getconf", "seq", "tsort", "pr", "echo", "ls",
];

/// Two-word commands that read whatever their arguments.
const READS_TWO: &[(&str, &str)] = &[("docker", "ps"), ("docker", "images")];

/// What xargs may run (`SAFE_TARGET_COMMANDS_FOR_XARGS`): commands with no
/// flag that writes, runs code or reaches the network.
const XARGS_TARGETS: &[&str] = &["echo", "printf", "wc", "grep", "head", "tail"];

/// Whether one command, its words with quoting removed, only reads.
pub(crate) fn reads_only(words: &[&str]) -> bool {
    !words.is_empty() && (by_flags(words) || by_shape(words))
}

/// `isCommandSafeViaFlagParsing`: an allowlisted command whose every flag is
/// allowlisted, and whose extra check finds nothing that writes.
fn by_flags(words: &[&str]) -> bool {
    let Some(config) = ALLOWLIST.iter().find(|c| {
        let cmd: Vec<&str> = c.command.split(' ').collect();
        words.len() >= cmd.len() && words[..cmd.len()] == cmd[..]
    }) else {
        return false;
    };
    let start = config.command.split(' ').count();
    let args = &words[start..];
    if config.command == "git ls-remote"
        && args.iter().any(|a| !a.starts_with('-') && (a.contains("://") || a.contains('@') || a.contains(':')))
    {
        return false;
    }
    if !flags_are_safe(words, start, config) {
        return false;
    }
    !writes_anyway(config.command, args, words)
}

/// `validateFlags`: every flag is on the command's list with the argument it
/// takes; a bundle (`-nr`) only of argument-less flags.
fn flags_are_safe(tokens: &[&str], start: usize, config: &Config) -> bool {
    let name = tokens[0];
    let flag_like = |t: &str| {
        let mut c = t.chars();
        c.next() == Some('-') && c.next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    };
    let find = |flag: &str| config.flags.iter().find(|(f, _)| *f == flag).map(|(_, a)| *a);
    let mut i = start;
    while i < tokens.len() {
        let token = tokens[i];
        if name == "xargs" && (!token.starts_with('-') || token == "--") {
            let target = if token == "--" { tokens.get(i + 1).copied() } else { Some(token) };
            return target.is_some_and(|t| XARGS_TARGETS.contains(&t));
        }
        if token == "--" {
            if config.double_dash {
                return true;
            }
            i += 1;
            continue;
        }
        if !(token.len() > 1 && flag_like(token)) {
            i += 1;
            continue;
        }
        let (flag, inline) = match token.split_once('=') {
            Some((f, v)) => (f, Some(v)),
            None => (token, None),
        };
        let Some(arg) = find(flag) else {
            // git: -<number> is -n <number>.
            if name == "git" && flag.len() > 1 && flag[1..].chars().all(|c| c.is_ascii_digit()) {
                i += 1;
                continue;
            }
            // grep/rg: a number attached to its flag (-A20).
            if (name == "grep" || name == "rg") && !flag.starts_with("--") && flag.len() > 2 {
                if let Some(a @ (Arg::Number | Arg::String)) = find(&flag[..2]) {
                    let value = &flag[2..];
                    if value.chars().all(|c| c.is_ascii_digit()) && valid(value, a) {
                        i += 1;
                        continue;
                    }
                }
            }
            // A bundle of argument-less short flags (-nr).
            if !flag.starts_with("--") && flag.len() > 2 {
                if flag[1..].chars().all(|c| find(&format!("-{c}")) == Some(Arg::None)) {
                    i += 1;
                    continue;
                }
            }
            return false;
        };
        if arg == Arg::None {
            if inline.is_some() {
                return false;
            }
            i += 1;
            continue;
        }
        let value = match inline {
            Some(v) => {
                i += 1;
                v
            }
            None => {
                let Some(next) = tokens.get(i + 1).copied().filter(|n| !(n.len() > 1 && flag_like(n))) else {
                    return false;
                };
                i += 2;
                next
            }
        };
        let reverse_sort = flag == "--sort" && name == "git" && value.len() > 1 && value[1..2].chars().all(|c| c.is_ascii_alphabetic());
        if arg == Arg::String && value.starts_with('-') && !reverse_sort {
            return false;
        }
        if !valid(value, arg) {
            return false;
        }
    }
    true
}

/// `validateFlagArgument`.
fn valid(value: &str, arg: Arg) -> bool {
    match arg {
        Arg::None => false,
        Arg::Number => !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()),
        Arg::String => true,
        Arg::Char => value.chars().count() == 1,
        Arg::Braces => value == "{}",
        Arg::Eof => value == "EOF",
    }
}

/// The per-command checks after the flags: the shapes of an allowlisted
/// command that write after all.
fn writes_anyway(command: &str, args: &[&str], words: &[&str]) -> bool {
    let positional = |a: &&&str| !a.starts_with('-');
    match command {
        // BSD-style options with `e` print every process's environment.
        "ps" => args.iter().any(|a| !a.starts_with('-') && a.chars().all(|c| c.is_ascii_alphabetic()) && a.contains('e')),
        // A positional that isn't a +format sets the clock.
        "date" => {
            let mut i = 0;
            while i < args.len() {
                let a = args[i];
                if a.starts_with("--") && a.contains('=') {
                    i += 1;
                } else if a.starts_with('-') {
                    i += if matches!(a, "-d" | "--date" | "-r" | "--reference" | "--iso-8601" | "--rfc-3339") { 2 } else { 1 };
                } else if !a.starts_with('+') {
                    return true;
                } else {
                    i += 1;
                }
            }
            false
        }
        // A positional sets the hostname.
        "hostname" => !args.iter().all(|a| {
            let long = a.strip_prefix("--").is_some_and(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_alphabetic() || c == '-'));
            let short = a.len() == 2 && a.starts_with('-') && a[1..].chars().all(|c| c.is_ascii_alphabetic());
            long || short
        }),
        // +m writes a mount supplement file.
        "lsof" => args.iter().any(|a| a.starts_with("+m")),
        "tput" => {
            const DANGEROUS: &[&str] = &[
                "init", "reset", "rs1", "rs2", "rs3", "is1", "is2", "is3", "iprog", "if", "rf", "clear", "flash", "mc0",
                "mc4", "mc5", "mc5i", "mc5p", "pfkey", "pfloc", "pfx", "pfxl", "smcup", "rmcup",
            ];
            let mut i = 0;
            let mut after_dash_dash = false;
            while i < args.len() {
                let a = args[i];
                if a == "--" {
                    after_dash_dash = true;
                    i += 1;
                } else if !after_dash_dash && a.starts_with('-') {
                    if a == "-S" || (!a.starts_with("--") && a.len() > 2 && a.contains('S')) {
                        return true;
                    }
                    i += if a == "-T" { 2 } else { 1 };
                } else {
                    if DANGEROUS.contains(&a) {
                        return true;
                    }
                    i += 1;
                }
            }
            false
        }
        // Only `show` or a ref: expire, delete and exists write the reflog.
        "git reflog" => args.iter().find(positional).is_some_and(|a| matches!(*a, "expire" | "delete" | "exists")),
        // One remote name, and -n.
        "git remote show" => {
            let names: Vec<&&str> = args.iter().filter(|a| **a != "-n").collect();
            names.len() != 1 || !names[0].chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        }
        "git remote" => args.iter().any(|a| *a != "-v" && *a != "--verbose"),
        // A name without --list creates the tag or branch.
        "git tag" => names_without_list(args, &["--contains", "--no-contains", "--merged", "--no-merged", "--points-at", "--sort", "--format", "-n"], &[]),
        "git branch" => names_without_list(args, &["--contains", "--no-contains", "--points-at", "--sort"], &["--merged", "--no-merged"]),
        "pyright" => args.iter().any(|a| *a == "--watch" || *a == "-w"),
        "sed" => !sed_only_prints(&words[1..]),
        _ => false,
    }
}

/// `git tag` / `git branch`: a positional that isn't a pattern after
/// `-l`/`--list`, or the argument of a flag that takes an optional one, is a
/// name to create.
fn names_without_list(args: &[&str], with_value: &[&str], optional_value: &[&str]) -> bool {
    let mut i = 0;
    let mut listing = false;
    let mut after_dash_dash = false;
    let mut last_flag = "";
    while i < args.len() {
        let a = args[i];
        if a == "--" && !after_dash_dash {
            after_dash_dash = true;
            last_flag = "";
            i += 1;
            continue;
        }
        if !after_dash_dash && a.starts_with('-') {
            if a == "--list" || a == "-l" || (!a.starts_with("--") && a.len() > 2 && !a.contains('=') && a[1..].contains('l')) {
                listing = true;
            }
            if let Some((flag, _)) = a.split_once('=') {
                last_flag = flag;
                i += 1;
            } else {
                last_flag = a;
                i += if with_value.contains(&a) { 2 } else { 1 };
            }
        } else {
            if !listing && !optional_value.contains(&last_flag) {
                return true;
            }
            i += 1;
        }
    }
    false
}

/// `sedCommandIsAllowedByAllowlist` (read-only mode): `-n` with only print
/// commands (`p`, `3p`, `1,5p`), or one `s/…/…/flags` on stdin with no file.
fn sed_only_prints(args: &[&str]) -> bool {
    let mut flags = Vec::new();
    let mut expressions = Vec::new();
    let mut files = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        if a == "-e" || a == "--expression" {
            match args.get(i + 1) {
                Some(e) => expressions.push(*e),
                None => return false,
            }
            i += 2;
            continue;
        }
        if let Some(e) = a.strip_prefix("--expression=") {
            expressions.push(e);
        } else if a.starts_with('-') && a != "--" {
            flags.push(a);
        } else if a != "--" {
            files.push(a);
        }
        i += 1;
    }
    if expressions.is_empty() && !files.is_empty() {
        expressions.push(files.remove(0));
    }
    let flags_in = |allowed: &[&str]| {
        flags.iter().all(|f| {
            if !f.starts_with("--") && f.len() > 2 {
                f[1..].chars().all(|c| allowed.contains(&format!("-{c}").as_str()))
            } else {
                allowed.contains(f)
            }
        })
    };
    let print = |cmd: &str| {
        let body = cmd.strip_suffix('p');
        body.is_some_and(|b| {
            b.is_empty()
                || b.chars().all(|c| c.is_ascii_digit())
                || b.split_once(',').is_some_and(|(x, y)| {
                    !x.is_empty() && !y.is_empty() && x.chars().all(|c| c.is_ascii_digit()) && y.chars().all(|c| c.is_ascii_digit())
                })
        })
    };
    let quiet = flags.iter().any(|f| *f == "-n" || *f == "--quiet" || *f == "--silent" || (!f.starts_with("--") && f.contains('n')));
    let printing = quiet
        && flags_in(&["-n", "--quiet", "--silent", "-E", "--regexp-extended", "-r", "-z", "--zero-terminated", "--posix"])
        && !expressions.is_empty()
        && expressions.iter().all(|e| e.split(';').all(|c| print(c.trim())));
    let substituting = files.is_empty()
        && flags_in(&["-E", "--regexp-extended", "-r", "--posix"])
        && expressions.len() == 1
        && !expressions[0].contains(';')
        && substitution(expressions[0].trim());
    printing || substituting
}

/// `s/pattern/replacement/flags` with `/` delimiters and flags only from
/// g, p, i, I, m, M and one digit.
fn substitution(expr: &str) -> bool {
    let Some(rest) = expr.strip_prefix("s/") else { return false };
    let bytes = rest.as_bytes();
    let (mut delimiters, mut last, mut i) = (0, 0, 0);
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'/' => {
                delimiters += 1;
                last = i;
                i += 1;
            }
            _ => i += 1,
        }
    }
    if delimiters != 2 {
        return false;
    }
    let flags = &rest[last + 1..];
    flags.chars().all(|c| "gpimIM123456789".contains(c)) && flags.chars().filter(|c| c.is_ascii_digit()).count() <= 1
}

/// The commands checked by shape rather than by a flag table.
fn by_shape(words: &[&str]) -> bool {
    let args = &words[1..];
    let flag = |a: &str| a.starts_with('-');
    match words[0] {
        name if READS.contains(&name) => true,
        name if READS_TWO.iter().any(|(a, b)| *a == name && args.first() == Some(b)) => true,
        "pwd" | "whoami" | "alias" => args.is_empty(),
        // Flags only: a file argument would be written.
        "uniq" => {
            let mut i = 0;
            while i < args.len() {
                let a = args[i];
                if matches!(a, "-f" | "-s" | "-w") && args.get(i + 1).is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) {
                    i += 2;
                } else if flag(a) {
                    i += 1;
                } else {
                    return false;
                }
            }
            true
        }
        "node" => args == ["-v"] || args == ["--version"],
        "python" | "python3" => args == ["--version"],
        "history" => args.is_empty() || (args.len() == 1 && args[0].chars().all(|c| c.is_ascii_digit())),
        "arch" => args.is_empty() || args == ["-h"] || args == ["--help"],
        "ip" => args == ["addr"],
        "ifconfig" => {
            args.is_empty()
                || (args.len() == 1
                    && args[0].starts_with(|c: char| c.is_ascii_alphabetic())
                    && args[0].chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        }
        "cd" => args.len() <= 1,
        // Filters and files; not the flags that read code or files into jq.
        "jq" => {
            let dangerous = regex::Regex::new(r"(?:^|\s)(?:-f\b|--from-file|--rawfile|--slurpfile|--run-tests|-L\b|--library-path)|\benv\b|\$ENV\b")
                .expect("valid pattern");
            let first_positional = args.iter().position(|a| !flag(a));
            !dangerous.is_match(&args.join(" "))
                && first_positional.is_some_and(|p| args[p..].iter().all(|a| !flag(a)))
        }
        // Not the actions that delete, run or write.
        "find" => !args.iter().any(|a| {
            matches!(*a, "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir" | "-fprint" | "-fprint0" | "-fls" | "-fprintf")
        }),
        _ => false,
    }
}
