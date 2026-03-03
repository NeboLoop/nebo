use std::fs;

use serde::{Deserialize, Serialize};

use crate::defaults;
use types::NeboError;
use types::constants::{DEFAULT_ACCESS_EXPIRE, DEFAULT_REFRESH_TOKEN_EXPIRE, files};

/// Local settings that can't be in the embedded YAML.
/// Auto-generated on first run, stored in `<data_dir>/settings.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(rename = "accessSecret")]
    pub access_secret: String,
    #[serde(rename = "accessExpire")]
    pub access_expire: i64,
    #[serde(rename = "refreshTokenExpire")]
    pub refresh_token_expire: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            access_secret: String::new(),
            access_expire: DEFAULT_ACCESS_EXPIRE,
            refresh_token_expire: DEFAULT_REFRESH_TOKEN_EXPIRE,
        }
    }
}

/// Load local settings, creating defaults if needed.
pub fn load_settings() -> Result<Settings, NeboError> {
    let dir = defaults::ensure_data_dir()?;
    let path = dir.join(files::SETTINGS_JSON);

    // Try to load existing
    if let Ok(data) = fs::read_to_string(&path)
        && let Ok(mut settings) = serde_json::from_str::<Settings>(&data) {
            if settings.access_secret.is_empty() {
                settings.access_secret = generate_secret();
                save_settings(&settings)?;
            }
            return Ok(settings);
        }

    // Create new settings with generated secret
    let settings = Settings {
        access_secret: generate_secret(),
        ..Default::default()
    };
    save_settings(&settings)?;
    Ok(settings)
}

/// Persist settings to disk.
pub fn save_settings(settings: &Settings) -> Result<(), NeboError> {
    let dir = defaults::data_dir()?;
    let path = dir.join(files::SETTINGS_JSON);

    // Ensure directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            NeboError::Config(format!("failed to create settings directory: {e}"))
        })?;
    }

    let data = serde_json::to_string_pretty(settings)
        .map_err(|e| NeboError::Config(format!("failed to serialize settings: {e}")))?;

    fs::write(&path, data)?;

    // Set restrictive permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(&path, perms)?;
    }

    Ok(())
}

/// Generate a cryptographically secure random secret (32 bytes, hex-encoded).
fn generate_secret() -> String {
    use rand::Rng;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill(&mut bytes);
    hex::encode(bytes)
}
