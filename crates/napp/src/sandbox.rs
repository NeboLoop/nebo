use std::path::Path;

use crate::NappError;

/// Environment variables that are always blocked (reference for allowlist policy).
const _BLOCKED_ENV_VARS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GOOGLE_API_KEY",
    "JWT_SECRET",
    "DATABASE_URL",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "GITHUB_TOKEN",
    "STRIPE_SECRET_KEY",
];

/// System environment variables that are allowed through.
const ALLOWED_SYSTEM_VARS: &[&str] = &[
    "PATH", "HOME", "TMPDIR", "LANG", "LC_ALL", "TZ",
];

/// Build a sanitized environment for an app process.
pub fn sanitize_env(
    app_id: &str,
    app_name: &str,
    app_version: &str,
    app_dir: &str,
    sock_path: &str,
    data_dir: &str,
) -> Vec<(String, String)> {
    let mut env = Vec::new();

    // Nebo app-specific vars
    env.push(("NEBO_APP_ID".into(), app_id.into()));
    env.push(("NEBO_APP_NAME".into(), app_name.into()));
    env.push(("NEBO_APP_VERSION".into(), app_version.into()));
    env.push(("NEBO_APP_DIR".into(), app_dir.into()));
    env.push(("NEBO_APP_SOCK".into(), sock_path.into()));
    env.push(("NEBO_APP_DATA".into(), data_dir.into()));

    // Allowlisted system vars
    for var in ALLOWED_SYSTEM_VARS {
        if let Ok(val) = std::env::var(var) {
            env.push((var.to_string(), val));
        }
    }

    env
}

/// Validate a binary before execution.
pub fn validate_binary(path: &Path, max_size: u64) -> Result<(), NappError> {
    let meta = std::fs::symlink_metadata(path)
        .map_err(|e| NappError::Sandbox(format!("stat binary: {}", e)))?;

    // Reject symlinks
    if meta.file_type().is_symlink() {
        return Err(NappError::Sandbox("binary is a symlink".into()));
    }

    // Must be a regular file
    if !meta.is_file() {
        return Err(NappError::Sandbox("binary is not a regular file".into()));
    }

    // Size limit
    if meta.len() > max_size {
        return Err(NappError::Sandbox(format!(
            "binary exceeds size limit ({} > {})",
            meta.len(),
            max_size
        )));
    }

    // Must be executable (Unix)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 == 0 {
            return Err(NappError::Sandbox("binary is not executable".into()));
        }
    }

    // Validate native binary format
    let mut file = std::fs::File::open(path)?;
    let mut magic = [0u8; 4];
    use std::io::Read;
    file.read_exact(&mut magic)?;

    let is_native = magic == [0x7f, 0x45, 0x4c, 0x46] // ELF
        || magic == [0xfe, 0xed, 0xfa, 0xce]            // Mach-O 32
        || magic == [0xfe, 0xed, 0xfa, 0xcf]            // Mach-O 64
        || magic == [0xce, 0xfa, 0xed, 0xfe]            // Mach-O 32 swapped
        || magic == [0xcf, 0xfa, 0xed, 0xfe]            // Mach-O 64 swapped
        || magic == [0xca, 0xfe, 0xba, 0xbe]            // Universal
        || magic[..2] == [0x4d, 0x5a];                  // PE

    if !is_native {
        if magic[..2] == [0x23, 0x21] {
            // "#!" — shebang
            return Err(NappError::Sandbox("scripts not allowed".into()));
        }
        return Err(NappError::Sandbox("not a native binary".into()));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_env_blocks_secrets() {
        let env = sanitize_env("app-1", "TestApp", "1.0", "/apps/app-1", "/tmp/app.sock", "/apps/app-1/data");

        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"NEBO_APP_ID"));
        assert!(keys.contains(&"NEBO_APP_NAME"));
        assert!(!keys.contains(&"ANTHROPIC_API_KEY"));
        assert!(!keys.contains(&"JWT_SECRET"));
    }

    #[test]
    fn test_sanitize_env_includes_system() {
        let env = sanitize_env("x", "X", "1", "/x", "/x.sock", "/x/data");
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        // PATH should be included if set
        if std::env::var("PATH").is_ok() {
            assert!(keys.contains(&"PATH"));
        }
    }
}
