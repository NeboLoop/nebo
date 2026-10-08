use std::sync::OnceLock;

use tracing::debug;

/// Write-once encryptor set at startup. OnceLock (not mutable state) because
/// tools and agent code need encryption without access to AppState.
static ENCRYPTOR: OnceLock<mcp::crypto::Encryptor> = OnceLock::new();

/// Prefix for encrypted values (base64-encoded ciphertext).
const ENCRYPTED_PREFIX: &str = "enc:";

/// Initialize the credential system with a resolved encryption key.
/// Must be called once at startup.
pub fn init(encryptor: mcp::crypto::Encryptor) {
    if ENCRYPTOR.set(encryptor).is_err() {
        debug!("credential encryptor already initialized");
    }
}

/// Check if the credential system is initialized.
pub fn is_initialized() -> bool {
    ENCRYPTOR.get().is_some()
}

/// Encrypt a plaintext value and return with the `enc:` prefix. A value
/// already encrypted, and the empty value (no secret at all), come back as
/// they are, so encrypting twice never wraps a secret in itself.
/// Errors when the encryptor is not initialized: never a plaintext fallback.
pub fn encrypt(plaintext: &str) -> Result<String, String> {
    if plaintext.is_empty() || is_encrypted(plaintext) {
        return Ok(plaintext.to_string());
    }
    let enc = ENCRYPTOR
        .get()
        .ok_or_else(|| "credential encryptor not initialized".to_string())?;

    let b64 = enc
        .encrypt_b64(plaintext.as_bytes())
        .map_err(|e| format!("encryption failed: {}", e))?;

    Ok(format!("{}{}", ENCRYPTED_PREFIX, b64))
}

/// Decrypt a value. If it doesn't have the `enc:` prefix, returns it as-is (plaintext).
/// Returns an error only if decryption actually fails.
pub fn decrypt(value: &str) -> Result<String, String> {
    if !value.starts_with(ENCRYPTED_PREFIX) {
        return Ok(value.to_string());
    }

    let enc = ENCRYPTOR
        .get()
        .ok_or_else(|| "credential encryptor not initialized".to_string())?;

    let b64 = &value[ENCRYPTED_PREFIX.len()..];
    let decrypted = enc
        .decrypt_b64(b64)
        .map_err(|e| format!("decryption failed: {}", e))?;

    String::from_utf8(decrypted).map_err(|e| format!("invalid UTF-8 after decryption: {}", e))
}

/// A provider profile's key in the clear, for the moment a client that
/// presents it is built. Keys are stored encrypted (`enc:`); a key that will
/// not decrypt (the master key changed) is no key: logged, and empty.
pub fn profile_key(profile: &db::models::AuthProfile) -> String {
    decrypt(&profile.api_key).unwrap_or_else(|e| {
        tracing::warn!(profile = %profile.id, provider = %profile.provider, error = %e, "provider key could not be decrypted");
        String::new()
    })
}

/// Encrypt every provider key still stored in the clear, in place — in the
/// live database and its local copies (`db::Store::reseal_profile_keys`).
/// Run on every start, after `init`: a key already encrypted, and an empty
/// one, are left alone, so a second run changes nothing. Returns how many
/// keys the live database had in the clear.
pub fn encrypt_stored_profile_keys(store: &db::Store) -> Result<usize, String> {
    store
        .reseal_profile_keys(|key| {
            if key.is_empty() || is_encrypted(key) {
                Ok(None)
            } else {
                encrypt(key).map(Some)
            }
        })
        .map_err(|e| e.to_string())
}

/// A plugin setting's value in the clear, for the plugin's env. A secret
/// setting is stored encrypted (`enc:`): one that will not decrypt, or one
/// still in the clear (never after `encrypt_stored_plugin_secrets`), is no
/// value: logged, and empty.
pub fn plugin_setting_value(setting: &db::models::PluginSetting) -> String {
    if setting.is_secret == 0 || setting.setting_value.is_empty() {
        return setting.setting_value.clone();
    }
    if !is_encrypted(&setting.setting_value) {
        tracing::warn!(plugin = %setting.plugin_id, key = %setting.setting_key, "secret plugin setting is not encrypted; ignored");
        return String::new();
    }
    decrypt(&setting.setting_value).unwrap_or_else(|e| {
        tracing::warn!(plugin = %setting.plugin_id, key = %setting.setting_key, error = %e, "secret plugin setting could not be decrypted");
        String::new()
    })
}

/// Encrypt every secret plugin setting still stored in the clear, in place,
/// in the live database and its local copies (`db::Store::reseal_plugin_secrets`).
/// `declared` is the (slug, key) pairs the installed plugins declare secret
/// (`napp::plugin::PluginManifest::is_secret_setting`). Run on every start,
/// after `init` and before any plugin's env is built: a second run changes
/// nothing. Returns how many settings the live database had changed.
pub fn encrypt_stored_plugin_secrets(
    store: &db::Store,
    declared: &[(String, String)],
) -> Result<usize, String> {
    store
        .reseal_plugin_secrets(declared, |value| {
            if value.is_empty() || is_encrypted(value) {
                Ok(None)
            } else {
                encrypt(value).map(Some)
            }
        })
        .map_err(|e| e.to_string())
}

/// Check if a value is encrypted (has the `enc:` prefix).
pub fn is_encrypted(value: &str) -> bool {
    value.starts_with(ENCRYPTED_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_encryptor() {
        let enc = mcp::crypto::Encryptor::generate();
        let _ = ENCRYPTOR.set(enc);
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        setup_encryptor();
        let original = "sk-my-api-key-12345";
        let encrypted = encrypt(original).unwrap();

        assert!(encrypted.starts_with(ENCRYPTED_PREFIX));
        assert!(is_encrypted(&encrypted));

        let decrypted = decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, original);
    }

    #[test]
    fn test_decrypt_plaintext_passthrough() {
        setup_encryptor();
        let plaintext = "not-encrypted-value";
        assert!(!is_encrypted(plaintext));
        let result = decrypt(plaintext).unwrap();
        assert_eq!(result, plaintext);
    }

    #[test]
    fn encrypting_twice_and_encrypting_nothing_change_nothing() {
        setup_encryptor();
        let once = encrypt("sk-twice").unwrap();
        assert_eq!(encrypt(&once).unwrap(), once);
        assert_eq!(decrypt(&encrypt(&once).unwrap()).unwrap(), "sk-twice");
        assert_eq!(encrypt("").unwrap(), "");
    }

    /// Every byte under `dir`, every file: the database, its WAL, the ring.
    fn bytes_under(dir: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(bytes_under(&path));
            } else {
                out.push((path.clone(), std::fs::read(&path).unwrap()));
            }
        }
        out
    }

    /// A key typed before keys were encrypted is encrypted in place on the
    /// next start — in the live database and in the copy the ring took of it —
    /// and still opens to the same key for the client that presents it. A
    /// second start changes nothing; an empty key stays empty.
    #[test]
    fn keys_stored_in_the_clear_are_encrypted_in_place_once() {
        setup_encryptor();
        const SENTINEL: &str = "sk-SENTINEL-at-rest-4242424242";
        let dir = std::env::temp_dir().join(format!("nebo-reseal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = db::Store::new(dir.join("nebo.db").to_str().unwrap()).unwrap();
        store.create_auth_profile("a", "Mine", "anthropic", SENTINEL, None, None, 0, 1, None, None).unwrap();
        store.create_auth_profile("o", "Local", "ollama", "", None, None, 0, 1, Some("local"), None).unwrap();
        let already = encrypt("sk-already").unwrap();
        store.create_auth_profile("e", "Sealed", "openai", &already, None, None, 0, 1, None, None).unwrap();
        store.snapshot("manual").unwrap();
        assert!(
            bytes_under(&dir).iter().any(|(_, b)| b.windows(SENTINEL.len()).any(|w| w == SENTINEL.as_bytes())),
            "the fixture really starts with the key in the clear"
        );

        assert_eq!(encrypt_stored_profile_keys(&store).unwrap(), 1);
        let get = |id: &str| store.get_auth_profile(id).unwrap().unwrap();
        assert!(is_encrypted(&get("a").api_key));
        assert_eq!(profile_key(&get("a")), SENTINEL, "the client still gets the key");
        assert_eq!(get("o").api_key, "", "no key stays no key");
        assert_eq!(get("e").api_key, already, "an encrypted key is not encrypted again");
        assert_eq!(profile_key(&get("e")), "sk-already");

        for (path, bytes) in bytes_under(&dir) {
            assert!(
                !bytes.windows(SENTINEL.len()).any(|w| w == SENTINEL.as_bytes()),
                "the key is in the clear at rest in {}",
                path.display()
            );
        }

        let sealed = get("a").api_key;
        assert_eq!(encrypt_stored_profile_keys(&store).unwrap(), 0, "a second start changes nothing");
        assert_eq!(get("a").api_key, sealed);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn setting(value: &str, is_secret: i64) -> db::models::PluginSetting {
        db::models::PluginSetting {
            id: "napp-odoo:K".into(),
            plugin_id: "napp-odoo".into(),
            setting_key: "K".into(),
            setting_value: value.into(),
            is_secret,
            created_at: 0,
            updated_at: 0,
        }
    }

    /// A secret plugin setting opens to what was typed; a setting that isn't
    /// secret is its value; a secret still in the clear is not read at all.
    #[test]
    fn a_plugin_secret_round_trips_and_plaintext_is_not_read() {
        setup_encryptor();
        let sealed = encrypt("odoo-key-123").unwrap();
        assert!(is_encrypted(&sealed));
        assert_eq!(plugin_setting_value(&setting(&sealed, 1)), "odoo-key-123");
        assert_eq!(plugin_setting_value(&setting("https://acme.odoo.com", 0)), "https://acme.odoo.com");
        assert_eq!(plugin_setting_value(&setting("odoo-key-123", 1)), "", "no legacy read path");
        assert_eq!(plugin_setting_value(&setting("", 1)), "");
    }

    /// Plugin secrets typed before they were encrypted are encrypted in place
    /// on the next start — in the live database and the ring's copy — and
    /// open to the same value for the plugin's env. A setting the plugin
    /// declares secret is sealed and marked secret even when it was saved as
    /// not secret; one that is neither stays as it is. A second start
    /// changes nothing.
    #[test]
    fn plugin_secrets_stored_in_the_clear_are_encrypted_in_place_once() {
        setup_encryptor();
        const SENTINEL: &str = "odoo-SENTINEL-at-rest-5151515151";
        const DECLARED: &str = "odoo-SENTINEL-declared-6262626262";
        let dir = std::env::temp_dir().join(format!("nebo-reseal-plugin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = db::Store::new(dir.join("nebo.db").to_str().unwrap()).unwrap();
        store.upsert_installed_plugin("odoo", "Odoo", "1.0.0", "", "", "", "unverified").unwrap();
        store.upsert_plugin_setting_by_slug("odoo", "ODOO_API_KEY", SENTINEL, true).unwrap();
        store.upsert_plugin_setting_by_slug("odoo", "ODOO_PASSWORD", DECLARED, false).unwrap();
        store.upsert_plugin_setting_by_slug("odoo", "ODOO_REGION", "eu", false).unwrap();
        store.upsert_plugin_setting_by_slug("odoo", "ODOO_EMPTY", "", true).unwrap();
        store.snapshot("manual").unwrap();
        assert!(
            bytes_under(&dir).iter().any(|(_, b)| b.windows(SENTINEL.len()).any(|w| w == SENTINEL.as_bytes())),
            "the fixture really starts with the secret in the clear"
        );

        let declared = vec![("odoo".to_string(), "ODOO_PASSWORD".to_string())];
        assert_eq!(encrypt_stored_plugin_secrets(&store, &declared).unwrap(), 2);
        let get = |key: &str| {
            store.list_plugin_settings_by_slug("odoo").unwrap().into_iter().find(|s| s.setting_key == key).unwrap()
        };
        assert!(is_encrypted(&get("ODOO_API_KEY").setting_value));
        assert_eq!(plugin_setting_value(&get("ODOO_API_KEY")), SENTINEL, "the plugin still gets the key");
        assert_eq!(get("ODOO_PASSWORD").is_secret, 1, "a declared secret is marked secret");
        assert_eq!(plugin_setting_value(&get("ODOO_PASSWORD")), DECLARED);
        assert_eq!(get("ODOO_REGION").setting_value, "eu", "a setting that isn't secret is left alone");
        assert_eq!(get("ODOO_EMPTY").setting_value, "", "no value stays no value");

        for (path, bytes) in bytes_under(&dir) {
            for secret in [SENTINEL, DECLARED] {
                assert!(
                    !bytes.windows(secret.len()).any(|w| w == secret.as_bytes()),
                    "a plugin secret is in the clear at rest in {}",
                    path.display()
                );
            }
        }

        let sealed = get("ODOO_API_KEY").setting_value;
        assert_eq!(encrypt_stored_plugin_secrets(&store, &declared).unwrap(), 0, "a second start changes nothing");
        assert_eq!(get("ODOO_API_KEY").setting_value, sealed);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_is_encrypted() {
        assert!(is_encrypted("enc:abc123"));
        assert!(!is_encrypted("plaintext"));
        assert!(!is_encrypted(""));
    }
}
