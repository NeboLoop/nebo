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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chromiumoxide::{Browser, Page};
use futures::StreamExt;
use rand::Rng;
use serde_json::{Value, json};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::BrowserError;
use crate::human_input;

/// Bound for a single CDP operation. Obscura ops are normally sub-second; a hang
/// past this means the browser/connection is wedged, so we fail fast (and, for
/// `new_page`, recycle the whole connection) instead of trapping the tool for
/// minutes — the long-session wedge this module previously suffered.
const NEW_PAGE_TIMEOUT: Duration = Duration::from_secs(30);
const NAV_TIMEOUT: Duration = Duration::from_secs(45);
const EVAL_TIMEOUT: Duration = Duration::from_secs(20);

/// How to launch the bundled Obscura browser (resolved once, used on lazy init).
#[derive(Clone)]
pub struct ObscuraConfig {
    /// Path to the `obscura` binary.
    pub binary: PathBuf,
    /// Persistent profile dir (cookies/storage). None = ephemeral.
    pub storage_dir: Option<PathBuf>,
    /// Anti-detection + tracker blocking.
    pub stealth: bool,
    /// Where to capture Obscura's own log (navigations + CDP errors) so a misbehaving
    /// tier-2 browse leaves a durable trail. None = discard. Appended to.
    pub log_path: Option<PathBuf>,
}

/// The launched Obscura process + browser + its open pages. Recreated on demand
/// via [`CdpBridge::get_core`] whenever the previous connection dies.
struct CdpCore {
    /// The `obscura serve` process — killed on drop (`Command::kill_on_drop`).
    _obscura: Child,
    browser: Browser,
    /// One tab per `session_id` (1:1 sub-agent→tab). Locked only to get/insert, never across a
    /// page operation, so sessions navigate/read concurrently.
    pages: Mutex<HashMap<String, Page>>,
    /// Flipped to `false` by the CDP event-loop task when the connection ends. A
    /// dead core is dropped + relaunched on the next [`CdpBridge::get_core`] call —
    /// this is what stops a wedged Obscura from being trapped forever.
    alive: Arc<AtomicBool>,
}

/// Tier-2 backend: the bundled Obscura headless browser driven over CDP. Launches
/// lazily, and **relaunches** if the connection dies or wedges (so a long session
/// can't permanently lose the built-in browser).
pub struct CdpBridge {
    config: ObscuraConfig,
    core: Mutex<Option<Arc<CdpCore>>>,
    /// True once tier-2 has been launched at least once (sync status, no lock).
    launched: AtomicBool,
    /// Last pointer position per session — the start of the next human mouse path
    /// (mirrors the extension's per-tab `lastMousePos`).
    mouse_pos: Mutex<HashMap<String, (f64, f64)>>,
}

impl CdpBridge {
    pub fn new(config: ObscuraConfig) -> Self {
        Self {
            config,
            core: Mutex::new(None),
            launched: AtomicBool::new(false),
            mouse_pos: Mutex::new(HashMap::new()),
        }
    }

    /// Return a live Obscura core, launching (or relaunching, if the previous one
    /// died) as needed. The lock is held across launch so two callers can't spawn
    /// two Obscura processes.
    async fn get_core(&self) -> Result<Arc<CdpCore>, BrowserError> {
        let mut guard = self.core.lock().await;
        if let Some(core) = guard.as_ref() {
            if core.alive.load(Ordering::Relaxed) {
                return Ok(core.clone());
            }
            // Connection died — drop it (kill_on_drop terminates the old process) and relaunch.
            warn!("Obscura CDP connection dead — relaunching tier-2 browser");
            *guard = None;
        }
        let core = Arc::new(self.launch().await?);
        *guard = Some(core.clone());
        self.launched.store(true, Ordering::Relaxed);
        Ok(core)
    }

    /// Drop the current core so the next [`get_core`] relaunches. Called when an
    /// operation wedges (e.g. `new_page` times out) — the browser is unhealthy.
    async fn recycle(&self) {
        *self.core.lock().await = None;
    }

    /// Spawn a fresh Obscura process and connect over CDP.
    async fn launch(&self) -> Result<CdpCore, BrowserError> {
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
        // Capture Obscura's own log (navigations + CDP errors) to a file so a
        // misbehaving tier-2 browse leaves a durable trail. `info` keeps it useful
        // without the per-command `debug` firehose. Falls back to discarding if the
        // log file can't be opened.
        let log_file = self.config.log_path.as_ref().and_then(|p| {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            std::fs::OpenOptions::new().create(true).append(true).open(p).ok()
        });
        cmd.env("RUST_LOG", "obscura=info,obscura_cdp=info");
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(match log_file {
                Some(f) => std::process::Stdio::from(f),
                None => std::process::Stdio::null(),
            })
            .kill_on_drop(true);
        let child = cmd
            .spawn()
            .map_err(|e| BrowserError::Other(format!("failed to launch obscura: {e}")))?;

        // Wait for Obscura's CDP endpoint to come up before connecting.
        wait_for_cdp(port, Duration::from_secs(15)).await?;

        let (browser, mut handler) = Browser::connect(format!("http://127.0.0.1:{port}"))
            .await
            .map_err(|e| BrowserError::CdpConnection(e.to_string()))?;
        // Drive the CDP event loop for the life of the browser; mark the core dead
        // when it ends so the next caller relaunches instead of hanging on a corpse.
        let alive = Arc::new(AtomicBool::new(true));
        let alive_task = alive.clone();
        tokio::spawn(async move {
            while let Some(ev) = handler.next().await {
                if ev.is_err() {
                    break;
                }
            }
            alive_task.store(false, Ordering::Relaxed);
        });
        info!("Obscura connected over CDP");
        Ok(CdpCore {
            _obscura: child,
            browser,
            pages: Mutex::new(HashMap::new()),
            alive,
        })
    }

    /// Get (or open) the tab for a session — one page per `session_id`.
    async fn page_for(&self, session_id: &str) -> Result<Page, BrowserError> {
        let core = self.get_core().await?;
        {
            let map = core.pages.lock().await;
            if let Some(p) = map.get(session_id) {
                return Ok(p.clone());
            }
        }
        // Bound `new_page` — a wedged Obscura otherwise hangs the tool for minutes.
        // On timeout or error, recycle the connection so the NEXT call relaunches
        // into a fresh browser (self-healing) instead of staying stuck.
        let page = match tokio::time::timeout(
            NEW_PAGE_TIMEOUT,
            core.browser.new_page("about:blank"),
        )
        .await
        {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => {
                self.recycle().await;
                return Err(BrowserError::Other(format!("cdp new_page: {e}")));
            }
            Err(_) => {
                self.recycle().await;
                return Err(BrowserError::Timeout(
                    "cdp new_page timed out — recycled the built-in browser, retry".into(),
                ));
            }
        };
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
                match tokio::time::timeout(NAV_TIMEOUT, page.goto(url)).await {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => {
                        return Err(BrowserError::Other(format!("cdp navigate: {e}")));
                    }
                    Err(_) => {
                        return Err(BrowserError::Timeout(format!(
                            "cdp navigate timed out: {url}"
                        )));
                    }
                }
                Ok(json!({ "ok": true, "url": url }))
            }
            "read_page" => {
                let page = self.page_for(session_id).await?;
                // Visible text — enough for reading search results and article content. The
                // interactive accessibility/ref surface (click/fill targets) is Phase 2.
                let eval = match tokio::time::timeout(
                    EVAL_TIMEOUT,
                    page.evaluate(
                        "document.body && document.body.innerText ? document.body.innerText \
                         : (document.documentElement ? document.documentElement.innerText : '')",
                    ),
                )
                .await
                {
                    Ok(Ok(e)) => e,
                    Ok(Err(e)) => return Err(BrowserError::Other(format!("cdp read_page: {e}"))),
                    Err(_) => {
                        return Err(BrowserError::Timeout("cdp read_page timed out".into()));
                    }
                };
                let text: String = eval
                    .into_value()
                    .map_err(|e| BrowserError::Other(format!("cdp read_page decode: {e}")))?;
                Ok(json!({ "pageContent": text }))
            }
            "evaluate" => {
                let expression = args
                    .get("expression")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| BrowserError::Other("evaluate requires 'expression'".into()))?;
                let page = self.page_for(session_id).await?;
                let eval =
                    match tokio::time::timeout(EVAL_TIMEOUT, page.evaluate(expression)).await {
                        Ok(Ok(e)) => e,
                        Ok(Err(e)) => {
                            return Err(BrowserError::Other(format!("cdp evaluate: {e}")));
                        }
                        Err(_) => {
                            return Err(BrowserError::Timeout("cdp evaluate timed out".into()));
                        }
                    };
                // Same result key the extension uses, so callers read one shape.
                let text = match eval.value() {
                    Some(Value::String(s)) => s.clone(),
                    Some(v) => v.to_string(),
                    None => String::new(),
                };
                Ok(json!({ "text": text }))
            }
            // Humanized input — exact parity with the extension (curved mouse path,
            // human click hold, typing cadence). Targets an element by CSS `selector`
            // or explicit `coordinate`.
            "click" => {
                let page = self.page_for(session_id).await?;
                let (x, y) = self.resolve_point(&page, args).await?;
                let from = self.mouse_pos.lock().await.get(session_id).copied();
                let pos = human_input::human_click(&page, from, x, y).await?;
                self.mouse_pos
                    .lock()
                    .await
                    .insert(session_id.to_string(), pos);
                // Obscura's synthesized mouse click does not reliably move keyboard
                // focus to form fields (a headless quirk). The human mouse motion
                // above is what bot-detection observes; this focus() just guarantees
                // a following `type` lands in the field the agent clicked.
                if let Some(sel) = args.get("selector").and_then(|v| v.as_str()) {
                    let expr = format!(
                        "(() => {{ const el = document.querySelector({sel}); \
                         if (el && typeof el.focus === 'function') el.focus(); }})()",
                        sel = serde_json::to_string(sel).unwrap_or_else(|_| "''".into()),
                    );
                    let _ = page.evaluate(expr).await;
                }
                Ok(json!({ "text": format!("Clicked at ({:.0}, {:.0})", x, y) }))
            }
            "type" => {
                let text = args
                    .get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| BrowserError::Other("type requires 'text'".into()))?;
                let page = self.page_for(session_id).await?;
                human_input::human_type(&page, text).await?;
                Ok(json!({ "text": format!("Typed {} chars", text.chars().count()) }))
            }
            "press" => {
                let key = args
                    .get("key")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| BrowserError::Other("press requires 'key'".into()))?;
                let page = self.page_for(session_id).await?;
                human_input::press_key(&page, key).await?;
                Ok(json!({ "text": format!("Pressed {key}") }))
            }
            other => Err(BrowserError::Other(format!(
                "built-in browser (CDP) does not support '{other}' yet"
            ))),
        }
    }

    /// Resolve a click target to viewport CSS-pixel coordinates: explicit
    /// `coordinate: [x, y]`, or a CSS `selector` whose center is found via JS
    /// (scrolled into view first). Errors if neither resolves.
    async fn resolve_point(
        &self,
        page: &Page,
        args: &Value,
    ) -> Result<(f64, f64), BrowserError> {
        if let Some(arr) = args.get("coordinate").and_then(|v| v.as_array()) {
            if let (Some(x), Some(y)) = (
                arr.first().and_then(|v| v.as_f64()),
                arr.get(1).and_then(|v| v.as_f64()),
            ) {
                return Ok((x, y));
            }
        }
        let selector = args
            .get("selector")
            .and_then(|v| v.as_str())
            .ok_or_else(|| BrowserError::Other("click requires 'selector' or 'coordinate'".into()))?;
        let expr = format!(
            "(() => {{ const el = document.querySelector({sel}); if (!el) return null; \
             el.scrollIntoView({{block:'center', inline:'center'}}); \
             const r = el.getBoundingClientRect(); \
             if (r.width === 0 && r.height === 0) return null; \
             return [r.left + r.width/2, r.top + r.height/2]; }})()",
            sel = serde_json::to_string(selector).unwrap_or_else(|_| "''".into()),
        );
        let eval = tokio::time::timeout(EVAL_TIMEOUT, page.evaluate(expr))
            .await
            .map_err(|_| BrowserError::Timeout("cdp resolve_point timed out".into()))?
            .map_err(|e| BrowserError::Other(format!("cdp resolve_point: {e}")))?;
        let point: Option<(f64, f64)> = eval.into_value().ok();
        point.ok_or_else(|| {
            BrowserError::Other(format!("element not found or not visible: {selector}"))
        })
    }

    /// True once the managed Chrome has been launched (i.e. tier-2 is in use).
    pub fn is_active(&self) -> bool {
        self.launched.load(Ordering::Relaxed)
    }

    /// Close the tab a session opened (best-effort) — mirrors the extension's `close_session_tabs`.
    pub async fn close_session(&self, session_id: &str) {
        // Snapshot the core out of the lock, then close the page without holding it.
        let core = self.core.lock().await.clone();
        if let Some(core) = core {
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

/// Resolve the bundled `obscura` binary, deployment-agnostic so the same resolver works for
/// the Tauri desktop app and a headless server build alike:
///   1. `OBSCURA_BIN` env          — explicit override (Docker/k8s/CI/dev)
///   2. `current_exe()` sibling dir — bundled next to the running binary. Covers BOTH a Tauri
///      `externalBin` sidecar (placed in `Contents/MacOS/` next to `nebo-desktop`, signed +
///      notarized) AND a server image that `COPY`s obscura beside the server binary. Mirrors
///      how the `nebo` relay binary is resolved.
///   3. `<data_dir>/bin/obscura`    — downloaded-update path
///   4. `$PATH`                     — system install (e.g. `/usr/local/bin`)
pub fn find_obscura(data_dir: &str) -> Option<PathBuf> {
    if let Ok(p) = std::env::var("OBSCURA_BIN") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("obscura");
            if sibling.exists() {
                return Some(sibling);
            }
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

#[cfg(test)]
mod tests {
    //! Live tier-2 tests. They spawn the real Obscura binary, so they're
    //! `#[ignore]`d (run with `cargo test -p nebo-browser -- --ignored`) and
    //! skip cleanly when the binary isn't present. They guard the wedge fix:
    //! many fresh tabs + concurrent agents must not hang or trap the backend.
    use super::*;

    fn try_bridge() -> Option<CdpBridge> {
        let bin = find_obscura(".")?;
        Some(CdpBridge::new(ObscuraConfig {
            binary: bin,
            storage_dir: None,
            stealth: true,
            log_path: None,
        }))
    }

    /// Opening a fresh tab + navigate + read across many sessions must keep
    /// working — the leaked-target wedge made new_page hang after a while.
    #[tokio::test]
    #[ignore = "requires the obscura binary; run with --ignored"]
    async fn many_sessions_do_not_wedge() {
        let Some(bridge) = try_bridge() else {
            eprintln!("obscura binary not found — skipping");
            return;
        };
        for i in 0..25 {
            let sid = format!("sess-{i}");
            let url = format!("data:text/html,<body>page-{i}</body>");
            bridge
                .execute("navigate", &json!({ "url": url }), &sid)
                .await
                .unwrap_or_else(|e| panic!("navigate iter {i} failed: {e}"));
            let res = bridge
                .execute("read_page", &json!({}), &sid)
                .await
                .unwrap_or_else(|e| panic!("read_page iter {i} failed: {e}"));
            assert!(
                res.get("pageContent").is_some(),
                "iter {i}: read_page returned no pageContent"
            );
            bridge.close_session(&sid).await;
        }
    }

    /// Humanized click + type land real characters in a focused input — proves
    /// the CDP input synthesis (curved move, click, keydown/keyup cadence) works
    /// end to end, not just that it compiles.
    #[tokio::test]
    #[ignore = "requires the obscura binary; run with --ignored"]
    async fn human_click_and_type_fills_input() {
        let Some(bridge) = try_bridge() else {
            eprintln!("obscura binary not found — skipping");
            return;
        };
        let sid = "human-input";
        let html = "data:text/html,<body style='margin:40px'>\
                    <input id='box' style='width:300px;height:30px'></body>";
        bridge
            .execute("navigate", &json!({ "url": html }), sid)
            .await
            .expect("navigate");
        bridge
            .execute("click", &json!({ "selector": "#box" }), sid)
            .await
            .expect("click");
        bridge
            .execute("type", &json!({ "text": "hello world" }), sid)
            .await
            .expect("type");
        let v = bridge
            .execute(
                "evaluate",
                &json!({ "expression": "document.getElementById('box').value" }),
                sid,
            )
            .await
            .expect("evaluate");
        assert_eq!(
            v.get("text").and_then(|t| t.as_str()),
            Some("hello world"),
            "humanized typing should fill the input"
        );
        bridge.close_session(sid).await;
    }

    /// Concurrent sub-agents each drive their own tab on the shared browser.
    #[tokio::test]
    #[ignore = "requires the obscura binary; run with --ignored"]
    async fn concurrent_agents_share_the_browser() {
        let Some(bridge) = try_bridge() else {
            eprintln!("obscura binary not found — skipping");
            return;
        };
        let bridge = Arc::new(bridge);
        let mut handles = Vec::new();
        for i in 0..8 {
            let b = bridge.clone();
            handles.push(tokio::spawn(async move {
                let sid = format!("agent-{i}");
                let url = format!("data:text/html,<body>agent-{i}</body>");
                b.execute("navigate", &json!({ "url": url }), &sid)
                    .await
                    .expect("navigate");
                let res = b
                    .execute("read_page", &json!({}), &sid)
                    .await
                    .expect("read_page");
                b.close_session(&sid).await;
                res.get("pageContent").is_some()
            }));
        }
        for (i, h) in handles.into_iter().enumerate() {
            assert!(h.await.expect("task panicked"), "agent {i} got no content");
        }
    }
}
