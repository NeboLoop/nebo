//! Bundle management — downloads, caches, and verifies VM rootfs images.
//!
//! Mirrors the Cowork pattern:
//! ```text
//! Bundle directory: ~/.nebo/vm/bundles/
//!   rootfs.img              ← Linux root filesystem (downloaded per version SHA)
//!   .rootfs.img.origin      ← SHA version tracker
//!   rootfs.img.zst          ← Compressed cache for faster reinstalls
//!   sessiondata.img         ← Persistent user data (survives reinstalls)
//!   .auto_reinstall_attempted ← Marker (present during reinstall cycle)
//!
//! Download URL pattern:
//!   https://cdn.neboai.com/vm/{arch}/{sha}/rootfs.img.zst
//!
//! Resolution order:
//!   1. Local rootfs.img exists and SHA matches → use it
//!   2. Local .zst cache exists → decompress + verify
//!   3. CDN download with streaming zstd decompression + SHA-256 verify
//! ```

use crate::error::{VmError, VmResult};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Base URL for VM image downloads.
const CDN_BASE: &str = "https://cdn.neboai.com/vm";

/// Bundle directory layout.
pub struct Bundle {
    /// Root of the bundle directory (~/.nebo/vm/bundles/).
    dir: PathBuf,
    /// Expected SHA-256 of the rootfs image.
    expected_sha: String,
    /// Architecture: "arm64" or "x64".
    arch: String,
}

/// Current state of the rootfs in the bundle.
#[derive(Debug, PartialEq, Eq)]
pub enum BundleState {
    /// rootfs.img present and SHA matches.
    Ready,
    /// Compressed cache available, needs decompression.
    Cached,
    /// Nothing local, needs CDN download.
    NeedsDownload,
    /// rootfs.img present but SHA mismatch (stale version).
    Stale,
}

impl Bundle {
    /// Create a bundle manager for the given version SHA.
    pub fn new(expected_sha: &str) -> VmResult<Self> {
        let dir = bundle_dir()?;
        let arch = if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "x64"
        };

        Ok(Self {
            dir,
            expected_sha: expected_sha.to_string(),
            arch: arch.to_string(),
        })
    }

    /// Create a bundle manager with a custom directory (for testing / dev).
    pub fn with_dir(dir: PathBuf, expected_sha: &str) -> Self {
        let arch = if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "x64"
        };
        Self {
            dir,
            expected_sha: expected_sha.to_string(),
            arch: arch.to_string(),
        }
    }

    /// Path to the rootfs disk image.
    pub fn rootfs_path(&self) -> PathBuf {
        self.dir.join("rootfs.img")
    }

    /// Path to the compressed cache.
    fn cache_path(&self) -> PathBuf {
        self.dir.join("rootfs.img.zst")
    }

    /// Path to the SHA origin tracker.
    fn origin_path(&self) -> PathBuf {
        self.dir.join(".rootfs.img.origin")
    }

    /// Path to the auto-reinstall marker.
    fn reinstall_marker(&self) -> PathBuf {
        self.dir.join(".auto_reinstall_attempted")
    }

    /// Path to session data (persists across rootfs updates).
    pub fn sessiondata_path(&self) -> PathBuf {
        self.dir.join("sessiondata.img")
    }

    /// Check current state of the bundle.
    pub fn state(&self) -> BundleState {
        let rootfs = self.rootfs_path();
        let cache = self.cache_path();

        if rootfs.exists() {
            // Check if SHA matches
            if self.origin_matches() {
                BundleState::Ready
            } else {
                BundleState::Stale
            }
        } else if cache.exists() {
            BundleState::Cached
        } else {
            BundleState::NeedsDownload
        }
    }

    /// Ensure the rootfs is available and verified. Downloads if needed.
    pub async fn ensure_rootfs(&self) -> VmResult<PathBuf> {
        std::fs::create_dir_all(&self.dir)?;

        match self.state() {
            BundleState::Ready => {
                info!(path = %self.rootfs_path().display(), "rootfs ready");
                Ok(self.rootfs_path())
            }
            BundleState::Cached => {
                info!("decompressing cached rootfs.img.zst");
                self.decompress_cache().await?;
                self.verify_sha()?;
                self.write_origin()?;
                Ok(self.rootfs_path())
            }
            BundleState::Stale => {
                info!(
                    expected = %self.expected_sha,
                    "rootfs SHA mismatch, re-downloading"
                );
                // Keep old .zst cache, download new version
                self.download_from_cdn().await?;
                self.verify_sha()?;
                self.write_origin()?;
                Ok(self.rootfs_path())
            }
            BundleState::NeedsDownload => {
                info!("rootfs not found, downloading from CDN");
                self.download_from_cdn().await?;
                self.verify_sha()?;
                self.write_origin()?;
                Ok(self.rootfs_path())
            }
        }
    }

    /// Download rootfs from CDN with streaming zstd decompression.
    async fn download_from_cdn(&self) -> VmResult<()> {
        let url = format!(
            "{}/{}/{}/rootfs.img.zst",
            CDN_BASE, self.arch, self.expected_sha
        );
        info!(url = %url, "downloading rootfs from CDN");

        let response = reqwest::get(&url).await.map_err(|e| {
            VmError::ImageNotFound(format!("CDN download failed: {e}"))
        })?;

        if !response.status().is_success() {
            return Err(VmError::ImageNotFound(format!(
                "CDN returned HTTP {}",
                response.status()
            )));
        }

        // Stream to .zst file first (keeps cache for reinstalls)
        let cache_path = self.cache_path();
        let bytes = response.bytes().await.map_err(|e| {
            VmError::TransferFailed(format!("download stream error: {e}"))
        })?;
        std::fs::write(&cache_path, &bytes)?;
        info!(
            size_mb = bytes.len() / (1024 * 1024),
            path = %cache_path.display(),
            "cached compressed rootfs"
        );

        // Decompress .zst → rootfs.img
        self.decompress_cache().await
    }

    /// Decompress rootfs.img.zst → rootfs.img.
    async fn decompress_cache(&self) -> VmResult<()> {
        let cache_path = self.cache_path();
        let rootfs_path = self.rootfs_path();

        let compressed = std::fs::read(&cache_path)?;
        let decompressed = zstd::decode_all(compressed.as_slice()).map_err(|e| {
            VmError::TransferFailed(format!("zstd decompression failed: {e}"))
        })?;

        std::fs::write(&rootfs_path, &decompressed)?;
        info!(
            compressed_mb = compressed.len() / (1024 * 1024),
            decompressed_mb = decompressed.len() / (1024 * 1024),
            "decompressed rootfs"
        );

        Ok(())
    }

    /// Verify rootfs.img SHA-256 matches expected.
    fn verify_sha(&self) -> VmResult<()> {
        let rootfs_path = self.rootfs_path();
        let actual_sha = sha256_file(&rootfs_path)?;

        if actual_sha != self.expected_sha {
            // Delete the bad image
            let _ = std::fs::remove_file(&rootfs_path);
            return Err(VmError::TransferFailed(format!(
                "SHA-256 mismatch: expected {} got {}",
                self.expected_sha, actual_sha
            )));
        }

        info!(sha = %actual_sha, "rootfs SHA-256 verified");
        Ok(())
    }

    /// Write the origin file recording which SHA this rootfs belongs to.
    fn write_origin(&self) -> VmResult<()> {
        std::fs::write(self.origin_path(), &self.expected_sha)?;
        Ok(())
    }

    /// Check if the origin file matches the expected SHA.
    fn origin_matches(&self) -> bool {
        match std::fs::read_to_string(self.origin_path()) {
            Ok(sha) => sha.trim() == self.expected_sha,
            Err(_) => false,
        }
    }

    /// Attempt self-healing: delete rootfs and markers, re-download on next boot.
    pub fn attempt_reinstall(&self) -> VmResult<()> {
        if self.reinstall_marker().exists() {
            warn!("auto-reinstall already attempted, skipping");
            return Err(VmError::TransferFailed(
                "auto-reinstall already attempted this session".to_string(),
            ));
        }

        // Write marker to prevent infinite loops
        std::fs::write(self.reinstall_marker(), "1")?;

        // Delete rootfs + origin (preserve sessiondata.img)
        let _ = std::fs::remove_file(self.rootfs_path());
        let _ = std::fs::remove_file(self.origin_path());

        info!("cleared rootfs for reinstall (sessiondata preserved)");
        Ok(())
    }

    /// Clear the reinstall marker after a successful boot.
    pub fn clear_reinstall_marker(&self) {
        let _ = std::fs::remove_file(self.reinstall_marker());
    }
}

/// Default bundle directory: ~/.nebo/vm/bundles/
fn bundle_dir() -> VmResult<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| VmError::ImageNotFound("cannot determine home directory".to_string()))?;
    Ok(home.join(".nebo").join("vm").join("bundles"))
}

/// Compute SHA-256 of a file.
fn sha256_file(path: &Path) -> VmResult<String> {
    let bytes = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let hash = hasher.finalize();
    Ok(hex::encode(hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Published SHA-256 test vector for the ASCII string "abc".
    const SHA_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    fn bundle_in(dir: &Path, sha: &str) -> Bundle {
        Bundle::with_dir(dir.to_path_buf(), sha)
    }

    /// INVARIANT: sha256_file computes the canonical SHA-256 hex digest
    /// (locked against the published test vector for "abc").
    #[test]
    fn sha256_file_matches_known_vector() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        std::fs::write(&p, "abc").unwrap();
        assert_eq!(sha256_file(&p).unwrap(), SHA_ABC);
    }

    /// INVARIANT: the bundle file layout is fixed — rootfs.img, sessiondata.img,
    /// rootfs.img.zst, and the dot-file trackers live directly in the bundle dir.
    #[test]
    fn bundle_paths_are_stable() {
        let dir = tempfile::tempdir().unwrap();
        let b = bundle_in(dir.path(), "sha1");
        assert_eq!(b.rootfs_path(), dir.path().join("rootfs.img"));
        assert_eq!(b.sessiondata_path(), dir.path().join("sessiondata.img"));
        assert_eq!(b.cache_path(), dir.path().join("rootfs.img.zst"));
        assert_eq!(b.origin_path(), dir.path().join(".rootfs.img.origin"));
    }

    /// INVARIANT: state resolution order — a present rootfs always wins over the
    /// cache (Ready when origin matches, Stale otherwise, even with a .zst
    /// present); cache-only is Cached; nothing local is NeedsDownload.
    #[test]
    fn state_resolution_order() {
        let dir = tempfile::tempdir().unwrap();
        let b = bundle_in(dir.path(), "sha1");

        assert_eq!(b.state(), BundleState::NeedsDownload);

        std::fs::write(b.cache_path(), "zst").unwrap();
        assert_eq!(b.state(), BundleState::Cached);

        // rootfs with no origin tracker: Stale, even though the cache exists
        std::fs::write(b.rootfs_path(), "img").unwrap();
        assert_eq!(b.state(), BundleState::Stale);

        std::fs::write(b.origin_path(), "sha1").unwrap();
        assert_eq!(b.state(), BundleState::Ready);

        std::fs::write(b.origin_path(), "other-sha").unwrap();
        assert_eq!(b.state(), BundleState::Stale);
    }

    /// INVARIANT: origin comparison trims whitespace so a trailing newline never
    /// forces a spurious re-download.
    #[test]
    fn origin_trims_whitespace() {
        let dir = tempfile::tempdir().unwrap();
        let b = bundle_in(dir.path(), "sha1");
        std::fs::write(b.rootfs_path(), "img").unwrap();
        std::fs::write(b.origin_path(), "sha1\n").unwrap();
        assert_eq!(b.state(), BundleState::Ready);
    }

    /// INVARIANT: attempt_reinstall clears rootfs + origin but PRESERVES
    /// sessiondata.img, refuses to run a second time while the marker stands,
    /// and runs again once the marker is cleared.
    #[test]
    fn reinstall_preserves_sessiondata_and_runs_once() {
        let dir = tempfile::tempdir().unwrap();
        let b = bundle_in(dir.path(), "sha1");
        std::fs::write(b.rootfs_path(), "img").unwrap();
        std::fs::write(b.origin_path(), "sha1").unwrap();
        std::fs::write(b.sessiondata_path(), "userdata").unwrap();

        b.attempt_reinstall().unwrap();
        assert!(!b.rootfs_path().exists());
        assert!(!b.origin_path().exists());
        assert!(b.sessiondata_path().exists(), "sessiondata must survive reinstall");
        assert_eq!(b.state(), BundleState::NeedsDownload);

        // Second attempt is blocked by the marker (infinite-loop guard)
        assert!(b.attempt_reinstall().is_err());

        // Clearing the marker (successful boot) re-arms self-healing
        b.clear_reinstall_marker();
        assert!(b.attempt_reinstall().is_ok());
    }
}
