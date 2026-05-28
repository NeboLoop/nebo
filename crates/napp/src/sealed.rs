use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::Sha256;

use crate::NappError;

const NONCE_SIZE: usize = 12;
const KEY_SIZE: usize = 32;
const HKDF_INFO: &[u8] = b"neboai-license-v1";

/// Gzip magic bytes — plain (unsealed) tar.gz payloads start with these.
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// Derives a per-artifact license key from a master secret using HKDF-SHA256.
///
/// The salt is the artifact_id only — license scope (user/bot) is NOT part of
/// derivation. Authorization is server-side; the same key works regardless of
/// who holds the license. This means sealed .napp files never need re-download
/// on license transfer.
pub fn derive_license_key(master_secret: &[u8], artifact_id: &str) -> [u8; KEY_SIZE] {
    let hkdf = Hkdf::<Sha256>::new(Some(artifact_id.as_bytes()), master_secret);
    let mut key = [0u8; KEY_SIZE];
    // expand cannot fail when output length <= 255 * hash length
    hkdf.expand(HKDF_INFO, &mut key)
        .expect("HKDF expand failed: output length within bounds");
    key
}

/// Encrypts a tar.gz payload with a license key, producing a sealed payload.
///
/// Output format: `[12-byte random nonce | AES-256-GCM ciphertext + 16-byte tag]`
pub fn seal_payload(payload: &[u8], license_key: &[u8; KEY_SIZE]) -> Result<Vec<u8>, NappError> {
    let cipher = Aes256Gcm::new_from_slice(license_key)
        .map_err(|e| NappError::Extraction(format!("seal: invalid key: {}", e)))?;

    let mut nonce_bytes = [0u8; NONCE_SIZE];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, payload)
        .map_err(|e| NappError::Extraction(format!("seal: encryption failed: {}", e)))?;

    let mut result = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

/// Decrypts a sealed payload back to the original tar.gz.
///
/// Input format: `[12-byte nonce | AES-256-GCM ciphertext + 16-byte tag]`
pub fn unseal_payload(sealed: &[u8], license_key: &[u8; KEY_SIZE]) -> Result<Vec<u8>, NappError> {
    if sealed.len() < NONCE_SIZE + 16 {
        return Err(NappError::Extraction(format!(
            "unseal: data too short ({} bytes, need at least {})",
            sealed.len(),
            NONCE_SIZE + 16
        )));
    }

    let cipher = Aes256Gcm::new_from_slice(license_key)
        .map_err(|e| NappError::Extraction(format!("unseal: invalid key: {}", e)))?;

    let nonce = Nonce::from_slice(&sealed[..NONCE_SIZE]);
    let ciphertext = &sealed[NONCE_SIZE..];

    cipher.decrypt(nonce, ciphertext).map_err(|_| {
        NappError::Extraction("unseal: decryption failed (wrong key or corrupted data)".into())
    })
}

/// Checks whether a .napp payload (after envelope unwrap) is sealed or plain.
///
/// Plain payloads are tar.gz and start with gzip magic bytes `0x1f 0x8b`.
/// Sealed payloads start with a 12-byte random nonce, which will not match.
pub fn is_sealed(payload: &[u8]) -> bool {
    if payload.len() < 2 {
        return false;
    }
    payload[..2] != GZIP_MAGIC
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_targz() -> Vec<u8> {
        // Create a minimal valid gzip stream (empty gzip with magic bytes)
        vec![
            0x1f, 0x8b, 0x08, 0x00, // magic + method + flags
            0x00, 0x00, 0x00, 0x00, // mtime
            0x00, 0x03, // xfl + OS
            0x03, 0x00, // empty deflate block
            0x00, 0x00, 0x00, 0x00, // CRC32
            0x00, 0x00, 0x00, 0x00, // input size
        ]
    }

    #[test]
    fn test_round_trip() {
        let payload = sample_targz();
        let key = derive_license_key(b"test-master-secret", "artifact-123");
        let sealed = seal_payload(&payload, &key).unwrap();
        let unsealed = unseal_payload(&sealed, &key).unwrap();
        assert_eq!(unsealed, payload);
    }

    #[test]
    fn test_wrong_key_fails() {
        let payload = sample_targz();
        let key1 = derive_license_key(b"secret-1", "artifact-123");
        let key2 = derive_license_key(b"secret-2", "artifact-123");
        let sealed = seal_payload(&payload, &key1).unwrap();
        assert!(unseal_payload(&sealed, &key2).is_err());
    }

    #[test]
    fn test_is_sealed_plain_targz() {
        let payload = sample_targz();
        assert!(!is_sealed(&payload));
    }

    #[test]
    fn test_is_sealed_encrypted() {
        let payload = sample_targz();
        let key = derive_license_key(b"secret", "art-1");
        let sealed = seal_payload(&payload, &key).unwrap();
        assert!(is_sealed(&sealed));
    }

    #[test]
    fn test_different_artifact_ids_produce_different_keys() {
        let secret = b"same-master-secret";
        let key1 = derive_license_key(secret, "artifact-aaa");
        let key2 = derive_license_key(secret, "artifact-bbb");
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_different_secrets_produce_different_keys() {
        let key1 = derive_license_key(b"secret-alpha", "artifact-123");
        let key2 = derive_license_key(b"secret-beta", "artifact-123");
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_unseal_too_short() {
        let key = [0u8; KEY_SIZE];
        assert!(unseal_payload(&[0u8; 10], &key).is_err());
    }

    #[test]
    fn test_is_sealed_empty() {
        assert!(!is_sealed(&[]));
        assert!(!is_sealed(&[0x1f]));
    }

    #[test]
    fn test_large_payload_round_trip() {
        let payload = vec![0x1f, 0x8b, 0x08, 0x00]
            .into_iter()
            .chain(std::iter::repeat(0xAB).take(1_000_000))
            .collect::<Vec<u8>>();
        let key = derive_license_key(b"big-secret", "big-artifact");
        let sealed = seal_payload(&payload, &key).unwrap();
        let unsealed = unseal_payload(&sealed, &key).unwrap();
        assert_eq!(unsealed, payload);
    }

    #[test]
    fn test_deterministic_key_derivation() {
        let key1 = derive_license_key(b"secret", "artifact-123");
        let key2 = derive_license_key(b"secret", "artifact-123");
        assert_eq!(key1, key2);
    }
}
