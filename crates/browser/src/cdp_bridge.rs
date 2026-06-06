//! CDP bridge — tier-2 "built-in Rust Chrome" browser backend.
//!
//! Launches a Nebo-managed Chrome (via [`crate::chrome::RunningChrome`]) with remote debugging
//! and drives it directly over the Chrome DevTools Protocol using `chromiumoxide` — no extension,
//! no native-messaging relay. Used by [`crate::executor::ActionExecutor`] as the fallback when the
//! user's Chrome extension is unavailable. One CDP page (tab) per `session_id` preserves the 1:1
//! sub-agent→tab model. The browser is launched **lazily** on first use, so users whose extension
//! works never pay for a second Chrome.

use std::collections::HashMap;

use chromiumoxide::{Browser, Page};
use futures::StreamExt;
use serde_json::{Value, json};
use tokio::sync::{Mutex, OnceCell};
use tracing::info;

use crate::BrowserError;
use crate::chrome::RunningChrome;

/// How to launch the managed Chrome (resolved once, used on lazy init).
#[derive(Clone)]
pub struct CdpLaunchConfig {
    pub executable: Option<String>,
    pub user_data_dir: String,
    pub cdp_port: u16,
    pub headless: bool,
    pub no_sandbox: bool,
}

/// The launched browser + its open pages. Created once via [`CdpBridge::core`].
struct CdpCore {
    /// Keep the Chrome process alive — `RunningChrome::drop` kills it.
    _chrome: RunningChrome,
    browser: Browser,
    /// One tab per `session_id` (1:1 sub-agent→tab). Locked only to get/insert, never across a
    /// page operation, so sessions navigate/read concurrently.
    pages: Mutex<HashMap<String, Page>>,
}

/// Tier-2 backend: a Nebo-launched Chrome driven over CDP. Launches lazily on first use.
pub struct CdpBridge {
    launch: CdpLaunchConfig,
    core: OnceCell<CdpCore>,
}

impl CdpBridge {
    pub fn new(launch: CdpLaunchConfig) -> Self {
        Self {
            launch,
            core: OnceCell::new(),
        }
    }

    /// Launch (once) the managed Chrome and connect over CDP; subsequent calls reuse it.
    async fn core(&self) -> Result<&CdpCore, BrowserError> {
        self.core
            .get_or_try_init(|| async {
                info!(port = self.launch.cdp_port, "launching built-in Chrome (CDP tier-2)");
                let chrome = RunningChrome::launch(
                    self.launch.executable.as_deref(),
                    &self.launch.user_data_dir,
                    self.launch.cdp_port,
                    self.launch.headless,
                    self.launch.no_sandbox,
                )
                .await?;
                let ws = chrome.ws_url().await?;
                let (browser, mut handler) = Browser::connect(ws)
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
                info!("built-in Chrome connected over CDP");
                Ok(CdpCore {
                    _chrome: chrome,
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
