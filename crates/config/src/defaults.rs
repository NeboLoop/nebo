use std::fs;
use std::path::PathBuf;

use types::NeboError;
use types::constants::files;

/// Returns the platform-appropriate data directory.
///
/// - macOS:   ~/Library/Application Support/Nebo/
/// - Windows: %AppData%\Nebo\
/// - Linux:   ~/.config/nebo/
///
/// Set `NEBO_DATA_DIR` to override.
pub fn data_dir() -> Result<PathBuf, NeboError> {
    if let Ok(dir) = std::env::var("NEBO_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }

    let config_dir = dirs::config_dir()
        .ok_or_else(|| NeboError::DataDir("cannot determine config directory".into()))?;

    // Linux: lowercase per XDG convention
    // macOS/Windows: title case per platform convention
    let name = if cfg!(target_os = "linux") {
        "nebo"
    } else {
        "Nebo"
    };

    Ok(config_dir.join(name))
}

/// Creates the data directory if it doesn't exist and returns its path.
pub fn ensure_data_dir() -> Result<PathBuf, NeboError> {
    let dir = data_dir()?;
    fs::create_dir_all(&dir).map_err(|e| {
        NeboError::DataDir(format!("failed to create data directory: {e}"))
    })?;
    // Ensure the data/ subdirectory exists for the database
    fs::create_dir_all(dir.join("data")).map_err(|e| {
        NeboError::DataDir(format!("failed to create data/data directory: {e}"))
    })?;
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

// ── Artifact Directory Helpers ─────────────────────────────────────

/// Returns the `nebo/` directory for marketplace (sealed) artifacts.
pub fn nebo_dir() -> Result<PathBuf, NeboError> {
    Ok(data_dir()?.join("nebo"))
}

/// Returns the `user/` directory for user-created (loose) artifacts.
pub fn user_dir() -> Result<PathBuf, NeboError> {
    Ok(data_dir()?.join("user"))
}

/// Artifact type subdirectories.
const ARTIFACT_TYPES: &[&str] = &["skills", "tools", "workflows", "roles"];

/// Returns the `bundled/` directory for skills shipped with the app.
pub fn bundled_skills_dir() -> Result<PathBuf, NeboError> {
    Ok(data_dir()?.join("bundled").join("skills"))
}

/// Ensure all artifact directories exist for both namespaces.
///
/// Creates:
/// - `<data_dir>/bundled/skills/`
/// - `<data_dir>/nebo/{skills,tools,workflows,roles}/`
/// - `<data_dir>/user/{skills,tools,workflows,roles}/`
/// - `<data_dir>/data/`
pub fn ensure_artifact_dirs() -> Result<(), NeboError> {
    let data = data_dir()?;

    // Ensure data/ for database
    fs::create_dir_all(data.join("data")).map_err(|e| {
        NeboError::DataDir(format!("failed to create data/ directory: {e}"))
    })?;

    // Ensure bundled skills directory
    fs::create_dir_all(data.join("bundled").join("skills")).map_err(|e| {
        NeboError::DataDir(format!("failed to create bundled/skills directory: {e}"))
    })?;

    // Create nebo/ and user/ subdirectories
    for namespace in &["nebo", "user"] {
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
