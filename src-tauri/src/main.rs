#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::Read as _;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use tauri::{
    LogicalPosition, Manager, WebviewUrl, WebviewWindowBuilder,
    menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    webview::NewWindowResponse,
};
use tauri_plugin_global_shortcut::GlobalShortcutExt;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

const SERVER_URL: &str = "http://localhost:27895";

/// In dev mode, load from Vite dev server for HMR. In production, load from the backend.
fn frontend_url() -> &'static str {
    if cfg!(debug_assertions) {
        "http://localhost:5173"
    } else {
        SERVER_URL
    }
}

/// Set to true once the window has been restored and shown.
/// Prevents saving stale state from initial creation events.
static WINDOW_READY: AtomicBool = AtomicBool::new(false);

/// Dedup external URL opens — on_navigation and on_new_window can both fire for the same URL.
static LAST_OPENED_URL: Mutex<Option<(String, Instant)>> = Mutex::new(None);

/// Debounce sleep/wake reconnect — Tauri fires RunEvent::Resumed many times per wake.
static LAST_RESUME: Mutex<Option<Instant>> = Mutex::new(None);

/// Domains that Stripe's PaymentElement, Link, hCaptcha, and 3D-Secure need to load inside the webview.
fn is_stripe_domain(host: &str) -> bool {
    host.ends_with(".stripe.com")
        || host == "stripe.com"
        || host.ends_with(".stripecdn.com")
        || host == "stripecdn.com"
        || host.ends_with(".stripe.network")
        || host == "stripe.network"
        || host.ends_with(".hcaptcha.com")
        || host == "hcaptcha.com"
        || host.ends_with(".link.co")
        || host == "link.co"
}

// ── Window state: always stored as logical pixels ──────────────────────
// All window states (main + app windows) are stored in a single JSON map
// keyed by window label: { "main": {...}, "app-portfolio": {...}, ... }

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct WindowState {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// All window states keyed by label.
type WindowStates = std::collections::HashMap<String, WindowState>;

fn state_path() -> PathBuf {
    config::data_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("window-state.json")
}

fn load_all_states() -> WindowStates {
    let data = match std::fs::read_to_string(state_path()) {
        Ok(d) => d,
        Err(_) => return WindowStates::new(),
    };
    // Try new format (map of label → state)
    if let Ok(map) = serde_json::from_str::<WindowStates>(&data) {
        return map;
    }
    // Migrate old format (single WindowState → map with "main" key)
    if let Ok(s) = serde_json::from_str::<WindowState>(&data) {
        let mut map = WindowStates::new();
        map.insert("main".to_string(), s);
        return map;
    }
    WindowStates::new()
}

fn load_state(label: &str) -> Option<WindowState> {
    let map = load_all_states();
    let s = map.get(label)?;
    if s.width < 400.0 || s.height < 300.0 {
        return None;
    }
    Some(s.clone())
}

/// Read current window geometry, convert physical → logical, and write to disk.
fn save_state(window: &tauri::Window) {
    let Ok(scale) = window.scale_factor() else {
        return;
    };
    let Ok(size) = window.inner_size() else {
        return;
    };
    let Ok(pos) = window.outer_position() else {
        return;
    };

    // Physical → logical
    let width = size.width as f64 / scale;
    let height = size.height as f64 / scale;
    let x = pos.x as f64 / scale;
    let y = pos.y as f64 / scale;

    if width < 400.0 || height < 300.0 {
        return;
    }

    let state = WindowState {
        x,
        y,
        width,
        height,
    };

    let mut map = load_all_states();
    map.insert(window.label().to_string(), state);
    if let Ok(data) = serde_json::to_string(&map) {
        let _ = std::fs::write(state_path(), data);
    }
}

/// Tauri command: get saved window state for a given label.
/// Returns { x, y, width, height } or null if no saved state.
#[tauri::command]
fn get_window_state(label: String) -> Option<WindowState> {
    load_state(&label)
}

/// Tauri command: save a Work-panel artifact to ~/Downloads and reveal it.
/// WKWebView ignores the anchor `download` attribute, so the desktop build
/// saves natively. `file_name` must be a bare name inside the files dir.
#[tauri::command]
fn save_artifact(file_name: String) -> Result<String, String> {
    if file_name.contains('/') || file_name.contains('\\') || file_name.starts_with('.') {
        return Err("invalid file name".into());
    }
    let src = config::data_dir()
        .map_err(|e| e.to_string())?
        .join("files")
        .join(&file_name);
    if !src.is_file() {
        return Err(format!("file not found: {file_name}"));
    }
    let downloads = dirs::download_dir().ok_or("no Downloads directory")?;
    // Don't overwrite an existing download: name.ext, name (1).ext, …
    let mut dest = downloads.join(&file_name);
    if dest.exists() {
        let stem = std::path::Path::new(&file_name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| file_name.clone());
        let ext = std::path::Path::new(&file_name)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        for n in 1.. {
            let candidate = downloads.join(format!("{stem} ({n}){ext}"));
            if !candidate.exists() {
                dest = candidate;
                break;
            }
        }
    }
    std::fs::copy(&src, &dest).map_err(|e| e.to_string())?;
    reveal_in_file_manager(&dest);
    Ok(dest.to_string_lossy().into_owned())
}

/// Select the file in the platform file manager (Finder/Explorer); on Linux,
/// open the containing directory.
fn reveal_in_file_manager(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn();
    #[cfg(target_os = "linux")]
    let _ = open::that(path.parent().unwrap_or(path));
}

/// Percent-encode a query value (unreserved chars pass through). Avoids a url-crate dep.
fn pct(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

static NOTIFY_SEQ: AtomicUsize = AtomicUsize::new(0);

/// Show a branded notification HUD: a frameless, transparent, always-on-top window at
/// the top-right of the primary monitor, loading the `/notify` overlay route. Replaces
/// the osascript `display alert` modal for owner reminders/alerts. The window
/// auto-dismisses itself (the overlay closes its own window on timeout/dismiss).
#[tauri::command]
fn show_notification(
    app: tauri::AppHandle,
    title: String,
    body: String,
    agent: Option<String>,
    kind: Option<String>,
    time: Option<String>,
    accent: Option<String>,
) -> Result<(), String> {
    let url = format!(
        "{}/notify?title={}&body={}&agent={}&kind={}&time={}&accent={}",
        SERVER_URL,
        pct(&title),
        pct(&body),
        pct(agent.as_deref().unwrap_or("Nebo")),
        pct(kind.as_deref().unwrap_or("reminder")),
        pct(time.as_deref().unwrap_or("")),
        pct(accent.as_deref().unwrap_or("violet")),
    );

    const W: f64 = 412.0;
    const H: f64 = 200.0;
    const MARGIN: f64 = 14.0;

    // Stack newer notifications below older still-open ones (top-right column).
    let open = app
        .webview_windows()
        .keys()
        .filter(|l| l.starts_with("notify-"))
        .count() as f64;
    let seq = NOTIFY_SEQ.fetch_add(1, Ordering::Relaxed);
    let label = format!("notify-{seq}");

    let win = WebviewWindowBuilder::new(
        &app,
        &label,
        WebviewUrl::External(url.parse().map_err(|e| format!("bad notify url: {e}"))?),
    )
    .title("Nebo")
    .inner_size(W, H)
    .resizable(false)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    .visible(false)
    .build()
    .map_err(|e| format!("create notify window: {e}"))?;

    if let Ok(Some(mon)) = win.primary_monitor() {
        let scale = mon.scale_factor();
        let mon_w = mon.size().width as f64 / scale;
        let x = (mon_w - W - MARGIN).max(MARGIN);
        let y = MARGIN + open * (H - 24.0);
        let _ = win.set_position(LogicalPosition::new(x, y));
    }
    let _ = win.show();
    Ok(())
}

/// Open a URL in the system browser, deduplicating rapid repeats of the same URL.
fn open_external(url: &str) {
    let mut last = LAST_OPENED_URL.lock().unwrap();
    if let Some((ref prev_url, ref when)) = *last {
        if prev_url == url && when.elapsed().as_millis() < 2000 {
            tracing::debug!("Suppressed duplicate open for: {url}");
            return;
        }
    }
    *last = Some((url.to_string(), Instant::now()));
    tracing::info!("Opening external URL in browser: {url}");
    let _ = open::that(url);
}

// ── neboapp:// protocol helpers ────────────────────────────────────────

/// Resolve the UI directory for an app agent by scanning the filesystem.
/// Checks user agents first (higher priority), then marketplace agents.
fn resolve_app_ui_dir(agent_id: &str) -> Option<PathBuf> {
    let data_dir = config::data_dir().ok()?;

    // Installs are keyed by SLUG and the UI is nested under a VERSION dir
    // (`agents/<slug>/<version>/ui`); `agent_id` is the artifact UUID. So match
    // the UUID against the `id` in each candidate's manifest.json rather than the
    // directory name. Among matches, prefer the newest version dir. (The
    // directory-name == agent_id case is still handled, for loose/user agents
    // stored by id without a manifest.)
    let mut best: Option<(String, PathBuf)> = None; // (version-key, ui_dir)
    for sub in &["user/agents", "nebo/agents"] {
        let agents_dir = data_dir.join(sub);
        if !agents_dir.is_dir() {
            continue;
        }
        let Ok(slug_dirs) = std::fs::read_dir(&agents_dir) else {
            continue;
        };
        for slug_entry in slug_dirs.flatten() {
            let slug_dir = slug_entry.path();
            if !slug_dir.is_dir() {
                continue;
            }
            let name_match = slug_entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(agent_id);

            // Candidate dirs holding a ui/: the slug dir itself (loose agents)
            // and each version subdir (marketplace installs).
            let mut candidates: Vec<PathBuf> = vec![slug_dir.clone()];
            if let Ok(versions) = std::fs::read_dir(&slug_dir) {
                candidates.extend(versions.flatten().map(|v| v.path()).filter(|p| p.is_dir()));
            }
            for cand in candidates {
                let ui = cand.join("ui");
                if !ui.is_dir() {
                    continue;
                }
                let id_match = cand
                    .join("manifest.json")
                    .to_str()
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                    .and_then(|m| m.get("id").and_then(|v| v.as_str()).map(String::from))
                    .is_some_and(|id| id.eq_ignore_ascii_case(agent_id));
                if !(id_match || name_match) {
                    continue;
                }
                // Prefer the newest version dir among matches.
                let ver_key = cand.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if best.as_ref().is_none_or(|(k, _)| ver_key > *k) {
                    best = Some((ver_key, ui));
                }
            }
        }
    }
    best.map(|(_, ui)| ui)
}

/// Generate the bridge script + meta tags injected into every HTML page served
/// via the neboapp:// protocol.
///
/// WebKit treats custom-scheme pages as opaque origins and silently blocks
/// cross-origin fetch to `http://localhost:*`. The bridge intercepts fetch and
/// XHR, rewriting HTTP URLs that target the Nebo server back to `neboapp://`
/// so they route through `register_uri_scheme_protocol` (which proxies
/// server-side via ureq — no CORS needed).
fn neboapp_bridge(agent_id: &str) -> String {
    format!(
        concat!(
            r#"<meta name="nebo-app-id" content="{id}">"#,
            r#"<meta name="htmx-config" content='{{"selfRequestsOnly":false}}'>"#,
            r#"<script>(function(){{var O="neboapp://{id}";"#,
            r#"function rw(u){{if(typeof u!=="string")return u;"#,
            r#"var h=u.match(/^https?:\/\/(?:localhost|127\.0\.0\.1):27895(\/.*)/);if(h)return O+h[1];"#,
            r#"return u}}"#,
            r#"var F=window.fetch;window.fetch=function(i,o){{"#,
            r#"if(typeof i==="string")i=rw(i);"#,
            r#"else if(i instanceof Request)i=new Request(rw(i.url),i);"#,
            r#"return F.call(this,i,o)}};"#,
            r#"var X=XMLHttpRequest.prototype.open;"#,
            r#"XMLHttpRequest.prototype.open=function(){{"#,
            r#"arguments[1]=rw(arguments[1]);return X.apply(this,arguments)}}}})();</script>"#,
        ),
        id = agent_id
    )
}

/// Simple MIME type detection from file extension.
fn mime_from_extension(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "application/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("wasm") => "application/wasm",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        _ => "application/octet-stream",
    }
}

// ────────────────────────────────────────────────────────────────────────

fn main() {
    // Chrome native messaging: if Chrome launched us, run as a relay — no GUI.
    // Chrome passes `chrome-extension://EXTENSION_ID/` as the sole argument.
    {
        let args: Vec<String> = std::env::args().collect();
        if args.iter().any(|a| a.starts_with("chrome-extension://")) {
            let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
            if let Err(e) = rt.block_on(run_native_messaging()) {
                eprintln!("[nebo-relay] error: {e}");
                std::process::exit(1);
            }
            return;
        }
    }

    dotenvy::dotenv().ok();

    // Terminal layer (with ANSI colors)
    let env_filter =
        || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let stdout_layer = fmt::layer().with_filter(env_filter());

    // File layer (append to <data_dir>/logs/nebo.log)
    let file_layer = config::data_dir().ok().and_then(|dir| {
        let log_dir = dir.join("logs");
        std::fs::create_dir_all(&log_dir).ok()?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("nebo.log"))
            .ok()?;
        Some(
            fmt::layer()
                .with_writer(Mutex::new(file))
                .with_ansi(false)
                .with_filter(env_filter()),
        )
    });

    tracing_subscriber::registry()
        .with(stdout_layer)
        .with(file_layer)
        .init();

    // Install panic hook so panics are logged before the process dies
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".into());
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "Box<dyn Any>".into()
        };
        tracing::error!(location = %location, "PANIC: {}", payload);
        eprintln!("PANIC at {}: {}", location, payload);
        // Also write to crash log in case tracing is broken
        if let Ok(dir) = config::data_dir() {
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("logs/nebo-crash.log"))
                .and_then(|mut f| {
                    use std::io::Write;
                    writeln!(f, "PANIC at {}: {}", location, payload)
                });
        }
    }));

    let mut cfg = config::Config::load_embedded().expect("failed to load config");
    let settings = config::load_settings().expect("failed to load settings");
    cfg.auth.access_secret = settings.access_secret;
    cfg.auth.access_expire = settings.access_expire;
    cfg.auth.refresh_token_expire = settings.refresh_token_expire;
    config::ensure_data_dir().expect("failed to create data directory");

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(async {
            tracing::info!("starting Nebo server thread");
            match server::run(cfg, true).await {
                Ok(()) => tracing::info!("server shut down cleanly"),
                Err(e) => tracing::error!("server exited with error: {e}"),
            }
        });
        // Server exited (SIGTERM/Ctrl+C) — exit the whole process so the Tauri window closes too
        tracing::info!("server thread exited, terminating process");
        std::process::exit(0);
    });

    wait_for_server();

    let saved = load_state("main");

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_window_state,
            save_artifact,
            show_notification
        ])
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .register_uri_scheme_protocol("neboapp", |_ctx, request| {
            let uri = request.uri();
            let agent_id = uri.host().unwrap_or("");
            let path = uri.path();
            let method = request.method().as_str();

            tracing::debug!(agent_id, path, method, "neboapp:// protocol handler");

            // CORS preflight — WebKit treats custom-scheme pages as opaque
            // origins, so every fetch() is cross-origin.
            if method == "OPTIONS" {
                return http::Response::builder()
                    .status(204)
                    .header("Access-Control-Allow-Origin", "*")
                    .header("Access-Control-Allow-Methods", "GET, POST, PUT, PATCH, DELETE, OPTIONS")
                    .header("Access-Control-Allow-Headers", "Content-Type, Authorization")
                    .header("Access-Control-Max-Age", "86400")
                    .body(Vec::new())
                    .unwrap();
            }

            // Resolve the agent's UI directory
            let ui_dir = match resolve_app_ui_dir(agent_id) {
                Some(dir) => dir,
                None => {
                    return http::Response::builder()
                        .status(404)
                        .header("Content-Type", "text/plain")
                        .header("Access-Control-Allow-Origin", "*")
                        .body(format!("App not found: {agent_id}").into_bytes())
                        .unwrap();
                }
            };

            // Prevent directory traversal
            let clean_path = path.trim_start_matches('/');
            if clean_path.contains("..") {
                return http::Response::builder()
                    .status(400)
                    .header("Content-Type", "text/plain")
                    .header("Access-Control-Allow-Origin", "*")
                    .body(b"Bad Request".to_vec())
                    .unwrap();
            }

            // Try exact file (empty path → index.html)
            let file_path = if clean_path.is_empty() {
                ui_dir.join("index.html")
            } else {
                ui_dir.join(clean_path)
            };

            if file_path.is_file() {
                if let Ok(data) = std::fs::read(&file_path) {
                    let is_html = mime_from_extension(&file_path).starts_with("text/html");
                    let body = if is_html {
                        let html = String::from_utf8_lossy(&data);
                        let bridge = neboapp_bridge(agent_id);
                        html.replacen("<head>", &format!("<head>{bridge}"), 1)
                            .into_bytes()
                    } else {
                        data
                    };
                    return http::Response::builder()
                        .status(200)
                        .header("Content-Type", mime_from_extension(&file_path))
                        .header("Access-Control-Allow-Origin", "*")
                        .body(body)
                        .unwrap();
                }
            }

            // Not a static file — proxy to the Nebo server.
            let query = uri.query().unwrap_or("");
            let req_body = request.body().clone();
            let content_type_in = request
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();

            let urls = if query.is_empty() {
                vec![
                    format!("http://127.0.0.1:27895{}", path),
                    format!("http://127.0.0.1:27895/apps/{}/api{}", agent_id, path),
                ]
            } else {
                vec![
                    format!("http://127.0.0.1:27895{}?{}", path, query),
                    format!("http://127.0.0.1:27895/apps/{}/api{}?{}", agent_id, path, query),
                ]
            };

            let agent = ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(30))
                .build();

            for url in &urls {
                tracing::debug!(url, method, "neboapp proxy attempt");
                let req = match method {
                    "POST" => agent.post(url),
                    "PUT" => agent.put(url),
                    "DELETE" => agent.delete(url),
                    "PATCH" => agent.patch(url),
                    _ => agent.get(url),
                };
                let result = if matches!(method, "POST" | "PUT" | "PATCH") && !req_body.is_empty() {
                    req.set("Content-Type", &content_type_in)
                        .send_bytes(&req_body)
                } else {
                    req.call()
                };
                match &result {
                    Ok(resp) => tracing::debug!(status = resp.status(), url, "neboapp proxy response"),
                    Err(e) => tracing::debug!(error = %e, url, "neboapp proxy error"),
                }
                if let Ok(resp) = result {
                    let status = resp.status();
                    if status != 404 {
                        let ct = resp
                            .header("Content-Type")
                            .unwrap_or("application/octet-stream")
                            .to_string();
                        let mut body = Vec::new();
                        let _ = resp.into_reader().read_to_end(&mut body);
                        return http::Response::builder()
                            .status(status)
                            .header("Content-Type", ct)
                            .header("Access-Control-Allow-Origin", "*")
                            .body(body)
                            .unwrap();
                    }
                }
            }

            // SPA fallback: if proxy returned 404 or failed, and path has
            // no file extension, serve index.html for client-side routing.
            let ext = std::path::Path::new(path).extension();
            if ext.is_none() || ext.and_then(|e| e.to_str()) == Some("html") {
                let index = ui_dir.join("index.html");
                if let Ok(data) = std::fs::read(&index) {
                    let html = String::from_utf8_lossy(&data);
                    let bridge = neboapp_bridge(agent_id);
                    let body = html.replacen("<head>", &format!("<head>{bridge}"), 1);
                    return http::Response::builder()
                        .status(200)
                        .header("Content-Type", "text/html; charset=utf-8")
                        .header("Access-Control-Allow-Origin", "*")
                        .body(body.into_bytes())
                        .unwrap();
                }
            }

            http::Response::builder()
                .status(404)
                .header("Content-Type", "text/plain")
                .header("Access-Control-Allow-Origin", "*")
                .body(b"Not Found".to_vec())
                .unwrap()
        })
        .setup(move |app| {
            // Use saved logical dimensions or defaults
            let (w, h) = saved
                .as_ref()
                .map(|s| (s.width, s.height))
                .unwrap_or((1280.0, 860.0));

            let window = WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::External(frontend_url().parse().unwrap()),
            )
            .title("Nebo")
            .inner_size(w, h)
            .min_inner_size(800.0, 600.0)
            .visible(false)
            .on_navigation(|url| {
                let host = url.host_str().unwrap_or("");
                if host == "localhost" || host == "127.0.0.1" || is_stripe_domain(host) {
                    return true;
                }
                open_external(url.as_str());
                false
            })
            .on_new_window(|url, _features| {
                let host = url.host_str().unwrap_or("");
                // Allow localhost URLs (workspace pop-outs, etc.)
                if host == "localhost" || host == "127.0.0.1" {
                    return NewWindowResponse::Allow;
                }
                if is_stripe_domain(host) {
                    return NewWindowResponse::Allow;
                }
                open_external(url.as_str());
                NewWindowResponse::Deny
            })
            .build()?;

            // Restore saved position using LogicalPosition — Tauri handles DPI
            if let Some(ref s) = saved {
                let _ = window.set_position(LogicalPosition::new(s.x, s.y));
            }

            WINDOW_READY.store(true, Ordering::SeqCst);
            let _ = window.show();

            // Build tray
            let show = MenuItemBuilder::with_id("show", "Show Nebo").build(app)?;
            let hide = MenuItemBuilder::with_id("hide", "Hide").build(app)?;
            let sep1 = PredefinedMenuItem::separator(app)?;
            let check_update = MenuItemBuilder::with_id("check_update", "Check for Updates...").build(app)?;
            let help = MenuItemBuilder::with_id("help", "Help & Documentation").build(app)?;
            let feedback = MenuItemBuilder::with_id("feedback", "Send Feedback").build(app)?;
            let sep2 = PredefinedMenuItem::separator(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Quit Nebo").build(app)?;
            let menu = MenuBuilder::new(app)
                .items(&[&show, &hide, &sep1, &check_update, &help, &feedback, &sep2, &quit])
                .build()?;

            let tray_icon =
                tauri::image::Image::from_bytes(include_bytes!("../icons/tray-icon.png"))?;

            let _tray = TrayIconBuilder::new()
                .icon(tray_icon)
                .icon_as_template(true)
                .menu(&menu)
                .tooltip("Nebo — Running")
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.unminimize();
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "hide" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.hide();
                        }
                    }
                    "check_update" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.unminimize();
                            let _ = w.show();
                            let _ = w.set_focus();
                            let _ = w.eval("if(window.__NEBO_CHECK_UPDATE__)window.__NEBO_CHECK_UPDATE__()");
                        }
                    }
                    "help" => {
                        open_external("https://neboai.com/docs");
                    }
                    "feedback" => {
                        open_external("https://github.com/NeboLoop/nebo/issues");
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.unminimize();
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            // Global hotkey: Cmd+Shift+Space (macOS) / Ctrl+Shift+Space (others)
            // Toggles a floating prompt window for quick input
            {
                use tauri_plugin_global_shortcut::ShortcutState;

                let handle = app.handle().clone();
                app.global_shortcut().on_shortcut(
                    if cfg!(target_os = "macos") {
                        "CmdOrCtrl+Shift+Space"
                    } else {
                        "Ctrl+Shift+Space"
                    },
                    move |_app, shortcut, event| {
                        if event.state != ShortcutState::Pressed {
                            return;
                        }
                        let _ = shortcut; // silence unused warning
                        toggle_prompt_window(&handle);
                    },
                )?;
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            match event {
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    save_state(window);
                    if window.label() == "main" {
                        // Hide to tray — the server keeps running in the background.
                        // Users quit via tray menu "Quit Nebo" or Cmd+Q.
                        api.prevent_close();
                        let _ = window.hide();
                    }
                    // App windows (app-*) close normally — state was saved above.
                }
                tauri::WindowEvent::DragDrop(event) => {
                    // Tauri intercepts file drops at the OS level — browser ondrop never fires.
                    // Push dropped paths into the Svelte input via eval() on the webview.
                    if let Some(wv) = window.app_handle().get_webview_window(window.label()) {
                        match event {
                            tauri::DragDropEvent::Enter { .. } => {
                                let _ = wv.eval("if(window.__NEBO_DRAG_ENTER__)window.__NEBO_DRAG_ENTER__()");
                            }
                            tauri::DragDropEvent::Leave => {
                                let _ = wv.eval("if(window.__NEBO_DRAG_LEAVE__)window.__NEBO_DRAG_LEAVE__()");
                            }
                            tauri::DragDropEvent::Drop { paths, .. } => {
                                let json_paths: Vec<String> = paths
                                    .iter()
                                    .filter_map(|p| p.to_str())
                                    .map(|s| s.to_string())
                                    .collect();
                                if let Ok(json) = serde_json::to_string(&json_paths) {
                                    let js = format!("if(window.__NEBO_INSERT_FILES__)window.__NEBO_INSERT_FILES__({json})");
                                    let _ = wv.eval(&js);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => {
                    // Only save after the main window has been fully initialized.
                    // App windows are always ready (created by user action).
                    let ready = window.label() != "main"
                        || WINDOW_READY.load(Ordering::SeqCst);
                    if ready
                        && !window.is_minimized().unwrap_or_default()
                        && !window.is_maximized().unwrap_or_default()
                    {
                        save_state(window);
                    }
                }
                _ => {}
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Nebo desktop")
        .run(|_app, event| {
            if let tauri::RunEvent::Resumed { .. } = event {
                // Debounce: Tauri fires Resumed many times per wake cycle.
                // Only reconnect if >5s since last resume event.
                let should_reconnect = {
                    let mut last = LAST_RESUME.lock().unwrap();
                    let now = Instant::now();
                    if last.map_or(true, |t| now.duration_since(t).as_secs() >= 5) {
                        *last = Some(now);
                        true
                    } else {
                        false
                    }
                };
                if should_reconnect {
                    tracing::info!("system resumed from sleep, triggering NeboAI reconnect");
                    // Fire-and-forget POST to the local backend — raw TCP to avoid extra deps.
                    std::thread::spawn(|| {
                        use std::io::Write;
                        if let Ok(mut stream) = std::net::TcpStream::connect("127.0.0.1:27895") {
                            let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(5)));
                            let _ = stream.write_all(
                                b"POST /api/v1/neboai/reconnect HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n"
                            );
                        }
                    });
                }
            }
        });
}

/// Toggle the floating prompt window. Creates it on first use, then shows/hides.
fn toggle_prompt_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("prompt") {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        } else {
            let _ = win.show();
            let _ = win.set_focus();
        }
    } else {
        // Create a small centered floating window pointing at the prompt route
        let url = format!("{}/prompt", SERVER_URL);
        match WebviewWindowBuilder::new(app, "prompt", WebviewUrl::External(url.parse().unwrap()))
            .title("Nebo")
            .inner_size(600.0, 80.0)
            .resizable(false)
            .decorations(false)
            .always_on_top(true)
            .visible(true)
            .center()
            .build()
        {
            Ok(win) => {
                let _ = win.set_focus();
            }
            Err(e) => {
                tracing::warn!("Failed to create prompt window: {e}");
            }
        }
    }
}

fn wait_for_server() {
    use std::net::TcpStream;

    for _ in 0..60 {
        if TcpStream::connect("127.0.0.1:27895").is_ok() {
            tracing::info!("Server is ready");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    tracing::warn!("Server did not become ready in 15s, launching window anyway");
}

// ── Chrome native messaging relay ─────────────────────────────────────
// Lightweight stdin/stdout bridge between Chrome extension and Nebo server (WS).
// Extension ←stdin/stdout→ this process ←WebSocket→ Nebo server

async fn run_native_messaging() -> anyhow::Result<()> {
    use futures::{SinkExt, StreamExt};
    use std::sync::Arc;
    use tokio::io::AsyncReadExt;

    eprintln!("[nebo-relay] starting native messaging bridge");

    let ws_url = "ws://127.0.0.1:27895/ws/extension";

    // Retry WS connection with backoff — server may not be ready yet
    let ws_stream = {
        let mut attempts = 0u32;
        loop {
            match tokio_tungstenite::connect_async(ws_url).await {
                Ok((stream, _)) => {
                    eprintln!("[nebo-relay] connected to server at {ws_url}");
                    break stream;
                }
                Err(e) if attempts < 10 => {
                    attempts += 1;
                    let delay = std::cmp::min(500 * 2u64.pow(attempts - 1), 5000);
                    eprintln!(
                        "[nebo-relay] WS connect attempt {attempts}/10 failed ({e}), retrying in {delay}ms"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                }
                Err(e) => {
                    eprintln!("[nebo-relay] giving up after 10 attempts: {e}");
                    std::process::exit(1);
                }
            }
        }
    };

    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // Detect which browser launched this relay
    let browser = detect_parent_browser();
    eprintln!("[nebo-relay] detected browser: {browser}");

    let hello = serde_json::json!({
        "type": "hello",
        "browser": browser,
        "relay": true,
    });
    let _ = ws_tx
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&hello).unwrap().into(),
        ))
        .await;

    let mut stdin = tokio::io::stdin();
    let stdout = Arc::new(tokio::sync::Mutex::new(tokio::io::stdout()));

    let stdout_send = stdout.clone();

    // stdin → WS
    let send_task = tokio::spawn(async move {
        loop {
            let mut len_buf = [0u8; 4];
            if stdin.read_exact(&mut len_buf).await.is_err() {
                eprintln!("[nebo-relay] stdin closed");
                break;
            }
            let len = u32::from_le_bytes(len_buf) as usize;
            if len > 1_048_576 {
                eprintln!("[nebo-relay] message too large: {len} bytes");
                break;
            }

            let mut body = vec![0u8; len];
            if stdin.read_exact(&mut body).await.is_err() {
                break;
            }

            let msg: serde_json::Value = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[nebo-relay] malformed JSON: {e}");
                    continue;
                }
            };

            let msg_type = msg["type"].as_str().unwrap_or("");

            match msg_type {
                "hello" => {
                    let resp = serde_json::json!({"type": "connected"});
                    let _ = write_native_message(&stdout_send, &resp).await;
                    let text = serde_json::to_string(&msg).unwrap();
                    let _ = ws_tx
                        .send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
                        .await;
                    continue;
                }
                "ping" => {
                    let resp = serde_json::json!({"type": "pong"});
                    let _ = write_native_message(&stdout_send, &resp).await;
                    continue;
                }
                _ => {}
            }

            let text = serde_json::to_string(&msg).unwrap();
            if ws_tx
                .send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
                .await
                .is_err()
            {
                eprintln!("[nebo-relay] WS send failed");
                break;
            }
        }
    });

    // WS → stdout
    let stdout_recv = stdout.clone();
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                    if write_native_message(&stdout_recv, &parsed).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    tokio::select! {
        _ = send_task => {}
        _ = recv_task => {}
    }

    eprintln!("[nebo-relay] shutting down");
    std::process::exit(0);
}

async fn write_native_message(
    stdout: &tokio::sync::Mutex<tokio::io::Stdout>,
    msg: &serde_json::Value,
) -> Result<(), std::io::Error> {
    use tokio::io::AsyncWriteExt;
    let json_bytes = serde_json::to_vec(msg).unwrap();
    let len = (json_bytes.len() as u32).to_le_bytes();
    let mut out = stdout.lock().await;
    out.write_all(&len).await?;
    out.write_all(&json_bytes).await?;
    out.flush().await?;
    Ok(())
}

fn detect_parent_browser() -> String {
    #[cfg(unix)]
    {
        let ppid = std::os::unix::process::parent_id();
        if let Ok(output) = std::process::Command::new("ps")
            .args(["-p", &ppid.to_string(), "-o", "comm="])
            .output()
        {
            let parent = String::from_utf8_lossy(&output.stdout)
                .trim()
                .to_lowercase();
            if parent.contains("brave") {
                return "brave".into();
            }
            if parent.contains("chrome") {
                return "chrome".into();
            }
            if parent.contains("firefox") {
                return "firefox".into();
            }
            if parent.contains("edge") {
                return "edge".into();
            }
            if parent.contains("arc") {
                return "arc".into();
            }
            if !parent.is_empty() {
                return parent;
            }
        }
    }
    "unknown".into()
}
