mod apply;

use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tracing::warn;

const RELEASE_URL: &str = "https://cdn.neboloop.com/releases/version.json";
const RELEASE_DOWNLOAD_URL: &str = "https://cdn.neboloop.com/releases";
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Outcome of an update check.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Check the CDN for a newer version.
pub async fn check(current_version: &str) -> Result<CheckResult, UpdateError> {
    let client = reqwest::Client::builder()
        .timeout(CHECK_TIMEOUT)
        .user_agent(format!("nebo/{}", current_version))
        .build()?;

    let resp = client.get(RELEASE_URL).send().await?;
    if !resp.status().is_success() {
        return Err(UpdateError::Other(format!(
            "version check returned {}",
            resp.status()
        )));
    }

    let manifest: VersionManifest = resp.json().await?;
    let latest = normalize_version(&manifest.version);
    let current = normalize_version(current_version);

    let available = latest != current && current != "dev" && is_newer(&latest, &current);
    let method = detect_install_method();

    Ok(CheckResult {
        available,
        current_version: current_version.to_string(),
        latest_version: manifest.version,
        release_url: manifest.release_url,
        published_at: manifest.published_at,
        install_method: method.to_string(),
        can_auto_update: method == "direct",
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

    if path.contains("/opt/homebrew/") || path.contains("/usr/local/Cellar/") {
        return "homebrew";
    }

    #[cfg(target_os = "linux")]
    {
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
pub fn asset_name() -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    if os == "windows" {
        format!("nebo-{}-{}.exe", os, arch)
    } else {
        format!("nebo-{}-{}", os, arch)
    }
}

/// Progress callback: (downloaded_bytes, total_bytes).
pub type ProgressFn = Box<dyn Fn(u64, u64) + Send>;

/// Download the release binary for the given tag to a temp file.
pub async fn download(
    tag: &str,
    progress: Option<ProgressFn>,
) -> Result<std::path::PathBuf, UpdateError> {
    let asset = asset_name();
    let url = format!("{}/{}/{}", RELEASE_DOWNLOAD_URL, tag, asset);

    let client = reqwest::Client::builder()
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
    let tmp_path = std::env::temp_dir().join(format!("nebo-update-{}", uuid::Uuid::new_v4()));

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

/// Verify SHA256 checksum of the downloaded binary against checksums.txt from CDN.
pub async fn verify_checksum(
    binary_path: &std::path::Path,
    tag: &str,
) -> Result<(), UpdateError> {
    let asset = asset_name();
    let url = format!("{}/{}/checksums.txt", RELEASE_DOWNLOAD_URL, tag);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("nebo-updater")
        .build()?;

    let resp = client.get(&url).send().await?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(());
    }
    if !resp.status().is_success() {
        return Err(UpdateError::Other(format!(
            "checksums returned {}",
            resp.status()
        )));
    }

    let body = resp.text().await?;

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
            UpdateError::Other(format!("asset {} not found in checksums.txt", asset))
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

/// Apply the update: replace current binary and restart.
pub fn apply_update(new_binary_path: &std::path::Path) -> Result<(), UpdateError> {
    apply::apply(new_binary_path)
}

/// Register a pre-apply hook (called before process restart).
pub fn set_pre_apply_hook(f: Box<dyn Fn() + Send>) {
    apply::set_pre_apply_hook(f);
}

/// Periodically checks for updates in the background.
pub struct BackgroundChecker {
    version: String,
    interval: Duration,
    notify: Box<dyn Fn(CheckResult) + Send + Sync>,
    last_notified: Mutex<Option<String>>,
}

impl BackgroundChecker {
    pub fn new(
        version: String,
        interval: Duration,
        notify: impl Fn(CheckResult) + Send + Sync + 'static,
    ) -> Self {
        Self {
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
        match check(&self.version).await {
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
        let name = asset_name();
        assert!(name.starts_with("nebo-"));
    }

    #[test]
    fn test_detect_install_method() {
        let method = detect_install_method();
        assert!(["direct", "homebrew", "package_manager"].contains(&method));
    }
}
