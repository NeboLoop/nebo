use std::sync::RwLock;
use std::time::{Duration, Instant};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::NappError;

/// Cached ED25519 public key from NeboLoop.
pub struct SigningKeyProvider {
    neboloop_url: String,
    key: RwLock<Option<CachedKey>>,
    ttl: Duration,
}

struct CachedKey {
    key: VerifyingKey,
    fetched_at: Instant,
}

#[derive(serde::Deserialize)]
struct SigningKeyResponse {
    public_key: String, // base64-encoded ED25519 public key
}

impl SigningKeyProvider {
    pub fn new(neboloop_url: &str) -> Self {
        Self {
            neboloop_url: neboloop_url.to_string(),
            key: RwLock::new(None),
            ttl: Duration::from_secs(86400), // 24 hours
        }
    }

    /// Get the signing key (cached for 24h).
    pub async fn get_key(&self) -> Result<VerifyingKey, NappError> {
        // Check cache
        {
            let cache = self.key.read().unwrap();
            if let Some(ref cached) = *cache {
                if cached.fetched_at.elapsed() < self.ttl {
                    return Ok(cached.key);
                }
            }
        }

        self.refresh().await
    }

    /// Force refresh the signing key.
    pub async fn refresh(&self) -> Result<VerifyingKey, NappError> {
        let url = format!("{}/api/v1/apps/signing-key", self.neboloop_url);
        let resp: SigningKeyResponse = reqwest::get(&url)
            .await
            .map_err(|e| NappError::Signing(format!("fetch signing key: {}", e)))?
            .json()
            .await
            .map_err(|e| NappError::Signing(format!("parse signing key: {}", e)))?;

        use base64::Engine;
        let key_bytes = base64::engine::general_purpose::STANDARD
            .decode(&resp.public_key)
            .map_err(|e| NappError::Signing(format!("decode key: {}", e)))?;

        let key_array: [u8; 32] = key_bytes
            .try_into()
            .map_err(|_| NappError::Signing("invalid key length".into()))?;

        let key = VerifyingKey::from_bytes(&key_array)
            .map_err(|e| NappError::Signing(format!("invalid key: {}", e)))?;

        let mut cache = self.key.write().unwrap();
        *cache = Some(CachedKey {
            key,
            fetched_at: Instant::now(),
        });

        info!("refreshed signing key");
        Ok(key)
    }
}

/// Signatures file (signatures.json in .napp).
#[derive(serde::Deserialize)]
pub struct SignaturesFile {
    pub manifest_signature: String, // base64-encoded ED25519 signature
    pub binary_hash: String,        // hex SHA256 of binary
    pub binary_signature: String,   // base64-encoded ED25519 signature
}

/// Verify .napp signatures.
pub fn verify_signatures(
    key: &VerifyingKey,
    app_dir: &std::path::Path,
) -> Result<(), NappError> {
    let sigs_path = app_dir.join("signatures.json");
    let sigs_data = std::fs::read_to_string(&sigs_path)
        .map_err(|e| NappError::Signing(format!("read signatures.json: {}", e)))?;
    let sigs: SignaturesFile = serde_json::from_str(&sigs_data)
        .map_err(|e| NappError::Signing(format!("parse signatures.json: {}", e)))?;

    use base64::Engine;

    // 1. Verify manifest signature
    let manifest_data = std::fs::read(app_dir.join("manifest.json"))
        .map_err(|e| NappError::Signing(format!("read manifest: {}", e)))?;
    let manifest_sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(&sigs.manifest_signature)
        .map_err(|e| NappError::Signing(format!("decode manifest sig: {}", e)))?;
    let manifest_sig = Signature::from_slice(&manifest_sig_bytes)
        .map_err(|e| NappError::Signing(format!("invalid manifest sig: {}", e)))?;
    key.verify(&manifest_data, &manifest_sig)
        .map_err(|_| NappError::Signing("manifest signature verification failed".into()))?;

    // 2. Find and verify binary
    let binary_path = find_binary(app_dir)?;
    let binary_data = std::fs::read(&binary_path)
        .map_err(|e| NappError::Signing(format!("read binary: {}", e)))?;

    // Verify hash
    let mut hasher = Sha256::new();
    hasher.update(&binary_data);
    let actual_hash = hex::encode(hasher.finalize());
    if actual_hash != sigs.binary_hash {
        return Err(NappError::Signing(format!(
            "binary hash mismatch: expected {}, got {}",
            sigs.binary_hash, actual_hash
        )));
    }

    // Verify binary signature
    let binary_sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(&sigs.binary_signature)
        .map_err(|e| NappError::Signing(format!("decode binary sig: {}", e)))?;
    let binary_sig = Signature::from_slice(&binary_sig_bytes)
        .map_err(|e| NappError::Signing(format!("invalid binary sig: {}", e)))?;
    key.verify(&binary_data, &binary_sig)
        .map_err(|_| NappError::Signing("binary signature verification failed".into()))?;

    info!("signatures verified");
    Ok(())
}

/// Find the binary in an app directory.
fn find_binary(app_dir: &std::path::Path) -> Result<std::path::PathBuf, NappError> {
    for name in &["binary", "app"] {
        let path = app_dir.join(name);
        if path.exists() {
            return Ok(path);
        }
    }
    // Check tmp/ directory
    let tmp = app_dir.join("tmp");
    if tmp.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&tmp) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if let Ok(meta) = path.metadata() {
                            if meta.permissions().mode() & 0o111 != 0 {
                                return Ok(path);
                            }
                        }
                    }
                    #[cfg(not(unix))]
                    return Ok(path);
                }
            }
        }
    }
    Err(NappError::NotFound("no binary found in app directory".into()))
}

/// Checks NeboLoop's revocation list with caching.
pub struct RevocationChecker {
    neboloop_url: String,
    cache: RwLock<Option<RevocationCache>>,
    ttl: Duration,
}

struct RevocationCache {
    revoked: std::collections::HashSet<String>,
    fetched_at: Instant,
}

impl RevocationChecker {
    pub fn new(neboloop_url: &str) -> Self {
        Self {
            neboloop_url: neboloop_url.to_string(),
            cache: RwLock::new(None),
            ttl: Duration::from_secs(3600), // 1 hour
        }
    }

    /// Check if an app has been revoked.
    pub async fn is_revoked(&self, app_id: &str) -> Result<bool, NappError> {
        // Check cache
        {
            let cache = self.cache.read().unwrap();
            if let Some(ref c) = *cache {
                if c.fetched_at.elapsed() < self.ttl {
                    return Ok(c.revoked.contains(app_id));
                }
            }
        }

        // Fetch fresh
        let url = format!("{}/api/v1/apps/revocations", self.neboloop_url);
        let resp = reqwest::get(&url).await;

        match resp {
            Ok(r) if r.status().is_success() => {
                #[derive(serde::Deserialize)]
                struct RevocationList {
                    #[serde(default)]
                    revoked: Vec<String>,
                }
                let list: RevocationList = r.json().await
                    .map_err(|e| NappError::Signing(format!("parse revocations: {}", e)))?;

                let set: std::collections::HashSet<String> = list.revoked.into_iter().collect();
                let is_revoked = set.contains(app_id);

                let mut cache = self.cache.write().unwrap();
                *cache = Some(RevocationCache {
                    revoked: set,
                    fetched_at: Instant::now(),
                });

                Ok(is_revoked)
            }
            Ok(r) => {
                warn!(status = %r.status(), "revocation check failed");
                Ok(false) // Fail open
            }
            Err(e) => {
                warn!(error = %e, "revocation check failed");
                Ok(false) // Fail open
            }
        }
    }
}
