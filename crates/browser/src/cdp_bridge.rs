//! CDP bridge — tier-2 "built-in browser" backend, powered by **Obscura**.
//!
//! Launches the bundled [Obscura](https://github.com/h4ckf0r0day/obscura) headless browser
//! (`obscura serve --stealth`) on an ephemeral loopback port and drives it over the Chrome
//! DevTools Protocol via `chromiumoxide`. Obscura is a lightweight (30 MB), stealthy, headless
//! Rust browser with real JS (V8) — so tier 2 is invisible (no window) and never touches the
//! user's installed Chrome. Used by [`crate::executor::ActionExecutor`] as the fallback when the
//! user's Chrome extension is unavailable. One CDP page (tab) per `session_id` preserves the 1:1
//! sub-agent→tab model. Launched **lazily** on first use, so extension users never pay for it.

use std::collections::HashMap;
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::Duration;

use chromiumoxide::{Browser, Page};
use futures::StreamExt;
use rand::Rng;
use serde_json::{Value, json};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, OnceCell};
use tracing::info;

use crate::BrowserError;

/// How to launch the bundled Obscura browser (resolved once, used on lazy init).
#[derive(Clone)]
pub struct ObscuraConfig {
    /// Path to the `obscura` binary.
    pub binary: PathBuf,
    /// Persistent profile dir (cookies/storage). None = ephemeral.
    pub storage_dir: Option<PathBuf>,
    /// Anti-detection + tracker blocking.
    pub stealth: bool,
}

/// The launched Obscura process + browser + its open pages. Created once via [`CdpBridge::core`].
struct CdpCore {
    /// The `obscura serve` process — killed on drop (`Command::kill_on_drop`).
    _obscura: Child,
    browser: Browser,
    /// One tab per `session_id` (1:1 sub-agent→tab). Locked only to get/insert, never across a
    /// page operation, so sessions navigate/read concurrently.
    pages: Mutex<HashMap<String, Page>>,
}

/// Tier-2 backend: the bundled Obscura headless browser driven over CDP. Launches lazily.
pub struct CdpBridge {
    config: ObscuraConfig,
    core: OnceCell<CdpCore>,
}

impl CdpBridge {
    pub fn new(config: ObscuraConfig) -> Self {
        Self {
            config,
            core: OnceCell::new(),
        }
    }

    /// Launch (once) Obscura's CDP server and connect; subsequent calls reuse it.
    async fn core(&self) -> Result<&CdpCore, BrowserError> {
        self.core
            .get_or_try_init(|| async {
                // Random high loopback port — zero collisions across concurrent instances.
                let port = random_high_port()?;
                info!(port, binary = %self.config.binary.display(), "launching Obscura (CDP tier-2)");

                let mut cmd = Command::new(&self.config.binary);
                cmd.arg("serve")
                    .arg("--host")
                    .arg("127.0.0.1")
                    .arg("--port")
                    .arg(port.to_string());
                if self.config.stealth {
                    cmd.arg("--stealth");
                }
                if let Some(dir) = &self.config.storage_dir {
                    cmd.arg("--storage-dir").arg(dir);
                }
                cmd.stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .kill_on_drop(true);
                let child = cmd
                    .spawn()
                    .map_err(|e| BrowserError::Other(format!("failed to launch obscura: {e}")))?;

                // Wait for Obscura's CDP endpoint to come up before connecting.
                wait_for_cdp(port, Duration::from_secs(15)).await?;

                let (browser, mut handler) = Browser::connect(format!("http://127.0.0.1:{port}"))
                    .await
                    .map_err(|e| BrowserError::CdpConnection(e.to_string()))?;
                // Drive the CDP event loop for the life of the browser.
                tokio::spawn(async move {
                    while let Some(ev) = handler.next().await {
                        if ev.is_err() {
                            break;
                        }
                    }
                });
                info!("Obscura connected over CDP");
                Ok(CdpCore {
                    _obscura: child,
                    browser,
                    pages: Mutex::new(HashMap::new()),
                })
            })
            .await
    }

    /// Get (or open) the tab for a session — one page per `session_id`.
    async fn page_for(&self, session_id: &str) -> Result<Page, BrowserError> {
        let core = self.core().await?;
        {
            let map = core.pages.lock().await;
            if let Some(p) = map.get(session_id) {
                return Ok(p.clone());
            }
        }
        let page = core
            .browser
            .new_page("about:blank")
            .await
            .map_err(|e| BrowserError::Other(format!("cdp new_page: {e}")))?;
        core.pages
            .lock()
            .await
            .insert(session_id.to_string(), page.clone());
        Ok(page)
    }

    /// Execute a browser tool over CDP. Phase-1 surface: `navigate` + `read_page`.
    pub async fn execute(
        &self,
        tool: &str,
        args: &Value,
        session_id: &str,
    ) -> Result<Value, BrowserError> {
        match tool {
            "navigate" => {
                let url = args
                    .get("url")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| BrowserError::Other("navigate requires 'url'".into()))?;
                let page = self.page_for(session_id).await?;
                page.goto(url)
                    .await
                    .map_err(|e| BrowserError::Other(format!("cdp navigate: {e}")))?;
                Ok(json!({ "ok": true, "url": url }))
            }
            "read_page" => {
                let page = self.page_for(session_id).await?;
                // Visible text — enough for reading search results and article content. The
                // interactive accessibility/ref surface (click/fill targets) is Phase 2.
                let text: String = page
                    .evaluate(
                        "document.body && document.body.innerText ? document.body.innerText \
                         : (document.documentElement ? document.documentElement.innerText : '')",
                    )
                    .await
                    .map_err(|e| BrowserError::Other(format!("cdp read_page: {e}")))?
                    .into_value()
                    .map_err(|e| BrowserError::Other(format!("cdp read_page decode: {e}")))?;
                Ok(json!({ "pageContent": text }))
            }
            other => Err(BrowserError::Other(format!(
                "built-in browser (CDP) does not support '{other}' yet"
            ))),
        }
    }

    /// True once the managed Chrome has been launched + connected (i.e. tier-2 is in use).
    pub fn is_active(&self) -> bool {
        self.core.get().is_some()
    }

    /// Close the tab a session opened (best-effort) — mirrors the extension's `close_session_tabs`.
    pub async fn close_session(&self, session_id: &str) {
        if let Some(core) = self.core.get() {
            let page = core.pages.lock().await.remove(session_id);
            if let Some(page) = page {
                let _ = page.close().await;
            }
        }
    }
}

/// Pick a free, **random high** loopback TCP port. The random high range + a bind-test means the
/// chosen port can't collide with another listener, even across many concurrent Obscura instances.
fn random_high_port() -> Result<u16, BrowserError> {
    for _ in 0..64 {
        let port: u16 = rand::thread_rng().gen_range(30000..=60000);
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            // Listener dropped here → the port is free for Obscura to bind immediately.
            return Ok(port);
        }
    }
    // Fallback: let the OS hand out any free ephemeral port.
    let l = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| BrowserError::Other(format!("no free port: {e}")))?;
    l.local_addr()
        .map(|a| a.port())
        .map_err(|e| BrowserError::Other(e.to_string()))
}

/// Poll Obscura's CDP `/json/version` endpoint until it responds (ready) or times out.
async fn wait_for_cdp(port: u16, timeout: Duration) -> Result<(), BrowserError> {
    let url = format!("http://127.0.0.1:{port}/json/version");
    let client = reqwest::Client::new();
    let start = std::time::Instant::now();
    loop {
        if start.elapsed() > timeout {
            return Err(BrowserError::Timeout("obscura CDP not ready in time".into()));
        }
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// Resolve the bundled `obscura` binary: `OBSCURA_BIN` env → `<data_dir>/bin/obscura` → `$PATH`.
pub fn find_obscura(data_dir: &str) -> Option<PathBuf> {
    if let Ok(p) = std::env::var("OBSCURA_BIN") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    let bundled = PathBuf::from(data_dir).join("bin").join("obscura");
    if bundled.exists() {
        return Some(bundled);
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let cand = dir.join("obscura");
            if cand.exists() {
                return Some(cand);
            }
        }
    }
    None
}
