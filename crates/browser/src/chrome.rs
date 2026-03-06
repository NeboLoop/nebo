use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use tracing::info;

use crate::BrowserError;

/// Detect Chrome binary path for the current platform.
pub fn find_chrome() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let paths = [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
        ];
        for path in &paths {
            let p = PathBuf::from(path);
            if p.exists() {
                return Some(p);
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        let names = [
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "brave-browser",
            "microsoft-edge",
        ];
        for name in &names {
            if let Ok(output) = std::process::Command::new("which").arg(name).output() {
                if output.status.success() {
                    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if !path.is_empty() {
                        return Some(PathBuf::from(path));
                    }
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        let paths = [
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe",
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        ];
        for path in &paths {
            let p = PathBuf::from(path);
            if p.exists() {
                return Some(p);
            }
        }
        // Try registry
        if let Ok(output) = std::process::Command::new("reg")
            .args([
                "query",
                r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\chrome.exe",
                "/ve",
            ])
            .output()
        {
            if output.status.success() {
                let out = String::from_utf8_lossy(&output.stdout);
                for line in out.lines() {
                    if line.contains("REG_SZ") {
                        if let Some(path) = line.split("REG_SZ").nth(1) {
                            let path = path.trim();
                            if !path.is_empty() {
                                return Some(PathBuf::from(path));
                            }
                        }
                    }
                }
            }
        }
    }

    None
}

/// A running Chrome instance managed by Nebo.
pub struct RunningChrome {
    pub pid: u32,
    pub executable: PathBuf,
    pub user_data_dir: String,
    pub cdp_port: u16,
    child: tokio::process::Child,
}

impl RunningChrome {
    /// Launch Chrome with remote debugging enabled.
    pub async fn launch(
        executable: Option<&str>,
        user_data_dir: &str,
        cdp_port: u16,
        headless: bool,
        no_sandbox: bool,
    ) -> Result<Self, BrowserError> {
        let exe_path = match executable {
            Some(p) => PathBuf::from(p),
            None => find_chrome().ok_or(BrowserError::ChromeNotFound)?,
        };

        // Ensure user data dir exists
        std::fs::create_dir_all(user_data_dir)?;

        // Clean up stale lock files
        for lock_file in &["SingletonLock", "SingletonSocket", "SingletonCookie"] {
            let lock_path = format!("{}/{}", user_data_dir, lock_file);
            let _ = std::fs::remove_file(lock_path);
        }

        let mut args = vec![
            format!("--remote-debugging-port={}", cdp_port),
            format!("--user-data-dir={}", user_data_dir),
            "--no-first-run".to_string(),
            "--no-default-browser-check".to_string(),
            "--disable-background-timer-throttling".to_string(),
            "--disable-backgrounding-occluded-windows".to_string(),
        ];

        if headless {
            args.push("--headless=new".to_string());
        }
        if no_sandbox {
            args.push("--no-sandbox".to_string());
        }

        let child = Command::new(&exe_path)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| BrowserError::Other(format!("failed to launch Chrome: {}", e)))?;

        let pid = child.id().unwrap_or(0);
        info!(pid, port = cdp_port, "launched Chrome");

        let chrome = Self {
            pid,
            executable: exe_path,
            user_data_dir: user_data_dir.to_string(),
            cdp_port,
            child,
        };

        // Wait for CDP port to become responsive
        chrome.wait_for_cdp(Duration::from_secs(15)).await?;

        Ok(chrome)
    }

    /// Wait for the CDP endpoint to respond.
    async fn wait_for_cdp(&self, timeout: Duration) -> Result<(), BrowserError> {
        let url = format!("http://127.0.0.1:{}/json/version", self.cdp_port);
        let client = reqwest::Client::new();
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() > timeout {
                return Err(BrowserError::Timeout(
                    "CDP port not responsive after timeout".into(),
                ));
            }

            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => return Ok(()),
                _ => tokio::time::sleep(Duration::from_millis(200)).await,
            }
        }
    }

    /// Get the CDP WebSocket URL.
    pub async fn ws_url(&self) -> Result<String, BrowserError> {
        let url = format!("http://127.0.0.1:{}/json/version", self.cdp_port);
        let resp: serde_json::Value = reqwest::get(&url).await?.json().await?;
        resp["webSocketDebuggerUrl"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| BrowserError::CdpConnection("no webSocketDebuggerUrl".into()))
    }

    /// Kill the Chrome process.
    pub async fn kill(&mut self) {
        let _ = self.child.kill().await;
        info!(pid = self.pid, "killed Chrome");
    }
}

impl Drop for RunningChrome {
    fn drop(&mut self) {
        // Best-effort kill on drop
        let _ = self.child.start_kill();
    }
}
