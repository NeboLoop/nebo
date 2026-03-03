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
