mod apply;

use std::borrow::Cow;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tracing::warn;

const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Where a binary's releases live and how they are verified.
///
/// `{base_url}/version.json` is the pointer; `{base_url}/{tag}/{file}` holds the
/// release files: the per-platform binaries (`{binary}-{os}-{arch}[.exe]`) and
/// the checksums file (`sha256sum` format).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feed {
    /// Release root on the CDN, without a trailing slash.
    pub base_url: Cow<'static, str>,
    /// Binary name, the prefix of every per-platform asset.
    pub binary: Cow<'static, str>,
    /// Checksums file name inside each tag directory.
    pub checksums: Cow<'static, str>,
    /// Raw ed25519 public key. When set, `{checksums}.sig` (base64 of the
    /// 64-byte signature over the exact checksums bytes) must verify before any
    /// hash in the checksums file is trusted.
    pub signing_key: Option<[u8; 32]>,
}

/// Nebo's own release feed.
pub const NEBO: Feed = Feed {
    base_url: Cow::Borrowed("https://cdn.neboai.com/releases"),
    binary: Cow::Borrowed("nebo"),
    checksums: Cow::Borrowed("checksums.txt"),
    signing_key: None,
};

/// What [`apply_update`] does after the new binary is in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyMode {
    /// Restart as the new version: `execve` in place on Unix (same PID, so a
    /// service manager keeps supervising it), spawn-and-exit on Windows, and a
    /// deferred helper swap for app bundles. Does not return on success.
    Restart,
    /// Replace the binary on disk and return. The running process keeps the old
    /// version; the caller restarts whatever runs the binary. Direct installs only.
    ReplaceOnly,
}

/// Outcome of an update check.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    pub available: bool,
    pub current_version: String,
    pub latest_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    /// How Nebo was installed (direct, homebrew, package_manager).
    pub install_method: String,
    /// Whether the updater can auto-update (only "direct" installs).
    pub can_auto_update: bool,
}

#[derive(Debug, Deserialize)]
struct VersionManifest {
    version: String,
    release_url: Option<String>,
    published_at: Option<String>,
    /// Newest tag that has assets for each platform ("darwin", "linux",
    /// "windows"). A platform can lag the others (Windows while signing is
    /// blocked); this client follows its own platform's entry and falls back
    /// to `version` for pointers written before the field existed.
    #[serde(default)]
    platforms: std::collections::HashMap<String, String>,
}

impl VersionManifest {
    fn latest_for_this_platform(&self) -> String {
        let key = match std::env::consts::OS {
            "macos" => "darwin",
            other => other,
        };
        self.platforms
            .get(key)
            .filter(|v| !v.is_empty())
            .cloned()
            .unwrap_or_else(|| self.version.clone())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

/// Check the feed for a newer version.
pub async fn check(feed: &Feed, current_version: &str) -> Result<CheckResult, UpdateError> {
    let client = tls::http_client()
        .timeout(CHECK_TIMEOUT)
        .user_agent(format!("{}/{}", feed.binary, current_version))
        .build()?;

    let url = format!("{}/version.json", feed.base_url);
    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        return Err(UpdateError::Other(format!(
            "version check returned {}",
            resp.status()
        )));
    }

    let manifest: VersionManifest = resp.json().await?;
    let latest_tag = manifest.latest_for_this_platform();
    let latest = normalize_version(&latest_tag);
    let current = normalize_version(current_version);

    let available = latest != current && current != "dev" && is_newer(&latest, &current);
    let method = detect_install_method();

    Ok(CheckResult {
        available,
        current_version: current_version.to_string(),
        latest_version: latest_tag,
        release_url: manifest.release_url,
        published_at: manifest.published_at,
        install_method: method.to_string(),
        can_auto_update: method == "direct" || method == "app_bundle",
    })
}

/// Detect how Nebo was installed.
pub fn detect_install_method() -> &'static str {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(_) => return "direct",
    };
    let resolved = std::fs::canonicalize(&exe).unwrap_or(exe);
    let path = resolved.to_string_lossy().to_string();

    // Desktop app bundles — must update via installer, not raw binary replacement.
    // macOS: Nebo.app/Contents/MacOS/nebo
    // Windows: tauri puts the exe in a versioned dir or Program Files
    // Linux: AppImage or .deb-installed paths
    #[cfg(target_os = "macos")]
    if path.contains(".app/Contents/MacOS/") {
        return "app_bundle";
    }

    #[cfg(target_os = "windows")]
    if path.contains("\\Nebo\\") || path.contains("\\WindowsApps\\") {
        return "app_bundle";
    }

    if path.contains("/opt/homebrew/") || path.contains("/usr/local/Cellar/") {
        return "homebrew";
    }

    #[cfg(target_os = "linux")]
    {
        // Running inside an AppImage — the runtime sets $APPIMAGE to the .AppImage path.
        // Update via the bundled installer (single-file swap), not raw binary replacement.
        if std::env::var_os("APPIMAGE").is_some() {
            return "app_bundle";
        }

        if std::process::Command::new("dpkg")
            .args(["-S", &path])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return "package_manager";
        }
    }

    "direct"
}

/// Get the platform-specific asset name for downloads.
///
/// Maps Rust's `std::env::consts` values to CDN naming convention
/// (e.g. `macos` → `darwin`, `aarch64` → `arm64`, `x86_64` → `amd64`).
pub fn asset_name(feed: &Feed) -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => other,
    };
    if os == "windows" {
        format!("{}-{}-{}.exe", feed.binary, os, arch)
    } else {
        format!("{}-{}-{}", feed.binary, os, arch)
    }
}

/// CDN architecture label for the current platform.
fn cdn_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => other,
    }
}

/// Get the platform-specific app bundle asset name (DMG, MSI, AppImage).
pub fn bundle_asset_name(version: &str) -> Option<String> {
    let pkg_version = version.trim_start_matches('v');
    let arch = cdn_arch();
    match std::env::consts::OS {
        "macos" => Some(format!("Nebo-{}-{}.dmg", pkg_version, arch)),
        // CI builds an NSIS installer named `Nebo-{ver}-setup.exe` (no arch component).
        "windows" => Some(format!("Nebo-{}-setup.exe", pkg_version)),
        "linux" => Some(format!("Nebo-{}-{}.AppImage", pkg_version, arch)),
        _ => None,
    }
}

/// Progress callback: (downloaded_bytes, total_bytes).
pub type ProgressFn = Box<dyn Fn(u64, u64) + Send>;

/// Download the release asset for the given tag to a temp file.
///
/// For headless/CLI installs, downloads the raw binary.
/// For app bundles, downloads the platform installer (DMG, MSI, AppImage).
pub async fn download(
    feed: &Feed,
    tag: &str,
    progress: Option<ProgressFn>,
) -> Result<std::path::PathBuf, UpdateError> {
    let method = detect_install_method();
    let asset = if method == "app_bundle" {
        bundle_asset_name(tag)
            .ok_or_else(|| UpdateError::Other("no app bundle available for this platform".into()))?
    } else {
        asset_name(feed)
    };
    let url = format!("{}/{}/{}", feed.base_url, tag, asset);

    let client = tls::http_client()
        .timeout(Duration::from_secs(600))
        .user_agent("nebo-updater")
        .build()?;

    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        return Err(UpdateError::Other(format!(
            "download returned {}",
            resp.status()
        )));
    }

    let total = resp.content_length().unwrap_or(0);
    let tmp_path =
        std::env::temp_dir().join(format!("{}-update-{}", feed.binary, uuid::Uuid::new_v4()));

    let mut file = tokio::fs::File::create(&tmp_path).await?;
    let mut downloaded: u64 = 0;
    let mut stream = resp.bytes_stream();

    use futures::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        if let Some(ref cb) = progress {
            cb(downloaded, total);
        }
    }
    file.flush().await?;
    drop(file);

    // Make executable on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o755))?;
    }

    Ok(tmp_path)
}

/// Verify the SHA256 of the downloaded file against the feed's checksums file.
/// Automatically uses the correct asset name (bare binary for direct, DMG/MSI for app_bundle).
///
/// When the feed has a `signing_key`, the checksums file's ed25519 signature
/// (`{checksums}.sig`) is verified first and the hash is read from those same
/// verified bytes. Every failure is fatal: nothing unverified is ever applied.
pub async fn verify_checksum(
    feed: &Feed,
    binary_path: &std::path::Path,
    tag: &str,
) -> Result<(), UpdateError> {
    let method = detect_install_method();
    let asset = if method == "app_bundle" {
        bundle_asset_name(tag).unwrap_or_else(|| asset_name(feed))
    } else {
        asset_name(feed)
    };

    let client = tls::http_client()
        .timeout(Duration::from_secs(30))
        .user_agent("nebo-updater")
        .build()?;

    let sums_url = format!("{}/{}/{}", feed.base_url, tag, feed.checksums);
    let sums = fetch_release_file(&client, &sums_url, &feed.checksums).await?;

    if let Some(key) = feed.signing_key {
        let sig_name = format!("{}.sig", feed.checksums);
        let sig_url = format!("{}/{}/{}", feed.base_url, tag, sig_name);
        let sig = fetch_release_file(&client, &sig_url, &sig_name).await?;
        verify_signature(&key, &sums, &sig, &sig_name)?;
    }

    let body = std::str::from_utf8(&sums)
        .map_err(|_| UpdateError::Other(format!("{} is not text", feed.checksums)))?;

    let expected = body
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 && parts[1] == asset {
                Some(parts[0].to_string())
            } else {
                None
            }
        })
        .next()
        .ok_or_else(|| {
            UpdateError::Other(format!("asset {} not found in {}", asset, feed.checksums))
        })?;

    let data = std::fs::read(binary_path)?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let actual = hex::encode(hasher.finalize());

    if !actual.eq_ignore_ascii_case(&expected) {
        return Err(UpdateError::Other(format!(
            "checksum mismatch: expected {}, got {}",
            expected, actual
        )));
    }

    Ok(())
}

/// Fetch one release file. Fail closed: a missing checksums or signature file
/// must NOT silently skip verification.
async fn fetch_release_file(
    client: &reqwest::Client,
    url: &str,
    name: &str,
) -> Result<Vec<u8>, UpdateError> {
    let resp = client.get(url).send().await?;
    if !resp.status().is_success() {
        return Err(UpdateError::Other(format!(
            "{} returned {}",
            name,
            resp.status()
        )));
    }
    Ok(resp.bytes().await?.to_vec())
}

/// Verify a base64 ed25519 signature (Nebo's napp scheme) over `data`.
fn verify_signature(
    key: &[u8; 32],
    data: &[u8],
    sig_b64: &[u8],
    sig_name: &str,
) -> Result<(), UpdateError> {
    use base64::Engine;
    let bad = |why: &str| UpdateError::Other(format!("{} {}", sig_name, why));
    let key = ed25519_dalek::VerifyingKey::from_bytes(key)
        .map_err(|_| UpdateError::Other("release signing key is invalid".into()))?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(sig_b64.trim_ascii())
        .map_err(|_| bad("is not base64"))?;
    let sig = ed25519_dalek::Signature::from_slice(&raw).map_err(|_| bad("is malformed"))?;
    key.verify_strict(data, &sig)
        .map_err(|_| bad("does not match: the release was not signed by the release key"))
}

/// Apply the update: replace the current binary, then restart or not per `mode`.
///
/// `data_dir` is where the deferred helper writes `UPDATE_FAILED.json` on rollback,
/// so the restored app can surface an error to the user on next startup.
pub fn apply_update(
    new_binary_path: &std::path::Path,
    data_dir: &std::path::Path,
    mode: ApplyMode,
) -> Result<(), UpdateError> {
    apply::apply(new_binary_path, data_dir, mode)
}

/// Register a pre-apply hook (called before process restart).
pub fn set_pre_apply_hook(f: Box<dyn Fn() + Send>) {
    apply::set_pre_apply_hook(f);
}

/// Periodically checks for updates in the background.
pub struct BackgroundChecker {
    feed: Feed,
    version: String,
    interval: Duration,
    notify: Box<dyn Fn(CheckResult) + Send + Sync>,
    last_notified: Mutex<Option<String>>,
}

impl BackgroundChecker {
    pub fn new(
        feed: Feed,
        version: String,
        interval: Duration,
        notify: impl Fn(CheckResult) + Send + Sync + 'static,
    ) -> Self {
        Self {
            feed,
            version,
            interval,
            notify: Box::new(notify),
            last_notified: Mutex::new(None),
        }
    }

    /// Run the periodic check loop. Blocks until the token is cancelled.
    pub async fn run(&self, cancel: tokio_util::sync::CancellationToken) {
        // Initial delay: let the app boot
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(30)) => {},
            _ = cancel.cancelled() => return,
        }

        self.check_once().await;

        let mut interval = tokio::time::interval(self.interval);
        interval.tick().await; // consume immediate tick
        loop {
            tokio::select! {
                _ = interval.tick() => self.check_once().await,
                _ = cancel.cancelled() => return,
            }
        }
    }

    async fn check_once(&self) {
        match check(&self.feed, &self.version).await {
            Ok(result) if result.available => {
                let mut last = self.last_notified.lock().unwrap();
                if last.as_deref() == Some(&result.latest_version) {
                    return;
                }
                *last = Some(result.latest_version.clone());
                drop(last);
                (self.notify)(result);
            }
            Ok(_) => {}
            Err(e) => {
                warn!("update check failed: {}", e);
            }
        }
    }
}

fn normalize_version(v: &str) -> String {
    v.trim().trim_start_matches('v').to_string()
}

fn is_newer(latest: &str, current: &str) -> bool {
    let l = split_version(latest);
    let c = split_version(current);
    for i in 0..3 {
        if l[i] > c[i] {
            return true;
        }
        if l[i] < c[i] {
            return false;
        }
    }
    false
}

fn split_version(v: &str) -> [u32; 3] {
    let mut parts = [0u32; 3];
    for (i, s) in v.splitn(3, '.').enumerate() {
        if i < 3 {
            parts[i] = s.parse().unwrap_or(0);
        }
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_newer() {
        assert!(is_newer("1.2.3", "1.2.2"));
        assert!(is_newer("2.0.0", "1.9.9"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("0.9.0", "1.0.0"));
    }

    #[test]
    fn test_normalize_version() {
        assert_eq!(normalize_version("v1.2.3"), "1.2.3");
        assert_eq!(normalize_version("1.2.3"), "1.2.3");
        assert_eq!(normalize_version(" v0.1.0 "), "0.1.0");
    }

    #[test]
    fn test_asset_name() {
        let name = asset_name(&NEBO);
        assert!(
            name.starts_with("nebo-"),
            "expected nebo- prefix, got {}",
            name
        );

        // Verify CDN naming convention: darwin (not macos), arm64/amd64 (not aarch64/x86_64)
        assert!(
            !name.contains("macos"),
            "should use 'darwin' not 'macos': {}",
            name
        );
        assert!(
            !name.contains("aarch64"),
            "should use 'arm64' not 'aarch64': {}",
            name
        );
        assert!(
            !name.contains("x86_64"),
            "should use 'amd64' not 'x86_64': {}",
            name
        );

        #[cfg(target_os = "macos")]
        assert!(
            name.contains("darwin"),
            "macOS should map to darwin: {}",
            name
        );

        #[cfg(target_arch = "aarch64")]
        assert!(
            name.contains("arm64"),
            "aarch64 should map to arm64: {}",
            name
        );

        #[cfg(target_arch = "x86_64")]
        assert!(
            name.contains("amd64"),
            "x86_64 should map to amd64: {}",
            name
        );

        #[cfg(target_os = "windows")]
        assert!(
            name.ends_with(".exe"),
            "windows binary should have .exe: {}",
            name
        );

        #[cfg(not(target_os = "windows"))]
        assert!(
            !name.ends_with(".exe"),
            "non-windows binary should not have .exe: {}",
            name
        );
    }

    #[test]
    fn test_detect_install_method() {
        let method = detect_install_method();
        assert!(["direct", "homebrew", "package_manager", "app_bundle"].contains(&method));
    }

    #[test]
    fn test_bundle_asset_name() {
        // Strips a leading `v` from the tag.
        let mac = bundle_asset_name("v1.2.3");
        #[cfg(target_os = "macos")]
        assert_eq!(mac, Some(format!("Nebo-1.2.3-{}.dmg", cdn_arch())));

        // Windows must match the CI artifact exactly: NSIS `-setup.exe`, no arch, not `.msi`.
        #[cfg(target_os = "windows")]
        {
            let win = bundle_asset_name("v1.2.3").unwrap();
            assert_eq!(win, "Nebo-1.2.3-setup.exe");
            assert!(!win.contains(".msi"), "must not be an MSI: {}", win);
            assert!(
                !win.contains("amd64") && !win.contains("arm64"),
                "windows installer name carries no arch: {}",
                win
            );
        }

        #[cfg(target_os = "linux")]
        assert_eq!(
            bundle_asset_name("1.2.3"),
            Some(format!("Nebo-1.2.3-{}.AppImage", cdn_arch()))
        );

        let _ = mac;
    }

    #[test]
    fn nebo_feed_is_nebos_release_feed() {
        assert_eq!(NEBO.base_url, "https://cdn.neboai.com/releases");
        assert_eq!(NEBO.checksums, "checksums.txt");
        assert!(NEBO.signing_key.is_none());
        let link = Feed {
            base_url: "https://cdn.example.com/x".into(),
            binary: "nebo-link".into(),
            checksums: "SHA256SUMS".into(),
            signing_key: None,
        };
        assert!(asset_name(&link).starts_with("nebo-link-"));
    }

    // ── verify_checksum against a local fake CDN ─────────────────────

    use base64::Engine;
    use ed25519_dalek::{Signer, SigningKey};
    use tokio::io::AsyncReadExt;

    /// Serve `files` (path → body) over HTTP/1.1 on a random local port; any
    /// other path is a 404. Returns the base URL.
    async fn serve(files: Vec<(String, Vec<u8>)>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let files = std::sync::Arc::new(files);
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let files = files.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 1024];
                    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        match sock.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let req = String::from_utf8_lossy(&buf);
                    let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
                    let (status, body) = match files.iter().find(|(p, _)| *p == path) {
                        Some((_, b)) => ("200 OK", b.clone()),
                        None => ("404 Not Found", Vec::new()),
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(&body).await;
                });
            }
        });
        format!("http://{addr}")
    }

    /// A throwaway test keypair — never a real release key.
    fn test_key() -> SigningKey {
        let mut seed = [0u8; 32];
        seed[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        seed[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        SigningKey::from_bytes(&seed)
    }

    struct Release {
        feed: Feed,
        binary: std::path::PathBuf,
        _dir: TempDir,
    }

    struct TempDir(std::path::PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Publish a fake signed release `v1.0.0` and return a feed pointing at it
    /// plus a local file holding `downloaded` (what the client "downloaded").
    /// `sig` overrides the signature file (`None` = not published).
    async fn release(
        key: &SigningKey,
        trusted: [u8; 32],
        list_asset: bool,
        downloaded: &[u8],
        sig: Option<Option<Vec<u8>>>,
    ) -> Release {
        let feed_shape = Feed {
            base_url: "".into(),
            binary: "nebo-link".into(),
            checksums: "SHA256SUMS".into(),
            signing_key: Some(trusted),
        };
        let asset = asset_name(&feed_shape);
        let published = b"the real binary".to_vec();
        let mut sums = format!("{}  other-asset\n", hex::encode(Sha256::digest(b"x")));
        if list_asset {
            sums.push_str(&format!(
                "{}  {}\n",
                hex::encode(Sha256::digest(&published)),
                asset
            ));
        }
        let sums = sums.into_bytes();
        let good_sig = base64::engine::general_purpose::STANDARD
            .encode(key.sign(&sums).to_bytes())
            .into_bytes();
        let mut files = vec![("/v1.0.0/SHA256SUMS".to_string(), sums)];
        match sig {
            None => files.push(("/v1.0.0/SHA256SUMS.sig".into(), good_sig)),
            Some(Some(custom)) => files.push(("/v1.0.0/SHA256SUMS.sig".into(), custom)),
            Some(None) => {}
        }
        let base = serve(files).await;

        let dir = std::env::temp_dir().join(format!("nebo-updater-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let binary = dir.join("download");
        std::fs::write(&binary, downloaded).unwrap();
        Release {
            feed: Feed {
                base_url: base.into(),
                ..feed_shape
            },
            binary,
            _dir: TempDir(dir),
        }
    }

    async fn verify(r: &Release) -> Result<(), String> {
        verify_checksum(&r.feed, &r.binary, "v1.0.0")
            .await
            .map_err(|e| e.to_string())
    }

    #[tokio::test]
    async fn signed_release_verifies() {
        let key = test_key();
        let r = release(
            &key,
            key.verifying_key().to_bytes(),
            true,
            b"the real binary",
            None,
        )
        .await;
        verify(&r).await.expect("good signature + hash must verify");
    }

    #[tokio::test]
    async fn signature_by_another_key_is_rejected() {
        let key = test_key();
        let other = test_key();
        let r = release(
            &other,
            key.verifying_key().to_bytes(),
            true,
            b"the real binary",
            None,
        )
        .await;
        let err = verify(&r).await.unwrap_err();
        assert!(err.contains("does not match"), "{err}");
    }

    #[tokio::test]
    async fn garbage_signature_is_rejected() {
        let key = test_key();
        let r = release(
            &key,
            key.verifying_key().to_bytes(),
            true,
            b"the real binary",
            Some(Some(b"not base64 at all!".to_vec())),
        )
        .await;
        assert!(verify(&r).await.is_err());
    }

    #[tokio::test]
    async fn missing_signature_fails_closed() {
        let key = test_key();
        let r = release(
            &key,
            key.verifying_key().to_bytes(),
            true,
            b"the real binary",
            Some(None),
        )
        .await;
        let err = verify(&r).await.unwrap_err();
        assert!(err.contains("SHA256SUMS.sig returned 404"), "{err}");
    }

    #[tokio::test]
    async fn asset_absent_from_checksums_is_rejected() {
        let key = test_key();
        let r = release(
            &key,
            key.verifying_key().to_bytes(),
            false,
            b"the real binary",
            None,
        )
        .await;
        let err = verify(&r).await.unwrap_err();
        assert!(err.contains("not found in SHA256SUMS"), "{err}");
    }

    #[tokio::test]
    async fn tampered_binary_is_rejected() {
        let key = test_key();
        let r = release(
            &key,
            key.verifying_key().to_bytes(),
            true,
            b"a tampered binary",
            None,
        )
        .await;
        let err = verify(&r).await.unwrap_err();
        assert!(err.contains("checksum mismatch"), "{err}");
    }

    #[tokio::test]
    async fn unsigned_feed_skips_signature() {
        // Nebo's own feed has no key: checksums alone, as before.
        let key = test_key();
        let mut r = release(&key, [0; 32], true, b"the real binary", Some(None)).await;
        r.feed.signing_key = None;
        verify(&r)
            .await
            .expect("an unsigned feed verifies by hash only");
    }
}
