use std::sync::RwLock;
use std::time::{Duration, Instant};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::NappError;

/// NeboAI's ED25519 public key, embedded at compile time for offline verification.
///
/// Used to verify `.napp` envelopes from bundled resources when the network
/// may not be available (first launch, air-gapped installs).
pub const NEBOAI_PUBLIC_KEY: &[u8; 32] = include_bytes!("../neboai_public_key.bin");

/// Build a `VerifyingKey` from the embedded NeboAI public key.
pub fn builtin_verifying_key() -> Result<VerifyingKey, NappError> {
    VerifyingKey::from_bytes(NEBOAI_PUBLIC_KEY)
        .map_err(|e| NappError::Signing(format!("invalid embedded public key: {}", e)))
}

/// Cached ED25519 public key from NeboAI.
pub struct SigningKeyProvider {
    neboai_url: String,
    key: RwLock<Option<CachedKey>>,
    ttl: Duration,
}

struct CachedKey {
    key: VerifyingKey,
    fetched_at: Instant,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SigningKeyResponse {
    public_key: String, // base64-encoded ED25519 public key
}

impl SigningKeyProvider {
    pub fn new(neboai_url: &str) -> Self {
        Self {
            neboai_url: neboai_url.to_string(),
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
        let url = format!("{}/api/v1/apps/signing-key", self.neboai_url);
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
///
/// This is the exact shape NeboLoop emits (see `internal/packaging/napp.go`
/// `Signatures`). Binary artifacts (plugins, app sidecars) carry
/// `binarySha256` + `binarySignature` + `manifestSignature`; non-binary
/// artifacts (skills, agents, UI-only apps) carry only the `files` map of
/// path → hex SHA256. All fields are optional so one struct covers both.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignaturesFile {
    #[serde(default)]
    pub manifest_signature: Option<String>, // base64-encoded ED25519 signature
    #[serde(default, rename = "binarySha256")]
    pub binary_hash: Option<String>, // hex SHA256 of binary
    #[serde(default)]
    pub binary_signature: Option<String>, // base64-encoded ED25519 signature
    #[serde(default)]
    pub files: std::collections::HashMap<String, String>, // path → hex SHA256
}

/// Verify .napp signatures against NeboAI's signing key.
///
/// NeboLoop emits two shapes (see `internal/marketplace/binary.go`):
/// - Binary artifacts (plugins, app sidecars) carry `manifestSignature` +
///   `binarySha256` + `binarySignature` — full ED25519 verification.
/// - Non-binary artifacts (skills, agents, UI-only apps) carry only the
///   `files` integrity map; their origin is proven by the `.napp` envelope
///   signature, so here we integrity-check each file against its recorded hash.
///
/// `signatures.json` and the non-binary content files are read from the sealed
/// `.napp` at `napp_path` (via `read_napp_entry`, which verifies the envelope),
/// NOT from disk: non-binary content is never extracted (it would be copyable),
/// and reading from the signed archive roots the whole check in the NeboAI
/// envelope signature. `app_dir` is used only for the binary (which must be on
/// disk to execute) and the manifest.
pub fn verify_signatures(
    key: &VerifyingKey,
    app_dir: &std::path::Path,
    napp_path: &std::path::Path,
) -> Result<(), NappError> {
    let sigs_data = crate::reader::read_napp_entry(napp_path, "signatures.json")
        .map_err(|e| NappError::Signing(format!("read signatures.json: {}", e)))?;
    let sigs: SignaturesFile = serde_json::from_slice(&sigs_data)
        .map_err(|e| NappError::Signing(format!("parse signatures.json: {}", e)))?;

    use base64::Engine;

    // 1. Manifest signature — present only for binary artifacts.
    if let Some(manifest_sig_b64) = &sigs.manifest_signature {
        let manifest_data = std::fs::read(app_dir.join("manifest.json"))
            .map_err(|e| NappError::Signing(format!("read manifest: {}", e)))?;
        let manifest_sig_bytes = base64::engine::general_purpose::STANDARD
            .decode(manifest_sig_b64)
            .map_err(|e| NappError::Signing(format!("decode manifest sig: {}", e)))?;
        let manifest_sig = Signature::from_slice(&manifest_sig_bytes)
            .map_err(|e| NappError::Signing(format!("invalid manifest sig: {}", e)))?;
        key.verify(&manifest_data, &manifest_sig)
            .map_err(|_| NappError::Signing("manifest signature verification failed".into()))?;
    }

    // 2. Binary artifact — verify the binary's hash and ED25519 signature.
    if let (Some(expected_hash), Some(binary_sig_b64)) =
        (sigs.binary_hash.as_ref(), sigs.binary_signature.as_ref())
    {
        let binary_path = find_binary(app_dir)?;
        let binary_data = std::fs::read(&binary_path)
            .map_err(|e| NappError::Signing(format!("read binary: {}", e)))?;

        let mut hasher = Sha256::new();
        hasher.update(&binary_data);
        let actual_hash = hex::encode(hasher.finalize());
        if actual_hash != *expected_hash {
            return Err(NappError::Signing(format!(
                "binary hash mismatch: expected {}, got {}",
                expected_hash, actual_hash
            )));
        }

        let binary_sig_bytes = base64::engine::general_purpose::STANDARD
            .decode(binary_sig_b64)
            .map_err(|e| NappError::Signing(format!("decode binary sig: {}", e)))?;
        let binary_sig = Signature::from_slice(&binary_sig_bytes)
            .map_err(|e| NappError::Signing(format!("invalid binary sig: {}", e)))?;
        key.verify(&binary_data, &binary_sig)
            .map_err(|_| NappError::Signing("binary signature verification failed".into()))?;
    } else if !sigs.files.is_empty() {
        // 3. Non-binary artifact — integrity-check each recorded file. Origin
        // authenticity is established by the .napp envelope signature.
        for (rel, expected) in &sigs.files {
            // Read the signed file from inside the .napp — non-binary content is
            // never extracted to disk (it would be copyable); the envelope
            // signature already authenticates these bytes.
            let data = crate::reader::read_napp_entry(napp_path, rel)
                .map_err(|e| NappError::Signing(format!("read {}: {}", rel, e)))?;
            let mut hasher = Sha256::new();
            hasher.update(&data);
            let actual = hex::encode(hasher.finalize());
            if actual != *expected {
                return Err(NappError::Signing(format!(
                    "file hash mismatch for {}: expected {}, got {}",
                    rel, expected, actual
                )));
            }
        }
    }

    info!("signatures verified");
    Ok(())
}

/// Find the binary in an app directory.
///
/// Plugins keep the binary at the root (`binary`/`app`); app sidecars live
/// under `bin/<name>` (see NeboLoop `buildBinaryNappFiles`). The legacy `tmp/`
/// location is still scanned for older installs.
fn find_binary(app_dir: &std::path::Path) -> Result<std::path::PathBuf, NappError> {
    for name in &["binary", "app"] {
        let path = app_dir.join(name);
        if path.exists() {
            return Ok(path);
        }
    }
    // App sidecars: bin/<name>. Legacy installs: tmp/<name>.
    for sub in &["bin", "tmp"] {
        let dir = app_dir.join(sub);
        if let Some(found) = first_executable_in(&dir) {
            return Ok(found);
        }
    }
    Err(NappError::NotFound(
        "no binary found in app directory".into(),
    ))
}

/// Return the first regular, executable file directly inside `dir`, if any.
fn first_executable_in(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    if !dir.is_dir() {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = path.metadata() {
                if meta.permissions().mode() & 0o111 != 0 {
                    return Some(path);
                }
            }
        }
        #[cfg(not(unix))]
        return Some(path);
    }
    None
}

/// Checks NeboAI's revocation list with caching.
pub struct RevocationChecker {
    neboai_url: String,
    cache: RwLock<Option<RevocationCache>>,
    ttl: Duration,
}

struct RevocationCache {
    revoked: std::collections::HashSet<String>,
    fetched_at: Instant,
}

impl RevocationChecker {
    pub fn new(neboai_url: &str) -> Self {
        Self {
            neboai_url: neboai_url.to_string(),
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
        let url = format!("{}/api/v1/apps/revocations", self.neboai_url);
        let resp = reqwest::get(&url).await;

        match resp {
            Ok(r) if r.status().is_success() => {
                #[derive(serde::Deserialize)]
                struct RevocationList {
                    #[serde(default, alias = "revoked")]
                    revocations: Vec<String>,
                }
                let list: RevocationList = r
                    .json()
                    .await
                    .map_err(|e| NappError::Signing(format!("parse revocations: {}", e)))?;

                let set: std::collections::HashSet<String> = list.revocations.into_iter().collect();
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

#[cfg(test)]
mod tests {
    use super::*;

    // The exact shape NeboLoop emits for a binary artifact (plugin / app
    // sidecar) — camelCase keys, see internal/marketplace/binary.go.
    #[test]
    fn parses_neboloop_binary_signatures_shape() {
        let json = r#"{
            "keyId": "abc123",
            "algorithm": "ed25519",
            "binarySha256": "deadbeef",
            "binarySignature": "c2ln",
            "manifestSignature": "bXNpZw=="
        }"#;
        let sigs: SignaturesFile = serde_json::from_str(json).expect("must parse camelCase");
        assert_eq!(sigs.binary_hash.as_deref(), Some("deadbeef"));
        assert_eq!(sigs.binary_signature.as_deref(), Some("c2ln"));
        assert_eq!(sigs.manifest_signature.as_deref(), Some("bXNpZw=="));
        assert!(sigs.files.is_empty());
    }

    // The shape NeboLoop emits for a non-binary artifact (skill / agent /
    // UI-only app) — only the per-file integrity map, no signatures.
    #[test]
    fn parses_neboloop_files_map_shape() {
        let json = r#"{
            "keyId": "abc123",
            "algorithm": "ed25519",
            "files": { "SKILL.md": "aa", "scripts/run.py": "bb" }
        }"#;
        let sigs: SignaturesFile = serde_json::from_str(json).expect("must parse files map");
        assert!(sigs.manifest_signature.is_none());
        assert!(sigs.binary_hash.is_none());
        assert_eq!(sigs.files.get("SKILL.md").map(String::as_str), Some("aa"));
        assert_eq!(sigs.files.len(), 2);
    }

    // A non-binary artifact verifies via the files integrity map, reading both
    // signatures.json and each file from inside the sealed .napp (never disk —
    // non-binary content is not extracted). Matching hashes pass; a file in the
    // archive that doesn't match its recorded hash fails. Origin is proven by
    // the envelope.
    #[test]
    fn files_map_integrity_check_detects_tampering() {
        use flate2::Compression;
        use flate2::write::GzEncoder;

        let dir = std::env::temp_dir().join(format!("nebo-sig-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        // Build a raw tar.gz .napp (gzip magic → read without an envelope) holding
        // signatures.json + SKILL.md, where signatures.json records `recorded_hash`.
        let build_napp = |path: &std::path::Path, skill_body: &[u8], recorded_hash: &str| {
            let sigs = format!(
                r#"{{ "keyId": "k", "algorithm": "ed25519", "files": {{ "SKILL.md": "{recorded_hash}" }} }}"#
            );
            let entries: [(&str, &[u8]); 2] =
                [("signatures.json", sigs.as_bytes()), ("SKILL.md", skill_body)];
            let file = std::fs::File::create(path).unwrap();
            let gz = GzEncoder::new(file, Compression::default());
            let mut builder = tar::Builder::new(gz);
            for (name, data) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_size(data.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder.append_data(&mut header, name, data).unwrap();
            }
            builder.finish().unwrap();
        };

        let body = b"hello world";
        let mut hasher = Sha256::new();
        hasher.update(body);
        let hash = hex::encode(hasher.finalize());

        // Key is unused on the files-map path, but the API requires one.
        let key = builtin_verifying_key().expect("embedded key");

        // Archive content matches the recorded hash → passes.
        let ok_napp = dir.join("ok.napp");
        build_napp(&ok_napp, body, &hash);
        assert!(
            verify_signatures(&key, &dir, &ok_napp).is_ok(),
            "matching hash must pass"
        );

        // Archive holds different bytes than the recorded hash → mismatch → fails.
        let bad_napp = dir.join("bad.napp");
        build_napp(&bad_napp, b"tampered", &hash);
        assert!(
            verify_signatures(&key, &dir, &bad_napp).is_err(),
            "tampered file must fail"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
