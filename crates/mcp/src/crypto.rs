use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;

use crate::McpError;

const NONCE_SIZE: usize = 12;
const KEY_SIZE: usize = 32;

/// AES-256-GCM encryption for sensitive credentials.
pub struct Encryptor {
    key: [u8; KEY_SIZE],
}

impl Encryptor {
    /// Create from a raw 32-byte key.
    pub fn new(key: [u8; KEY_SIZE]) -> Self {
        Self { key }
    }

    /// Derive a key from a passphrase using SHA-256.
    pub fn from_passphrase(passphrase: &str) -> Self {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(passphrase.as_bytes());
        let result = hasher.finalize();
        let mut key = [0u8; KEY_SIZE];
        key.copy_from_slice(&result);
        Self { key }
    }

    /// Generate a random key.
    pub fn generate() -> Self {
        let mut key = [0u8; KEY_SIZE];
        OsRng.fill_bytes(&mut key);
        Self { key }
    }

    /// Get the key bytes (for persistence).
    pub fn key_bytes(&self) -> &[u8; KEY_SIZE] {
        &self.key
    }

    /// Encrypt plaintext. Returns nonce + ciphertext as a single Vec.
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, McpError> {
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|e| McpError::Crypto(e.to_string()))?;

        let mut nonce_bytes = [0u8; NONCE_SIZE];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| McpError::Crypto(e.to_string()))?;

        let mut result = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
        result.extend_from_slice(&nonce_bytes);
        result.extend_from_slice(&ciphertext);
        Ok(result)
    }

    /// Decrypt data (nonce + ciphertext).
    pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>, McpError> {
        if data.len() < NONCE_SIZE {
            return Err(McpError::Crypto("data too short".into()));
        }

        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|e| McpError::Crypto(e.to_string()))?;

        let nonce = Nonce::from_slice(&data[..NONCE_SIZE]);
        let ciphertext = &data[NONCE_SIZE..];

        cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| McpError::Crypto(e.to_string()))
    }

    /// Encrypt and return as base64.
    pub fn encrypt_b64(&self, plaintext: &[u8]) -> Result<String, McpError> {
        use base64::Engine;
        let encrypted = self.encrypt(plaintext)?;
        Ok(base64::engine::general_purpose::STANDARD.encode(encrypted))
    }

    /// Decrypt from base64.
    pub fn decrypt_b64(&self, b64: &str) -> Result<Vec<u8>, McpError> {
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| McpError::Crypto(e.to_string()))?;
        self.decrypt(&data)
    }
}

/// Resolve or generate an encryption key.
/// Priority: env MCP_ENCRYPTION_KEY → env JWT_SECRET → persistent file → generate new.
pub fn resolve_encryption_key(data_dir: &std::path::Path) -> Encryptor {
    // 1. MCP_ENCRYPTION_KEY env
    if let Ok(key) = std::env::var("MCP_ENCRYPTION_KEY") {
        return Encryptor::from_passphrase(&key);
    }

    // 2. JWT_SECRET env
    if let Ok(key) = std::env::var("JWT_SECRET") {
        return Encryptor::from_passphrase(&key);
    }

    // 3. Persistent file
    let key_file = data_dir.join(".mcp-key");
    if let Ok(data) = std::fs::read(&key_file) {
        if data.len() == KEY_SIZE {
            let mut key = [0u8; KEY_SIZE];
            key.copy_from_slice(&data);
            return Encryptor::new(key);
        }
    }

    // 4. Generate and persist
    let enc = Encryptor::generate();
    let _ = std::fs::write(&key_file, enc.key_bytes());
    enc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt() {
        let enc = Encryptor::generate();
        let plaintext = b"hello world secret";
        let encrypted = enc.encrypt(plaintext).unwrap();
        let decrypted = enc.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_decrypt_b64() {
        let enc = Encryptor::from_passphrase("test-key");
        let plaintext = b"api-token-12345";
        let b64 = enc.encrypt_b64(plaintext).unwrap();
        let decrypted = enc.decrypt_b64(&b64).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_different_keys_fail() {
        let enc1 = Encryptor::generate();
        let enc2 = Encryptor::generate();
        let encrypted = enc1.encrypt(b"secret").unwrap();
        assert!(enc2.decrypt(&encrypted).is_err());
    }
}
