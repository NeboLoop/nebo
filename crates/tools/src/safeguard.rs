use std::path::Path;

use crate::nebo_files::NeboFiles;

/// Rule keys whose calls read or change files on this machine: the file
/// safeguard and the folder fence apply to them.
const FILE_KEYS: &[&str] = &[
    "read_file",
    "write_file",
    "edit_file",
    "share_file",
    "convert_file",
    "checkpoint_files",
    "list_checkpoints",
    "restore_checkpoint",
    "write_plan",
    "check_plan",
    "exit_plan_mode",
];

/// Validate a tool call against hard safety limits, keyed on the call's
/// rule key (`DynTool::rule_key`) so every shape of a job meets the same
/// limit. `ctx` is the run the call belongs to: Nebo's own files are fenced
/// but for the parts that run works in. Returns None if safe, or
/// Some(error_message) if blocked.
/// This check is unconditional — cannot be bypassed by any setting.
pub fn check_safeguard(rule_key: &str, input: &serde_json::Value, ctx: &crate::origin::ToolContext) -> Option<String> {
    let fence = NeboFiles::of(&ctx.session_id);
    let run = Run { fence: fence.as_ref(), cwd: ctx.cwd.as_deref().map(Path::new) };
    match rule_key {
        "run_command" => check_shell_safeguard(input, &run),
        // Text typed into a running command may be typed into a shell (a
        // terminal running `bash`): it meets the same limits as a command.
        "send_input" => input
            .get("text")
            .and_then(|v| v.as_str())
            .and_then(|text| scan_command_text(text.trim(), &run)),
        key if FILE_KEYS.contains(&key) || key == "edit_notebook" => check_file_safeguard(key, input, &run),
        _ => None,
    }
}

/// What the safeguard reads about the run a call belongs to.
struct Run<'a> {
    /// Nebo's own files, fenced for this run.
    fence: Option<&'a NeboFiles>,
    /// The folder the run's relative paths are taken from.
    cwd: Option<&'a Path>,
}

impl Run<'_> {
    /// The refusal for a call that reaches `path`, one of Nebo's own files.
    fn nebo_file(&self, path: &str) -> Option<String> {
        let fence = self.fence?;
        fence.closes(Path::new(path), self.cwd).then(|| nebo_file_refusal(path, fence))
    }
}

/// The refusal for reaching one of Nebo's own files, in the one wording
/// every door uses.
pub fn nebo_file_refusal(path: &str, fence: &NeboFiles) -> String {
    format!(
        "BLOCKED: {path:?} is one of Nebo's own files (its settings, logs, database and other internals). \
         An employee never reads or changes them, and they are not where your work is: what Nebo knows \
         reaches you through your tools. Your working files are under {}. \
         This is a hard safety limit that cannot be overridden",
        fence.workspace().display()
    )
}

/// Check if a tool call respects the allowed_paths restriction.
/// If allowed_paths is empty, all paths are allowed (unrestricted).
/// File reads are always allowed. Only writes/edits/deletes are restricted.
/// Shell commands are restricted to running within allowed directories.
pub fn check_path_scope(
    rule_key: &str,
    input: &serde_json::Value,
    allowed_paths: &[String],
) -> Option<String> {
    if allowed_paths.is_empty() {
        return None;
    }

    match rule_key {
        "run_command" => check_shell_path_scope(input, allowed_paths),
        // A notebook edit writes its .ipynb: fenced like a file write.
        "edit_notebook" => {
            let path = input.get("notebook_path").and_then(|v| v.as_str()).unwrap_or("");
            if path.is_empty() {
                return None;
            }
            outside_allowed("edit", &[crate::file_tool::expand_path(path)], allowed_paths)
        }
        key if FILE_KEYS.contains(&key) => check_file_path_scope(key, input, allowed_paths),
        _ => None,
    }
}

/// The first of `paths` outside `allowed_paths`, as a BLOCKED message, or
/// None when every path is inside (or there is no fence). Used by the file
/// tool for checkpoint `paths[]` and for the manifest paths of a restore,
/// which the input-only scope check below cannot see.
pub fn outside_allowed(verb: &str, paths: &[String], allowed_paths: &[String]) -> Option<String> {
    if allowed_paths.is_empty() {
        return None;
    }
    for path in paths {
        let abs = std::path::absolute(Path::new(path))
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| path.clone());
        if !is_within_allowed(&abs, allowed_paths) {
            return Some(format!(
                "BLOCKED: cannot {} {:?} — this agent is restricted to: {}. \
                 Ask the owner to update the allowed directories in the Configure tab.",
                verb,
                path,
                allowed_paths.join(", ")
            ));
        }
    }
    None
}

fn check_file_path_scope(
    rule_key: &str,
    input: &serde_json::Value,
    allowed_paths: &[String],
) -> Option<String> {
    let action = verb(rule_key);
    let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("");

    // checkpoint/restore name their files in `paths[]` (restore may also
    // carry none and take them from the manifest — the file tool checks that).
    if matches!(rule_key, "checkpoint_files" | "restore_checkpoint") {
        let paths: Vec<String> = input
            .get("paths")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|p| p.as_str().map(String::from)).collect())
            .unwrap_or_default();
        return outside_allowed(action, &paths, allowed_paths);
    }

    // Only restrict the calls that change a file — reads are always allowed
    if rule_key != "write_file" && rule_key != "edit_file" {
        return None;
    }

    if path.is_empty() {
        return None;
    }

    let abs_path = match std::path::absolute(Path::new(path)) {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(_) => path.to_string(),
    };

    if is_within_allowed(&abs_path, allowed_paths) {
        return None;
    }

    Some(format!(
        "BLOCKED: cannot {} {:?} — this agent is restricted to: {}. \
         Ask the owner to update the allowed directories in the Configure tab.",
        action,
        path,
        allowed_paths.join(", ")
    ))
}

fn check_shell_path_scope(input: &serde_json::Value, allowed_paths: &[String]) -> Option<String> {
    let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
    let cwd = input.get("cwd").and_then(|v| v.as_str()).unwrap_or("");

    if command.is_empty() {
        return None;
    }

    // If cwd is specified, it must be within allowed paths
    if !cwd.is_empty() {
        let abs_cwd = match std::path::absolute(Path::new(cwd)) {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(_) => cwd.to_string(),
        };
        if !is_within_allowed(&abs_cwd, allowed_paths) {
            return Some(format!(
                "BLOCKED: cannot execute shell command in {:?} — this agent is restricted to: {}. \
                 Run it inside one of those directories or ask the owner to widen them in the Configure tab.",
                cwd,
                allowed_paths.join(", ")
            ));
        }
    }

    None
}

fn is_within_allowed(abs_path: &str, allowed_paths: &[String]) -> bool {
    for allowed in allowed_paths {
        let allowed_abs = match std::path::absolute(Path::new(allowed)) {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(_) => allowed.clone(),
        };
        if abs_path == allowed_abs || abs_path.starts_with(&format!("{}/", allowed_abs)) {
            return true;
        }
    }
    false
}

/// The verb a refusal names for a file call's rule key.
fn verb(rule_key: &str) -> &'static str {
    match rule_key {
        "read_file" => "read",
        "write_file" => "write",
        "edit_file" => "edit",
        "checkpoint_files" => "checkpoint",
        "restore_checkpoint" => "restore",
        _ => "use",
    }
}

fn check_file_safeguard(rule_key: &str, input: &serde_json::Value, run: &Run<'_>) -> Option<String> {
    let action = verb(rule_key);
    let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("");

    // Nebo's own files are off-limits for EVERY action, reads included: the
    // settings file holds the server's secret, the database holds sessions
    // and auth material, and out-of-band access bypasses every tool gate
    // (2026-08-01: a cloud agent "fixed" its schedules with raw sqlite3
    // INSERTs into its own DB; 2026-09-26: an employee read the settings
    // file's secret into its context). Every path the call names is checked.
    let notebook = input.get("notebook_path").and_then(|v| v.as_str());
    let listed = input.get("paths").and_then(|v| v.as_array()).into_iter().flatten().filter_map(|p| p.as_str());
    for named in std::iter::once(path).chain(notebook).chain(listed).filter(|p| !p.is_empty()) {
        if let Some(refusal) = run.nebo_file(named) {
            return Some(refusal);
        }
    }

    // Only guard the calls that change a file.
    if rule_key != "write_file" && rule_key != "edit_file" {
        return None;
    }

    if path.is_empty() {
        return None;
    }

    let abs_path = std::path::absolute(Path::new(path)).ok()?;
    let abs_str = abs_path.to_string_lossy();

    if let Some(reason) = is_protected_path(&abs_str) {
        return Some(format!(
            "BLOCKED: cannot {} {:?} — {}. \
             This is a hard safety limit that cannot be overridden. \
             Ask the owner to make this change themselves.",
            action, path, reason
        ));
    }

    // Also check resolved symlinks
    if let Ok(resolved) = std::fs::canonicalize(&abs_path) {
        let resolved_str = resolved.to_string_lossy();
        if resolved_str != abs_str {
            if let Some(reason) = is_protected_path(&resolved_str) {
                return Some(format!(
                    "BLOCKED: cannot {} {:?} — {}. \
                     This is a hard safety limit that cannot be overridden. \
                     Ask the owner to make this change themselves.",
                    action, path, reason
                ));
            }
        }
    }

    None
}

fn check_shell_safeguard(input: &serde_json::Value, run: &Run<'_>) -> Option<String> {
    let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");

    // The caller dispatched on the rule key (`run_command`): this call runs
    // `command`, whatever shape carried it.
    if command.is_empty() {
        return None;
    }

    let cmd = command.trim();

    // Scan the command string itself.
    if let Some(reason) = scan_command_text(cmd, run) {
        return Some(reason);
    }

    // Defense-in-depth: if the command runs a LOCAL shell script
    // (`bash X.sh`, `./X.sh`, `source X.sh`, …), scan the script's contents with
    // the same checks — the command string alone (`bash X.sh`) hides whatever the
    // script does. Best-effort: a static scan catches obvious destructive content
    // (rm -rf /, sudo, dd-to-device); it cannot beat obfuscation/indirection, so
    // it's a speed bump, not a guarantee. The command still requires approval
    // anyway (interpreters are never allowlisted).
    if let Some(reason) = scan_referenced_script(cmd, run) {
        return Some(reason);
    }

    None
}

/// Run the unconditional dangerous-pattern checks over a piece of command text
/// (the command itself, or a script's contents). Returns a BLOCK reason if any
/// hard-safety pattern is present.
fn scan_command_text(text: &str, run: &Run<'_>) -> Option<String> {
    // Shell is the easy way around the file tools' fence (sqlite3, cat,
    // grep, …): a command that names one of Nebo's own files is refused
    // before it runs. The command itself runs confined as well (`confine`),
    // so a path spelled another way still can't be read.
    if let Some(fence) = run.fence
        && let Some(path) = fence.named_in(text, run.cwd)
    {
        return Some(nebo_file_refusal(&path, fence));
    }
    let lower = text.to_lowercase();
    if has_sudo(&lower) {
        return Some(
            "BLOCKED: sudo is not permitted. \
             Nebo must never run commands with elevated privileges. \
             This is a hard safety limit that cannot be overridden. \
             Ask the owner to make this change themselves."
                .to_string(),
        );
    }
    if has_su(&lower) {
        return Some(
            "BLOCKED: su is not permitted. \
             Nebo must never run commands as another user. \
             This is a hard safety limit that cannot be overridden"
                .to_string(),
        );
    }
    if let Some(reason) = check_destructive_command(text, &lower) {
        return Some(format!(
            "BLOCKED: {}. \
             This is a hard safety limit that cannot be overridden. \
             Ask the owner to make this change themselves.",
            reason
        ));
    }
    None
}

/// If `cmd` invokes a local shell script, read it and scan its contents. Returns
/// a BLOCK reason naming the script when dangerous content is found.
fn scan_referenced_script(cmd: &str, run: &Run<'_>) -> Option<String> {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    let path = referenced_script_path(&parts)?;
    let content = std::fs::read_to_string(&path).ok()?;
    // Skip pathologically large files (not worth the scan; not a typical script).
    if content.len() > 1_000_000 {
        return None;
    }
    scan_command_text(&content, run)
        .map(|reason| format!("{} (found inside the script {})", reason, path))
}

/// The local shell-script path a command would execute, if any:
/// `./x.sh` / `/abs/x.sh` / `../x.sh`, or `bash|sh|zsh|dash|ksh|source|. <script>`.
fn referenced_script_path(parts: &[&str]) -> Option<String> {
    let first = *parts.first()?;
    if first.starts_with("./") || first.starts_with('/') || first.starts_with("../") {
        return Some(first.to_string());
    }
    const SHELL_INTERPS: &[&str] = &["bash", "sh", "zsh", "dash", "ksh", "source", "."];
    if SHELL_INTERPS.contains(&first) {
        // First non-flag argument is the script.
        return parts
            .iter()
            .skip(1)
            .find(|a| !a.starts_with('-'))
            .map(|s| s.to_string());
    }
    None
}

fn has_sudo(cmd_lower: &str) -> bool {
    if cmd_lower.starts_with("sudo ") || cmd_lower.starts_with("sudo\t") {
        return true;
    }
    let separators = [
        " | sudo ",
        "| sudo ",
        " && sudo ",
        "&& sudo ",
        " ; sudo ",
        "; sudo ",
        " || sudo ",
        "|| sudo ",
    ];
    for sep in &separators {
        if cmd_lower.contains(sep) {
            return true;
        }
    }
    if cmd_lower.contains("$(sudo ") || cmd_lower.contains("`sudo ") {
        return true;
    }
    false
}

fn has_su(cmd_lower: &str) -> bool {
    if cmd_lower.starts_with("su ") || cmd_lower.starts_with("su\t") || cmd_lower == "su" {
        return true;
    }
    let separators = [" | su ", " && su ", " ; su ", " || su "];
    for sep in &separators {
        if cmd_lower.contains(sep) {
            return true;
        }
    }
    false
}

fn check_destructive_command(_cmd: &str, cmd_lower: &str) -> Option<String> {
    // Block rm -rf / or rm -rf /*
    if is_root_wipe(cmd_lower) {
        return Some(
            "cannot delete root filesystem — this would destroy the operating system".to_string(),
        );
    }

    // Block dd to block devices
    if cmd_lower.contains("dd ")
        && (cmd_lower.contains("of=/dev/") || cmd_lower.contains("of= /dev/"))
    {
        return Some(
            "cannot write to block devices with dd — this could destroy disk data".to_string(),
        );
    }

    // Block disk formatting/partitioning commands
    let format_cmds = [
        ("mkfs", "cannot format filesystems"),
        ("fdisk", "cannot modify disk partition tables"),
        ("gdisk", "cannot modify GPT partition tables"),
        ("parted", "cannot modify disk partitions"),
        ("wipefs", "cannot wipe filesystem signatures"),
    ];
    for (pattern, reason) in &format_cmds {
        if cmd_lower.starts_with(pattern) || cmd_lower.contains(&format!(" {}", pattern)) {
            return Some(reason.to_string());
        }
    }

    // Block fork bombs
    if cmd_lower.contains(":(){ :|:& };:") {
        return Some("fork bomb detected — this would crash the system".to_string());
    }

    // Block writing to /dev/ (except /dev/null, /dev/stdout, /dev/stderr)
    if cmd_lower.contains("> /dev/") || cmd_lower.contains(">/dev/") {
        let safe_devs = ["/dev/null", "/dev/stdout", "/dev/stderr"];
        let is_safe = safe_devs.iter().any(|d| {
            cmd_lower.contains(&format!("> {}", d)) || cmd_lower.contains(&format!(">{}", d))
        });
        if !is_safe {
            return Some(
                "cannot write to device files — this could damage hardware or corrupt data"
                    .to_string(),
            );
        }
    }

    None
}

fn is_root_wipe(cmd_lower: &str) -> bool {
    let wipe_patterns = [
        "rm -rf /",
        "rm -fr /",
        "rm -rf /*",
        "rm -fr /*",
        "rm -rf --no-preserve-root /",
        "rm -rf --no-preserve-root /*",
    ];
    for p in &wipe_patterns {
        if let Some(idx) = cmd_lower.find(p) {
            let after = &cmd_lower[idx + p.len()..];
            let last_char = p.as_bytes()[p.len() - 1];
            if last_char == b'/'
                && (after.is_empty()
                    || after.starts_with(' ')
                    || after.starts_with(';')
                    || after.starts_with('&'))
            {
                return true;
            }
            if last_char == b'*' {
                return true;
            }
        }
    }
    false
}

/// Check if an absolute path is a protected system directory.
fn is_protected_path(abs_path: &str) -> Option<String> {
    #[cfg(target_os = "macos")]
    return is_protected_path_darwin(abs_path);

    #[cfg(target_os = "linux")]
    return is_protected_path_linux(abs_path);

    #[cfg(target_os = "windows")]
    return is_protected_path_windows(abs_path);

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    return is_protected_path_linux(abs_path);
}

#[cfg(target_os = "macos")]
fn is_protected_path_darwin(abs_path: &str) -> Option<String> {
    if abs_path == "/" {
        return Some("this is the root filesystem".to_string());
    }

    let protected = [
        ("/System", "macOS system files (SIP-protected)"),
        ("/usr/bin", "system binaries"),
        ("/usr/sbin", "system admin binaries"),
        ("/usr/lib", "system libraries"),
        ("/bin", "core system binaries"),
        ("/sbin", "core system admin binaries"),
        ("/etc", "system configuration"),
    ];

    for (prefix, reason) in &protected {
        if abs_path == *prefix || abs_path.starts_with(&format!("{}/", prefix)) {
            return Some(reason.to_string());
        }
    }

    is_protected_user_path(abs_path)
}

#[cfg(any(
    target_os = "linux",
    not(any(target_os = "macos", target_os = "windows"))
))]
fn is_protected_path_linux(abs_path: &str) -> Option<String> {
    if abs_path == "/" {
        return Some("this is the root filesystem".to_string());
    }

    let protected = [
        ("/bin", "core system binaries"),
        ("/sbin", "core system admin binaries"),
        ("/usr/bin", "system binaries"),
        ("/usr/sbin", "system admin binaries"),
        ("/usr/lib", "system libraries"),
        ("/boot", "boot loader and kernel"),
        ("/etc", "system configuration"),
        ("/proc", "kernel process filesystem"),
        ("/sys", "kernel sysfs"),
        ("/dev", "device files"),
    ];

    for (prefix, reason) in &protected {
        if abs_path == *prefix || abs_path.starts_with(&format!("{}/", prefix)) {
            return Some(reason.to_string());
        }
    }

    is_protected_user_path(abs_path)
}

#[cfg(target_os = "windows")]
fn is_protected_path_windows(abs_path: &str) -> Option<String> {
    let abs_lower = abs_path.to_lowercase();

    let protected = [
        ("c:\\windows", "Windows system directory"),
        ("c:\\program files", "installed program files"),
        (
            "c:\\program files (x86)",
            "installed program files (32-bit)",
        ),
    ];

    for (prefix, reason) in &protected {
        if abs_lower == *prefix || abs_lower.starts_with(&format!("{}\\", prefix)) {
            return Some(reason.to_string());
        }
    }

    is_protected_user_path(abs_path)
}

fn is_protected_user_path(abs_path: &str) -> Option<String> {
    use std::path::Path;

    let home = dirs::home_dir()?;
    let abs = Path::new(abs_path);

    // Protect Nebo's own data directory (database, config, etc.)
    // Nebo must never delete or overwrite its own database — this is catastrophic self-harm.
    for (path, reason) in nebo_data_dirs() {
        let protected = Path::new(&path);
        if abs == protected || abs.starts_with(protected) {
            return Some(reason);
        }
    }

    let sensitive = [
        (".ssh", "SSH keys and configuration"),
        (".gnupg", "GPG keys and configuration"),
        (".aws/credentials", "AWS credentials"),
        (".aws/config", "AWS configuration"),
        (".kube/config", "Kubernetes credentials"),
        (".docker/config.json", "Docker registry credentials"),
    ];

    for (rel, reason) in &sensitive {
        let protected = home.join(rel);
        if abs == protected.as_path() || abs.starts_with(&protected) {
            return Some(reason.to_string());
        }
    }

    None
}

/// Returns the Nebo data directory paths that must be protected from writes/deletes.
///
/// Derived from `config::data_dir()` so this stays consistent with the actual
/// data location on every platform (and honors `NEBO_DATA_DIR`).
fn nebo_data_dirs() -> Vec<(String, String)> {
    let data_reason =
        "Nebo database directory — deleting this would destroy all agent data".to_string();
    let appdata_reason = "Nebo appdata directory — deleting this would destroy all artifact data (plugin databases, skill files, etc.)".to_string();

    let Ok(base) = config::data_dir() else {
        return vec![];
    };

    vec![
        (base.join("data").to_string_lossy().into_owned(), data_reason),
        (
            base.join("appdata").to_string_lossy().into_owned(),
            appdata_reason,
        ),
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn path_scope_covers_paths_array_and_checkpoint_ids() {
        use super::*;
        let allowed = vec!["/proj".to_string()];
        // checkpoint names its files in `paths[]`, not `path`.
        let inside = serde_json::json!({"resource": "file", "action": "checkpoint", "paths": ["/proj/a.rs", "/proj/src/b.rs"]});
        assert!(check_path_scope("checkpoint_files", &inside, &allowed).is_none());
        let outside = serde_json::json!({"resource": "file", "action": "checkpoint", "paths": ["/proj/a.rs", "/etc/hosts"]});
        let msg = check_path_scope("checkpoint_files", &outside, &allowed).expect("blocked");
        assert!(msg.contains("BLOCKED") && msg.contains("/etc/hosts"), "{msg}");
        // restore with an explicit subset is fenced the same way; the
        // manifest-path case (no `paths`) is the file tool's job.
        let restore = serde_json::json!({"resource": "file", "action": "restore", "checkpoint": "cp-1", "paths": ["/tmp/x"]});
        assert!(check_path_scope("restore_checkpoint", &restore, &allowed).is_some());
        // The manifest helper: first offender named, no fence = nothing blocked.
        assert!(outside_allowed("restore", &["/tmp/x".into()], &[]).is_none());
        let msg = outside_allowed("restore", &["/proj/ok".into(), "/tmp/x".into()], &allowed).expect("blocked");
        assert!(msg.contains("/tmp/x"), "{msg}");
    }

    use super::*;

    #[test]
    fn test_sudo_detection() {
        assert!(has_sudo("sudo rm -rf /tmp"));
        assert!(has_sudo("ls | sudo rm"));
        assert!(has_sudo("echo test && sudo cat /etc/shadow"));
        assert!(!has_sudo("ls -la"));
        assert!(!has_sudo("sudoku"));
    }

    #[test]
    fn test_root_wipe_detection() {
        assert!(is_root_wipe("rm -rf /"));
        assert!(is_root_wipe("rm -rf /*"));
        assert!(!is_root_wipe("rm -rf /tmp/test"));
    }

    /// A run with no fence: the limits that hold whatever the folder.
    fn bare() -> Run<'static> {
        Run { fence: None, cwd: None }
    }

    /// Nebo's own files are closed to every file action and every command
    /// that names them, reads included; the workspace stays open.
    #[test]
    fn nebo_own_files_are_closed_to_every_action() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("nebo-home");
        std::fs::create_dir_all(root.join("files")).unwrap();
        let fence = NeboFiles::at(&root, &root.join("sessions/s1"));
        let run = Run { fence: Some(&fence), cwd: None };
        let at = |rel: &str| root.join(rel).to_string_lossy().into_owned();

        for key in ["read_file", "write_file", "edit_file", "share_file", "convert_file"] {
            for closed in ["data/nebo.db", "settings.json", "logs/nebo.log"] {
                let r = check_file_safeguard(key, &serde_json::json!({ "path": at(closed) }), &run);
                assert!(r.as_deref().is_some_and(|m| m.contains("Nebo's own files")), "{key} {closed}: {r:?}");
            }
            assert!(check_file_safeguard(key, &serde_json::json!({ "path": at("files/draft.md") }), &run).is_none(), "{key}");
        }
        let r = check_file_safeguard("edit_notebook", &serde_json::json!({ "notebook_path": at("logs/x.ipynb") }), &run);
        assert!(r.is_some(), "a notebook path");
        let r = check_file_safeguard("checkpoint_files", &serde_json::json!({ "paths": [at("files/a"), at("settings.json")] }), &run);
        assert!(r.is_some(), "any of a checkpoint's paths");

        // The sqlite3 hole, and the settings file read through the shell.
        for cmd in [
            format!("sqlite3 {} \"INSERT INTO workflows VALUES ('x')\"", at("data/nebo.db")),
            format!("cat {} | head -50", at("settings.json")),
            format!("grep -ri gmail {}", at("logs/nebo.log")),
        ] {
            let r = check_shell_safeguard(&serde_json::json!({ "command": cmd }), &run);
            assert!(r.is_some(), "{cmd}");
        }
        let r = check_shell_safeguard(&serde_json::json!({ "command": format!("cat {}", at("files/draft.md")) }), &run);
        assert!(r.is_none(), "{r:?}");
    }

    #[test]
    fn test_shell_safeguard() {
        let input = serde_json::json!({
            "resource": "bash",
            "action": "exec",
            "command": "sudo rm -rf /tmp"
        });
        assert!(check_shell_safeguard(&input, &bare()).is_some());

        let safe = serde_json::json!({
            "resource": "bash",
            "action": "exec",
            "command": "ls -la"
        });
        assert!(check_shell_safeguard(&safe, &bare()).is_none());
    }

    #[test]
    fn safeguards_follow_the_rule_key() {
        // The registry passes the call's rule key, so every tool shape that
        // runs a command or writes a file meets the same guard.
        let run = crate::origin::ToolContext::default();
        let check_safeguard = |key: &str, input: &serde_json::Value| check_safeguard(key, input, &run);
        let shell_sudo = serde_json::json!({
            "resource": "shell", "action": "exec", "command": "sudo rm -rf /tmp"
        });
        assert!(check_safeguard("run_command", &shell_sudo).is_some());

        let wipe = serde_json::json!({ "command": "rm -rf /" });
        assert!(check_safeguard("run_command", &wipe).is_some());

        // File guard fires for protected system paths.
        let file_write = serde_json::json!({
            "resource": "file", "action": "write", "path": "/etc/passwd"
        });
        assert!(check_safeguard("write_file", &file_write).is_some());
        assert!(check_safeguard("search_web", &file_write).is_none(), "other keys have no file guard");

        // Safe commands pass.
        let safe = serde_json::json!({
            "resource": "shell", "action": "exec", "command": "ls -la"
        });
        assert!(check_safeguard("run_command", &safe).is_none());
        // Typing into a running command meets the command limits: a
        // terminal running a shell is a shell.
        let typed = |text: &str| serde_json::json!({ "task_id": "bg-1", "text": text });
        assert!(check_safeguard("send_input", &typed("sudo rm -rf /tmp\n")).is_some());
        assert!(check_safeguard("send_input", &typed("rm -rf /\n")).is_some());
        assert!(check_safeguard("send_input", &typed("yes\n")).is_none());
        assert!(check_safeguard("send_input", &serde_json::json!({ "task_id": "bg-1", "keys": ["Enter"] })).is_none());
    }

    #[test]
    fn shell_path_scope_blocks_an_outside_cwd() {
        let allowed = vec!["/Users/me/ws".to_string()];
        let input = serde_json::json!({
            "action": "exec", "command": "ls", "cwd": "/private/etc"
        });
        assert!(check_path_scope("run_command", &input, &allowed).is_some());
    }

    #[test]
    fn file_path_scope_fences_writes_not_reads() {
        let allowed = vec!["/Users/me/workspace".to_string()];

        // os file write OUTSIDE the allowed dir is blocked.
        let outside = serde_json::json!({
            "resource": "file", "action": "write", "path": "/etc/passwd"
        });
        assert!(check_path_scope("write_file", &outside, &allowed).is_some());

        // os file write INSIDE the allowed dir is permitted.
        let inside = serde_json::json!({
            "resource": "file", "action": "write", "path": "/Users/me/workspace/report.md"
        });
        assert!(check_path_scope("write_file", &inside, &allowed).is_none());

        // Reads are never path-scoped.
        let read = serde_json::json!({
            "resource": "file", "action": "read", "path": "/etc/hosts"
        });
        assert!(check_path_scope("read_file", &read, &allowed).is_none());

        // Empty allowed_paths = no scoping (must not block).
        assert!(check_path_scope("write_file", &outside, &[]).is_none());
    }
}
