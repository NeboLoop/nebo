use std::fs;
use std::path::PathBuf;

use types::NeboError;
use types::constants::files;

/// Returns the platform-native data directory (the Nebo root):
///
/// - macOS:   `~/Library/Application Support/Nebo`
/// - Windows: `%APPDATA%\Nebo`
/// - Linux:   `~/.local/share/nebo`
///
/// Set `NEBO_HOME` to override the root. (`NEBO_DATA_DIR` is the deprecated
/// spelling — still honored for one release — but that name now belongs to a
/// different concept: a per-artifact persistent data directory passed to each
/// plugin/app/skill process. Keeping both meanings under one name was the
/// source of artifacts writing their DB to the wrong place.)
pub fn data_dir() -> Result<PathBuf, NeboError> {
    if let Ok(dir) = std::env::var("NEBO_HOME") {
        return Ok(PathBuf::from(dir));
    }
    if let Ok(dir) = std::env::var("NEBO_DATA_DIR") {
        tracing::warn!(
            "NEBO_DATA_DIR is deprecated as the Nebo root override and will be removed; \
             use NEBO_HOME instead (NEBO_DATA_DIR now means a per-artifact data directory)"
        );
        return Ok(PathBuf::from(dir));
    }

    let base = dirs::data_dir()
        .ok_or_else(|| NeboError::DataDir("cannot determine data directory".into()))?;

    let name = if cfg!(target_os = "linux") {
        "nebo"
    } else {
        "Nebo"
    };

    Ok(base.join(name))
}

/// Returns the old platform-specific data directory path (pre-v5).
/// Used by the migration to find data to move.
pub fn legacy_data_dir() -> Option<PathBuf> {
    let config_dir = dirs::config_dir()?;
    let name = if cfg!(target_os = "linux") {
        "nebo"
    } else {
        "Nebo"
    };
    Some(config_dir.join(name))
}

/// Creates the data directory if it doesn't exist and returns its path.
pub fn ensure_data_dir() -> Result<PathBuf, NeboError> {
    let dir = data_dir()?;
    fs::create_dir_all(&dir)
        .map_err(|e| NeboError::DataDir(format!("failed to create data directory: {e}")))?;
    // Ensure the data/ subdirectory exists for the database
    fs::create_dir_all(dir.join("data"))
        .map_err(|e| NeboError::DataDir(format!("failed to create data/data directory: {e}")))?;
    Ok(dir)
}

/// Reads the bot_id from `<data_dir>/bot_id`.
/// Returns `None` if the file doesn't exist or the value isn't a valid 36-char UUID.
pub fn read_bot_id() -> Option<String> {
    let dir = data_dir().ok()?;
    let data = fs::read_to_string(dir.join(files::BOT_ID)).ok()?;
    let id = data.trim().to_string();
    if id.len() == 36 { Some(id) } else { None }
}

/// Persists the bot_id to `<data_dir>/bot_id` with read-only permissions.
pub fn write_bot_id(id: &str) -> Result<(), NeboError> {
    let dir = data_dir()?;
    let path = dir.join(files::BOT_ID);
    // Remove existing file (may be read-only)
    let _ = fs::remove_file(&path);
    fs::write(&path, id)?;
    // Set read-only on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o400);
        fs::set_permissions(&path, perms)?;
    }
    Ok(())
}

/// Ensure a bot_id exists. Reads from file first; if absent, generates a new UUID and persists it.
pub fn ensure_bot_id() -> String {
    if let Some(id) = read_bot_id() {
        return id;
    }
    let id = uuid::Uuid::new_v4().to_string();
    if let Err(e) = write_bot_id(&id) {
        tracing::warn!("failed to persist new bot_id: {}", e);
    }
    id
}

/// Read or generate the per-install extension relay secret, stored at
/// `<data_dir>/.extension-secret` (mode 0600). It authenticates the native
/// messaging relay to the local server on `/ws/extension` — a value that a
/// hostile web page (no filesystem access) can never present, closing the
/// localhost-WS / DNS-rebinding path into the browser-control channel. Both the
/// server (generates at startup) and the relay (reads on connect) resolve the
/// same default data dir, so they agree without any inter-process handoff.
pub fn ensure_extension_secret() -> Result<String, NeboError> {
    if let Some(existing) = read_extension_secret() {
        return Ok(existing);
    }
    let secret = {
        use rand::Rng;
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill(&mut bytes);
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    let dir = data_dir()?;
    let path = dir.join(files::EXTENSION_SECRET);
    let _ = fs::remove_file(&path);
    fs::write(&path, &secret)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(secret)
}

/// Read the extension relay secret without generating one (relay side).
/// Returns `None` if the file is absent or empty.
pub fn read_extension_secret() -> Option<String> {
    let dir = data_dir().ok()?;
    let s = fs::read_to_string(dir.join(files::EXTENSION_SECRET)).ok()?;
    let s = s.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

// ── Artifact Directory Helpers ─────────────────────────────────────

/// Returns the `nebo/` directory for marketplace (sealed) artifacts.
pub fn nebo_dir() -> Result<PathBuf, NeboError> {
    Ok(data_dir()?.join("nebo"))
}

/// Returns the `user/` directory for user-created (loose) artifacts.
pub fn user_dir() -> Result<PathBuf, NeboError> {
    Ok(data_dir()?.join("user"))
}

/// Returns the `appdata/` directory for persistent artifact data.
///
/// This tree is **never** touched by the update system. Artifacts own their
/// data and are responsible for their own schema migrations across versions.
/// Layout: `appdata/{plugins,skills,agents}/<slug>/`
pub fn appdata_dir() -> Result<PathBuf, NeboError> {
    Ok(data_dir()?.join("appdata"))
}

/// Artifact type subdirectories.
const ARTIFACT_TYPES: &[&str] = &["skills", "agents"];

/// Ensure all artifact directories exist for both namespaces.
///
/// Creates:
/// - `<data_dir>/nebo/{skills,agents}/`
/// - `<data_dir>/user/{skills,agents}/`
/// - `<data_dir>/data/`
/// - `<data_dir>/files/large_inputs/`
///
/// Bundled skills/agents are embedded in the binary and loaded from memory
/// — no filesystem directory needed.
pub fn ensure_artifact_dirs() -> Result<(), NeboError> {
    let data = data_dir()?;

    // Ensure data/ for database
    fs::create_dir_all(data.join("data"))
        .map_err(|e| NeboError::DataDir(format!("failed to create data/ directory: {e}")))?;

    // Ensure files/large_inputs directory for large input offloading
    fs::create_dir_all(data.join("files").join("large_inputs")).map_err(|e| {
        NeboError::DataDir(format!(
            "failed to create files/large_inputs directory: {e}"
        ))
    })?;

    // Create nebo/, user/, and appdata/ subdirectories
    for namespace in &["nebo", "user", "appdata"] {
        for artifact_type in ARTIFACT_TYPES {
            let dir = data.join(namespace).join(artifact_type);
            fs::create_dir_all(&dir).map_err(|e| {
                NeboError::DataDir(format!(
                    "failed to create {}/{} directory: {e}",
                    namespace, artifact_type
                ))
            })?;
        }
    }
    // appdata/plugins (not in ARTIFACT_TYPES since plugins aren't a top-level artifact dir)
    fs::create_dir_all(data.join("appdata").join("plugins")).map_err(|e| {
        NeboError::DataDir(format!("failed to create appdata/plugins directory: {e}"))
    })?;

    Ok(())
}

/// Resolve the filesystem path for a sealed .napp artifact.
///
/// Returns `<data_dir>/nebo/<artifact_type>/<qualified_name>/<version>.napp`
///
/// # Example
/// ```ignore
/// let path = artifact_napp_path("skills", "@acme/skills/sales-qualification", "1.0.0")?;
/// // => <data_dir>/nebo/skills/@acme/skills/sales-qualification/1.0.0.napp
/// ```
pub fn artifact_napp_path(
    artifact_type: &str,
    qualified_name: &str,
    version: &str,
) -> Result<PathBuf, NeboError> {
    let dir = nebo_dir()?.join(artifact_type).join(qualified_name);
    Ok(dir.join(format!("{}.napp", version)))
}

/// Returns the user artifact directory for a given type and name.
///
/// # Example
/// ```ignore
/// let path = user_artifact_path("skills", "my-custom-skill")?;
/// // => <data_dir>/user/skills/my-custom-skill/
/// ```
pub fn user_artifact_path(artifact_type: &str, name: &str) -> Result<PathBuf, NeboError> {
    Ok(user_dir()?.join(artifact_type).join(name))
}

// ── Bundled Resources ─────────────────────────────────────────────

/// Returns the path to bundled `.napp` files inside the app bundle.
///
/// - macOS:   `<exe_dir>/../Resources/bundled-napps/`
/// - Windows: `<exe_dir>/resources/bundled-napps/`
/// - Linux:   `<exe_dir>/../resources/bundled-napps/`
///
/// Returns `None` if the directory doesn't exist (dev/CLI mode).
pub fn bundled_napps_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;

    let dir = if cfg!(target_os = "macos") {
        // Nebo.app/Contents/MacOS/nebo → Nebo.app/Contents/Resources/bundled-napps/
        exe_dir.join("../Resources/bundled-napps")
    } else {
        // Linux/Windows: <exe_dir>/resources/bundled-napps/
        exe_dir.join("resources/bundled-napps")
    };

    if dir.is_dir() { Some(dir) } else { None }
}

/// Checks if the setup has been marked as complete.
pub fn is_setup_complete() -> Result<bool, NeboError> {
    let dir = data_dir()?;
    Ok(dir.join(files::SETUP_COMPLETE).exists())
}

/// Creates the `.setup-complete` file with the current Unix timestamp.
pub fn mark_setup_complete() -> Result<(), NeboError> {
    let dir = ensure_data_dir()?;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    fs::write(dir.join(files::SETUP_COMPLETE), timestamp.to_string())?;
    Ok(())
}
