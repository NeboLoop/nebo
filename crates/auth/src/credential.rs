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

/// Encrypt a plaintext value and return with the `enc:` prefix.
/// Returns the original value if the encryptor is not initialized.
pub fn encrypt(plaintext: &str) -> Result<String, String> {
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
    fn test_is_encrypted() {
        assert!(is_encrypted("enc:abc123"));
        assert!(!is_encrypted("plaintext"));
        assert!(!is_encrypted(""));
    }
}
