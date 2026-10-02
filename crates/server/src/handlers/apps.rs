//! App platform handlers.
//!
//! Serves static UI assets, proxies requests to sidecar binaries,
//! and provides storage/agent/janus endpoints for the @neboai/app-sdk.

use std::path::{Path as StdPath, PathBuf};

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use futures::Stream;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::handlers::{HandlerResult, to_error_response};
use crate::state::AppState;
use db;

/// Validate the per-app auth token from the `Authorization: Bearer <token>` header.
///
/// Returns Ok(()) if the token matches the running app's token, or if the
/// request arrived through the bot's own tunnel (`X-Nebo-Tunnel-Auth` stamped
/// by comm::tunnel with a per-boot process-local secret): the hub already
/// owner-authenticated it, so an app UI opened at /t/<botID>/apps/… may invoke
/// without holding the sidecar's NEBO_APP_TOKEN. A drive-by page can't forge
/// the stamp — the secret never leaves this process, and the tunnel strips
/// inbound copies.
/// Returns 401 if the token is missing/invalid, or if the app has no running lifecycle.
async fn validate_app_token(
    state: &AppState,
    agent_id: &str,
    headers: &axum::http::HeaderMap,
) -> Result<(), Response> {
    if crate::middleware::came_through_tunnel(headers) {
        return Ok(());
    }
    if desktop_window_reaches(headers, agent_id, config::read_install_key().as_deref()) {
        return Ok(());
    }
    // The app's page as Nebo's headless browser opened it for a screenshot:
    // its console and its own calls are the app's.
    if crate::middleware::app_view_admits(headers, agent_id) {
        return Ok(());
    }

    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    let token = match token {
        Some(t) => t,
        None => {
            return Err((StatusCode::UNAUTHORIZED, "missing Authorization: Bearer <token>").into_response());
        }
    };

    let lifecycles = state.app_lifecycles.read().await;
    let lifecycle = match lifecycles.get(agent_id) {
        Some(lc) => lc,
        None => {
            return Err((StatusCode::UNAUTHORIZED, "app not running").into_response());
        }
    };

    let expected = lifecycle.app_token().await;
    if expected.is_empty() || token != expected {
        return Err((StatusCode::UNAUTHORIZED, "invalid app token").into_response());
    }

    Ok(())
}

/// The desktop's app window reaching its own app's routes (storage, agents,
/// janus, identity, ...): its `neboapp://` proxy is the owner's own client
/// and proves itself with the install key, and it passes on the page's
/// origin, `neboapp://<id>`, by the rule the app socket admits a window by
/// (`ws::from_its_desktop_window`). A window reaches only its own app.
fn desktop_window_reaches(headers: &axum::http::HeaderMap, agent_id: &str, install_key: Option<&str>) -> bool {
    bearer_is(headers, install_key) && crate::handlers::ws::from_its_desktop_window(headers, agent_id)
}

/// Check that the app has `network:{domain}` permission for the target URL.
async fn check_network_permission(
    state: &AppState,
    agent_id: &str,
    url: &str,
) -> Result<(), Response> {
    let domain = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(String::from));
    let domain = match domain {
        Some(d) => d,
        None => {
            return Err((StatusCode::BAD_REQUEST, "invalid URL").into_response());
        }
    };

    let lifecycles = state.app_lifecycles.read().await;
    if let Some(lifecycle) = lifecycles.get(agent_id) {
        let perm = format!("network:{}", domain);
        if !lifecycle.has_permission(&perm).await {
            return Err((
                StatusCode::FORBIDDEN,
                format!("app lacks permission: {}", perm),
            )
                .into_response());
        }
    }
    Ok(())
}

/// Check that the app has `subagent:{target}` permission to invoke another agent.
async fn check_subagent_permission(
    state: &AppState,
    app_agent_id: &str,
    target_agent_id: &str,
) -> Result<(), Response> {
    // Self-invocation is always allowed
    if app_agent_id == target_agent_id {
        return Ok(());
    }
    let lifecycles = state.app_lifecycles.read().await;
    if let Some(lifecycle) = lifecycles.get(app_agent_id) {
        let perm = format!("subagent:{}", target_agent_id);
        if !lifecycle.has_permission(&perm).await {
            return Err((
                StatusCode::FORBIDDEN,
                format!("app lacks permission: {}", perm),
            )
                .into_response());
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct InvokeRequest {
    message: String,
    agent: Option<String>,
    data: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvokeResponse {
    text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct JanusRequest {
    messages: Vec<ai::Message>,
    model: Option<String>,
    temperature: Option<f64>,
    max_tokens: Option<i32>,
    system: Option<String>,
}

/// Resolve the UI directory for an app agent, and whether its manifest
/// declares `device:motion` (the page may then read the gyroscope and
/// accelerometer).
async fn resolve_app_ui(state: &AppState, agent_id: &str) -> Option<(PathBuf, bool)> {
    let agents = state.agent_loader.list().await;
    let fs_match = agents.iter().find(|a| {
        a.id.as_deref() == Some(agent_id)
            || a.agent_def.name.eq_ignore_ascii_case(agent_id)
    });
    let motion = fs_match.and_then(|a| a.app_window()).is_some_and(|w| w.motion);
    if let Some(p) = fs_match.and_then(|a| a.app_ui_path.clone()) {
        return served_ui_dir(p).map(|p| (p, motion));
    }
    // Fall back to DB
    if let Ok(Some(a)) = state.store.get_agent(agent_id) {
        if let Some(p) = a.app_ui_path {
            return served_ui_dir(PathBuf::from(p)).map(|p| (p, motion));
        }
    }
    None
}

/// The one folder an app's page is served from: its `ui/`. A recorded path
/// that names the app's package folder instead (its root holds the build's
/// source `index.html`, which points at `./src/main.tsx`) is taken to its
/// `ui/`; a package with no `ui/` serves nothing.
fn served_ui_dir(recorded: PathBuf) -> Option<PathBuf> {
    if recorded.file_name().is_some_and(|n| n == "ui") {
        return Some(recorded);
    }
    let ui = recorded.join("ui");
    ui.is_dir().then_some(ui)
}

/// The file a request for `path` inside an app's `ui/` is answered with:
/// the file itself, else for a page route (no extension, or `.html`) the
/// entry page (`index.html`, then `200.html`). A missing script, style or
/// asset is a 404, never the entry page: HTML sent for a module script is
/// refused by the browser with a misleading MIME or CORS error.
fn resolve_ui_file(ui: &StdPath, path: &str) -> Result<PathBuf, StatusCode> {
    let clean_path = path.trim_start_matches('/');
    if clean_path.contains("..") {
        return Err(StatusCode::BAD_REQUEST);
    }
    let file_path = ui.join(clean_path);
    if file_path.is_file() {
        return Ok(file_path);
    }
    let ext = StdPath::new(clean_path).extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
    if !matches!(ext.as_deref(), None | Some("html" | "htm")) {
        return Err(StatusCode::NOT_FOUND);
    }
    ["index.html", "200.html"].into_iter().map(|f| ui.join(f)).find(|f| f.is_file()).ok_or(StatusCode::NOT_FOUND)
}

/// Where an app page's root-absolute references (`/assets/x.js`,
/// `url(/fonts/a.woff)`) really live: the app's `ui/`, reached from the
/// served file by `prefix` (`./`, `../`, or `ui/` for the slashless root).
/// A page is served under a sub-path (`/apps/<id>/ui/` on the bot,
/// `/t/<bot>/apps/<id>/ui/` through the tunnel), so a root path left as it
/// is lands on the bot's or the hub's own root and misses.
#[derive(Debug, Clone)]
struct Rebase<'a> {
    ui: &'a StdPath,
    prefix: String,
}

impl<'a> Rebase<'a> {
    /// For the file at `rel` (the request path inside `ui/`, as the browser
    /// sees it); `rel` empty is the slashless root `/apps/<id>/ui`.
    fn new(ui: &'a StdPath, rel: &str) -> Self {
        let prefix = if rel.is_empty() {
            "ui/".to_string()
        } else {
            match rel.trim_start_matches('/').matches('/').count() {
                0 => "./".to_string(),
                n => "../".repeat(n),
            }
        };
        Rebase { ui, prefix }
    }

    /// The `ui/` file a root path names, when there is one.
    fn names_a_file(&self, root_path: &str) -> bool {
        let path = root_path.split(['?', '#']).next().unwrap_or("");
        !path.is_empty() && !path.contains("..") && self.ui.join(path).is_file()
    }

    /// `text` (an entry page or a stylesheet) with each root-absolute
    /// `src`/`href`/`poster` attribute and CSS `url()` that names a file in
    /// `ui/` made relative to it. Anything else (another site, the bot's
    /// own `/sdk/` and `/api/`, a file the app does not have) is left alone.
    fn apply(&self, text: &str) -> String {
        static ATTR: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        static URL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let attr = ATTR.get_or_init(|| {
            regex::Regex::new(r#"(?i)(\b(?:src|href|poster)\s*=\s*["'])/([^/"'][^"']*)"#).unwrap()
        });
        let url = URL.get_or_init(|| regex::Regex::new(r#"(?i)(\burl\(\s*["']?)/([^/"')][^"')]*)"#).unwrap());
        let swap = |c: &regex::Captures<'_>| {
            if self.names_a_file(&c[2]) {
                format!("{}{}{}", &c[1], self.prefix, &c[2])
            } else {
                c[0].to_string()
            }
        };
        let text = attr.replace_all(text, swap);
        url.replace_all(&text, swap).into_owned()
    }

    /// The names at the top of `ui/`, which the page's bridge rewrites a
    /// root-absolute `fetch`/XHR into (a model, a texture, a level file).
    fn roots(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.ui)
            .map(|d| d.flatten().filter_map(|e| e.file_name().to_str().map(str::to_string)).collect())
            .unwrap_or_default();
        names.retain(|n| !n.starts_with('.'));
        names.sort();
        names
    }
}

/// GET /apps/{agent_id}/ui/ — serve the app's index.html at the root path.
pub async fn serve_app_ui_root(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    serve_app_ui_inner(&state, &agent_id, "", &headers).await
}

/// GET /apps/{agent_id}/ui/*path — serve static app assets with SPA fallback.
pub async fn serve_app_ui(
    State(state): State<AppState>,
    Path((agent_id, path)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    serve_app_ui_inner(&state, &agent_id, &path, &headers).await
}

fn range_header(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::RANGE).and_then(|v| v.to_str().ok())
}

async fn serve_app_ui_inner(state: &AppState, agent_id: &str, path: &str, headers: &HeaderMap) -> Response {
    let (ui_path, motion) = match resolve_app_ui(state, agent_id).await {
        Some(p) => p,
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    let target = match resolve_ui_file(&ui_path, path) {
        Ok(t) => t,
        Err(s) => return s.into_response(),
    };

    // Developer tooling goes only into the owner's own apps, never into one
    // installed from the marketplace (`app_dev::is_own_app`). An own app's
    // entry HTML always carries the developer script's console capture and
    // reload listener, so its employee builds it without a setting; App
    // Developer mode adds the floating console, and nothing is cached.
    let own = own_app(&state.store, agent_id);
    let devtools = own.as_ref().map(|a| Devtools {
        employee: &a.name,
        console: state.store.app_developer_mode(),
        desktop: None,
    });
    // The entry page is rebased for the address the browser asked for; a
    // stylesheet for its own place in `ui/`.
    let at = if is_entry_html(&target) {
        path.to_string()
    } else {
        target.strip_prefix(&ui_path).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default()
    };
    let rebase = Rebase::new(&ui_path, &at);
    serve_ui_file(&target, headers, devtools, motion, Some(&rebase)).await
}

/// The developer script an app page carries: whose page it is (for "Send
/// to <employee>"), and whether App Developer mode's floating console shows.
/// Present only for the owner's own apps.
#[derive(Debug, Clone, Copy)]
struct Devtools<'a> {
    employee: &'a str,
    console: bool,
    /// The desktop's app window (`neboapp://<id>/`), where the page's own
    /// address names no route: the app's id and the pass its socket carries.
    desktop: Option<DesktopPage<'a>>,
}

/// What the developer script in a desktop app window is told, since its
/// address (`neboapp://<id>/`) is not one of this server's.
#[derive(Debug, Clone, Copy)]
struct DesktopPage<'a> {
    app_id: &'a str,
    /// A pass for this one app's socket (`napp::app_view`): the window's
    /// WebSocket goes to this server directly, carrying no session.
    pass: &'a str,
    port: u16,
}

/// How long a desktop app window's socket pass lasts: longer than a window
/// stays open between reloads; each load gets a new one.
const DESKTOP_PASS_TTL: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);

/// The store and port the desktop's `neboapp://` handler answers from. It
/// runs in this process, outside the router, so `run` hands it them here.
static DESKTOP: std::sync::OnceLock<(std::sync::Arc<db::Store>, u16)> = std::sync::OnceLock::new();

/// Called once by `run`: the desktop's app windows read this store.
pub(crate) fn serve_desktop_from(store: std::sync::Arc<db::Store>, port: u16) {
    let _ = DESKTOP.set((store, port));
}

/// For the desktop's `neboapp://` handler: what an app's entry page carries
/// after the desktop bridge, by the one rule the HTTP path follows
/// (`serve_app_ui_inner`): the owner's own app gets the developer script
/// (console capture and the reload listener, the floating console with App
/// Developer mode on); an app installed from the marketplace gets nothing.
pub fn desktop_developer_script(agent_id: &str) -> String {
    DESKTOP.get().map(|(store, port)| desktop_script(store, *port, agent_id)).unwrap_or_default()
}

/// For the desktop's app-window menu: whether `agent_id` is one of the
/// owner's own apps, the only ones built and published from here.
pub fn desktop_is_own_app(agent_id: &str) -> bool {
    DESKTOP.get().is_some_and(|(store, _)| own_app(store, agent_id).is_some())
}

fn desktop_script(store: &db::Store, port: u16, agent_id: &str) -> String {
    let Some(app) = own_app(store, agent_id) else { return String::new() };
    let pass = napp::app_view::grant(&app.id, DESKTOP_PASS_TTL);
    developer_script(Devtools {
        employee: &app.name,
        console: store.app_developer_mode(),
        desktop: Some(DesktopPage { app_id: &app.id, pass: &pass, port }),
    })
}

impl Devtools<'_> {
    /// App Developer mode is on: nothing is stored or tagged.
    fn no_store(d: Option<Devtools<'_>>) -> bool {
        d.is_some_and(|d| d.console)
    }
}

/// The app row `agent_id` (an id, or a name) names, when it is one of the
/// owner's own apps.
fn own_app(store: &db::Store, agent_id: &str) -> Option<db::models::Agent> {
    store
        .get_agent(agent_id)
        .ok()
        .flatten()
        .or_else(|| store.get_agent_by_name(agent_id).ok().flatten())
        .filter(tools::app_dev::is_own_app)
}

/// Whether the file is an app's entry HTML (the SPA fallback included).
fn is_entry_html(target: &StdPath) -> bool {
    matches!(target.file_name().and_then(|n| n.to_str()), Some("index.html" | "200.html"))
}

/// Serve one resolved app file, cached by the one rule for app files:
///
/// 1. A content-hashed name (`main-0a8ksftt.js`, what an app build emits)
///    is kept for a year: a rebuild renames it, so there is nothing to bust.
/// 2. Everything else (`index.html`, `assets/hero.mp4`) is `no-cache` with a
///    strong `ETag`: every open asks, and an unchanged file answers `304`
///    with no body. Because `index.html` is always asked for, it always
///    names the newest hashed files.
/// 3. With App Developer mode on (`devtools` with its console), every file
///    is `no-store`: the guaranteed way past any cache while an employee is
///    building. The owner's own app (`devtools`, mode on or off) has its
///    entry page `no-store` always: a rebuild shows on the next load.
///
/// `motion`: the manifest declares `device:motion`, so the page's
/// Permissions-Policy lets it read the gyroscope and accelerometer.
///
/// `rebase`: the entry page and a stylesheet have their root-absolute
/// references to the app's own files made relative ([`Rebase`]); the
/// response also lets the page's own origin read it (`crossorigin` module
/// scripts and stylesheets send one).
async fn serve_ui_file(
    target: &StdPath,
    headers: &HeaderMap,
    devtools: Option<Devtools<'_>>,
    motion: bool,
    rebase: Option<&Rebase<'_>>,
) -> Response {
    let is_entry = is_entry_html(target);
    let is_css = mime_from_path(target).starts_with("text/css");
    // The owner's own app (`devtools`) is being built: its entry page is
    // never kept, so the next open or reload always names the newest build.
    let no_store = Devtools::no_store(devtools) || (is_entry && devtools.is_some());
    let meta = match fs::metadata(target).await {
        Ok(m) => m,
        Err(e) => {
            warn!(path = %target.display(), error = %e, "failed to read app UI file");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let mut response = if is_entry {
        // Entry HTML gets the SDK's documented meta-tag escape hatches
        // (nebo-app-id / nebo-base-url), mirroring Tauri's neboapp_bridge
        // for the HTTP path. The prefix (e.g. /t/<botID> through the
        // tunnel) is only knowable in the browser, so a head-first inline
        // script derives both from location before the SDK loads. Apps
        // that ship their own meta tags win — the script only fills gaps.
        let contents = match fs::read(target).await {
            Ok(c) => inject_app_bridge(c, devtools, rebase),
            Err(e) => {
                warn!(path = %target.display(), error = %e, "failed to read app UI file");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };
        // The tag is of what goes out (the injected scripts included), so a
        // Nebo update that changes them is a changed page.
        let etag = body_etag(&contents);
        if !no_store && not_modified(headers, &etag) {
            not_modified_response(&etag)
        } else {
            let mut r = Response::new(Body::from(contents));
            set_etag(&mut r, no_store, &etag);
            r
        }
    } else if let (true, Some(rebase)) = (is_css, rebase) {
        // A stylesheet goes out rebased, whole, tagged by what goes out.
        let contents = match fs::read_to_string(target).await {
            Ok(c) => rebase.apply(&c).into_bytes(),
            Err(e) => {
                warn!(path = %target.display(), error = %e, "failed to read app UI file");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };
        let etag = body_etag(&contents);
        if !no_store && not_modified(headers, &etag) {
            not_modified_response(&etag)
        } else {
            let mut r = Response::new(Body::from(contents));
            set_etag(&mut r, no_store, &etag);
            r
        }
    } else {
        let etag = file_etag(&meta);
        if !no_store && not_modified(headers, &etag) {
            not_modified_response(&etag)
        } else {
            // A range whose If-Range names an older file gets the whole new one.
            let range = if if_range_holds(headers, &etag) { range_header(headers) } else { None };
            let mut r = match serve_app_ui_range(target, range).await {
                Some(r) => r,
                None => match fs::read(target).await {
                    Ok(contents) => Response::new(Body::from(contents)),
                    Err(e) => {
                        warn!(path = %target.display(), error = %e, "failed to read app UI file");
                        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                    }
                },
            };
            r.headers_mut().insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            set_etag(&mut r, no_store, &etag);
            r
        }
    };

    // A 304 carries no body, so no content type either.
    let has_body = response.status() != StatusCode::NOT_MODIFIED;
    let h = response.headers_mut();
    if has_body {
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(mime_from_path(target))
                .unwrap_or(HeaderValue::from_static("application/octet-stream")),
        );
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(app_file_cache_control(target, no_store)));
    if let Some(origin) = own_origin(headers) {
        h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        h.append(header::VARY, HeaderValue::from_static("Origin"));
    }
    if motion {
        h.insert(
            "permissions-policy",
            HeaderValue::from_static(crate::middleware::PERMISSIONS_POLICY_WITH_MOTION),
        );
    }
    response
}

/// The request's `Origin` when it is the page's own: the same host the
/// request was sent to (a `crossorigin` module script or stylesheet on a
/// page served from here, over the tunnel or on this machine). Another
/// site's origin gets nothing.
fn own_origin(headers: &HeaderMap) -> Option<HeaderValue> {
    let origin = headers.get(header::ORIGIN)?;
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let origin_host = origin.to_str().ok()?.split_once("://")?.1;
    origin_host.eq_ignore_ascii_case(host).then(|| origin.clone())
}

/// The `Cache-Control` an app file goes out with (see `serve_ui_file`).
fn app_file_cache_control(target: &StdPath, developer: bool) -> &'static str {
    if developer {
        "no-store"
    } else if !is_entry_html(target) && is_content_hashed(target) {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}

/// A file whose name carries its content hash: `<name>-<hash>.<ext>`, the
/// hash 8 to 64 letters and digits of one case (`main-0a8ksftt.js` from an
/// app build, `chunk-5JFTZ4CW.js`, `app-a1b2c3d4.css`) with a digit set
/// between two letters somewhere in it, which a hash nearly always has and a
/// hand-made name ("hero-section2", "map-1stfloor", "shot-20260930") does
/// not. A hashed file caught by mistake would be kept for a year, so the
/// rule leans the other way: a real hash it misses is only asked about on
/// each open, like any other file.
fn is_content_hashed(target: &StdPath) -> bool {
    let Some(name) = target.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let Some((stem, ext)) = name.split_once('.') else {
        return false;
    };
    let Some((base, hash)) = stem.rsplit_once('-') else {
        return false;
    };
    if base.is_empty() || ext.is_empty() || !(8..=64).contains(&hash.len()) {
        return false;
    }
    let b = hash.as_bytes();
    let one_case = b.iter().all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
        || b.iter().all(|c| c.is_ascii_digit() || c.is_ascii_uppercase());
    one_case
        && b.windows(3).any(|w| w[0].is_ascii_alphabetic() && w[1].is_ascii_digit() && w[2].is_ascii_alphabetic())
}

/// Strong tag for a file on disk, from its size and modification time.
fn file_etag(meta: &std::fs::Metadata) -> String {
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("\"{:x}-{:x}\"", meta.len(), mtime)
}

/// Strong tag for a body built here (entry HTML after injection).
fn body_etag(body: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(body);
    format!("\"e-{}\"", hex::encode(&digest[..16]))
}

/// `If-None-Match` names this tag (weak comparison, as the header asks).
fn not_modified(headers: &HeaderMap, etag: &str) -> bool {
    let Some(inm) = headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    inm.split(',').map(str::trim).any(|t| t == "*" || t.trim_start_matches("W/") == etag)
}

/// A range applies: no `If-Range`, or one naming exactly this file (strong
/// comparison; a date or another tag means the client's copy is stale).
fn if_range_holds(headers: &HeaderMap, etag: &str) -> bool {
    match headers.get(header::IF_RANGE).and_then(|v| v.to_str().ok()) {
        None => true,
        Some(v) => v.trim() == etag,
    }
}

fn not_modified_response(etag: &str) -> Response {
    let mut r = Response::new(Body::empty());
    *r.status_mut() = StatusCode::NOT_MODIFIED;
    set_etag(&mut r, false, etag);
    r
}

/// Tag the response, except with App Developer mode on (nothing is kept).
fn set_etag(r: &mut Response, no_store: bool, etag: &str) {
    if !no_store {
        if let Ok(v) = HeaderValue::from_str(etag) {
            r.headers_mut().insert(header::ETAG, v);
        }
    }
}

/// Answer a `Range` request for an app asset: 206 with just those bytes, or
/// 416 past the end. None means no range applies; the caller sends the file.
async fn serve_app_ui_range(target: &std::path::Path, range: Option<&str>) -> Option<Response> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let len = fs::metadata(target).await.ok()?.len();
    let reply = |status: StatusCode, body: Vec<u8>, content_range: String| {
        let mut r = Response::new(Body::from(body));
        *r.status_mut() = status;
        let h = r.headers_mut();
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime_from_path(target)));
        h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
        if let Ok(v) = HeaderValue::from_str(&content_range) {
            h.insert(header::CONTENT_RANGE, v);
        }
        r
    };
    match byte_range(range, len) {
        ByteRange::Full => None,
        ByteRange::Unsatisfiable => Some(reply(StatusCode::RANGE_NOT_SATISFIABLE, Vec::new(), format!("bytes */{len}"))),
        ByteRange::Partial(start, end) => {
            let mut file = fs::File::open(target).await.ok()?;
            file.seek(std::io::SeekFrom::Start(start)).await.ok()?;
            let mut buf = vec![0u8; (end - start + 1) as usize];
            file.read_exact(&mut buf).await.ok()?;
            Some(reply(StatusCode::PARTIAL_CONTENT, buf, format!("bytes {start}-{end}/{len}")))
        }
    }
}

/// Prepend the app-bridge script to served entry HTML (after `<head>` when
/// present, else at the top). It appends the SDK's `nebo-app-id` /
/// `nebo-base-url` meta tags computed from the page's own location, which is
/// the only place the public prefix (`/t/<botID>` through the tunnel, nothing
/// on desktop) is knowable. Skips tags the app already declares.
///
/// `devtools` is set for the owner's own apps: the developer script follows
/// the bridge, before any of the app's own scripts, so it sees the app's
/// first log line.
fn inject_app_bridge(contents: Vec<u8>, devtools: Option<Devtools<'_>>, rebase: Option<&Rebase<'_>>) -> Vec<u8> {
    // `R`: the names at the top of the app's `ui/`. A root-absolute fetch or
    // XHR into one of them (`fetch('/models/bird.glb')`, a Three.js loader)
    // is sent to the app's own `ui/`, where the file is.
    const BRIDGE: &str = r#"<script data-nebo-bridge>(function(){var m=location.pathname.match(/^(.*?)\/apps\/([^/]+)\/ui(?:\/|$)/);if(!m)return;function add(n,c){if(document.querySelector('meta[name="'+n+'"]'))return;var e=document.createElement('meta');e.setAttribute('name',n);e.setAttribute('content',c);document.head.appendChild(e);}add('nebo-app-id',decodeURIComponent(m[2]));add('nebo-base-url',location.origin+m[1]);var R=__NEBO_UI_ROOTS__;if(!R.length)return;var B=m[1]+'/apps/'+m[2]+'/ui/';function rw(u){if(typeof u!=='string')return u;var o=location.origin,p=u;if(p.indexOf(o+'/')===0)p=p.slice(o.length);else if(p.charAt(0)!=='/'||p.charAt(1)==='/')return u;if(p.indexOf(B)===0)return u;var s=p.slice(1).split(/[\/?#]/)[0];return R.indexOf(s)<0?u:B+p.slice(1);}var F=window.fetch;if(typeof F==='function')window.fetch=function(i,o){if(typeof i==='string')i=rw(i);else if(i&&typeof i.url==='string'&&rw(i.url)!==i.url)i=new Request(rw(i.url),i);return F.call(this,i,o);};var X=XMLHttpRequest.prototype.open;XMLHttpRequest.prototype.open=function(){if(arguments.length>1)arguments[1]=rw(String(arguments[1]));return X.apply(this,arguments);};})();</script>"#;
    let html = match String::from_utf8(contents) {
        Ok(h) => h,
        Err(e) => return e.into_bytes(), // non-UTF-8: serve verbatim
    };
    let roots = rebase.map(Rebase::roots).unwrap_or_default();
    // `<` escaped: a file name can never close the script element.
    let bridge = BRIDGE.replace(
        "__NEBO_UI_ROOTS__",
        &serde_json::to_string(&roots).unwrap_or_else(|_| "[]".into()).replace('<', "\\u003c"),
    );
    let html = match rebase {
        Some(r) => r.apply(&html),
        None => html,
    };
    let scripts = match devtools {
        Some(d) => format!("{bridge}{}", developer_script(d)),
        None => bridge,
    };
    let lower = html.to_ascii_lowercase();
    let out = if let Some(idx) = lower.find("<head>") {
        let at = idx + "<head>".len();
        format!("{}{}{}", &html[..at], scripts, &html[at..])
    } else {
        format!("{}{}", scripts, html)
    };
    out.into_bytes()
}

/// The developer script (`app_devtools.js`) as an inline `<script>`, told
/// the app employee's name for its "Send to <employee>" button and whether
/// the floating console shows (App Developer mode).
fn developer_script(d: Devtools<'_>) -> String {
    const SCRIPT: &str = include_str!("app_devtools.js");
    let mut config = serde_json::json!({ "employee": d.employee, "console": d.console });
    if let Some(w) = d.desktop {
        // Its fetches go through the window's own scheme (the desktop proxies
        // them to this server); its socket comes here with the pass.
        let id = urlencoding::encode(w.app_id);
        config["appId"] = w.app_id.into();
        config["api"] = format!("neboapp://{id}/api/v1/apps/{id}").into();
        config["socket"] = format!("ws://127.0.0.1:{}/k/{}/ws/app/{id}", w.port, w.pass).into();
    }
    // `<` escaped: a name can never close the script element.
    let config = config.to_string().replace('<', "\\u003c");
    format!(
        "<script data-nebo-devtools>{}</script>",
        SCRIPT.replace("__NEBO_DEVTOOLS_CONFIG__", &config)
    )
}

/// The name the app's employee goes by (its id when the row is gone).
fn app_employee_name(state: &AppState, agent_id: &str) -> String {
    state
        .store
        .get_agent(agent_id)
        .ok()
        .flatten()
        .map(|a| a.name)
        .unwrap_or_else(|| agent_id.to_string())
}

/// GET /sdk/nebo.global.js — serve the app SDK IIFE build for vanilla/HTMX apps.
pub async fn serve_sdk_iife() -> Response {
    // Dev freshness first (live node_modules on a source checkout), then the
    // copy embedded with the SPA (app/static/sdk/ → build/sdk/) — the ONLY
    // form that exists in the shipped image; the node_modules path 404'd on
    // every cloud bot.
    let path =
        StdPath::new(env!("CARGO_MANIFEST_DIR")).join("../../app/node_modules/@neboai/app-sdk/dist/nebo.global.js");
    let contents: Option<Vec<u8>> = match fs::read(&path).await {
        Ok(c) => Some(c),
        Err(_) => crate::spa::embedded_asset("sdk/nebo.global.js").map(|f| f.data.into_owned()),
    };
    match contents {
        Some(contents) => {
            let mut response = Response::new(Body::from(contents));
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/javascript; charset=utf-8"),
            );
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600"),
            );
            response
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// GET/PUT/DELETE /apps/{agent_id}/storage/{key} — app-scoped KV storage.
///
/// The one store an app's page and its employee share (`tools::app_data`,
/// which owns the encoding). Every write tells the app's open views
/// (`app_data_changed`), so a second window of the same app refreshes too.
pub async fn get_storage(
    State(state): State<AppState>,
    Path((agent_id, key)): Path<(String, String)>,
    headers: axum::http::HeaderMap,
) -> Response {
    if let Err(r) = validate_app_token(&state, &agent_id, &headers).await {
        return r;
    }
    match tools::app_data::read(&state.store, &agent_id, &key) {
        Ok(Some(v)) => axum::Json(serde_json::json!({ "key": key, "value": v })).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            warn!(error = %e, "app storage get failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn put_storage(
    State(state): State<AppState>,
    Path((agent_id, key)): Path<(String, String)>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> Response {
    if let Err(r) = validate_app_token(&state, &agent_id, &headers).await {
        return r;
    }
    let value = match body.get("value") {
        Some(v) => v.to_string(),
        None => return StatusCode::BAD_REQUEST.into_response(),
    };
    match tools::app_data::write(&state.store, &agent_id, &key, &value) {
        Ok(()) => {
            state.hub.broadcast(
                tools::app_data::CHANGED_EVENT,
                tools::app_data::changed(&agent_id, &[&key], "set", "page"),
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => {
            warn!(error = %e, "app storage put failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn delete_storage(
    State(state): State<AppState>,
    Path((agent_id, key)): Path<(String, String)>,
    headers: axum::http::HeaderMap,
) -> Response {
    if let Err(r) = validate_app_token(&state, &agent_id, &headers).await {
        return r;
    }
    match tools::app_data::remove(&state.store, &agent_id, &key) {
        Ok(()) => {
            state.hub.broadcast(
                tools::app_data::CHANGED_EVENT,
                tools::app_data::changed(&agent_id, &[&key], "delete", "page"),
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => {
            warn!(error = %e, "app storage delete failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn list_storage(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Response {
    if let Err(r) = validate_app_token(&state, &agent_id, &headers).await {
        return r;
    }
    match tools::app_data::list(&state.store, &agent_id) {
        Ok(items) => axum::Json(serde_json::json!({ "items": items })).into_response(),
        Err(e) => {
            warn!(error = %e, "app storage list failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// The longest batch one devlog POST keeps.
const DEVLOG_MAX_BATCH: usize = 200;

/// One console line the developer script sends.
#[derive(Debug, Deserialize)]
struct DevlogEntry {
    #[serde(default)]
    level: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    time: i64,
}

#[derive(Debug, Deserialize)]
struct DevlogBatch {
    #[serde(default)]
    entries: Vec<DevlogEntry>,
}

/// Whether `agent_id`'s console may be written or sent from this request,
/// before the app's token is checked: the id is one of the owner's own apps
/// (the ones whose pages carry the developer script; never one installed
/// from the marketplace), and a page that names itself (its Referer, an app
/// page at `/apps/<id>/ui/`) is that same app — one app's page never writes
/// into another app's console.
fn devlog_target(store: &db::Store, agent_id: &str, headers: &axum::http::HeaderMap) -> Result<(), StatusCode> {
    match store.get_agent(agent_id) {
        Ok(Some(a)) if tools::app_dev::is_own_app(&a) => {}
        _ => return Err(StatusCode::NOT_FOUND),
    }
    let page_app = headers
        .get(header::REFERER)
        .and_then(|v| v.to_str().ok())
        .and_then(|r| r.split("/apps/").nth(1))
        .and_then(|rest| rest.split_once("/ui").map(|(id, _)| id.to_string()));
    if let Some(page_app) = page_app {
        let page_app = urlencoding::decode(&page_app).map(|c| c.into_owned()).unwrap_or(page_app);
        if page_app != agent_id {
            return Err(StatusCode::FORBIDDEN);
        }
    }
    Ok(())
}

/// The devlog routes' gate: [`devlog_target`], then the same app-token
/// check every app route makes, or the install key: the desktop's app
/// window posts through its `neboapp://` proxy, the owner's own client,
/// which proves itself with the key.
async fn devlog_gate(state: &AppState, agent_id: &str, headers: &axum::http::HeaderMap) -> Result<(), Response> {
    devlog_target(&state.store, agent_id, headers).map_err(|s| s.into_response())?;
    if bearer_is(headers, config::read_install_key().as_deref()) {
        return Ok(());
    }
    validate_app_token(state, agent_id, headers).await
}

/// Whether the request's bearer token is `key`.
fn bearer_is(headers: &axum::http::HeaderMap, key: Option<&str>) -> bool {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match (token, key) {
        (Some(t), Some(k)) => !k.is_empty() && crate::handlers::ws::constant_time_eq(t, k),
        _ => false,
    }
}

/// POST /apps/{agent_id}/devlog — the developer script's console batches,
/// kept in the app's ring for `app_console`. The body is read whatever its
/// content type: a page's last batch goes by `sendBeacon`.
pub async fn post_devlog(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if let Err(r) = devlog_gate(&state, &agent_id, &headers).await {
        return r;
    }
    match keep_devlog(&agent_id, &body) {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(s) => s.into_response(),
    }
}

/// Keep one devlog body's entries in the app's ring; how many were kept.
fn keep_devlog(agent_id: &str, body: &[u8]) -> Result<usize, StatusCode> {
    let batch: DevlogBatch = serde_json::from_slice(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(tools::app_console::record(
        agent_id,
        batch
            .entries
            .iter()
            .take(DEVLOG_MAX_BATCH)
            .map(|e| (e.level.as_str(), e.message.as_str(), e.source.as_str(), e.time)),
    ))
}

/// The errors "Send to <employee>" sends at most.
const DEVLOG_SEND_ERRORS: usize = 20;

/// The message "Send to <employee>" posts: the app's last errors, as its
/// console recorded them. `None` when there are none.
fn devlog_send_prompt(app_name: &str, entries: &[tools::app_console::Entry]) -> Option<String> {
    let errors: Vec<&tools::app_console::Entry> = entries.iter().filter(|e| e.level == "error").collect();
    if errors.is_empty() {
        return None;
    }
    let skip = errors.len().saturating_sub(DEVLOG_SEND_ERRORS);
    let lines: Vec<String> = errors[skip..].iter().map(|e| tools::app_console::format_entry(e)).collect();
    Some(format!(
        "These errors came up in {app_name} (from its console):\n\n```\n{}\n```\n\nPlease fix them.",
        lines.join("\n")
    ))
}

/// POST /apps/{agent_id}/devlog/send — "Send to <employee>": the app's last
/// errors go into the conversation the owner has open with the app's
/// employee (the one with the latest activity; a new thread when there is
/// none), through the same path as a message he types in its composer
/// (`ws::dispatch_owner_message`), and the employee works on them there. It answers
/// `{"status": "dispatched", ...}` only once the message is on its way;
/// anything else is an `{"error": ...}` the page shows as it is.
pub async fn send_devlog(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if let Err(r) = devlog_gate(&state, &agent_id, &headers).await {
        return r;
    }
    keep_page_errors(&agent_id, &body);
    let refuse = |status: StatusCode, error: String| (status, axum::Json(serde_json::json!({ "error": error }))).into_response();
    let name = app_employee_name(&state, &agent_id);
    let entries = tools::app_console::recent(&agent_id, None, tools::app_console::RING_CAPACITY);
    let Some(prompt) = devlog_send_prompt(&name, &entries) else {
        return refuse(StatusCode::UNPROCESSABLE_ENTITY, "No errors to send.".into());
    };
    if !state.agent_registry.read().await.contains_key(&agent_id) {
        return refuse(StatusCode::CONFLICT, format!("{name} is turned off. Turn it on, then send again."));
    }
    // The conversation the owner is in with the employee: the one with the
    // latest activity (the rule a phone or loop message to the employee
    // follows, `resolve_agent_session_key`), so the errors and the work on
    // them land where he reads; a new thread when there is none.
    let session_key = match state.store.get_latest_agent_chat(&agent_id) {
        Ok(Some(_)) => crate::resolve_agent_session_key(&state, &agent_id),
        _ => match crate::handlers::agents::create_agent_thread(&state, &agent_id) {
            Ok((_, key)) => key,
            Err(e) => {
                warn!(error = %e, agent_id = %agent_id, "could not open a conversation for the app's errors");
                return refuse(StatusCode::INTERNAL_SERVER_ERROR, format!("Could not send to {name}. Try again."));
            }
        },
    };
    // The composer's own path (live 2026-10-02: through `chat_with_agent`
    // the owner's open chat never showed the message or the reply until he
    // left it and came back): every open view of the conversation, here or
    // on his phone, shows the message and streams the work on it. A phone
    // conversation the session last answered gets the reply too.
    let reply_to = crate::reply_route::of(&state, &session_key).and_then(|r| r.comm_reply());
    crate::handlers::ws::dispatch_owner_message(&state, &session_key, &agent_id, prompt, reply_to).await;
    axum::Json(serde_json::json!({ "sessionId": session_key, "agentId": agent_id, "status": "dispatched" })).into_response()
}

/// The errors the page sent with "Send to <employee>" (`{"entries": [...]}`,
/// or nothing), kept in the app's console beside what its batches already
/// brought, each once: the send never depends on a batch having landed.
fn keep_page_errors(agent_id: &str, body: &[u8]) {
    let Ok(page) = serde_json::from_slice::<DevlogBatch>(body) else { return };
    let held = tools::app_console::recent(agent_id, None, tools::app_console::RING_CAPACITY);
    tools::app_console::record(
        agent_id,
        page.entries
            .iter()
            .take(DEVLOG_MAX_BATCH)
            .filter(|e| e.level == "error")
            .filter(|e| !held.iter().any(|h| h.time_ms == e.time && h.message == e.message))
            .map(|e| (e.level.as_str(), e.message.as_str(), e.source.as_str(), e.time)),
    );
}

/// POST /apps/{agent_id}/agents/invoke — run the app's agent and collect text.
pub async fn invoke_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<InvokeRequest>,
) -> Response {
    if let Err(r) = validate_app_token(&state, &agent_id, &headers).await {
        return r;
    }
    let (target_agent_id, agent_name) = match validate_app_agent(&state.store, &agent_id, body.agent.as_deref()) {
        Ok(v) => v,
        Err(e) => return to_error_response(e).into_response(),
    };
    if let Err(r) = check_subagent_permission(&state, &agent_id, &target_agent_id).await {
        return r;
    }
    match run_agent_collect(&state, &target_agent_id, &agent_name, body).await {
        Ok((text, tools)) => axum::Json(InvokeResponse { text, tools }).into_response(),
        Err(e) => to_error_response(e).into_response(),
    }
}

/// POST /apps/{agent_id}/agents/stream — run the app's agent and stream SSE chunks.
pub async fn stream_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<InvokeRequest>,
) -> Response {
    if let Err(r) = validate_app_token(&state, &agent_id, &headers).await {
        return r;
    }
    let (target_agent_id, agent_name) =
        match validate_app_agent(&state.store, &agent_id, body.agent.as_deref()) {
            Ok(v) => v,
            Err(e) => return to_error_response(e).into_response(),
        };
    if let Err(r) = check_subagent_permission(&state, &agent_id, &target_agent_id).await {
        return r;
    }
    let stream = run_agent_sse(state, target_agent_id, agent_name, body).await;
    Sse::new(stream).into_response()
}

/// POST /apps/{agent_id}/janus/complete — direct provider completion for apps.
pub async fn janus_complete(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<JanusRequest>,
) -> Response {
    if let Err(r) = validate_app_token(&state, &agent_id, &headers).await {
        return r;
    }
    if let Err(e) = validate_app_agent(&state.store, &agent_id, None) {
        return to_error_response(e).into_response();
    }
    match run_janus_collect(&state, body).await {
        Ok((text, usage)) => {
            axum::Json(serde_json::json!({ "text": text, "usage": usage })).into_response()
        }
        Err(e) => to_error_response(e).into_response(),
    }
}

/// POST /apps/{agent_id}/janus/stream — direct provider SSE streaming for apps.
pub async fn janus_stream(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<JanusRequest>,
) -> Response {
    if let Err(r) = validate_app_token(&state, &agent_id, &headers).await {
        return r;
    }
    if let Err(e) = validate_app_agent(&state.store, &agent_id, None) {
        return to_error_response(e).into_response();
    }
    let stream = run_janus_sse(state, body).await;
    Sse::new(stream).into_response()
}

/// POST /apps/{agent_id}/janus/decide — a typed decision for the app's page
/// (`nebo.decide`): the same request and the same decision client as the
/// `decide` tool (`tools::decide_tool::decide`), billed to the owner's
/// NeboAI account. Admitted like `janus/complete`: the app's own token, the
/// owner's tunnel, or its own desktop window; the id must be an app.
pub async fn janus_decide(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> Response {
    if let Err(r) = validate_app_token(&state, &agent_id, &headers).await {
        return r;
    }
    if let Err(e) = validate_app_agent(&state.store, &agent_id, None) {
        return to_error_response(e).into_response();
    }
    decide_for_app(state.decide.as_deref(), &agent_id, &body).await
}

/// The decision itself, once the app is admitted: 400 for a malformed
/// request (ours or Janus's), 429 when nothing on the account can pay or
/// decisions are throttled, 503 without NeboAI, 502 only when the service
/// itself fails.
async fn decide_for_app(client: Option<&ai::DecideClient>, app_id: &str, body: &serde_json::Value) -> Response {
    let trace = ai::RequestTrace { agent_id: app_id.to_string(), ..ai::RequestTrace::new("app_decide") };
    match tools::decide_tool::decide(client, &trace, body).await {
        Ok(out) => axum::Json(out).into_response(),
        Err(e) => {
            let status = match e {
                tools::decide_tool::DecideError::Invalid(_) => StatusCode::BAD_REQUEST,
                tools::decide_tool::DecideError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
                tools::decide_tool::DecideError::Unfunded | tools::decide_tool::DecideError::Busy => {
                    StatusCode::TOO_MANY_REQUESTS
                }
                tools::decide_tool::DecideError::Failed(_) => StatusCode::BAD_GATEWAY,
            };
            (status, axum::Json(serde_json::json!({ "error": e.to_string() }))).into_response()
        }
    }
}

fn connected_profile(
    store: &db::Store,
    agent_id: &str,
    slug: &str,
) -> Result<Option<serde_json::Value>, types::NeboError> {
    Ok(store.resolve_plugin_account_profile(agent_id, slug, None)?.map(|p| serde_json::json!({
        "id": p.id, "config_dir": p.config_dir, "account_label": p.account_label, "needs_reauth": p.needs_reauth
    })))
}

/// The native scheme proxy is a server-side request (no Origin). HTTP app
/// windows are same-origin; authenticated tunnel requests carry a private stamp.
fn app_connection_origin_allowed(headers: &axum::http::HeaderMap) -> bool {
    if crate::middleware::came_through_tunnel(headers) {
        return true;
    }
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let Some(origin) = origin.to_str().ok().and_then(|s| url::Url::parse(s).ok()) else {
        return false;
    };
    let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    matches!(origin.scheme(), "http" | "https")
        && origin[url::Position::BeforeHost..url::Position::AfterPort].eq_ignore_ascii_case(host)
}

/// ANY /apps/{agent_id}/api/*path — proxy HTTP request to sidecar via gRPC UIService.
///
/// Sidecar binaries communicate over Unix socket. Nebo converts the HTTP request
/// to a gRPC `UIService.HandleRequest` call and returns the response.
pub async fn proxy_to_sidecar(
    State(state): State<AppState>,
    Path((agent_id, path)): Path<(String, String)>,
    req: axum::http::Request<Body>,
) -> Response {
    // Block access to internal sidecar endpoints — these are for Nebo's internal
    // use only (tool discovery, health checks). Never expose to HTTP clients.
    let clean = path.trim_start_matches('/');
    if clean == "_tools" || clean.starts_with("_") {
        return StatusCode::FORBIDDEN.into_response();
    }

    let agent = match state.store.get_agent(&agent_id) {
        Ok(Some(a)) if a.is_app.unwrap_or(0) != 0 => a,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };

    // The app's supervised sidecar, started on first use. Whether it is up is
    // the supervisor's answer — never whether a socket file happens to exist.
    let Some(lifecycle) = crate::app_lifecycle::start(&state, &agent, false).await else {
        return sidecar_unavailable(&agent, None);
    };
    #[cfg(not(unix))]
    {
        let _ = (req, lifecycle);
        return (StatusCode::SERVICE_UNAVAILABLE, "sidecar proxy requires Unix sockets").into_response();
    }

    #[cfg(unix)]
    {
    // Extract HTTP request parts for gRPC
    let method = req.method().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    let mut headers_map = std::collections::HashMap::new();
    for (name, value) in req.headers() {
        // Native connection metadata is platform-owned, never browser-supplied.
        if name.as_str().starts_with("x-nebo-connected-") { continue; }
        if let Ok(v) = value.to_str() {
            headers_map.insert(name.to_string(), v.to_string());
        }
    }
    let declaration: serde_json::Value = serde_json::from_str(&agent.frontmatter).unwrap_or_default();
    let declaration = if declaration.get("requires").is_some() { declaration } else {
        super::agents::app_tool_dir(&agent)
            .and_then(|dir| std::fs::read(dir.join("agent.json")).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    };
    let mut connections = serde_json::Map::new();
    if let Some(plugins) = declaration.pointer("/requires/plugins").and_then(|v| v.as_array()) {
        for slug in plugins.iter().filter_map(|v| v.as_str()) {
            // Same account resolver as plugin tool calls. No global fallback and
            // no caller-controlled account or filesystem path. Only declared deps.
            if state.plugin_store.get_manifest(slug).and_then(|m| m.auth)
                .and_then(|a| a.profile_dir_env).is_none() { continue; }
            match connected_profile(&state.store, &agent_id, slug) {
                Ok(Some(profile)) => { connections.insert(slug.to_string(), profile); }
                Ok(None) => {},
                Err(_) => return (StatusCode::SERVICE_UNAVAILABLE, "could not resolve app connection").into_response(),
            }
        }
    }
    // First-party NeboAI apps (`requires.neboai: true`) reuse the owner's own
    // NeboAI sign-in: no separate consent, the app is just who Nebo is.
    let neboai_token = if declaration.pointer("/requires/neboai").and_then(|v| v.as_bool()) == Some(true) {
        crate::codes::neboai_token(&state).unwrap_or_default()
    } else { String::new() };
    // Credential-bearing sidecar requests must come from the app UI (or the
    // native protocol proxy, which has no browser Origin), never a foreign page.
    if (!connections.is_empty() || !neboai_token.is_empty()) && !app_connection_origin_allowed(req.headers()) {
        return (StatusCode::FORBIDDEN, "app connection request has an untrusted origin").into_response();
    }
    headers_map.insert("x-nebo-connected-profiles".into(), serde_json::Value::Object(connections).to_string());
    if !neboai_token.is_empty() { headers_map.insert("x-nebo-connected-neboai".into(), neboai_token); }
    let body_bytes = match axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024).await {
        Ok(b) => b.to_vec(),
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

        let grpc_req = proto::HttpRequest {
            method,
            path: path.trim_start_matches('/').to_string(),
            query,
            headers: headers_map,
            body: body_bytes,
        };

        match lifecycle.serve(grpc_req).await {
            Ok(inner) => {
                let status = StatusCode::from_u16(inner.status_code as u16)
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                let mut response = Response::builder().status(status);
                for (name, value) in &inner.headers {
                    if let Ok(v) = HeaderValue::from_str(value) {
                        response = response.header(name.as_str(), v);
                    }
                }
                response
                    .body(Body::from(inner.body))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
            }
            Err(unavailable) => sidecar_unavailable(&agent, Some(&unavailable)),
        }
    }
}

/// The answer when an app's sidecar cannot serve a request: a status the app
/// can act on, the real reason in words, and the supervisor's state (the same
/// shape as the `sidecar_state` event). `Retry-After` while it is coming back.
fn sidecar_unavailable(
    agent: &db::models::Agent,
    unavailable: Option<&crate::app_lifecycle::Unavailable>,
) -> Response {
    let name = agent.name.as_str();
    let Some(u) = unavailable else {
        let body = serde_json::json!({
            "error": format!("{name} has no program to run on this computer."),
            "sidecar": { "state": "none" },
        });
        return (StatusCode::NOT_FOUND, axum::Json(body)).into_response();
    };
    let body = serde_json::json!({ "error": u.message(name), "sidecar": u.state.wire() });
    let (status, retry_after) = match &u.state {
        napp::supervisor::SidecarState::Starting => (StatusCode::SERVICE_UNAVAILABLE, Some(1)),
        napp::supervisor::SidecarState::Restarting { retry_in, .. } => {
            (StatusCode::SERVICE_UNAVAILABLE, Some(retry_in.as_secs().max(1)))
        }
        napp::supervisor::SidecarState::Running(_) => (StatusCode::BAD_GATEWAY, None),
        _ => (StatusCode::SERVICE_UNAVAILABLE, None),
    };
    let mut resp = (status, axum::Json(body)).into_response();
    if let Some(secs) = retry_after {
        if let Ok(v) = HeaderValue::from_str(&secs.to_string()) {
            resp.headers_mut().insert(header::RETRY_AFTER, v);
        }
    }
    resp
}

/// GET /apps/{agent_id}/sidecar — the app's sidecar state, as `sidecar_state`
/// carries it (`none` for an app with no program, or one not started).
pub async fn sidecar_state(State(state): State<AppState>, Path(agent_id): Path<String>) -> Response {
    let lifecycle = state.app_lifecycles.read().await.get(&agent_id).cloned();
    let wire = match lifecycle {
        Some(lc) => lc.state().wire(),
        None => serde_json::json!({ "state": "none" }),
    };
    axum::Json(wire).into_response()
}

/// POST /apps/{agent_id}/sidecar/restart — "Try again": bring the app's
/// sidecar up now through its supervisor (starting it if nothing has), and
/// answer with the state it settled in.
pub async fn restart_sidecar(State(state): State<AppState>, Path(agent_id): Path<String>) -> Response {
    let agent = match state.store.get_agent(&agent_id) {
        Ok(Some(a)) if a.is_app.unwrap_or(0) != 0 => a,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let existing = state.app_lifecycles.read().await.get(&agent_id).cloned();
    let settled = match existing {
        Some(lc) => lc.revive(crate::app_lifecycle::REQUEST_WAIT).await,
        None => match crate::app_lifecycle::start(&state, &agent, false).await {
            Some(lc) => lc.settled(crate::app_lifecycle::REQUEST_WAIT).await,
            None => return sidecar_unavailable(&agent, None),
        },
    };
    axum::Json(settled.wire()).into_response()
}

fn validate_app_agent(
    store: &db::Store,
    app_agent_id: &str,
    override_agent: Option<&str>,
) -> Result<(String, String), types::NeboError> {
    let app = store
        .get_agent(app_agent_id)?
        .ok_or(types::NeboError::NotFound)?;
    if app.is_app.unwrap_or(0) == 0 {
        return Err(types::NeboError::Unauthorized);
    }
    if let Some(target) = override_agent {
        let agent = store
            .get_agent(target)?
            .ok_or(types::NeboError::NotFound)?;
        return Ok((agent.id, agent.name));
    }
    Ok((app.id, app.name))
}

async fn run_agent_collect(
    state: &AppState,
    agent_id: &str,
    agent_name: &str,
    body: InvokeRequest,
) -> Result<(String, Vec<serde_json::Value>), types::NeboError> {
    let mut rx = start_app_agent_run(state, agent_id, agent_name, body).await?;
    let mut text = String::new();
    let mut tools = Vec::new();
    while let Some(event) = rx.recv().await {
        match event.event_type {
            ai::StreamEventType::Text => text.push_str(&event.text),
            ai::StreamEventType::ToolCall | ai::StreamEventType::ToolResult => {
                if let Some(tool_call) = event.tool_call {
                    if let Ok(value) = serde_json::to_value(tool_call) {
                        tools.push(value);
                    }
                }
            }
            ai::StreamEventType::Error => {
                if let Some(error) = event.error {
                    return Err(types::NeboError::Internal(error));
                }
            }
            _ => {}
        }
    }
    Ok((text, tools))
}

async fn run_agent_sse(
    state: AppState,
    agent_id: String,
    agent_name: String,
    body: InvokeRequest,
) -> impl Stream<Item = Result<Event, std::convert::Infallible>> {
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    tokio::spawn(async move {
        match start_app_agent_run(&state, &agent_id, &agent_name, body).await {
            Ok(mut events) => {
                while let Some(event) = events.recv().await {
                    let data = serde_json::json!({
                        "text": event.text,
                        "done": false,
                        "type": format!("{:?}", event.event_type),
                    });
                    if tx
                        .send(Ok(Event::default().data(data.to_string())))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                let _ = tx
                    .send(Ok(Event::default().data(r#"{"text":"","done":true}"#)))
                    .await;
            }
            Err(e) => {
                let data = serde_json::json!({ "error": e.to_string(), "done": true });
                let _ = tx.send(Ok(Event::default().data(data.to_string()))).await;
            }
        }
    });
    futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    })
}

async fn start_app_agent_run(
    state: &AppState,
    agent_id: &str,
    agent_name: &str,
    body: InvokeRequest,
) -> Result<tokio::sync::mpsc::Receiver<ai::StreamEvent>, types::NeboError> {
    let session_key = format!("app:{}:api", agent_id);
    let cancel_token = CancellationToken::new();
    let entity_config = crate::entity_config::resolve_for_chat(&state.store, "agent", agent_id);
    let mention_context = body.data.map(|data| format!("App data context: {}", data));
    crate::chat_dispatch::run_chat_events(
        state,
        crate::chat_dispatch::ChatConfig {
            session_key,
            prompt: body.message,
            user_id: String::new(),
            channel: "app".to_string(),
            origin: tools::Origin::App,
            door: types::permissions::Door::Chat,
            agent_id: agent_id.to_string(),
            cancel_token,
            lane: types::constants::lanes::EVENTS.to_string(),
            comm_reply: None,
            entity_config,
            images: vec![],
            attachments: vec![],
            entity_name: agent_name.to_string(),
            origin_agent_id: None,
            mention_context,
            tool_scope: None,
            channel_ctx: None,
            handoff_depth: 0,
            seed_taint: vec![],
            tool_allowlist: None,
            hidden_prompt: false,
            coworker: None,
            audience: None,
            cwd: None,
            model_override: None,
            client_id: None,
        },
    )
    .await
}

async fn run_janus_collect(
    state: &AppState,
    body: JanusRequest,
) -> Result<(String, Option<ai::UsageInfo>), types::NeboError> {
    let mut rx = start_janus_stream(state, body).await?;
    let mut text = String::new();
    let mut usage = None;
    while let Some(event) = rx.recv().await {
        if event.event_type == ai::StreamEventType::Text {
            text.push_str(&event.text);
        }
        if event.usage.is_some() {
            usage = event.usage;
        }
        if let Some(error) = event.error {
            return Err(types::NeboError::Internal(error));
        }
    }
    Ok((text, usage))
}

async fn run_janus_sse(
    state: AppState,
    body: JanusRequest,
) -> impl Stream<Item = Result<Event, std::convert::Infallible>> {
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    tokio::spawn(async move {
        match start_janus_stream(&state, body).await {
            Ok(mut events) => {
                while let Some(event) = events.recv().await {
                    let data = serde_json::json!({
                        "text": event.text,
                        "done": false,
                    });
                    if tx
                        .send(Ok(Event::default().data(data.to_string())))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                let _ = tx.send(Ok(Event::default().data("[DONE]"))).await;
            }
            Err(e) => {
                let data = serde_json::json!({ "error": e.to_string(), "done": true });
                let _ = tx.send(Ok(Event::default().data(data.to_string()))).await;
            }
        }
    });
    futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    })
}

async fn start_janus_stream(
    state: &AppState,
    body: JanusRequest,
) -> Result<tokio::sync::mpsc::Receiver<ai::StreamEvent>, types::NeboError> {
    let providers = state.harness.providers();
    let providers = providers.read().await;
    let provider = providers
        .first()
        .cloned()
        .ok_or_else(|| types::NeboError::Internal("no providers available".into()))?;
    drop(providers);

    let req = ai::ChatRequest {
        tool_choice: Default::default(),
        messages: body.messages,
        tools: vec![],
        max_tokens: body.max_tokens.unwrap_or(2000),
        temperature: body.temperature.unwrap_or(0.7),
        system: body.system.unwrap_or_default(),
        model: body.model.unwrap_or_default(),
        enable_thinking: false,
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: Some(CancellationToken::new()),
        trace: ai::RequestTrace::new("app_llm"),
        tool_credential: None,
        chat_id: String::new(),
        ask_channels: None,
        permission_mode: None,
        linked_context: None,
    };
    provider
        .stream(&req)
        .await
        .map_err(|e| types::NeboError::Internal(format!("provider stream failed: {e}")))
}

/// POST /apps/{agent_id}/http/proxy — CORS-free outbound HTTP proxy.
pub async fn http_proxy(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> Response {
    if let Err(r) = validate_app_token(&state, &agent_id, &headers).await {
        return r;
    }
    let url = match body.get("url").and_then(|v| v.as_str()) {
        Some(u) => u.to_string(),
        None => return (StatusCode::BAD_REQUEST, "missing url field").into_response(),
    };

    // Enforce network: permission from manifest
    if let Err(r) = check_network_permission(&state, &agent_id, &url).await {
        return r;
    }

    let method = body
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("GET")
        .to_uppercase();

    let headers: Vec<(String, String)> = body
        .get("headers")
        .and_then(|h| h.as_object())
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                .collect()
        })
        .unwrap_or_default();

    let req_body = body.get("body").and_then(|b| b.as_str()).map(String::from);

    let client = match tls::http_client().build() {
        Ok(client) => client,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("proxy error: {}", e)).into_response(),
    };
    let mut req = match method.as_str() {
        "GET" => client.get(&url),
        "POST" => client.post(&url),
        "PUT" => client.put(&url),
        "DELETE" => client.delete(&url),
        "PATCH" => client.patch(&url),
        _ => return (StatusCode::BAD_REQUEST, "unsupported method").into_response(),
    };

    for (k, v) in &headers {
        req = req.header(k.as_str(), v.as_str());
    }

    // Add identifying header
    req = req.header("X-Nebo-App", &agent_id);

    if let Some(b) = req_body {
        req = req.body(b);
    }

    match req.send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let resp_headers: Vec<(String, String)> = resp
                .headers()
                .iter()
                .filter_map(|(k, v)| Some((k.to_string(), v.to_str().ok()?.to_string())))
                .collect();
            let body_bytes = resp.bytes().await.unwrap_or_default();
            axum::Json(serde_json::json!({
                "status": status,
                "headers": resp_headers.into_iter().collect::<std::collections::HashMap<_, _>>(),
                "body": String::from_utf8_lossy(&body_bytes),
            }))
            .into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, format!("proxy error: {}", e)).into_response(),
    }
}

/// GET /apps/{agent_id}/identity — expose agent context for the app SDK.
pub async fn get_identity(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let agent = state
        .store
        .get_agent(&agent_id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    if agent.is_app.unwrap_or(0) == 0 {
        return Err(to_error_response(types::NeboError::Unauthorized));
    }

    // Parse frontmatter for model + skills
    let frontmatter_val: serde_json::Value = if !agent.frontmatter.is_empty() {
        serde_json::from_str(&agent.frontmatter).unwrap_or_default()
    } else {
        serde_json::Value::Null
    };
    let model = frontmatter_val
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let skills: Vec<&str> = frontmatter_val
        .get("skills")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|s| s.as_str()).collect())
        .unwrap_or_default();

    // Extract persona body (markdown after frontmatter)
    let (_yaml_str, persona_body) =
        napp::agent::split_frontmatter(&agent.agent_md).unwrap_or_default();

    // Compute display name
    let display_name = agent
        .app_window_config
        .as_ref()
        .and_then(|cfg_str| serde_json::from_str::<serde_json::Value>(cfg_str).ok())
        .and_then(|cfg| cfg.get("title").and_then(|t| t.as_str().map(|s| s.to_string())))
        .filter(|t| !t.is_empty())
        .or_else(|| {
            persona_body.lines().find_map(|line| {
                line.trim()
                    .strip_prefix("# ")
                    .map(|h| h.trim().to_string())
                    .filter(|h| !h.is_empty())
            })
        })
        .unwrap_or_else(|| agent.name.clone());

    // Parse input_values from DB
    let input_values: serde_json::Value =
        serde_json::from_str(&agent.input_values).unwrap_or(serde_json::json!({}));

    Ok(axum::Json(serde_json::json!({
        "id": agent.id,
        "name": agent.name,
        "displayName": display_name,
        "description": agent.description,
        "persona": persona_body,
        "model": model,
        "skills": skills,
        "inputValues": input_values,
    })))
}

/// Determine MIME type from file extension.
/// Content type of a file an app page ships. The one table for the HTTP path
/// and the desktop `neboapp://` path, so a file types the same on both. With
/// `nosniff` on, a wrong type here is a broken asset (a model or film served as
/// octet-stream never loads).
pub fn mime_from_path(path: &std::path::Path) -> &'static str {
    let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "application/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("txt") => "text/plain; charset=utf-8",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("ktx2") => "image/ktx2",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("wasm") => "application/wasm",
        Some("mp4" | "m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("ogg" | "oga") => "audio/ogg",
        Some("opus") => "audio/opus",
        Some("m4a") => "audio/mp4",
        Some("aac") => "audio/aac",
        Some("flac") => "audio/flac",
        Some("glb") => "model/gltf-binary",
        Some("gltf") => "model/gltf+json",
        _ => "application/octet-stream",
    }
}

/// What a request's `Range` header asks of a file `len` bytes long.
#[derive(Debug, PartialEq, Eq)]
pub enum ByteRange {
    /// No usable range: send the whole file (200). Multi-range requests land
    /// here too; serving the whole file is a valid answer to them.
    Full,
    /// Inclusive byte offsets to send (206).
    Partial(u64, u64),
    /// A range that starts past the end (416).
    Unsatisfiable,
}

/// Resolve a single `bytes=` range. Media elements scrub and seek with these;
/// WebKit will not play a video whose server answers them with a plain 200.
pub fn byte_range(header: Option<&str>, len: u64) -> ByteRange {
    let Some(spec) = header.and_then(|h| h.trim().strip_prefix("bytes=")) else {
        return ByteRange::Full;
    };
    if spec.contains(',') {
        return ByteRange::Full;
    }
    let Some((a, b)) = spec.trim().split_once('-') else {
        return ByteRange::Full;
    };
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() {
        // Suffix: the last n bytes.
        return match b.parse::<u64>() {
            Ok(0) => ByteRange::Unsatisfiable,
            Ok(_) if len == 0 => ByteRange::Unsatisfiable,
            Ok(n) => ByteRange::Partial(len.saturating_sub(n), len - 1),
            Err(_) => ByteRange::Full,
        };
    }
    let Ok(start) = a.parse::<u64>() else {
        return ByteRange::Full;
    };
    if start >= len {
        return ByteRange::Unsatisfiable;
    }
    let end = if b.is_empty() {
        len - 1
    } else {
        match b.parse::<u64>() {
            Ok(e) if e >= start => e.min(len - 1),
            Ok(_) => return ByteRange::Unsatisfiable,
            Err(_) => return ByteRange::Full,
        }
    };
    ByteRange::Partial(start, end)
}

#[cfg(test)]
mod connection_origin_tests {
    use super::*;
    #[test]
    fn connected_accounts_are_scoped_to_the_app_and_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let store = db::Store::new(dir.path().join("test.db").to_str().unwrap()).unwrap();
        store
            .upsert_plugin_account_profile("a1", "app-a", "mail", "Primary", "/a/primary")
            .unwrap();
        store
            .upsert_plugin_account_profile("a2", "app-a", "mail", "Secondary", "/a/secondary")
            .unwrap();
        store
            .upsert_plugin_account_profile("b1", "app-b", "mail", "Other account", "/b/primary")
            .unwrap();
        assert_eq!(
            connected_profile(&store, "app-a", "mail").unwrap().unwrap()["config_dir"],
            "/a/primary"
        );
        assert_eq!(
            connected_profile(&store, "app-b", "mail").unwrap().unwrap()["config_dir"],
            "/b/primary"
        );
        assert!(
            connected_profile(&store, "app-c", "mail")
                .unwrap()
                .is_none()
        );
        assert!(
            connected_profile(&store, "app-a", "other-plugin")
                .unwrap()
                .is_none()
        );
        store.set_plugin_account_reauth("a1", true).unwrap();
        assert_eq!(
            connected_profile(&store, "app-a", "mail").unwrap().unwrap()["needs_reauth"],
            true
        );
    }
    #[test]
    fn native_and_same_origin_only() {
        let mut h = axum::http::HeaderMap::new();
        h.insert(header::HOST, "127.0.0.1:27895".parse().unwrap());
        assert!(app_connection_origin_allowed(&h));
        h.insert(header::ORIGIN, "http://127.0.0.1:27895".parse().unwrap());
        assert!(app_connection_origin_allowed(&h));
        h.insert(header::ORIGIN, "https://untrusted.example".parse().unwrap());
        assert!(!app_connection_origin_allowed(&h));
        h.insert(header::ORIGIN, "null".parse().unwrap());
        assert!(!app_connection_origin_allowed(&h));
        h.insert("x-nebo-tunnel-auth", "forged".parse().unwrap());
        assert!(!app_connection_origin_allowed(&h));
    }
}

#[cfg(test)]
mod developer_mode_tests {
    use super::*;
    use tools::registry::DynTool;

    fn store() -> (tempfile::TempDir, std::sync::Arc<db::Store>) {
        let dir = tempfile::tempdir().unwrap();
        let store = db::Store::new(dir.path().join("test.db").to_str().unwrap()).unwrap();
        (dir, std::sync::Arc::new(store))
    }

    fn developer_mode(store: &db::Store, on: bool) {
        store.update_settings(None, None, None, None, None, None, None, None, None, Some(on)).unwrap();
    }

    fn app(store: &db::Store, id: &str, name: &str) {
        store.create_agent(id, Some("agent"), name, "", "", "{}", None, None).unwrap();
        store.set_agent_app_fields(id, true, Some("/tmp/ui"), None, None).unwrap();
    }

    fn from_page(app: &str) -> axum::http::HeaderMap {
        let mut h = axum::http::HeaderMap::new();
        h.insert(header::REFERER, format!("https://neboai.com/t/bot-1/apps/{app}/ui/index.html").parse().unwrap());
        h
    }

    const PAGE: &str = "<!doctype html><html><head><script src=\"main.js\"></script></head><body></body></html>";

    fn dev(employee: &str, console: bool) -> Option<Devtools<'_>> {
        Some(Devtools { employee, console, desktop: None })
    }

    /// The desktop's app window carries what a served page carries, by the
    /// same rule: the owner's own app gets the developer script, told its
    /// routes and its socket's pass; an installed app gets nothing.
    #[test]
    fn a_desktop_app_window_gets_the_developer_script_by_the_same_rule() {
        let (_d, store) = store();
        app(&store, "app-a", "Racer");
        app(&store, "app-m", "Bought");
        store.set_agent_napp_path("app-m", "/data/nebo/agents/bought.napp").unwrap();

        assert_eq!(desktop_script(&store, 27895, "app-m"), "", "installed: no developer tooling");
        assert_eq!(desktop_script(&store, 27895, "nope"), "");

        let quiet = desktop_script(&store, 27895, "app-a");
        assert!(quiet.contains("data-nebo-devtools") && quiet.contains(r#""console":false"#), "{quiet}");
        assert!(quiet.contains(r#""appId":"app-a""#) && quiet.contains(r#""api":"neboapp://app-a/api/v1/apps/app-a""#));
        let pass = quiet
            .split("ws://127.0.0.1:27895/k/")
            .nth(1)
            .and_then(|r| r.split_once("/ws/app/app-a\""))
            .map(|(p, _)| p.to_string())
            .expect("the socket and its pass");
        assert!(napp::app_view::admits(&pass, "app-a") && !napp::app_view::admits(&pass, "app-m"));

        developer_mode(&store, true);
        assert!(desktop_script(&store, 27895, "Racer").contains(r#""console":true"#), "by name, the console shows");
        assert_eq!(desktop_script(&store, 27895, "app-m"), "", "installed, mode on");
    }

    #[test]
    fn the_developer_script_is_injected_only_into_the_owners_own_apps() {
        let off = String::from_utf8(inject_app_bridge(PAGE.as_bytes().to_vec(), None, None)).unwrap();
        assert!(off.contains("data-nebo-bridge"), "the bridge is always there");
        assert!(!off.contains("data-nebo-devtools"), "no developer script in an installed app");
        assert!(!off.contains("__neboDevtools"));

        let quiet = String::from_utf8(inject_app_bridge(PAGE.as_bytes().to_vec(), dev("Kart Racer", false), None)).unwrap();
        assert!(quiet.contains(r#""console":false"#) && quiet.contains(r#""employee":"Kart Racer""#), "reload and capture, no console: {quiet}");

        let on = String::from_utf8(inject_app_bridge(PAGE.as_bytes().to_vec(), dev("Kart Racer", true), None)).unwrap();
        let bridge = on.find("data-nebo-bridge").unwrap();
        let dev = on.find("data-nebo-devtools").expect("the developer script");
        let app = on.find("main.js").unwrap();
        assert!(bridge < dev && dev < app, "after the bridge, before the app's own scripts");
        assert!(on.contains(r#""console":true"#) && on.contains(r#""employee":"Kart Racer""#), "told the employee's name and to show the console");
        assert!(!on.contains("__NEBO_DEVTOOLS_CONFIG__"));
        assert_eq!(on.matches("</script>").count(), 3, "bridge, developer script, the app's own");
    }

    #[test]
    fn a_name_can_never_close_the_script() {
        let on = String::from_utf8(inject_app_bridge(PAGE.as_bytes().to_vec(), dev("</script><b>x", true), None)).unwrap();
        assert_eq!(on.matches("</script>").count(), 3, "{on}");
        assert!(on.contains(r"\u003c/script>\u003cb>x"));
    }

    /// The SDK in a desktop app window (`neboapp://<id>`, proxied with the
    /// install key and the page's origin) reaches its own app's store and
    /// round-trips a value; another app's routes, a missing or wrong key,
    /// and a request naming no window are refused.
    #[test]
    fn a_desktop_window_reaches_its_own_apps_store_and_no_other() {
        let (_d, store) = store();
        app(&store, "crm", "CRM");
        let window = |origin: Option<&str>, key: &str| {
            let mut h = axum::http::HeaderMap::new();
            h.insert(header::AUTHORIZATION, format!("Bearer {key}").parse().unwrap());
            if let Some(o) = origin {
                h.insert(header::ORIGIN, o.parse().unwrap());
            }
            h
        };
        let own = window(Some("neboapp://crm"), "key-1");
        assert!(desktop_window_reaches(&own, "crm", Some("key-1")));
        assert!(!desktop_window_reaches(&own, "notes", Some("key-1")), "another app's routes");
        assert!(!desktop_window_reaches(&own, "crm", Some("key-2")), "a wrong key");
        assert!(!desktop_window_reaches(&own, "crm", None), "no key on this install");
        assert!(!desktop_window_reaches(&window(None, "key-1"), "crm", Some("key-1")), "no window named");
        assert!(!desktop_window_reaches(&window(Some("http://evil.test"), "key-1"), "crm", Some("key-1")));

        // What put_storage then get_storage do for that window.
        let contact = serde_json::json!({ "name": "John Smith", "phone": "+1 555 0100" });
        let sent = serde_json::Value::String(contact.to_string()).to_string();
        tools::app_data::write(&store, "crm", "contact:john", &sent).unwrap();
        let raw = tools::app_data::read(&store, "crm", "contact:john").unwrap().unwrap();
        assert_eq!(tools::app_data::decode(&raw), contact);
        assert_eq!(tools::app_data::read(&store, "notes", "contact:john").unwrap(), None);
    }

    /// `nebo.decide` (POST /apps/{id}/janus/decide) is admitted like
    /// janus/complete: a desktop window reaches only its own app's route by
    /// the install key and its origin, and the id must be an app. Admitted,
    /// the request goes to the one decision client, traced as the app.
    #[tokio::test]
    async fn an_apps_decision_is_its_own_and_goes_to_the_one_client() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (_d, store) = store();
        app(&store, "crm", "CRM");
        store.create_agent("emp-1", Some("agent"), "Bookkeeper", "", "", "{}", None, None).unwrap();

        let mut own = axum::http::HeaderMap::new();
        own.insert(header::AUTHORIZATION, "Bearer key-1".parse().unwrap());
        own.insert(header::ORIGIN, "neboapp://crm".parse().unwrap());
        assert!(desktop_window_reaches(&own, "crm", Some("key-1")));
        assert!(!desktop_window_reaches(&own, "notes", Some("key-1")), "another app's decide route");
        assert!(!desktop_window_reaches(&own, "crm", Some("key-2")), "a wrong install key");
        assert!(validate_app_agent(&store, "crm", None).is_ok());
        assert!(matches!(validate_app_agent(&store, "emp-1", None), Err(types::NeboError::Unauthorized)), "not an app");
        assert!(matches!(validate_app_agent(&store, "nope", None), Err(types::NeboError::NotFound)));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 16384];
            let mut got = String::new();
            while !got.contains("\"questions\"") {
                let n = sock.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                got.push_str(&String::from_utf8_lossy(&buf[..n]));
            }
            *log.lock().unwrap() = got;
            let body = r#"{"model":"jev-1.13.0","answers":{"hot":{"type":"noul","noul":0.92}},"usage":{"input_tokens":40,"output_tokens":1,"cost_micro":4}}"#;
            let reply = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
            sock.write_all(reply.as_bytes()).await.unwrap();
        });
        let client = ai::DecideClient::new(&format!("http://{addr}"), || {
            Some(ai::Bearer { token: "owner-token".into(), bot_id: Some("bot-1".into()) })
        });
        let ask = serde_json::json!({ "state": { "status": "wants a quote" }, "questions": { "hot": { "type": "noul", "instructions": "`status` is a buying signal." } } });

        let ok = decide_for_app(Some(&client), "crm", &ask).await;
        assert_eq!(ok.status(), StatusCode::OK);
        let body = axum::body::to_bytes(ok.into_body(), 1 << 20).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["answers"]["hot"]["noul"], 0.92);
        assert_eq!(v["usage"]["cost_micro"], 4);
        let sent = seen.lock().unwrap().to_lowercase();
        assert!(sent.contains("authorization: bearer owner-token") && sent.contains("x-bot-id: bot-1"), "billed to the owner: {sent}");
        assert!(sent.contains("x-purpose: app_decide") && sent.contains("x-agent-id: crm"), "traced as the app: {sent}");

        assert_eq!(decide_for_app(None, "crm", &ask).await.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bad = serde_json::json!({ "state": "x", "questions": { "q": { "type": "maybe", "instructions": "?" } } });
        assert_eq!(decide_for_app(Some(&client), "crm", &bad).await.status(), StatusCode::BAD_REQUEST);
    }

    /// A decision service answering `status` with `body` on every call,
    /// counting the calls.
    async fn janus_answering(status: &'static str, body: &'static str) -> (ai::DecideClient, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = calls.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut buf = vec![0u8; 16384];
                let mut got = String::new();
                while !got.contains("\"questions\"") {
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    got.push_str(&String::from_utf8_lossy(&buf[..n]));
                }
                let reply = format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
                let _ = sock.write_all(reply.as_bytes()).await;
            }
        });
        let client = ai::DecideClient::new(&format!("http://{addr}"), || {
            Some(ai::Bearer { token: "owner-token".into(), bot_id: Some("bot-1".into()) })
        });
        (client, calls)
    }

    /// The route says what Janus said: its 400 is the page's bad request,
    /// its funding 429 is "nothing can pay" in plain words and is asked
    /// once (a retry gets the same answer), no sign-in is 503, and 502 is
    /// left for the service failing.
    #[tokio::test]
    async fn a_decision_refused_by_janus_keeps_its_reason() {
        let ask = serde_json::json!({ "state": "x", "questions": { "q": { "type": "noul", "instructions": "Is it?" } } });
        async fn answer(r: Response) -> (StatusCode, String) {
            let status = r.status();
            let body = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
            let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
            (status, v["error"].as_str().unwrap_or_default().to_string())
        }

        let (client, calls) = janus_answering(
            "429 Too Many Requests",
            r#"{"error":{"code":"USAGE_LIMIT_EXCEEDED","message":"no balance","retryable":false}}"#,
        )
        .await;
        let (status, error) = answer(decide_for_app(Some(&client), "crm", &ask).await).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert!(error.contains("used all the work included in your account"), "{error}");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1, "a funding 429 is never retried");

        let (client, _) = janus_answering("400 Bad Request", r#"{"error":{"message":"criteria must be an object"}}"#).await;
        let (status, error) = answer(decide_for_app(Some(&client), "crm", &ask).await).await;
        assert_eq!((status, error.as_str()), (StatusCode::BAD_REQUEST, "criteria must be an object"));

        let (client, _) = janus_answering("500 Internal Server Error", r#"{"error":{"message":"boom"}}"#).await;
        assert_eq!(decide_for_app(Some(&client), "crm", &ask).await.status(), StatusCode::BAD_GATEWAY);

        let signed_out = ai::DecideClient::new("http://127.0.0.1:9", || None);
        assert_eq!(decide_for_app(Some(&signed_out), "crm", &ask).await.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn the_install_key_is_the_desktop_windows_proof() {
        let mut h = axum::http::HeaderMap::new();
        assert!(!bearer_is(&h, Some("key-1")));
        h.insert(header::AUTHORIZATION, "Bearer key-1".parse().unwrap());
        assert!(bearer_is(&h, Some("key-1")));
        assert!(!bearer_is(&h, Some("key-2")));
        assert!(!bearer_is(&h, None));
        assert!(!bearer_is(&h, Some("")));
    }

    #[test]
    fn the_devlog_routes_take_only_an_own_apps_own_page() {
        let (_d, store) = store();
        app(&store, "app-a", "Racer");
        app(&store, "app-b", "Notes");
        app(&store, "app-m", "Bought");
        store.set_agent_napp_path("app-m", "/data/nebo/agents/bought.napp").unwrap();
        store.create_agent("emp-1", Some("agent"), "Bookkeeper", "", "", "{}", None, None).unwrap();

        assert_eq!(devlog_target(&store, "app-a", &from_page("app-a")), Ok(()), "an own app, mode off");
        assert_eq!(devlog_target(&store, "app-m", &from_page("app-m")), Err(StatusCode::NOT_FOUND), "installed");
        developer_mode(&store, true);
        assert_eq!(devlog_target(&store, "app-m", &from_page("app-m")), Err(StatusCode::NOT_FOUND), "installed, mode on");
        assert_eq!(devlog_target(&store, "app-a", &from_page("app-a")), Ok(()));
        assert_eq!(
            devlog_target(&store, "app-a", &axum::http::HeaderMap::new()),
            Ok(()),
            "no Referer: the app token decides"
        );
        assert_eq!(
            devlog_target(&store, "app-b", &from_page("app-a")),
            Err(StatusCode::FORBIDDEN),
            "one app's page never writes another app's console"
        );
        assert_eq!(devlog_target(&store, "emp-1", &from_page("emp-1")), Err(StatusCode::NOT_FOUND), "not an app");
        assert_eq!(devlog_target(&store, "nope", &from_page("nope")), Err(StatusCode::NOT_FOUND));
    }

    #[tokio::test]
    async fn devlog_entries_reach_app_console() {
        let (_d, store) = store();
        let id = format!("app-{}", uuid::Uuid::new_v4().simple());
        app(&store, &id, "Kart Racer");
        let body = serde_json::json!({"entries": [
            {"level": "log", "message": "booted", "source": "console", "time": 1_700_000_000_000i64},
            {"level": "error", "message": "Uncaught TypeError: car is undefined (/main.js:12:5)", "source": "error", "time": 1_700_000_000_500i64},
            {"level": "error", "message": "GET /track.json \u{2192} 404 Not Found", "source": "network", "time": 1_700_000_001_000i64}
        ]});
        assert_eq!(keep_devlog(&id, body.to_string().as_bytes()), Ok(3));
        assert_eq!(keep_devlog(&id, b"not json"), Err(StatusCode::BAD_REQUEST));
        assert_eq!(tools::app_console::error_count(&id), 2);

        let tool = tools::app_console::AppConsoleTool::new(store.clone());
        let ctx = tools::ToolContext::default();
        let off = tool.execute_dyn(&ctx, serde_json::json!({"app": "Kart Racer"})).await;
        assert!(off.is_error && off.content.contains("App Developer mode is off"), "{}", off.content);
        // With the mode off the app reads its own console.
        let own = tools::ToolContext { session_key: format!("agent:{id}:web"), ..Default::default() };
        let mine = tool.execute_dyn(&own, serde_json::json!({})).await;
        assert!(!mine.is_error && mine.content.contains("booted"), "{}", mine.content);

        developer_mode(&store, true);
        let read = tool.execute_dyn(&ctx, serde_json::json!({"app": "Kart Racer"})).await;
        assert!(!read.is_error, "{}", read.content);
        let booted = read.content.find("booted").unwrap();
        let uncaught = read.content.find("Uncaught TypeError").unwrap();
        let network = read.content.find("/track.json").unwrap();
        assert!(booted < uncaught && uncaught < network, "newest last: {}", read.content);

        let last = tools::app_console::recent(&id, None, 10).last().unwrap().seq;
        let none = tool.execute_dyn(&ctx, serde_json::json!({"app": id, "since": last})).await;
        assert!(none.content.contains("nothing new"), "{}", none.content);

        let prompt = devlog_send_prompt("Kart Racer", &tools::app_console::recent(&id, None, 500)).unwrap();
        assert!(prompt.contains("Uncaught TypeError") && prompt.contains("/track.json"), "{prompt}");
        assert!(!prompt.contains("booted"), "only errors are sent");
        assert_eq!(devlog_send_prompt("Kart Racer", &[]), None);
    }
}

#[cfg(test)]
mod media_serving_tests {
    use super::*;

    // Every range form a media element sends, resolved against a 1000-byte file.
    #[test]
    fn ranges_resolve_like_a_browser_expects() {
        let r = |h| byte_range(Some(h), 1000);
        assert_eq!(r("bytes=0-99"), ByteRange::Partial(0, 99));
        assert_eq!(r("bytes=500-"), ByteRange::Partial(500, 999));
        assert_eq!(r("bytes=-100"), ByteRange::Partial(900, 999));
        assert_eq!(r("bytes=900-5000"), ByteRange::Partial(900, 999));
        assert_eq!(r("bytes=-5000"), ByteRange::Partial(0, 999));
        assert_eq!(r("bytes=1000-"), ByteRange::Unsatisfiable);
        assert_eq!(r("bytes=50-10"), ByteRange::Unsatisfiable);
        assert_eq!(r("bytes=-0"), ByteRange::Unsatisfiable);
        assert_eq!(r("bytes=0-1,5-9"), ByteRange::Full);
        assert_eq!(r("items=0-9"), ByteRange::Full);
        assert_eq!(byte_range(None, 1000), ByteRange::Full);
        assert_eq!(byte_range(Some("bytes=0-"), 0), ByteRange::Unsatisfiable);
    }

    // A model or film served as octet-stream never loads under nosniff.
    #[test]
    fn media_files_carry_their_real_type() {
        let t = |name: &str| mime_from_path(std::path::Path::new(name));
        assert_eq!(t("a/tri.glb"), "model/gltf-binary");
        assert_eq!(t("scene.gltf"), "model/gltf+json");
        assert_eq!(t("film.MP4"), "video/mp4");
        assert_eq!(t("loop.webm"), "video/webm");
        assert_eq!(t("hit.mp3"), "audio/mpeg");
        assert_eq!(t("tex.ktx2"), "image/ktx2");
        assert_eq!(t("index.htm"), "text/html; charset=utf-8");
    }

    #[tokio::test]
    async fn a_range_reads_only_those_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let film = dir.path().join("film.mp4");
        std::fs::write(&film, (0..=255u8).collect::<Vec<_>>()).unwrap();

        let resp = serve_app_ui_range(&film, Some("bytes=10-19")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(resp.headers()[header::CONTENT_RANGE], "bytes 10-19/256");
        assert_eq!(resp.headers()[header::CONTENT_TYPE], "video/mp4");
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(body.as_ref(), (10..=19u8).collect::<Vec<_>>().as_slice());

        let past = serve_app_ui_range(&film, Some("bytes=300-")).await.unwrap();
        assert_eq!(past.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(past.headers()[header::CONTENT_RANGE], "bytes */256");

        assert!(serve_app_ui_range(&film, None).await.is_none());
    }
}

#[cfg(test)]
mod app_file_caching_tests {
    use super::*;

    fn req(pairs: &[(header::HeaderName, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(k.clone(), HeaderValue::from_str(v).unwrap());
        }
        h
    }

    fn get<'a>(r: &'a Response, name: header::HeaderName) -> Option<&'a str> {
        r.headers().get(name).and_then(|v| v.to_str().ok())
    }

    async fn body(r: Response) -> Vec<u8> {
        axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap().to_vec()
    }

    fn ui() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let ui = dir.path().to_path_buf();
        std::fs::create_dir_all(ui.join("assets")).unwrap();
        std::fs::write(ui.join("index.html"), "<html><head></head><body>hi</body></html>").unwrap();
        std::fs::write(ui.join("main-0a8ksftt.js"), "console.log(1)").unwrap();
        std::fs::write(ui.join("assets/hero.mp4"), (0..=255u8).collect::<Vec<_>>()).unwrap();
        (dir, ui)
    }

    // What a build emits is hashed; what a person names is not.
    #[test]
    fn content_hashed_names_are_told_from_hand_made_ones() {
        let hashed = |n: &str| is_content_hashed(StdPath::new(n));
        for name in ["main-0a8ksftt.js", "index-zh3p2264.js", "chunk-5JFTZ4CW.js", "app-a1b2c3d4.css", "assets/tex-9f8e7d6c5b4a.ktx2", "main-0a8ksftt.min.js"] {
            assert!(hashed(name), "{name} is hashed");
        }
        for name in [
            "index.html", "main.js", "assets/hero.mp4", "hero-section2.png", "map-1stfloor.png",
            "shot-20260930.png", "sprite-walking01.png", "bg-heroImage.png", "a1b2c3d4.js", "-0a8ksftt.js", "main-0a8ksftt",
            "main-0a8k.js",
        ] {
            assert!(!hashed(name), "{name} is not hashed");
        }
    }

    // A hashed file is kept for a year, and still tagged.
    #[tokio::test]
    async fn a_hashed_file_is_immutable_for_a_year() {
        let (_d, ui) = ui();
        let r = serve_ui_file(&ui.join("main-0a8ksftt.js"), &HeaderMap::new(), None, false, None).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(get(&r, header::CACHE_CONTROL), Some("public, max-age=31536000, immutable"));
        assert!(get(&r, header::ETAG).is_some_and(|t| t.starts_with('"')));
        assert_eq!(get(&r, header::CONTENT_TYPE), Some("application/javascript; charset=utf-8"));
    }

    // A hand-named asset asks every time, and an unchanged one costs a 304.
    #[tokio::test]
    async fn a_plain_asset_revalidates_and_answers_304_when_unchanged() {
        let (_d, ui) = ui();
        let film = ui.join("assets/hero.mp4");
        let first = serve_ui_file(&film, &HeaderMap::new(), None, false, None).await;
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(get(&first, header::CACHE_CONTROL), Some("no-cache"));
        assert_eq!(get(&first, header::ACCEPT_RANGES), Some("bytes"));
        let etag = get(&first, header::ETAG).unwrap().to_string();
        assert!(!etag.starts_with("W/"), "a strong tag");
        assert_eq!(body(first).await.len(), 256);

        let again = serve_ui_file(&film, &req(&[(header::IF_NONE_MATCH, &etag)]), None, false, None).await;
        assert_eq!(again.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(get(&again, header::ETAG), Some(etag.as_str()));
        assert_eq!(get(&again, header::CACHE_CONTROL), Some("no-cache"));
        assert!(get(&again, header::CONTENT_TYPE).is_none());
        assert!(body(again).await.is_empty());

        // Weak and listed forms of the same tag match too; another does not.
        let listed = format!("\"other\", W/{etag}");
        let r = serve_ui_file(&film, &req(&[(header::IF_NONE_MATCH, &listed)]), None, false, None).await;
        assert_eq!(r.status(), StatusCode::NOT_MODIFIED);
        let r = serve_ui_file(&film, &req(&[(header::IF_NONE_MATCH, "\"other\"")]), None, false, None).await;
        assert_eq!(r.status(), StatusCode::OK);

        // A changed file is a new tag: the old one gets the new bytes.
        std::fs::write(&film, b"new film").unwrap();
        let changed = serve_ui_file(&film, &req(&[(header::IF_NONE_MATCH, &etag)]), None, false, None).await;
        assert_eq!(changed.status(), StatusCode::OK);
        assert_ne!(get(&changed, header::ETAG), Some(etag.as_str()));
        assert_eq!(body(changed).await, b"new film");
    }

    // index.html is never kept without asking, and its tag covers what goes out.
    #[tokio::test]
    async fn the_entry_page_revalidates_and_answers_304() {
        let (_d, ui) = ui();
        let index = ui.join("index.html");
        let first = serve_ui_file(&index, &HeaderMap::new(), None, false, None).await;
        assert_eq!(get(&first, header::CACHE_CONTROL), Some("no-cache"));
        assert_eq!(get(&first, header::CONTENT_TYPE), Some("text/html; charset=utf-8"));
        let etag = get(&first, header::ETAG).unwrap().to_string();
        assert!(String::from_utf8(body(first).await).unwrap().contains("data-nebo-bridge"));

        let again = serve_ui_file(&index, &req(&[(header::IF_NONE_MATCH, &etag)]), None, false, None).await;
        assert_eq!(again.status(), StatusCode::NOT_MODIFIED);
        assert!(body(again).await.is_empty());

        std::fs::write(&index, "<html><head></head><body>v2</body></html>").unwrap();
        let changed = serve_ui_file(&index, &req(&[(header::IF_NONE_MATCH, &etag)]), None, false, None).await;
        assert_eq!(changed.status(), StatusCode::OK);
    }

    // App Developer mode: nothing is stored, nothing is tagged, nothing is a 304.
    #[tokio::test]
    async fn app_developer_mode_stores_nothing() {
        let (_d, ui) = ui();
        for name in ["index.html", "main-0a8ksftt.js", "assets/hero.mp4"] {
            let file = ui.join(name);
            let tagged = serve_ui_file(&file, &HeaderMap::new(), None, false, None).await;
            let etag = get(&tagged, header::ETAG).unwrap().to_string();
            let r = serve_ui_file(&file, &req(&[(header::IF_NONE_MATCH, &etag)]), Some(Devtools { employee: "Kart", console: true, desktop: None }), false, None).await;
            assert_eq!(r.status(), StatusCode::OK, "{name}");
            assert_eq!(get(&r, header::CACHE_CONTROL), Some("no-store"), "{name}");
            assert!(get(&r, header::ETAG).is_none(), "{name}");
        }
        let page = serve_ui_file(&ui.join("index.html"), &HeaderMap::new(), Some(Devtools { employee: "Kart", console: true, desktop: None }), false, None).await;
        assert!(String::from_utf8(body(page).await).unwrap().contains("data-nebo-devtools"));
    }

    // The owner's own app, App Developer mode off: its entry page is never
    // kept (a rebuild shows on the next load); its files keep the one rule.
    #[tokio::test]
    async fn an_own_apps_entry_page_is_never_kept() {
        let (_d, ui) = ui();
        let own = Some(Devtools { employee: "Kart", console: false, desktop: None });
        let page = serve_ui_file(&ui.join("index.html"), &HeaderMap::new(), own, false, None).await;
        assert_eq!(get(&page, header::CACHE_CONTROL), Some("no-store"));
        assert!(get(&page, header::ETAG).is_none());
        let hashed = serve_ui_file(&ui.join("main-0a8ksftt.js"), &HeaderMap::new(), own, false, None).await;
        assert_eq!(get(&hashed, header::CACHE_CONTROL), Some("public, max-age=31536000, immutable"));
        let film = serve_ui_file(&ui.join("assets/hero.mp4"), &HeaderMap::new(), own, false, None).await;
        assert_eq!(get(&film, header::CACHE_CONTROL), Some("no-cache"));
    }

    // Ranges carry the tag and the rule; an If-Range naming an older file gets the whole new one.
    #[tokio::test]
    async fn ranges_follow_the_tag() {
        let (_d, ui) = ui();
        let film = ui.join("assets/hero.mp4");
        let etag = get(&serve_ui_file(&film, &HeaderMap::new(), None, false, None).await, header::ETAG).unwrap().to_string();

        let part = serve_ui_file(&film, &req(&[(header::RANGE, "bytes=10-19"), (header::IF_RANGE, &etag)]), None, false, None).await;
        assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(get(&part, header::CONTENT_RANGE), Some("bytes 10-19/256"));
        assert_eq!(get(&part, header::ETAG), Some(etag.as_str()));
        assert_eq!(get(&part, header::CACHE_CONTROL), Some("no-cache"));
        assert_eq!(body(part).await, (10..=19u8).collect::<Vec<_>>());

        let stale = serve_ui_file(&film, &req(&[(header::RANGE, "bytes=10-19"), (header::IF_RANGE, "\"old\"")]), None, false, None).await;
        assert_eq!(stale.status(), StatusCode::OK);
        assert_eq!(body(stale).await.len(), 256);
    }

    // `device:motion` opens the gyroscope and accelerometer to that page only.
    #[tokio::test]
    async fn device_motion_opens_the_sensors_for_the_page() {
        let (_d, ui) = ui();
        let r = serve_ui_file(&ui.join("index.html"), &HeaderMap::new(), None, true, None).await;
        let policy = get(&r, header::HeaderName::from_static("permissions-policy")).unwrap();
        assert!(policy.contains("accelerometer=(self)") && policy.contains("gyroscope=(self)"), "{policy}");
        let r = serve_ui_file(&ui.join("index.html"), &HeaderMap::new(), None, false, None).await;
        assert!(r.headers().get("permissions-policy").is_none(), "the default comes from the middleware");
    }

    /// A Vite build without `base: './'`: root paths to its own files.
    fn vite_build() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let ui = dir.path().join("ui");
        std::fs::create_dir_all(ui.join("assets")).unwrap();
        std::fs::create_dir_all(ui.join("fonts")).unwrap();
        std::fs::write(
            ui.join("index.html"),
            concat!(
                "<!doctype html><html><head>",
                r#"<link rel="icon" type="image/svg+xml" href="/favicon.svg">"#,
                r#"<script type="module" crossorigin src="/assets/index-C_-Z6QB_.js"></script>"#,
                r#"<link rel="stylesheet" crossorigin href="/assets/index-Bx1y2z3q.css">"#,
                r#"<script src="/sdk/nebo.global.js"></script>"#,
                r#"<script src="https://cdn.example.com/three.js"></script>"#,
                "</head><body><div id=\"root\"></div></body></html>",
            ),
        )
        .unwrap();
        std::fs::write(ui.join("favicon.svg"), "<svg/>").unwrap();
        std::fs::write(ui.join("assets/index-C_-Z6QB_.js"), "console.log('flap')").unwrap();
        std::fs::write(ui.join("assets/index-Bx1y2z3q.css"), "@font-face{src:url(/fonts/a.woff2)}body{background:url('/gone.png')}").unwrap();
        std::fs::write(ui.join("fonts/a.woff2"), "w").unwrap();
        (dir, ui)
    }

    /// Where the browser goes for `reference` on the page at `page`.
    fn browser_resolves(page: &str, reference: &str) -> String {
        url::Url::parse(page).unwrap().join(reference).unwrap().path().to_string()
    }

    /// The `src` of the page's module script, as served.
    fn module_src(html: &str) -> String {
        let at = html.find(r#"type="module" crossorigin src=""#).expect("module script") + r#"type="module" crossorigin src=""#.len();
        html[at..].split('"').next().unwrap().to_string()
    }

    // A ui/ built with absolute /assets paths loads its JS: on the phone
    // (through the tunnel), on the bot, at a deep route and at the slashless root.
    #[tokio::test]
    async fn a_build_with_root_asset_paths_loads_its_js() {
        let (_d, ui) = vite_build();
        for (page, rel) in [
            ("https://neboai.com/t/bot-1/apps/app-1/ui/index.html", "index.html"),
            ("http://localhost:27895/apps/app-1/ui/index.html", "index.html"),
            ("https://neboai.com/t/bot-1/apps/app-1/ui/level/3", "level/3"),
            ("https://neboai.com/t/bot-1/apps/app-1/ui", ""),
        ] {
            let target = resolve_ui_file(&ui, rel).unwrap();
            let rebase = Rebase::new(&ui, rel);
            let html = String::from_utf8(body(serve_ui_file(&target, &HeaderMap::new(), None, false, Some(&rebase)).await).await).unwrap();
            let src = module_src(&html);
            let asked = browser_resolves(page, &src);
            let inside = asked.split("/apps/app-1/ui/").nth(1).unwrap_or_else(|| panic!("{page}: {src} leaves the app: {asked}"));
            let js = resolve_ui_file(&ui, inside).unwrap();
            let r = serve_ui_file(&js, &HeaderMap::new(), None, false, Some(&Rebase::new(&ui, inside))).await;
            assert_eq!(r.status(), StatusCode::OK, "{page}");
            assert_eq!(get(&r, header::CONTENT_TYPE), Some("application/javascript; charset=utf-8"), "{page}");
            assert_eq!(body(r).await, b"console.log('flap')", "{page}");
            assert!(browser_resolves(page, &html[html.find("rel=\"icon\"").unwrap()..].split('"').nth(5).unwrap()).ends_with("/apps/app-1/ui/favicon.svg"), "{html}");
            // The bot's own routes and other sites are left alone.
            assert!(html.contains(r#"src="/sdk/nebo.global.js""#), "{html}");
            assert!(html.contains(r#"src="https://cdn.example.com/three.js""#), "{html}");
            // The bridge sends a root-absolute fetch into the app's files.
            assert!(html.contains(r#"var R=["assets","favicon.svg","fonts","index.html"]"#), "{html}");
        }

        // A stylesheet's root paths resolve from its own place; a file the app lacks is left as written.
        let css = ui.join("assets/index-Bx1y2z3q.css");
        let r = serve_ui_file(&css, &HeaderMap::new(), None, false, Some(&Rebase::new(&ui, "assets/index-Bx1y2z3q.css"))).await;
        assert_eq!(get(&r, header::CONTENT_TYPE), Some("text/css; charset=utf-8"));
        assert!(get(&r, header::ETAG).is_some());
        let text = String::from_utf8(body(r).await).unwrap();
        assert_eq!(text, "@font-face{src:url(../fonts/a.woff2)}body{background:url('/gone.png')}");
    }

    // A module script with crossorigin loads: the page's own origin may read
    // it (over the tunnel and on this machine), another site may not.
    #[tokio::test]
    async fn a_module_script_with_crossorigin_loads() {
        let (_d, ui) = vite_build();
        let js = ui.join("assets/index-C_-Z6QB_.js");
        for (origin, host) in [("https://neboai.com", "neboai.com"), ("http://localhost:27895", "localhost:27895")] {
            let h = req(&[(header::ORIGIN, origin), (header::HOST, host), (header::HeaderName::from_static("sec-fetch-mode"), "cors")]);
            let r = serve_ui_file(&js, &h, None, false, Some(&Rebase::new(&ui, "assets/index-C_-Z6QB_.js"))).await;
            assert_eq!(r.status(), StatusCode::OK, "{origin}");
            assert_eq!(get(&r, header::CONTENT_TYPE), Some("application/javascript; charset=utf-8"), "{origin}");
            assert_eq!(get(&r, header::ACCESS_CONTROL_ALLOW_ORIGIN), Some(origin), "{origin}");
            assert!(r.headers().get_all(header::VARY).iter().any(|v| v == "Origin"), "{origin}");
        }
        let foreign = req(&[(header::ORIGIN, "https://evil.example"), (header::HOST, "neboai.com")]);
        let r = serve_ui_file(&js, &foreign, None, false, None).await;
        assert!(r.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
        // A missing script is a 404, never the entry page in its place.
        assert_eq!(resolve_ui_file(&ui, "assets/index-old.js"), Err(StatusCode::NOT_FOUND));
    }

    // The app folder's own index.html (the build's source, pointing at
    // ./src/main.tsx) is never served: only ui/ is.
    #[tokio::test]
    async fn the_root_index_html_is_never_served() {
        let (dir, ui) = vite_build();
        let pkg = dir.path();
        std::fs::create_dir_all(pkg.join("src")).unwrap();
        std::fs::write(pkg.join("index.html"), r#"<html><head></head><body><script type="module" src="./src/main.tsx"></script></body></html>"#).unwrap();
        std::fs::write(pkg.join("src/main.tsx"), "export {}").unwrap();

        assert_eq!(served_ui_dir(ui.clone()), Some(ui.clone()));
        assert_eq!(served_ui_dir(pkg.to_path_buf()), Some(ui.clone()), "a recorded package folder serves its ui/");
        let bare = tempfile::tempdir().unwrap();
        std::fs::write(bare.path().join("index.html"), "<p>source</p>").unwrap();
        assert_eq!(served_ui_dir(bare.path().to_path_buf()), None, "a package with no ui/ serves nothing");

        for rel in ["", "index.html", "level/3", "src/main.tsx", "../index.html", "%2e%2e/index.html"] {
            match resolve_ui_file(&ui, rel) {
                Ok(f) => {
                    assert_eq!(f, ui.join("index.html"), "{rel}");
                    let html = String::from_utf8(body(serve_ui_file(&f, &HeaderMap::new(), None, false, Some(&Rebase::new(&ui, rel))).await).await).unwrap();
                    assert!(!html.contains("main.tsx") && html.contains("index-C_-Z6QB_.js"), "{rel}: {html}");
                }
                Err(s) => assert!(s == StatusCode::NOT_FOUND || s == StatusCode::BAD_REQUEST, "{rel}: {s}"),
            }
        }
        assert_eq!(resolve_ui_file(&ui, "src/main.tsx"), Err(StatusCode::NOT_FOUND));
        assert_eq!(resolve_ui_file(&ui, "../index.html"), Err(StatusCode::BAD_REQUEST));
    }

    // The security middleware keeps a page's own policy and gives every other response the default.
    #[tokio::test]
    async fn the_security_layer_keeps_a_pages_own_policy() {
        use tower::ServiceExt;
        let app = axum::Router::new()
            .route(
                "/motion",
                axum::routing::get(|| async {
                    ([("permissions-policy", crate::middleware::PERMISSIONS_POLICY_WITH_MOTION)], "m")
                }),
            )
            .route("/plain", axum::routing::get(|| async { "p" }))
            .layer(axum::middleware::from_fn(crate::middleware::security_headers));
        let call = |path: &'static str| {
            let app = app.clone();
            async move {
                let r = app.oneshot(axum::http::Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
                r.headers()["permissions-policy"].to_str().unwrap().to_string()
            }
        };
        assert_eq!(call("/motion").await, crate::middleware::PERMISSIONS_POLICY_WITH_MOTION);
        assert_eq!(call("/plain").await, crate::middleware::PERMISSIONS_POLICY);
    }
}

/// The developer console, as it runs in a browser.
#[cfg(test)]
mod developer_console_tests {
    use super::*;

    /// `html`, as the bot serves an owner's own app's entry page (the
    /// developer script with its floating console), run in a headless
    /// Chrome for eight seconds of page time; then the console is opened and
    /// what it shows comes back: its rows, whether Send is enabled, and its
    /// status line. None when this machine has no Chrome.
    fn console_after(html: &str) -> Option<serde_json::Value> {
        let chrome = browser::chrome::find_chrome()?;
        const PROBE: &str = r#"<script>setTimeout(function(){var sh=document.querySelector('nebo-devtools').shadowRoot;sh.querySelector('.ndt-fab').click();setTimeout(function(){document.title=JSON.stringify({rows:[].map.call(sh.querySelectorAll('.ndt-row .ndt-msg'),function(e){return e.textContent}),send:!sh.querySelector('.ndt-primary').disabled,sendLabel:sh.querySelector('.ndt-primary').title,status:sh.querySelector('.ndt-status').textContent})},50)},7000)</script>"#;
        let page = inject_app_bridge(
            html.replace("</body>", &format!("{PROBE}</body>")).into_bytes(),
            Some(Devtools { employee: "Proof", console: true, desktop: None }),
            None,
        );
        // Where an app page is served from (`/apps/<id>/ui/`): the script
        // finds its app by its own address.
        let dir = tempfile::tempdir().unwrap();
        let ui = dir.path().join("apps/proof/ui");
        std::fs::create_dir_all(&ui).unwrap();
        let file = ui.join("index.html");
        std::fs::write(&file, page).unwrap();
        // Chrome prints the page and may then linger: the page is read up
        // to its end and the browser is closed.
        let mut chrome = std::process::Command::new(chrome)
            .args(["--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check"])
            .arg(format!("--user-data-dir={}", dir.path().join("profile").display()))
            .args(["--window-size=800,600", "--virtual-time-budget=9000", "--dump-dom"])
            .arg(format!("file://{}", file.display()))
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut stdout = chrome.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::io::Read;
            let (mut dom, mut buf) = (Vec::new(), [0u8; 8192]);
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 {
                    break;
                }
                dom.extend_from_slice(&buf[..n]);
                if dom.windows(7).any(|w| w == b"</html>") {
                    break;
                }
            }
            let _ = tx.send(dom);
        });
        let dom = rx.recv_timeout(std::time::Duration::from_secs(90)).unwrap_or_default();
        let _ = chrome.kill();
        let _ = chrome.wait();
        let dom = String::from_utf8_lossy(&dom);
        let title = dom.split("<title>").nth(1)?.split("</title>").next()?;
        let title = title.replace("&quot;", "\"").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&");
        Some(serde_json::from_str(&title).unwrap_or_else(|e| panic!("{e}: {title}")))
    }

    /// The console catches what goes wrong before the app's own code runs:
    /// a script that throws first thing, and a script that never loads.
    #[test]
    fn the_console_catches_an_early_error_and_a_failed_script_load() {
        let html = "<!doctype html><html><head><title>t</title><script>throw new Error('EARLY-BOOM before the app')</script>\
                    <script src=\"missing-MARK.js\"></script></head><body><h1>App</h1></body></html>";
        let Some(shown) = console_after(html) else {
            eprintln!("no Chrome on this machine — skipping");
            return;
        };
        let rows = shown["rows"].to_string();
        assert!(rows.contains("EARLY-BOOM before the app"), "{shown}");
        assert!(rows.contains("Failed to load") && rows.contains("missing-MARK.js"), "{shown}");
        assert_eq!(shown["send"], true, "{shown}");
    }

    /// Nothing logged: Send is off and says why, never pressable as if it
    /// worked. A page that draws nothing with no error says that instead.
    #[test]
    fn with_nothing_to_send_send_says_so_and_a_blank_page_is_named() {
        let Some(quiet) = console_after("<!doctype html><html><head><title>t</title></head><body><h1>Fine</h1></body></html>") else {
            eprintln!("no Chrome on this machine — skipping");
            return;
        };
        assert_eq!(quiet["rows"], serde_json::json!([]), "{quiet}");
        assert_eq!(quiet["send"], false, "{quiet}");
        assert_eq!(quiet["sendLabel"], "No errors to send.", "{quiet}");
        assert_eq!(quiet["status"], "No errors to send.", "{quiet}");

        let blank = console_after("<!doctype html><html><head><title>t</title></head><body><div id=\"root\"></div></body></html>").unwrap();
        assert!(
            blank["rows"].to_string().contains("The page loaded but drew nothing"),
            "{blank}"
        );
    }

    /// A small canvas stretched over the one that draws (live 2026-10-02:
    /// a global `canvas { width: 100% }` rule laid a 150-pixel minimap over a
    /// whole 3D game, which looked black with nothing in its console).
    #[test]
    fn a_canvas_stretched_over_the_drawing_is_named() {
        let html = "<!doctype html><html><head><title>t</title><style>canvas{position:absolute;top:0;left:0;width:100%;height:100%}</style></head>\
                    <body><canvas id=\"view\" width=\"800\" height=\"600\"></canvas><canvas id=\"minimap\" width=\"150\" height=\"150\"></canvas></body></html>";
        let Some(shown) = console_after(html) else {
            eprintln!("no Chrome on this machine — skipping");
            return;
        };
        let named = shown["rows"].as_array().unwrap().iter().filter_map(|r| r.as_str()).any(|r| {
            r.contains("<canvas id=\"minimap\"> drawn at 150\u{d7}150 is stretched to ") && r.contains("over the canvas under it")
        });
        assert!(named, "{shown}");
    }
}
