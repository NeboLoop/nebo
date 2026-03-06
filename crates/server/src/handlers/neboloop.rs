use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::response::{Html, Json};
use rand::RngCore;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tracing::{info, warn};
use uuid::Uuid;

use super::HandlerResult;
use crate::state::AppState;
use config;
use types::api::ErrorResponse;

const NEBOLOOP_OAUTH_CLIENT_ID: &str = "nbl_nebo_desktop";
const OAUTH_FLOW_TIMEOUT: Duration = Duration::from_secs(10 * 60);

// --- In-memory pending OAuth flows ---

struct OAuthFlowState {
    code_verifier: String,
    created_at: Instant,
    completed: bool,
    error: String,
    email: String,
    display_name: String,
    janus_provider: bool,
}

static PENDING_FLOWS: LazyLock<Mutex<HashMap<String, OAuthFlowState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// --- PKCE helpers (RFC 7636) ---

fn generate_code_verifier() -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

fn compute_code_challenge(verifier: &str) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let hash = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hash)
}

fn generate_state() -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let mut buf = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// Derive frontend URL from API URL.
/// e.g. "https://api.neboloop.com" → "https://neboloop.com"
fn neboloop_frontend_url(api_url: &str) -> String {
    match url::Url::parse(api_url) {
        Ok(mut u) => {
            let needs_rewrite = u
                .host_str()
                .map_or(false, |h| h.starts_with("api."));
            if needs_rewrite {
                let new_host = u.host_str().unwrap().strip_prefix("api.").unwrap().to_string();
                let _ = u.set_host(Some(&new_host));
            }
            u.to_string().trim_end_matches('/').to_string()
        }
        Err(_) => api_url.to_string(),
    }
}

// --- Handlers ---

#[derive(serde::Deserialize)]
pub struct OAuthStartParams {
    pub janus: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthStartResponse {
    pub authorize_url: String,
    pub state: String,
}

pub async fn oauth_start(
    State(state): State<AppState>,
    Query(params): Query<OAuthStartParams>,
) -> HandlerResult<OAuthStartResponse> {
    if !state.config.is_neboloop_enabled() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "NeboLoop integration is disabled".into(),
            }),
        ));
    }

    let flow_state = generate_state();
    let verifier = generate_code_verifier();
    let challenge = compute_code_challenge(&verifier);
    let janus_provider = params.janus.as_deref() == Some("true");

    let redirect_uri = format!("http://localhost:{}/auth/neboloop/callback", state.config.port);

    let authorize_params = [
        ("response_type", "code"),
        ("client_id", NEBOLOOP_OAUTH_CLIENT_ID),
        ("redirect_uri", &redirect_uri),
        ("scope", "openid profile email"),
        ("state", &flow_state),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
    ];

    let query_string: String = authorize_params
        .iter()
        .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&");

    let frontend_url = neboloop_frontend_url(&state.config.neboloop.api_url);
    let authorize_url = format!("{}/oauth/authorize?{}", frontend_url, query_string);

    // Store pending flow
    {
        let mut flows = PENDING_FLOWS.lock().await;
        // Cleanup expired flows while we're here
        flows.retain(|_, f| f.created_at.elapsed() < OAUTH_FLOW_TIMEOUT);
        flows.insert(
            flow_state.clone(),
            OAuthFlowState {
                code_verifier: verifier,
                created_at: Instant::now(),
                completed: false,
                error: String::new(),
                email: String::new(),
                display_name: String::new(),
                janus_provider,
            },
        );
    }

    // Open browser (server-side, same pattern as Go implementation)
    info!("Opening NeboLoop OAuth URL in system browser");
    if let Err(e) = open::that(&authorize_url) {
        warn!("Failed to open browser: {e}");
    }

    Ok(Json(OAuthStartResponse {
        authorize_url,
        state: flow_state,
    }))
}

// --- OAuth callback (browser redirect handler) ---

#[derive(serde::Deserialize)]
pub struct OAuthCallbackParams {
    pub state: Option<String>,
    pub code: Option<String>,
    pub error: Option<String>,
}

pub async fn oauth_callback(
    State(app_state): State<AppState>,
    Query(params): Query<OAuthCallbackParams>,
) -> Html<String> {
    let state_param = params.state.unwrap_or_default();
    let code = params.code.unwrap_or_default();
    let err_param = params.error.unwrap_or_default();

    let mut flows = PENDING_FLOWS.lock().await;

    let Some(flow) = flows.get_mut(&state_param) else {
        return callback_html("", "Invalid or expired OAuth state");
    };

    if !err_param.is_empty() {
        flow.error = err_param.clone();
        flow.completed = true;
        return callback_html("", &format!("Authentication was denied or failed: {err_param}"));
    }

    if code.is_empty() {
        flow.error = "missing authorization code".into();
        flow.completed = true;
        return callback_html("", "Missing authorization code");
    }

    let api_url = app_state.config.neboloop.api_url.clone();
    let redirect_uri = format!(
        "http://localhost:{}/auth/neboloop/callback",
        app_state.config.port
    );
    let code_verifier = flow.code_verifier.clone();
    let janus_provider = flow.janus_provider;

    // Exchange authorization code for tokens
    let token_resp = match exchange_oauth_code(&api_url, &code, &code_verifier, &redirect_uri).await
    {
        Ok(resp) => resp,
        Err(e) => {
            flow.error = e.to_string();
            flow.completed = true;
            return callback_html("", "Token exchange failed");
        }
    };

    // Get user info
    let user_info = match fetch_user_info(&api_url, &token_resp.access_token).await {
        Ok(info) => info,
        Err(e) => {
            flow.error = e.to_string();
            flow.completed = true;
            return callback_html("", "Failed to get user info");
        }
    };

    // Store NeboLoop profile in auth_profiles
    if let Err(e) = store_neboloop_profile(
        &app_state,
        &api_url,
        &user_info.id,
        &user_info.email,
        &token_resp.access_token,
        &token_resp.refresh_token,
        janus_provider,
    ) {
        warn!("Failed to store NeboLoop profile: {e}");
    }

    // Mark flow as completed
    flow.email = user_info.email.clone();
    flow.display_name = user_info.display_name.clone();
    flow.completed = true;

    callback_html(&user_info.email, "")
}

// --- OAuth status polling ---

#[derive(serde::Deserialize)]
pub struct OAuthStatusParams {
    pub state: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthStatusResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub async fn oauth_status(
    Query(params): Query<OAuthStatusParams>,
) -> HandlerResult<OAuthStatusResponse> {
    let state_param = params.state.unwrap_or_default();
    if state_param.is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "state parameter required".into(),
            }),
        ));
    }

    let mut flows = PENDING_FLOWS.lock().await;

    let Some(flow) = flows.get(&state_param) else {
        return Ok(Json(OAuthStatusResponse {
            status: "expired".into(),
            email: None,
            display_name: None,
            error: None,
        }));
    };

    if !flow.completed {
        return Ok(Json(OAuthStatusResponse {
            status: "pending".into(),
            email: None,
            display_name: None,
            error: None,
        }));
    }

    let resp = if flow.error.is_empty() {
        OAuthStatusResponse {
            status: "complete".into(),
            email: Some(flow.email.clone()),
            display_name: Some(flow.display_name.clone()),
            error: None,
        }
    } else {
        OAuthStatusResponse {
            status: "error".into(),
            email: None,
            display_name: None,
            error: Some(flow.error.clone()),
        }
    };

    // Clean up after status is read
    flows.remove(&state_param);

    Ok(Json(resp))
}

// --- Account status ---

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatusResponse {
    pub connected: bool,
    pub janus_provider: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

pub async fn account_status(
    State(state): State<AppState>,
) -> HandlerResult<AccountStatusResponse> {
    let profiles = state
        .store
        .list_active_auth_profiles_by_provider("neboloop")
        .unwrap_or_default();

    if profiles.is_empty() {
        return Ok(Json(AccountStatusResponse {
            connected: false,
            janus_provider: false,
            profile_id: None,
            owner_id: None,
            email: None,
        }));
    }

    let profile = &profiles[0];
    let mut owner_id = None;
    let mut email = None;
    let mut janus_provider = false;

    if let Some(ref meta_str) = profile.metadata {
        if let Ok(meta) = serde_json::from_str::<HashMap<String, String>>(meta_str) {
            owner_id = meta.get("owner_id").cloned();
            email = meta.get("email").cloned();
            janus_provider = meta.get("janus_provider").map_or(false, |v| v == "true");
        }
    }

    Ok(Json(AccountStatusResponse {
        connected: true,
        janus_provider,
        profile_id: Some(profile.id.clone()),
        owner_id,
        email,
    }))
}

// --- Bot connection status (NeboLoop MQTT) ---

/// GET /api/v1/neboloop/status — bot/WebSocket connection status.
pub async fn bot_status(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let profiles = state
        .store
        .list_active_auth_profiles_by_provider("neboloop")
        .unwrap_or_default();

    let ws_connected = state.comm_manager.is_connected().await;
    let bot_id = config::read_bot_id().unwrap_or_default();

    Ok(Json(serde_json::json!({
        "connected": ws_connected,
        "authenticated": !profiles.is_empty(),
        "botId": bot_id,
        "apiServer": state.config.neboloop.api_url,
    })))
}

// --- Janus AI usage ---

/// GET /api/v1/neboloop/janus/usage — Janus usage stats.
/// Returns zeroed-out usage when running in local mode (no Janus proxy).
pub async fn janus_usage() -> HandlerResult<serde_json::Value> {
    Ok(Json(serde_json::json!({
        "session": {
            "limitTokens": 0,
            "remainingTokens": 0,
            "usedTokens": 0,
            "percentUsed": 0,
        },
        "weekly": {
            "limitTokens": 0,
            "remainingTokens": 0,
            "usedTokens": 0,
            "percentUsed": 0,
        },
    })))
}

// --- Open NeboLoop in browser ---

/// GET /api/v1/neboloop/open — Open NeboLoop dashboard in system browser.
pub async fn open_neboloop(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let frontend_url = neboloop_frontend_url(&state.config.neboloop.api_url);
    // Best-effort: open browser, may fail in headless environments
    let _ = open::that(&frontend_url);
    Ok(Json(serde_json::json!({"ok": true})))
}

// --- Account disconnect ---

#[derive(serde::Serialize)]
pub struct DisconnectResponse {
    pub message: String,
}

pub async fn account_disconnect(
    State(state): State<AppState>,
) -> HandlerResult<DisconnectResponse> {
    let profiles = state
        .store
        .list_active_auth_profiles_by_provider("neboloop")
        .unwrap_or_default();

    for profile in &profiles {
        if let Err(e) = state.store.delete_auth_profile(&profile.id) {
            warn!("Failed to delete NeboLoop profile {}: {e}", profile.id);
        }
    }

    Ok(Json(DisconnectResponse {
        message: "Disconnected from NeboLoop".into(),
    }))
}

// --- HTTP helpers ---

#[derive(serde::Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    #[allow(dead_code)]
    token_type: Option<String>,
    #[allow(dead_code)]
    expires_in: Option<i64>,
    refresh_token: String,
    #[allow(dead_code)]
    scope: Option<String>,
}

#[derive(serde::Deserialize)]
struct OAuthUserInfo {
    #[serde(rename = "sub")]
    id: String,
    email: String,
    #[serde(rename = "name")]
    display_name: String,
}

async fn exchange_oauth_code(
    api_url: &str,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<OAuthTokenResponse, String> {
    let body = serde_json::json!({
        "grant_type": "authorization_code",
        "code": code,
        "redirect_uri": redirect_uri,
        "client_id": NEBOLOOP_OAUTH_CLIENT_ID,
        "code_verifier": code_verifier,
    });

    let resp = reqwest::Client::new()
        .post(format!("{api_url}/oauth/token"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("token request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("token endpoint returned {status}: {text}"));
    }

    resp.json::<OAuthTokenResponse>()
        .await
        .map_err(|e| format!("decode token response: {e}"))
}

async fn fetch_user_info(api_url: &str, access_token: &str) -> Result<OAuthUserInfo, String> {
    let resp = reqwest::Client::new()
        .get(format!("{api_url}/oauth/userinfo"))
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| format!("userinfo request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("userinfo endpoint returned {status}: {text}"));
    }

    resp.json::<OAuthUserInfo>()
        .await
        .map_err(|e| format!("decode userinfo response: {e}"))
}

pub(crate) fn store_neboloop_profile(
    app_state: &AppState,
    api_url: &str,
    owner_id: &str,
    email: &str,
    token: &str,
    refresh_token: &str,
    janus_provider: bool,
) -> Result<(), String> {
    let profiles = app_state
        .store
        .list_active_auth_profiles_by_provider("neboloop")
        .unwrap_or_default();

    // Carry forward janus_provider from existing profile if not explicitly set
    let janus = if janus_provider {
        true
    } else {
        profiles.iter().any(|p| {
            p.metadata
                .as_deref()
                .and_then(|m| serde_json::from_str::<HashMap<String, String>>(m).ok())
                .map_or(false, |meta| {
                    meta.get("janus_provider").map_or(false, |v| v == "true")
                })
        })
    };

    let mut metadata = HashMap::new();
    metadata.insert("owner_id", owner_id.to_string());
    metadata.insert("email", email.to_string());
    metadata.insert("refresh_token", refresh_token.to_string());
    if janus {
        metadata.insert("janus_provider", "true".to_string());
    }
    let metadata_json = serde_json::to_string(&metadata).unwrap_or_default();

    if let Some(existing) = profiles.first() {
        // Update existing profile
        app_state
            .store
            .update_auth_profile(
                &existing.id,
                email,
                token,
                None,
                Some(api_url),
                0,
                Some("oauth"),
                Some(&metadata_json),
            )
            .map_err(|e| e.to_string())?;

        // Delete any extra profiles
        for p in profiles.iter().skip(1) {
            // Best-effort: clean up duplicate profiles
            let _ = app_state.store.delete_auth_profile(&p.id);
        }
    } else {
        // Create new profile
        let id = Uuid::new_v4().to_string();
        app_state
            .store
            .create_auth_profile(
                &id,
                email,
                "neboloop",
                token,
                None,
                Some(api_url),
                0,
                1,
                Some("oauth"),
                Some(&metadata_json),
            )
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

// --- Code-based connect ---

#[derive(serde::Deserialize)]
pub struct ConnectRequest {
    pub code: String,
}

/// POST /neboloop/connect — Redeem a NEBO code to connect this bot to NeboLoop.
pub async fn connect_handler(
    State(state): State<AppState>,
    Json(body): Json<ConnectRequest>,
) -> HandlerResult<serde_json::Value> {
    let bot_id = crate::codes::redeem_nebo_code(&state, &body.code)
        .await
        .map_err(super::to_error_response)?;

    Ok(Json(serde_json::json!({
        "connected": true,
        "botId": bot_id
    })))
}

fn callback_html(_email: &str, err_msg: &str) -> Html<String> {
    let message = if !err_msg.is_empty() {
        format!("Sign-in failed: {err_msg}")
    } else {
        "Connected! You can close this window.".into()
    };

    Html(format!(
        r#"<!DOCTYPE html>
<html><head><title>NeboLoop</title>
<style>
body {{ font-family: -apple-system, sans-serif; display: flex; align-items: center;
  justify-content: center; min-height: 100vh; margin: 0; background: #f5f5f5; }}
p {{ font-size: 16px; color: #333; }}
</style>
</head>
<body>
<p>{message}</p>
<script>
// Try to close this window/tab automatically
setTimeout(function() {{ window.close(); }}, 1500);
</script>
</body></html>"#,
    ))
}
