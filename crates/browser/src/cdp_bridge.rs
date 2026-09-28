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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use base64::Engine;
use chromiumoxide::cdp::browser_protocol::browser::CloseParams;
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
use chromiumoxide::page::ScreenshotParams;
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
/// How long a browser asked to exit gets before it is killed.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(10);
const NAV_TIMEOUT: Duration = Duration::from_secs(45);
const EVAL_TIMEOUT: Duration = Duration::from_secs(20);
/// The page side of reads and ref actions: the extension's tree format and
/// ref resolution (see the file's header).
const PAGE_TREE_JS: &str = include_str!("page_tree.js");
/// What a read returns at most, as the extension's `read_page` default.
const READ_MAX_CHARS: u64 = 50_000;
/// The longest `wait`, as the extension's.
const MAX_WAIT: Duration = Duration::from_secs(30);

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
    /// The binary is a stock Chromium rather than Obscura (cloud image, or a
    /// desktop with Chrome but no bundled Obscura). Same CDP protocol, same
    /// bridge — only the launch arguments differ.
    pub chromium: bool,
}

/// The launched Obscura process + browser + its open pages. Recreated on demand
/// via [`CdpBridge::get_core`] whenever the previous connection dies.
struct CdpCore {
    /// The browser process. [`CdpCore::close`] asks it to exit; if a core is
    /// dropped without that, `Command::kill_on_drop` kills it.
    process: Mutex<Child>,
    browser: Browser,
    /// One tab per `session_id` (1:1 sub-agent→tab). Locked only to get/insert, never across a
    /// page operation, so sessions navigate/read concurrently.
    pages: Mutex<HashMap<String, Page>>,
    /// Flipped to `false` by the CDP event-loop task when the connection ends. A
    /// dead core is dropped + relaunched on the next [`CdpBridge::get_core`] call —
    /// this is what stops a wedged Obscura from being trapped forever.
    alive: Arc<AtomicBool>,
    /// When this core was launched — the epoch `last_used_s` counts from.
    started: std::time::Instant,
    /// Seconds-since-launch of the most recent use (touched on every
    /// [`CdpBridge::get_core`]). Read by the idle reaper.
    last_used_s: AtomicU64,
    /// The debugging port it listens on: one of Nebo's own while it runs.
    port: u16,
}

impl Drop for CdpCore {
    fn drop(&mut self) {
        types::own_ports::close(self.port);
    }
}

impl CdpCore {
    fn touch(&self) {
        self.last_used_s
            .store(self.started.elapsed().as_secs(), Ordering::Relaxed);
    }
    fn idle(&self) -> Duration {
        let last = self.last_used_s.load(Ordering::Relaxed);
        self.started.elapsed().saturating_sub(Duration::from_secs(last))
    }

    /// Ask the browser to exit, as a user quitting it would, and wait for
    /// it. Chromium then releases its profile whole: it removes its
    /// `SingletonLock` and flushes cookies and storage. A kill does neither —
    /// the profile stays locked and cannot be committed with the bot's state.
    /// A browser that does not exit within [`CLOSE_TIMEOUT`] is killed.
    async fn close(&self) {
        // The answer rarely arrives: the browser exits while replying.
        let _ = tokio::time::timeout(CLOSE_TIMEOUT, self.browser.execute(CloseParams::default())).await;
        let mut process = self.process.lock().await;
        if tokio::time::timeout(CLOSE_TIMEOUT, process.wait()).await.is_err() {
            warn!("built-in browser did not exit when asked — killing it");
            let _ = process.kill().await;
        }
    }
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
                core.touch();
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

    /// Shut the built-in browser down cleanly ([`CdpCore::close`]); the next
    /// use relaunches it. The lock is held until the browser has exited, so a
    /// relaunch never starts on a profile the old browser still holds. The
    /// graceful drain calls this before the bot's state is committed.
    pub async fn shutdown(&self) {
        let mut guard = self.core.lock().await;
        if let Some(core) = guard.take() {
            core.close().await;
            info!("built-in browser shut down");
        }
    }

    /// Idle-timeout reaper: tear the tier-2 browser down after `idle_after` with
    /// no CDP activity (observed live 2026-08-29: a cloud bot's chromium resident
    /// 6.7 days, holding ~450 MB of guest memory for nothing). Teardown is the
    /// same clean close as [`CdpBridge::shutdown`] — a killed Chromium leaves its
    /// profile locked — and the next job just relaunches. Runs for the life of
    /// the process; `idle_after` = zero disables it.
    pub fn spawn_idle_reaper(self: &Arc<Self>, idle_after: Duration) {
        if idle_after.is_zero() {
            return;
        }
        let bridge = Arc::downgrade(self);
        tokio::spawn(async move {
            let tick = idle_after.min(Duration::from_secs(30)).max(Duration::from_secs(5));
            loop {
                tokio::time::sleep(tick).await;
                let Some(bridge) = bridge.upgrade() else { return };
                let mut guard = bridge.core.lock().await;
                if guard.as_ref().is_some_and(|core| core.idle() >= idle_after) {
                    let core = guard.take().expect("checked");
                    info!(
                        idle_secs = core.idle().as_secs(),
                        "tier-2 browser idle — shutting it down (relaunches on next use)"
                    );
                    core.close().await;
                }
            }
        });
    }

    /// Spawn a fresh Obscura process and connect over CDP.
    async fn launch(&self) -> Result<CdpCore, BrowserError> {
        // Random high loopback port — zero collisions across concurrent instances.
        let port = random_high_port()?;
        // Nebo's own from now: no command an employee runs connects to it.
        types::own_ports::open(port);
        info!(port, binary = %self.config.binary.display(), "launching Obscura (CDP tier-2)");

        let mut cmd = Command::new(&self.config.binary);
        if self.config.chromium {
            // Headless Chromium exposes the same CDP endpoint Obscura serves.
            // --no-sandbox: in the cloud pod the Kata VM is the sandbox and the
            // uid-1000 container has no user namespaces for Chromium's own.
            cmd.arg("--headless=new")
                .arg(format!("--remote-debugging-port={port}"))
                .arg("--no-first-run")
                .arg("--no-default-browser-check")
                .arg("--disable-dev-shm-usage")
                .arg("--disable-gpu")
                .arg("--no-sandbox")
                .arg("about:blank");
            if let Some(dir) = &self.config.storage_dir {
                // A lock left by a Chromium that was killed, or by the one on
                // the pod this profile came from, makes Chromium refuse the
                // profile as "in use on another computer". This bridge is the
                // profile's only user and the browser before it has exited
                // (the core lock is held across close and launch), so any
                // lock here is stale.
                for lock in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
                    let _ = std::fs::remove_file(dir.join(lock));
                }
                cmd.arg(format!("--user-data-dir={}", dir.display()));
            }
        } else {
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
            process: Mutex::new(child),
            browser,
            pages: Mutex::new(HashMap::new()),
            alive,
            started: std::time::Instant::now(),
            last_used_s: AtomicU64::new(0),
            port,
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

    /// Execute a browser tool over CDP, with the extension's tool names,
    /// arguments and result shapes.
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
                let filter = args.get("filter").and_then(|v| v.as_str());
                let depth = args.get("depth").and_then(|v| v.as_u64());
                let max_chars = args.get("maxChars").and_then(|v| v.as_u64()).unwrap_or(READ_MAX_CHARS);
                let ref_id = args.get("refId").and_then(|v| v.as_str()).map(normalize_ref);
                let tree = self
                    .page_call(
                        &page,
                        &format!(
                            "window.__neboGenerateAccessibilityTree({}, {}, {max_chars}, {})",
                            json!(filter),
                            json!(depth),
                            json!(ref_id)
                        ),
                    )
                    .await?;
                if let Some(e) = tree.get("error").and_then(|v| v.as_str()) {
                    return Err(BrowserError::Other(e.to_string()));
                }
                Ok(tree)
            }
            "find" => {
                let query = args
                    .get("query")
                    .and_then(|v| v.as_str())
                    .filter(|q| !q.is_empty())
                    .ok_or_else(|| BrowserError::Other("query parameter is required".into()))?;
                let page = self.page_for(session_id).await?;
                let tree = self
                    .page_call(&page, "window.__neboGenerateAccessibilityTree('all')")
                    .await?;
                let content = tree.get("pageContent").and_then(|v| v.as_str()).unwrap_or("");
                Ok(json!({ "text": find_in_tree(content, query) }))
            }
            "wait" => {
                let seconds = args
                    .get("duration")
                    .and_then(|v| v.as_f64())
                    .or_else(|| args.get("ms").and_then(|v| v.as_f64()).map(|ms| ms / 1000.0))
                    .unwrap_or(0.0);
                if seconds <= 0.0 {
                    return Err(BrowserError::Other(
                        "Duration parameter is required and must be positive".into(),
                    ));
                }
                if seconds > MAX_WAIT.as_secs_f64() {
                    return Err(BrowserError::Other(format!(
                        "Duration cannot exceed {} seconds",
                        MAX_WAIT.as_secs()
                    )));
                }
                tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
                let plural = if seconds == 1.0 { "" } else { "s" };
                Ok(json!({ "text": format!("Waited for {seconds} second{plural}") }))
            }
            "screenshot" => {
                let page = self.page_for(session_id).await?;
                let shot = tokio::time::timeout(
                    EVAL_TIMEOUT,
                    page.screenshot(
                        ScreenshotParams::builder()
                            .format(CaptureScreenshotFormat::Jpeg)
                            .quality(75)
                            .build(),
                    ),
                )
                .await
                .map_err(|_| BrowserError::Timeout("cdp screenshot timed out".into()))?
                .map_err(|e| BrowserError::Other(format!("cdp screenshot: {e}")))?;
                let viewport = self
                    .page_call(&page, "[window.innerWidth, window.innerHeight]")
                    .await?;
                Ok(json!({
                    "data": base64::engine::general_purpose::STANDARD.encode(shot),
                    "format": "jpeg",
                    "encoding": "base64",
                    "width": viewport.get(0),
                    "height": viewport.get(1),
                }))
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
            // human click hold, typing cadence). Targets an element by its `ref`
            // from a read, a CSS `selector`, or an explicit `coordinate`.
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
                let target = if let Some(r) = args.get("ref").and_then(|v| v.as_str()) {
                    Some(format!(
                        "((window.__neboElementMap || {{}})[{}] || {{ deref: () => null }}).deref()",
                        json!(normalize_ref(r))
                    ))
                } else {
                    args.get("selector").and_then(|v| v.as_str()).map(|sel| {
                        format!("document.querySelector({})", json!(sel))
                    })
                };
                if let Some(target) = target {
                    let expr = format!(
                        "(() => {{ const el = {target}; \
                         if (el && typeof el.focus === 'function') el.focus(); }})()"
                    );
                    let _ = page.evaluate(expr).await;
                }
                self.settle(&page).await;
                Ok(json!({ "text": match args.get("ref").and_then(|v| v.as_str()) {
                    Some(r) => format!("Clicked on element {r}"),
                    None => format!("Clicked at ({:.0}, {:.0})", x, y),
                } }))
            }
            "type" => {
                let text = args
                    .get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| BrowserError::Other("type requires 'text'".into()))?;
                let page = self.page_for(session_id).await?;
                human_input::human_type(&page, text).await?;
                self.settle(&page).await;
                Ok(json!({ "text": format!("Typed {} chars", text.chars().count()) }))
            }
            "press" => {
                let key = args
                    .get("key")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| BrowserError::Other("press requires 'key'".into()))?;
                let page = self.page_for(session_id).await?;
                human_input::press_key(&page, key).await?;
                self.settle(&page).await;
                Ok(json!({ "text": format!("Pressed {key}") }))
            }
            other => Err(BrowserError::Other(format!(
                "built-in browser (CDP) does not support '{other}' yet"
            ))),
        }
    }

    /// Evaluate `call` in the page after [`PAGE_TREE_JS`] (idempotent), awaiting
    /// a promise, and return its value.
    async fn page_call(&self, page: &Page, call: &str) -> Result<Value, BrowserError> {
        let params = EvaluateParams::builder()
            .expression(format!("({PAGE_TREE_JS}, {call})"))
            .await_promise(true)
            .return_by_value(true)
            .build()
            .map_err(|e| BrowserError::Other(format!("cdp page script: {e}")))?;
        let eval = tokio::time::timeout(EVAL_TIMEOUT, page.evaluate(params))
            .await
            .map_err(|_| BrowserError::Timeout("cdp page script timed out".into()))?
            .map_err(|e| BrowserError::Other(format!("cdp page script: {e}")))?;
        Ok(eval.value().cloned().unwrap_or(Value::Null))
    }

    /// Let what an input action changed land before the page is read: the
    /// DOM quiet for 300 ms, at most 2 s, as the extension waits. A page that
    /// navigated away mid-wait has nothing left to wait for.
    async fn settle(&self, page: &Page) {
        let _ = self.page_call(page, "window.__neboDomStable(300, 2000)").await;
    }

    /// The viewport point a `ref` from a read names, scrolled into view. A
    /// ref the page no longer holds is looked up again after a fresh read,
    /// as the extension does.
    async fn resolve_ref(&self, page: &Page, r: &str) -> Result<(f64, f64), BrowserError> {
        let r = normalize_ref(r);
        let call = format!("window.__neboResolveRef({})", json!(r));
        let mut point = self.page_call(page, &call).await?;
        if point.is_null() {
            self.page_call(page, "window.__neboGenerateAccessibilityTree('all', 15)").await?;
            point = self.page_call(page, &call).await?;
        }
        serde_json::from_value::<(f64, f64)>(point).map_err(|_| {
            BrowserError::Other(format!(
                "No element found with reference: \"{r}\". The element may have been removed from the page. Use read_page to get fresh references."
            ))
        })
    }

    /// Resolve a click target to viewport CSS-pixel coordinates: a `ref` from
    /// a read, explicit `coordinate: [x, y]`, or a CSS `selector` whose center
    /// is found via JS (scrolled into view first). Errors if none resolves.
    async fn resolve_point(
        &self,
        page: &Page,
        args: &Value,
    ) -> Result<(f64, f64), BrowserError> {
        if let Some(r) = args.get("ref").and_then(|v| v.as_str()) {
            return self.resolve_ref(page, r).await;
        }
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
            .ok_or_else(|| BrowserError::Other("click requires 'ref', 'selector' or 'coordinate'".into()))?;
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

/// A ref as the page map keys it: `ref_N` (a bare `N` is accepted, as the
/// extension accepts it).
fn normalize_ref(r: &str) -> String {
    if r.starts_with("ref_") { r.to_string() } else { format!("ref_{r}") }
}

/// The extension's `find`: the tree lines that carry a ref and contain the
/// query (case-insensitive), at most 20.
fn find_in_tree(tree: &str, query: &str) -> String {
    let q = query.to_lowercase();
    let matches: Vec<&str> = tree
        .lines()
        .filter(|l| l.contains("[ref_") && l.to_lowercase().contains(&q))
        .map(str::trim)
        .take(20)
        .collect();
    if matches.is_empty() {
        return format!(
            "No elements found matching \"{query}\". Try a different search term or use read_page to see all elements on the page."
        );
    }
    format!(
        "Found {} element{} matching \"{query}\":\n\n{}",
        matches.len(),
        if matches.len() == 1 { "" } else { "s" },
        matches.join("\n")
    )
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
    let client = tls::http_client().build()?;
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
    // Windows binaries carry the .exe suffix; every probe below must use the
    // platform name or detection silently fails on Windows.
    const BIN_NAME: &str = if cfg!(windows) { "obscura.exe" } else { "obscura" };

    if let Ok(p) = std::env::var("OBSCURA_BIN") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join(BIN_NAME);
            if sibling.exists() {
                return Some(sibling);
            }
        }
    }
    let bundled = PathBuf::from(data_dir).join("bin").join(BIN_NAME);
    if bundled.exists() {
        return Some(bundled);
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let cand = dir.join(BIN_NAME);
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
            chromium: false,
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

    /// The idle reaper must tear the browser down after the configured quiet
    /// period — and a fresh use afterwards must relaunch cleanly. Guards the
    /// resident-forever regression (a cloud bot held chromium 6.7 days).
    #[tokio::test]
    #[ignore = "requires the obscura binary; run with --ignored"]
    async fn idle_reaper_tears_down_and_relaunches() {
        let Some(bridge) = try_bridge() else {
            eprintln!("obscura binary not found — skipping");
            return;
        };
        let bridge = Arc::new(bridge);
        bridge.spawn_idle_reaper(Duration::from_secs(2));

        let out = bridge
            .execute("navigate", &json!({"url": "https://example.com"}), "idle-test")
            .await
            .expect("navigate through tier-2");
        assert!(out.is_object() || out.is_string(), "navigate returned: {out}");
        assert!(bridge.core.lock().await.is_some(), "browser resident after use");

        // idle 2s + reaper tick (min clamp 5s) + slack
        let mut torn_down = false;
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if bridge.core.lock().await.is_none() {
                torn_down = true;
                break;
            }
        }
        assert!(torn_down, "idle reaper never tore the browser down");

        // next job relaunches transparently
        let out = bridge
            .execute("navigate", &json!({"url": "https://example.com"}), "idle-test-2")
            .await
            .expect("relaunch after idle teardown");
        assert!(out.is_object() || out.is_string());
        assert!(bridge.core.lock().await.is_some(), "browser relaunched");
    }

    /// A stale lock from another machine does not stop the launch; a clean
    /// shutdown leaves the Chromium profile unlocked, so the bot's state can
    /// commit it; the idle reaper takes the same path. A killed Chromium
    /// leaves `SingletonLock` behind.
    #[tokio::test]
    #[ignore = "requires a Chromium or Chrome; run with --ignored"]
    async fn shutdown_releases_the_chromium_profile() {
        let Some(binary) = crate::chrome::find_chrome() else {
            eprintln!("no Chromium found — skipping");
            return;
        };
        let profile = std::env::temp_dir().join(format!("nebo-cdp-close-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&profile);
        let bridge = Arc::new(CdpBridge::new(ObscuraConfig {
            binary,
            storage_dir: Some(profile.clone()),
            stealth: false,
            log_path: None,
            chromium: true,
        }));
        let lock = profile.join("SingletonLock");
        // Left by a Chromium on another pod: must not stop this one starting.
        std::fs::create_dir_all(&profile).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("another-pod-37", &lock).unwrap();

        bridge
            .execute("navigate", &json!({"url": "about:blank"}), "close-test")
            .await
            .expect("navigate");
        assert!(std::fs::symlink_metadata(&lock).is_ok(), "a running Chromium holds its profile");
        bridge.shutdown().await;
        assert!(bridge.core.lock().await.is_none());
        assert!(std::fs::symlink_metadata(&lock).is_err(), "shutdown left the profile locked");

        bridge.spawn_idle_reaper(Duration::from_secs(1));
        bridge
            .execute("navigate", &json!({"url": "about:blank"}), "close-test-2")
            .await
            .expect("relaunch after shutdown");
        let mut reaped = false;
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if bridge.core.lock().await.is_none() {
                reaped = true;
                break;
            }
        }
        assert!(reaped, "idle reaper never closed the browser");
        assert!(std::fs::symlink_metadata(&lock).is_err(), "the idle reaper left the profile locked");
        let _ = std::fs::remove_dir_all(&profile);
    }

    fn chromium_bridge(profile: &str) -> Option<CdpBridge> {
        let binary = crate::chrome::find_chrome()?;
        let dir = std::env::temp_dir().join(format!("nebo-cdp-{profile}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Some(CdpBridge::new(ObscuraConfig {
            binary,
            storage_dir: Some(dir),
            stealth: false,
            log_path: None,
            chromium: true,
        }))
    }

    /// The built-in browser speaks the extension's contract (v0.16.0 proof,
    /// web-browser-interaction: 15–17 calls because its snapshot had no refs
    /// and find, wait and screenshot were "not supported yet"). A read lists
    /// the page's controls with refs, a click by ref presses the control, a
    /// wait waits, the page is read after, find returns ref lines, and a
    /// screenshot is an image.
    #[tokio::test]
    #[ignore = "requires a Chromium or Chrome; run with --ignored"]
    async fn a_ref_from_a_read_clicks_and_the_page_reads_after() {
        let Some(bridge) = chromium_bridge("refs") else {
            eprintln!("no Chromium found — skipping");
            return;
        };
        let sid = "refs";
        let html = "data:text/html,<body><h1>Demo</h1><div id='out'></div>\
                    <button onclick=\"setTimeout(()=>{document.getElementById('out').innerHTML='<h4>Loaded text</h4>'},800)\">Start</button>\
                    <input placeholder='Your name'></body>";
        bridge.execute("navigate", &json!({ "url": html }), sid).await.expect("navigate");

        let snap = bridge.execute("read_page", &json!({ "filter": "interactive" }), sid).await.expect("read_page");
        let tree = snap["pageContent"].as_str().unwrap_or_default().to_string();
        let start = tree
            .lines()
            .find(|l| l.contains("button \"Start\""))
            .and_then(|l| l.split('[').nth(1)?.split(']').next())
            .unwrap_or_else(|| panic!("no Start button with a ref: {tree}"))
            .to_string();
        assert!(tree.contains("textbox \"Your name\" [ref_"), "{tree}");

        let found = bridge.execute("find", &json!({ "query": "start" }), sid).await.expect("find");
        assert!(found["text"].as_str().unwrap_or_default().contains(&format!("[{start}]")), "{found}");

        let clicked = bridge.execute("click", &json!({ "ref": start }), sid).await.expect("click by ref");
        assert_eq!(clicked["text"], json!(format!("Clicked on element {start}")));
        let waited = bridge.execute("wait", &json!({ "ms": 1500 }), sid).await.expect("wait");
        assert_eq!(waited["text"], json!("Waited for 1.5 seconds"));
        let after = bridge.execute("read_page", &json!({}), sid).await.expect("read after");
        assert!(after["pageContent"].as_str().unwrap_or_default().contains("heading \"Loaded text\""), "{after}");
        // The same element keeps its ref across reads.
        assert!(after["pageContent"].as_str().unwrap_or_default().contains(&format!("button \"Start\" [{start}]")), "{after}");

        let shot = bridge.execute("screenshot", &json!({}), sid).await.expect("screenshot");
        assert_eq!(shot["format"], json!("jpeg"));
        assert!(shot["data"].as_str().is_some_and(|d| d.len() > 100), "screenshot has no image data");
        bridge.shutdown().await;
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;

    fn bridge() -> CdpBridge {
        CdpBridge::new(ObscuraConfig {
            binary: PathBuf::from("/nonexistent/browser"),
            storage_dir: None,
            stealth: false,
            log_path: None,
            chromium: true,
        })
    }

    #[test]
    fn find_returns_the_ref_lines_that_match() {
        let tree = "heading \"Demo\" [ref_1]\n  button \"Start\" [ref_2]\n  option \"Start later\"\nlink \"Docs\" [ref_3]";
        assert_eq!(find_in_tree(tree, "START"), "Found 1 element matching \"START\":\n\nbutton \"Start\" [ref_2]");
        assert!(find_in_tree(tree, "checkout").starts_with("No elements found matching \"checkout\"."));
    }

    #[test]
    fn a_bare_ref_number_is_a_ref() {
        assert_eq!(normalize_ref("7"), "ref_7");
        assert_eq!(normalize_ref("ref_7"), "ref_7");
    }

    /// A wait needs no page, and has the extension's bounds.
    #[tokio::test]
    async fn wait_waits_within_the_extensions_bounds() {
        let b = bridge();
        assert_eq!(b.execute("wait", &json!({ "ms": 10 }), "s").await.unwrap()["text"], json!("Waited for 0.01 seconds"));
        assert!(b.execute("wait", &json!({}), "s").await.is_err());
        assert!(b.execute("wait", &json!({ "ms": 31_000 }), "s").await.is_err());
    }

}
