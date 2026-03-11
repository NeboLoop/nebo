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
        .build(tauri::generate_context!())
        .expect("error while building Nebo desktop")
        .run(|app, event| {
            // Cmd+Q / tray Quit fires ExitRequested — let it through.
            // Window state is already persisted on every move/resize.
            let _ = (&app, &event);
        });
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
