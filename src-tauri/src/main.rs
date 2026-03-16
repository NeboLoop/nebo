#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use tauri::{
    menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    webview::NewWindowResponse,
    LogicalPosition, Manager, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_global_shortcut::GlobalShortcutExt;
use tracing_subscriber::EnvFilter;

const SERVER_URL: &str = "http://localhost:27895";

/// Set to true once the window has been restored and shown.
/// Prevents saving stale state from initial creation events.
static WINDOW_READY: AtomicBool = AtomicBool::new(false);

/// Dedup external URL opens — on_navigation and on_new_window can both fire for the same URL.
static LAST_OPENED_URL: Mutex<Option<(String, Instant)>> = Mutex::new(None);

// ── Window state: always stored as logical pixels ──────────────────────

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct WindowState {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn state_path() -> PathBuf {
    config::data_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("window-state.json")
}

fn load_state() -> Option<WindowState> {
    let data = std::fs::read_to_string(state_path()).ok()?;
    let s: WindowState = serde_json::from_str(&data).ok()?;
    if s.width < 400.0 || s.height < 300.0 {
        return None;
    }
    Some(s)
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
    if let Ok(data) = serde_json::to_string(&state) {
        let _ = std::fs::write(state_path(), data);
    }
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

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let mut cfg = config::Config::load_embedded().expect("failed to load config");
    let settings = config::load_settings().expect("failed to load settings");
    cfg.auth.access_secret = settings.access_secret;
    cfg.auth.access_expire = settings.access_expire;
    cfg.auth.refresh_token_expire = settings.refresh_token_expire;
    config::ensure_data_dir().expect("failed to create data directory");

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(async {
            if let Err(e) = server::run(cfg, true).await {
                tracing::error!("Server error: {e}");
            }
        });
    });

    wait_for_server();

    let saved = load_state();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .setup(move |app| {
            // Use saved logical dimensions or defaults
            let (w, h) = saved
                .as_ref()
                .map(|s| (s.width, s.height))
                .unwrap_or((1280.0, 860.0));

            let window = WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::External(SERVER_URL.parse().unwrap()),
            )
            .title("Nebo")
            .inner_size(w, h)
            .min_inner_size(800.0, 600.0)
            .visible(false)
            .on_navigation(|url| {
                if url.host_str() == Some("localhost")
                    || url.host_str() == Some("127.0.0.1")
                {
                    return true;
                }
                open_external(url.as_str());
                false
            })
            .on_new_window(|url, _features| {
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
            let separator = PredefinedMenuItem::separator(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Quit Nebo").build(app)?;
            let menu = MenuBuilder::new(app)
                .items(&[&show, &hide, &separator, &quit])
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
                    // Hide to tray — the server keeps running in the background.
                    // Users quit via tray menu "Quit Nebo" or Cmd+Q.
                    api.prevent_close();
                    save_state(window);
                    let _ = window.hide();
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
                    if WINDOW_READY.load(Ordering::SeqCst)
                        && !window.is_minimized().unwrap_or_default()
                        && !window.is_maximized().unwrap_or_default()
                    {
                        save_state(window);
                    }
                }
                _ => {}
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Nebo desktop");
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
    use tokio::io::AsyncReadExt;
    use std::sync::Arc;

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
            let parent = String::from_utf8_lossy(&output.stdout).trim().to_lowercase();
            if parent.contains("brave") { return "brave".into(); }
            if parent.contains("chrome") { return "chrome".into(); }
            if parent.contains("firefox") { return "firefox".into(); }
            if parent.contains("edge") { return "edge".into(); }
            if parent.contains("arc") { return "arc".into(); }
            if !parent.is_empty() { return parent; }
        }
    }
    "unknown".into()
}
