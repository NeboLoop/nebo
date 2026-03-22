use std::path::Path;

/// Validate a tool call against hard safety limits.
/// Returns None if safe, or Some(error_message) if blocked.
/// This check is unconditional — cannot be bypassed by any setting.
pub fn check_safeguard(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    match tool_name {
        "system" | "file" => check_file_safeguard(input),
        "shell" => check_shell_safeguard(input),
        _ => None,
    }
}

/// Check if a tool call respects the allowed_paths restriction.
/// If allowed_paths is empty, all paths are allowed (unrestricted).
/// File reads are always allowed. Only writes/edits/deletes are restricted.
/// Shell commands are restricted to running within allowed directories.
pub fn check_path_scope(tool_name: &str, input: &serde_json::Value, allowed_paths: &[String]) -> Option<String> {
    if allowed_paths.is_empty() {
        return None;
    }

    match tool_name {
        "system" | "file" => check_file_path_scope(input, allowed_paths),
        "shell" => check_shell_path_scope(input, allowed_paths),
        _ => None,
    }
}

fn check_file_path_scope(input: &serde_json::Value, allowed_paths: &[String]) -> Option<String> {
    let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
    let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("");

    // Only restrict destructive actions — reads are always allowed
    if action != "write" && action != "edit" && action != "delete" && action != "move" && action != "copy" {
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
        action, path,
        allowed_paths.join(", ")
    ))
}

fn check_shell_path_scope(input: &serde_json::Value, allowed_paths: &[String]) -> Option<String> {
    let resource = input.get("resource").and_then(|v| v.as_str()).unwrap_or("");
    let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
    let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
    let cwd = input.get("cwd").and_then(|v| v.as_str()).unwrap_or("");

    if !resource.is_empty() && resource != "bash" {
        return None;
    }
    if action != "exec" || command.is_empty() {
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
                "BLOCKED: cannot execute shell command in {:?} — this agent is restricted to: {}",
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

fn check_file_safeguard(input: &serde_json::Value) -> Option<String> {
    let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
    let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("");

    // Only guard destructive actions
    if action != "write" && action != "edit" {
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
             If you need to modify system files, do it manually in a terminal",
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
                     If you need to modify system files, do it manually in a terminal",
                    action, path, reason
                ));
            }
        }
    }

    None
}

fn check_shell_safeguard(input: &serde_json::Value) -> Option<String> {
    let resource = input.get("resource").and_then(|v| v.as_str()).unwrap_or("");
    let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
    let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");

    // Only guard command execution
    if !resource.is_empty() && resource != "bash" {
        return None;
    }
    if action != "exec" {
        return None;
    }
    if command.is_empty() {
        return None;
    }

    let cmd = command.trim();
    let cmd_lower = cmd.to_lowercase();

    // Block sudo
    if has_sudo(&cmd_lower) {
        return Some(
            "BLOCKED: sudo is not permitted. \
             Nebo must never run commands with elevated privileges. \
             This is a hard safety limit that cannot be overridden. \
             If you need root access, run the command manually in a terminal"
                .to_string(),
        );
    }

    // Block su
    if has_su(&cmd_lower) {
        return Some(
            "BLOCKED: su is not permitted. \
             Nebo must never run commands as another user. \
             This is a hard safety limit that cannot be overridden"
                .to_string(),
        );
    }

    // Block destructive operations targeting root or system paths
    if let Some(reason) = check_destructive_command(cmd, &cmd_lower) {
        return Some(format!(
            "BLOCKED: {}. \
             This is a hard safety limit that cannot be overridden. \
             If you need to perform this operation, do it manually in a terminal",
            reason
        ));
    }

    None
}

fn has_sudo(cmd_lower: &str) -> bool {
    if cmd_lower.starts_with("sudo ") || cmd_lower.starts_with("sudo\t") {
        return true;
    }
    let separators = [
        " | sudo ", "| sudo ", " && sudo ", "&& sudo ",
        " ; sudo ", "; sudo ", " || sudo ", "|| sudo ",
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
        return Some("cannot delete root filesystem — this would destroy the operating system".to_string());
    }

    // Block dd to block devices
    if cmd_lower.contains("dd ") && (cmd_lower.contains("of=/dev/") || cmd_lower.contains("of= /dev/")) {
        return Some("cannot write to block devices with dd — this could destroy disk data".to_string());
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
            return Some("cannot write to device files — this could damage hardware or corrupt data".to_string());
        }
    }

    None
}

fn is_root_wipe(cmd_lower: &str) -> bool {
    let wipe_patterns = [
        "rm -rf /", "rm -fr /", "rm -rf /*", "rm -fr /*",
        "rm -rf --no-preserve-root /", "rm -rf --no-preserve-root /*",
    ];
    for p in &wipe_patterns {
        if let Some(idx) = cmd_lower.find(p) {
            let after = &cmd_lower[idx + p.len()..];
            let last_char = p.as_bytes()[p.len() - 1];
            if last_char == b'/' && (after.is_empty() || after.starts_with(' ') || after.starts_with(';') || after.starts_with('&')) {
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

#[cfg(any(target_os = "linux", not(any(target_os = "macos", target_os = "windows"))))]
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
        ("c:\\program files (x86)", "installed program files (32-bit)"),
    ];

    for (prefix, reason) in &protected {
        if abs_lower == *prefix || abs_lower.starts_with(&format!("{}\\", prefix)) {
            return Some(reason.to_string());
        }
    }

    None
}

fn is_protected_user_path(abs_path: &str) -> Option<String> {
    let home = dirs::home_dir()?;
    let home_str = home.to_string_lossy();

    // Protect Nebo's own data directory (database, config, etc.)
    // Nebo must never delete or overwrite its own database — this is catastrophic self-harm.
    for (path, reason) in nebo_data_dirs(&home_str) {
        if abs_path == path || abs_path.starts_with(&format!("{}/", path)) {
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
        let protected = format!("{}/{}", home_str, rel);
        if abs_path == protected || abs_path.starts_with(&format!("{}/", protected)) {
            return Some(reason.to_string());
        }
    }

    None
}

/// Returns the Nebo data directory paths that must be protected from writes/deletes.
fn nebo_data_dirs(home: &str) -> Vec<(String, String)> {
    let reason = "Nebo database directory — deleting this would destroy all agent data".to_string();

    // Check NEBO_DATA_DIR override first
    if let Ok(env_dir) = std::env::var("NEBO_DATA_DIR") {
        return vec![(format!("{}/data", env_dir), reason)];
    }

    // Platform-standard data directories
    #[cfg(target_os = "macos")]
    {
        vec![(
            format!("{}/Library/Application Support/Nebo/data", home),
            reason,
        )]
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            vec![(format!("{}\\Nebo\\data", appdata), reason)]
        } else {
            vec![]
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        vec![(format!("{}/.config/nebo/data", home), reason)]
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn test_nebo_data_dir_protected() {
        let home = dirs::home_dir().unwrap();
        let home_str = home.to_string_lossy();

        #[cfg(target_os = "macos")]
        let nebo_data = format!(
            "{}/Library/Application Support/Nebo/data/nebo.db",
            home_str
        );
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let nebo_data = format!("{}/.config/nebo/data/nebo.db", home_str);
        #[cfg(target_os = "windows")]
        let nebo_data = {
            let appdata = std::env::var("APPDATA").unwrap_or_default();
            format!("{}\\Nebo\\data\\nebo.db", appdata)
        };

        let input = serde_json::json!({
            "action": "write",
            "path": nebo_data,
        });
        let result = check_file_safeguard(&input);
        assert!(result.is_some(), "should block writes to Nebo data directory");
        assert!(
            result.unwrap().contains("Nebo database directory"),
            "should mention Nebo database"
        );
    }

    #[test]
    fn test_shell_safeguard() {
        let input = serde_json::json!({
            "resource": "bash",
            "action": "exec",
            "command": "sudo rm -rf /tmp"
        });
        assert!(check_shell_safeguard(&input).is_some());

        let safe = serde_json::json!({
            "resource": "bash",
            "action": "exec",
            "command": "ls -la"
        });
        assert!(check_shell_safeguard(&safe).is_none());
    }
}
