use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::response::{Html, Json};
use rand::RngCore;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tracing::{info, warn};
use uuid::Uuid;

use super::{HandlerResult, to_error_response};
use crate::codes::build_api_client;
use crate::state::AppState;
use config;
use types::NeboError;
use types::api::ErrorResponse;

const NEBOAI_OAUTH_CLIENT_ID: &str = "nbl_nebo_desktop";
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
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

fn compute_code_challenge(verifier: &str) -> String {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let hash = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hash)
}

fn generate_state() -> String {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let mut buf = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// Derive frontend URL from API URL.
/// e.g. "https://api.neboai.com" → "https://neboai.com"
fn neboai_frontend_url(api_url: &str) -> String {
    match url::Url::parse(api_url) {
        Ok(mut u) => {
            let needs_rewrite = u.host_str().map_or(false, |h| h.starts_with("api."));
            if needs_rewrite {
                let new_host = u
                    .host_str()
                    .unwrap()
                    .strip_prefix("api.")
                    .unwrap()
                    .to_string();
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
    /// Whether the server opened the system browser itself. False on headless
    /// platforms (Android) or when spawning the opener failed — the client must
    /// open `authorize_url` in that case.
    pub opened: bool,
}

pub async fn oauth_start(
    State(state): State<AppState>,
    Query(params): Query<OAuthStartParams>,
) -> HandlerResult<OAuthStartResponse> {
    if !state.config.is_neboai_enabled() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "NeboAI integration is disabled".into(),
            }),
        ));
    }

    let flow_state = generate_state();
    let verifier = generate_code_verifier();
    let challenge = compute_code_challenge(&verifier);
    let janus_provider = params.janus.as_deref() == Some("true");

    let redirect_uri = format!(
        "http://localhost:{}/auth/neboai/callback",
        state.config.port
    );

    let authorize_params = [
        ("response_type", "code"),
        ("client_id", NEBOAI_OAUTH_CLIENT_ID),
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

    let frontend_url = neboai_frontend_url(&state.config.neboai.api_url);
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
    info!("Opening NeboAI OAuth URL in system browser");
    let opened = match open::that(&authorize_url) {
        Ok(()) => true,
        Err(e) => {
            warn!("Failed to open browser: {e}");
            false
        }
    };

    Ok(Json(OAuthStartResponse {
        authorize_url,
        state: flow_state,
        opened,
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
        return callback_html(
            "",
            &format!("Authentication was denied or failed: {err_param}"),
        );
    }

    if code.is_empty() {
        flow.error = "missing authorization code".into();
        flow.completed = true;
        return callback_html("", "Missing authorization code");
    }

    let api_url = app_state.config.neboai.api_url.clone();
    let redirect_uri = format!(
        "http://localhost:{}/auth/neboai/callback",
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

    // Store NeboAI profile in auth_profiles
    if let Err(e) = store_neboai_profile(
        &app_state.store,
        &api_url,
        &user_info.id,
        &user_info.email,
        &user_info.display_name,
        &token_resp.access_token,
        &token_resp.refresh_token,
        janus_provider,
    ) {
        warn!("Failed to store NeboAI profile: {e}");
    }

    // Reload AI providers so Janus is available immediately
    super::provider::reload_providers(&app_state.store, &app_state.config, &app_state.harness, app_state.local_host.as_ref()).await;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
}

pub async fn account_status(State(state): State<AppState>) -> HandlerResult<AccountStatusResponse> {
    let profiles = state
        .store
        .list_all_active_auth_profiles_by_provider("neboai")
        .unwrap_or_default();

    if profiles.is_empty() {
        return Ok(Json(AccountStatusResponse {
            connected: false,
            janus_provider: false,
            profile_id: None,
            owner_id: None,
            email: None,
            display_name: None,
            plan: None,
        }));
    }

    let profile = &profiles[0];
    let mut owner_id = None;
    let mut email = None;
    let mut display_name = None;
    let mut janus_provider = false;

    if let Some(ref meta_str) = profile.metadata {
        if let Ok(meta) = serde_json::from_str::<HashMap<String, String>>(meta_str) {
            owner_id = meta.get("owner_id").cloned();
            email = meta.get("email").cloned();
            display_name = meta.get("display_name").cloned();
            janus_provider = meta.get("janus_provider").map_or(false, |v| v == "true");
        }
    }

    // Cloud bots: the provisioner seeds this profile with credentials only, so
    // the card would say "Connected" without saying WHOSE account. Ask NeboAI
    // who the credentials belong to and persist it into the profile metadata —
    // one fetch, then it's served locally forever.
    if email.is_none() {
        if let Ok(api) = crate::codes::build_api_client(&state) {
            if let Ok(me) = api.owner_me().await {
                email = me.get("email").and_then(|v| v.as_str()).map(String::from);
                display_name = me
                    .get("displayName")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                if owner_id.is_none() {
                    owner_id = me.get("id").and_then(|v| v.as_str()).map(String::from);
                }
                if email.is_some() {
                    let mut meta: HashMap<String, String> = profile
                        .metadata
                        .as_deref()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or_default();
                    if let Some(ref v) = email {
                        meta.insert("email".into(), v.clone());
                    }
                    if let Some(ref v) = display_name {
                        meta.insert("display_name".into(), v.clone());
                    }
                    if let Some(ref v) = owner_id {
                        meta.insert("owner_id".into(), v.clone());
                    }
                    if let Ok(s) = serde_json::to_string(&meta) {
                        let _ = state.store.update_auth_profile_metadata(&profile.id, &s);
                    }
                }
            }
        }
    }

    let plan = state.plan_tier.read().await.clone();
    let plan = if plan.is_empty() || plan == "free" {
        None
    } else {
        Some(plan)
    };

    Ok(Json(AccountStatusResponse {
        connected: true,
        janus_provider,
        profile_id: Some(profile.id.clone()),
        owner_id,
        email,
        display_name,
        plan,
    }))
}

// --- Bot connection status (NeboAI MQTT) ---

/// GET /api/v1/neboai/status — bot/WebSocket connection status.
pub async fn bot_status(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    let profiles = state
        .store
        .list_all_active_auth_profiles_by_provider("neboai")
        .unwrap_or_default();

    let ws_connected = state.comm_manager.is_connected().await;
    let bot_id = config::read_bot_id().unwrap_or_default();

    // The default id-based handle. Canonical formatting lives in one place
    // (`comm::handle::default_bot_handle`), mirroring neboloop's `defaultHandle`.
    let default_handle = if bot_id.is_empty() {
        String::new()
    } else {
        comm::handle::default_bot_handle(&bot_id, "")
    };

    Ok(Json(serde_json::json!({
        "connected": ws_connected,
        "authenticated": !profiles.is_empty(),
        "botId": bot_id,
        "defaultHandle": default_handle,
        "apiServer": state.config.neboai.api_url,
    })))
}

// --- The bot's name ---

/// The bot's name as NeboAI holds it, and where the owner renames it.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotNameResponse {
    /// "" when the bot is not paired.
    pub name: String,
    /// The bot's page on the NeboAI web app, where the owner renames it with
    /// their own session ("" when the bot is not paired). The bot's name is
    /// the owner's: Nebo reads it and never writes it.
    pub rename_url: String,
}

/// GET /api/v1/neboai/bot — the bot's name, read from the hub.
pub async fn get_bot(State(state): State<AppState>) -> HandlerResult<BotNameResponse> {
    let Ok(api) = build_api_client(&state) else {
        return Ok(Json(BotNameResponse { name: String::new(), rename_url: String::new() }));
    };
    let name = api.get_bot().await.map(|b| b.name).map_err(|e| {
        to_error_response(NeboError::Internal(format!(
            "Could not read the bot's name from NeboAI: {e}"
        )))
    })?;
    let rename_url = format!(
        "{}/app/manage/{}",
        neboai_frontend_url(&state.config.neboai.api_url),
        api.bot_id()
    );
    Ok(Json(BotNameResponse { name, rename_url }))
}

// --- The bot's own hosted address ---

/// The bot's own hosted email address, when its hub gives it one.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotEmailResponse {
    /// "" when the bot has no hosted address (not paired, or the hub offers none).
    pub address: String,
    /// The address that reaches one employee (`agentId` in the query): the
    /// bot's address with the employee's `+tag`. "" without an `agentId` or
    /// a hosted address.
    pub employee_address: String,
    pub sending_enabled: bool,
    pub daily_limit: i64,
    pub sent_today: i64,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct BotEmailQuery {
    pub agent_id: String,
}

/// GET /api/v1/neboai/email — the bot's own address, read from the hub.
pub async fn bot_email(
    State(state): State<AppState>,
    Query(q): Query<BotEmailQuery>,
) -> HandlerResult<BotEmailResponse> {
    let info = match crate::codes::build_api_client(&state) {
        Ok(api) => api.bot_email().await.unwrap_or_default(),
        Err(_) => comm::api_types::BotEmailInfo::default(),
    };
    let employee = (!q.agent_id.is_empty())
        .then(|| state.store.get_agent(&q.agent_id).ok().flatten())
        .flatten();
    let employee_address = employee
        .and_then(|a| comm::handle::employee_email_address(&info.address, &a.name))
        .unwrap_or_default();
    Ok(Json(BotEmailResponse {
        address: info.address,
        employee_address,
        sending_enabled: info.sending_enabled,
        daily_limit: info.daily_limit,
        sent_today: info.sent_today,
    }))
}

// --- Janus AI usage ---

/// Fetch usage directly from Janus GET /v1/usage and update the in-memory cache.
async fn fetch_janus_usage(state: &AppState) -> Result<crate::state::JanusUsage, NeboError> {
    let janus_url = &state.config.neboai.janus_url;
    let Some(token) = crate::codes::neboai_token(state) else {
        return Err(NeboError::Internal(
            "no neboai token for janus usage".into(),
        ));
    };
    let bot_id = config::read_bot_id().unwrap_or_default();

    let resp = tls::http_client()
        .build()
        .map_err(|e| NeboError::Internal(format!("janus usage fetch: {e}")))?
        .get(format!("{janus_url}/v1/usage"))
        .bearer_auth(&token)
        .header("X-Bot-ID", &bot_id)
        .send()
        .await
        .map_err(|e| NeboError::Internal(format!("janus usage fetch: {e}")))?;

    if !resp.status().is_success() {
        return Err(NeboError::Internal(format!(
            "janus usage: HTTP {}",
            resp.status()
        )));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| NeboError::Internal(format!("janus usage parse: {e}")))?;

    let now = chrono::Utc::now().to_rfc3339();

    // Janus /v1/usage response body structure:
    // all_models.{included, used_percent, reset_seconds} (the plan's month;
    //   older Janus sent session_*/weekly_* amounts, still read below)
    // grants.{free_available, gift_available}
    // credits.{balance_cents}
    // plan: string
    let am = &body["all_models"];
    let session_limit = am["session_limit"].as_u64().unwrap_or(0);
    let session_used = am["session_used"].as_u64().unwrap_or(0);
    let session_reset_secs = am["session_reset_seconds"].as_i64().unwrap_or(0);
    let weekly_limit = am["weekly_limit"].as_u64().unwrap_or(0);
    let weekly_used = am["weekly_used"].as_u64().unwrap_or(0);
    let weekly_reset_secs = am["weekly_reset_seconds"].as_i64().unwrap_or(0);
    let plan_reset_secs = am["reset_seconds"].as_i64().unwrap_or(0);
    let plan_reset_at = if plan_reset_secs > 0 {
        (chrono::Utc::now() + chrono::Duration::seconds(plan_reset_secs)).to_rfc3339()
    } else {
        String::new()
    };

    let session_reset_at = if session_reset_secs > 0 {
        (chrono::Utc::now() + chrono::Duration::seconds(session_reset_secs)).to_rfc3339()
    } else {
        String::new()
    };
    let weekly_reset_at = if weekly_reset_secs > 0 {
        (chrono::Utc::now() + chrono::Duration::seconds(weekly_reset_secs)).to_rfc3339()
    } else {
        String::new()
    };

    let usage = crate::state::JanusUsage {
        session_limit_credits: session_limit,
        session_remaining_credits: session_limit.saturating_sub(session_used),
        session_reset_at,
        weekly_limit_credits: weekly_limit,
        weekly_remaining_credits: weekly_limit.saturating_sub(weekly_used),
        weekly_reset_at,
        plan_included: am["included"].as_bool().unwrap_or(false),
        plan_used_percent: am["used_percent"].as_u64().unwrap_or(0).min(100),
        plan_reset_at,
        budget_free_available: body["grants"]["free_available"].as_u64().unwrap_or(0),
        budget_gift_available: body["grants"]["gift_available"].as_u64().unwrap_or(0),
        budget_credits_cents: body["credits"]["balance_cents"].as_u64().unwrap_or(0),
        // The pool actually being consumed, in FIFO drawdown order (free → gift →
        // credits) — NOT the plan name. body["plan"] was showing "free" even when
        // the free pool was empty and spend was coming from the gift pool.
        budget_active_pool: {
            let free = body["grants"]["free_available"].as_u64().unwrap_or(0);
            let gift = body["grants"]["gift_available"].as_u64().unwrap_or(0);
            let credits = body["credits"]["balance_cents"].as_u64().unwrap_or(0);
            if free > 0 {
                "free".to_string()
            } else if gift > 0 {
                "gift".to_string()
            } else if credits > 0 {
                "credits".to_string()
            } else {
                String::new()
            }
        },
        updated_at: now,
    };

    // Update in-memory cache
    *state.janus_usage.write().await = Some(usage.clone());

    Ok(usage)
}

/// Build the JSON response from a JanusUsage struct.
pub(crate) fn janus_usage_response(u: &crate::state::JanusUsage) -> serde_json::Value {
    let session_used = u
        .session_limit_credits
        .saturating_sub(u.session_remaining_credits);
    let session_pct = if u.session_limit_credits > 0 {
        ((session_used as f64 / u.session_limit_credits as f64) * 100.0).round() as u64
    } else {
        0
    };
    let weekly_used = u
        .weekly_limit_credits
        .saturating_sub(u.weekly_remaining_credits);
    let weekly_pct = if u.weekly_limit_credits > 0 {
        ((weekly_used as f64 / u.weekly_limit_credits as f64) * 100.0).round() as u64
    } else {
        0
    };

    serde_json::json!({
        "session": {
            "limitCredits": u.session_limit_credits,
            "remainingCredits": u.session_remaining_credits,
            "usedCredits": session_used,
            "percentUsed": session_pct,
            "resetAt": if u.session_reset_at.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(u.session_reset_at.clone()) },
        },
        "weekly": {
            "limitCredits": u.weekly_limit_credits,
            "remainingCredits": u.weekly_remaining_credits,
            "usedCredits": weekly_used,
            "percentUsed": weekly_pct,
            "resetAt": if u.weekly_reset_at.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(u.weekly_reset_at.clone()) },
        },
        // The plan as the customer sees it: a percentage, never an amount.
        "plan": {
            "included": u.plan_included,
            "percentUsed": u.plan_used_percent,
            "resetAt": if u.plan_reset_at.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(u.plan_reset_at.clone()) },
        },
        "budget": {
            "freeAvailable": u.budget_free_available,
            "giftAvailable": u.budget_gift_available,
            "creditsCents": u.budget_credits_cents,
            "activePool": if u.budget_active_pool.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(u.budget_active_pool.clone()) },
        },
        "updatedAt": if u.updated_at.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(u.updated_at.clone()) },
    })
}

/// GET /api/v1/neboai/janus/usage — Janus usage stats.
/// Returns cached data if available, otherwise fetches directly from Janus.
pub async fn janus_usage(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    // Try in-memory cache first
    let cached = state.janus_usage.read().await.clone();
    if let Some(ref u) = cached {
        return Ok(Json(janus_usage_response(u)));
    }

    // No cache — fetch from Janus directly
    match fetch_janus_usage(&state).await {
        Ok(u) => Ok(Json(janus_usage_response(&u))),
        Err(e) => {
            warn!("failed to fetch janus usage: {e}");
            // Return zeros rather than error so the page still renders
            Ok(Json(serde_json::json!({
                "session": { "limitCredits": 0, "remainingCredits": 0, "usedCredits": 0, "percentUsed": 0 },
                "weekly": { "limitCredits": 0, "remainingCredits": 0, "usedCredits": 0, "percentUsed": 0 },
                "plan": { "included": false, "percentUsed": 0 },
                "budget": { "freeAvailable": 0, "giftAvailable": 0, "creditsCents": 0 },
            })))
        }
    }
}

/// POST /api/v1/neboai/janus/usage/refresh — Force-refresh usage from Janus.
pub async fn janus_usage_refresh(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let u = fetch_janus_usage(&state).await.map_err(to_error_response)?;
    Ok(Json(janus_usage_response(&u)))
}

// --- Open NeboAI in browser ---

/// GET /api/v1/neboai/open — Open NeboAI dashboard in system browser.
pub async fn open_neboai(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    let frontend_url = neboai_frontend_url(&state.config.neboai.api_url);
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
        .list_all_active_auth_profiles_by_provider("neboai")
        .unwrap_or_default();

    for profile in &profiles {
        if let Err(e) = state.store.delete_auth_profile(&profile.id) {
            warn!("Failed to delete NeboAI profile {}: {e}", profile.id);
        }
    }
    // The hosted address goes with the pairing.
    crate::mail_intake::refresh_bot_address(&state).await;

    Ok(Json(DisconnectResponse {
        message: "Disconnected from NeboAI".into(),
    }))
}

// --- Billing response types ---

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingPriceInfo {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub stripe_price_id: String,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub amount_cents: i64,
    #[serde(default)]
    pub currency: String,
    #[serde(default)]
    pub interval: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub display_order: i32,
    #[serde(default)]
    pub boost_price_id: Option<String>,
    #[serde(default)]
    pub features: Vec<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingSubscription {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub stripe_subscription_id: String,
    #[serde(default)]
    pub plan: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub current_period_end: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentMethodInfo {
    #[serde(default)]
    pub id: String,
    #[serde(default, rename = "type")]
    pub method_type: String,
    #[serde(default)]
    pub brand: String,
    #[serde(default)]
    pub last_four: String,
    #[serde(default)]
    pub exp_month: i32,
    #[serde(default)]
    pub exp_year: i32,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvoiceInfo {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub amount_cents: i64,
    #[serde(default)]
    pub currency: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub hosted_url: Option<String>,
    #[serde(default)]
    pub pdf_url: Option<String>,
}

// --- Billing proxy handlers ---

/// GET /api/v1/neboai/billing/prices — list billing plans/prices.
pub async fn billing_prices(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .billing_prices()
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("billing_prices: {e}"))))?;
    let prices: Vec<BillingPriceInfo> = resp
        .get("prices")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    Ok(Json(serde_json::json!({
        "prices": prices,
    })))
}

/// GET /api/v1/neboai/billing/subscription — current subscription.
pub async fn billing_subscription(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api.billing_subscription().await.map_err(|e| {
        to_error_response(NeboError::Internal(format!("billing_subscription: {e}")))
    })?;
    let plan: String = resp
        .get("plan")
        .and_then(|v| v.as_str())
        .unwrap_or("free")
        .to_string();
    let subscriptions: Vec<BillingSubscription> = resp
        .get("subscriptions")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    Ok(Json(serde_json::json!({
        "plan": plan,
        "subscriptions": subscriptions,
    })))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutRequest {
    #[serde(default)]
    pub price_id: String,
    #[serde(default)]
    pub price_ids: Vec<String>,
    #[serde(default)]
    pub ui_mode: Option<String>,
}

/// POST /api/v1/neboai/billing/checkout — create Stripe checkout session.
pub async fn billing_checkout(
    State(state): State<AppState>,
    Json(body): Json<CheckoutRequest>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    // Support single priceId or array of priceIds
    let price_ids: Vec<String> = if !body.price_ids.is_empty() {
        body.price_ids
            .iter()
            .filter(|s| !s.is_empty())
            .cloned()
            .collect()
    } else if !body.price_id.is_empty() {
        vec![body.price_id.clone()]
    } else {
        vec![]
    };
    if price_ids.is_empty() {
        return Err(to_error_response(NeboError::Validation(
            "priceId or priceIds is required".into(),
        )));
    }
    let ui_mode = body.ui_mode.as_deref();
    let resp = api
        .billing_checkout_multi(&price_ids, ui_mode)
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("billing_checkout: {e}"))))?;
    // For redirect-based checkout, open in system browser; embedded mode returns clientSecret instead
    if ui_mode.is_none() || ui_mode == Some("hosted") {
        if let Some(url) = resp.get("checkoutUrl").and_then(|v| v.as_str()) {
            let _ = open::that(url);
        }
    }
    let client_secret: String = resp.get("clientSecret").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let publishable_key: String = resp.get("publishableKey").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let checkout_url: Option<String> = resp.get("checkoutUrl").and_then(|v| v.as_str()).map(|s| s.to_string());
    Ok(Json(serde_json::json!({
        "clientSecret": client_secret,
        "publishableKey": publishable_key,
        "checkoutUrl": checkout_url,
    })))
}

/// POST /api/v1/neboai/billing/subscribe — create inline subscription (returns clientSecret for PaymentElement).
pub async fn billing_subscribe(
    State(state): State<AppState>,
    Json(body): Json<CheckoutRequest>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let price_ids = if !body.price_ids.is_empty() {
        body.price_ids.clone()
    } else {
        vec![body.price_id.clone()]
    };
    let resp = api
        .billing_subscribe(&price_ids)
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("billing_subscribe: {e}"))))?;
    Ok(Json(resp))
}

/// POST /api/v1/neboai/billing/portal — open Stripe customer portal.
pub async fn billing_portal(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .billing_portal()
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("billing_portal: {e}"))))?;
    // Open portal URL in system browser (Stripe CSP blocks iframe embedding)
    if let Some(url) = resp.get("portalUrl").and_then(|v| v.as_str()) {
        let _ = open::that(url);
    }
    Ok(Json(resp))
}

/// POST /api/v1/neboai/billing/setup-intent — create Stripe SetupIntent for in-app Elements.
pub async fn billing_setup_intent(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api.billing_setup_intent().await.map_err(|e| {
        to_error_response(NeboError::Internal(format!("billing_setup_intent: {e}")))
    })?;
    let client_secret: String = resp.get("clientSecret").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let publishable_key: String = resp.get("publishableKey").and_then(|v| v.as_str()).unwrap_or("").to_string();
    Ok(Json(serde_json::json!({
        "clientSecret": client_secret,
        "publishableKey": publishable_key,
    })))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelRequest {
    pub subscription_id: String,
}

/// POST /api/v1/neboai/billing/cancel — cancel subscription.
pub async fn billing_cancel(
    State(state): State<AppState>,
    Json(body): Json<CancelRequest>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .billing_cancel(&body.subscription_id)
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("billing_cancel: {e}"))))?;
    Ok(Json(resp))
}

/// GET /api/v1/neboai/billing/invoices — list invoices.
pub async fn billing_invoices(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .billing_invoices()
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("billing_invoices: {e}"))))?;
    let invoices: Vec<InvoiceInfo> = resp
        .get("invoices")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    Ok(Json(serde_json::json!({
        "invoices": invoices,
    })))
}

/// GET /api/v1/neboai/billing/payment-methods — list payment methods.
pub async fn billing_payment_methods(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api.billing_payment_methods().await.map_err(|e| {
        to_error_response(NeboError::Internal(format!("billing_payment_methods: {e}")))
    })?;
    let methods: Vec<PaymentMethodInfo> = resp
        .get("methods")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    Ok(Json(serde_json::json!({
        "methods": methods,
    })))
}

/// GET /api/v1/neboai/referral-code — fetch or create the user's referral/invite code via NeboAI.
pub async fn referral_code(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp: serde_json::Value = api
        .referral_code()
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("referral_code: {e}"))))?;
    Ok(Json(resp))
}

// --- Marketplace subscription proxy handlers ---

#[derive(serde::Deserialize)]
pub struct MarketplaceSubscriptionRequest {
    #[serde(rename = "targetId")]
    pub target_id: String,
    #[serde(rename = "targetType")]
    pub target_type: String,
    #[serde(rename = "botCount", default = "default_bot_count")]
    pub bot_count: i32,
}

fn default_bot_count() -> i32 {
    1
}

/// POST /api/v1/neboai/marketplace/subscriptions — create marketplace subscription (Stripe Checkout).
pub async fn marketplace_create_subscription(
    State(state): State<AppState>,
    Json(body): Json<MarketplaceSubscriptionRequest>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .marketplace_create_subscription(&body.target_id, &body.target_type, body.bot_count)
        .await
        .map_err(|e| {
            to_error_response(NeboError::Internal(format!(
                "marketplace_create_subscription: {e}"
            )))
        })?;
    Ok(Json(resp))
}

/// GET /api/v1/neboai/marketplace/subscriptions — list active marketplace subscriptions.
pub async fn marketplace_list_subscriptions(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api.marketplace_list_subscriptions().await.map_err(|e| {
        to_error_response(NeboError::Internal(format!(
            "marketplace_list_subscriptions: {e}"
        )))
    })?;
    Ok(Json(resp))
}

/// GET /api/v1/neboai/entitlements — list the owner's entitlements ("restore
/// purchases"): what this account owns, for the UI to show + re-fetch keys.
pub async fn entitlements(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .entitlements()
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("entitlements: {e}"))))?;
    Ok(Json(resp))
}

/// POST /api/v1/neboai/marketplace/subscriptions/:id/cancel — cancel a marketplace subscription.
pub async fn marketplace_cancel_subscription(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .marketplace_cancel_subscription(&id)
        .await
        .map_err(|e| {
            to_error_response(NeboError::Internal(format!(
                "marketplace_cancel_subscription: {e}"
            )))
        })?;
    Ok(Json(resp))
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
        "client_id": NEBOAI_OAUTH_CLIENT_ID,
        "code_verifier": code_verifier,
    });

    let resp = tls::http_client()
        .build()
        .map_err(|e| format!("token request failed: {e}"))?
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
    let resp = tls::http_client()
        .build()
        .map_err(|e| format!("userinfo request failed: {e}"))?
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

pub(crate) fn store_neboai_profile(
    store: &db::Store,
    api_url: &str,
    owner_id: &str,
    email: &str,
    display_name: &str,
    token: &str,
    refresh_token: &str,
    janus_provider: bool,
) -> Result<(), String> {
    let profiles = store
        .list_all_active_auth_profiles_by_provider("neboai")
        .unwrap_or_default();

    // Default to janus enabled — it's the primary reason users connect to NeboAI.
    // Only disable if an existing profile explicitly has janus_provider="false".
    let janus = if janus_provider {
        true
    } else {
        // Check existing profiles: if any explicitly set to "false", respect that;
        // otherwise default to true (new connections always get Janus).
        let explicitly_disabled = profiles.iter().any(|p| {
            p.metadata
                .as_deref()
                .and_then(|m| serde_json::from_str::<HashMap<String, String>>(m).ok())
                .map_or(false, |meta| {
                    meta.get("janus_provider").map_or(false, |v| v == "false")
                })
        });
        !explicitly_disabled
    };

    let mut metadata = HashMap::new();
    metadata.insert("owner_id", owner_id.to_string());
    metadata.insert("email", email.to_string());
    metadata.insert("display_name", display_name.to_string());
    metadata.insert("refresh_token", refresh_token.to_string());
    if janus {
        metadata.insert("janus_provider", "true".to_string());
    }
    let metadata_json = serde_json::to_string(&metadata).unwrap_or_default();

    if let Some(existing) = profiles.first() {
        // Update existing profile
        store
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
            let _ = store.delete_auth_profile(&p.id);
        }
    } else {
        // Create new profile
        let id = Uuid::new_v4().to_string();
        store
            .create_auth_profile(
                &id,
                email,
                "neboai",
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

/// POST /neboai/connect — Redeem a NEBO code to connect this bot to NeboAI.
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

// ── Sharing a Work-panel file by link ─────────────────────────────────
//
// One way to share: a link at neboai.com/s/<token>. The file goes up through
// the one upload path (POST /api/v1/files/upload), then the hub keeps the
// link (/api/v1/shares): who can open it, until when, and turning it off.
// The desktop's share dialog and the phone's share screen both call these.

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareLinkQuery {
    /// The Work-panel file: its `/api/v1/files/...` reference.
    pub artifact: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareLinkResponse {
    /// The file's live link; none when it has no link (or it was turned off).
    pub share: Option<comm::api_types::FileShare>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetShareLinkRequest {
    /// The Work-panel file: its `/api/v1/files/...` reference.
    pub artifact: String,
    /// `link` (anyone with the link), `password`, or `private` (only you).
    pub access: String,
    /// A new password; empty keeps the link's current one.
    #[serde(default)]
    pub password: String,
    /// RFC 3339; empty = never.
    #[serde(default)]
    pub expires_at: String,
}

/// Only files in this bot's Work panel can be shared: a link makes a file
/// readable by others, so nothing outside `<data_dir>/files/` is offered.
fn shareable_artifact(artifact: &str) -> Result<&str, (axum::http::StatusCode, Json<ErrorResponse>)> {
    let artifact = artifact.trim();
    let rel = artifact.strip_prefix("/api/v1/files/").unwrap_or("");
    if rel.is_empty() || rel.split('/').any(|seg| seg == ".." || seg.is_empty()) {
        return Err(to_error_response(NeboError::Validation(
            "Only files in the Work panel can be shared.".into(),
        )));
    }
    Ok(artifact)
}

/// A hub refusal in the owner's words (its own sentence for a 4xx), else a
/// plain failure.
fn share_error(e: comm::CommError) -> (axum::http::StatusCode, Json<ErrorResponse>) {
    let refused = match &e {
        comm::CommError::Http { status, body } if (400..500).contains(status) => {
            serde_json::from_str::<serde_json::Value>(body)
                .ok()
                .and_then(|b| b["error"].as_str().map(str::to_owned))
        }
        _ => None,
    };
    match refused {
        Some(msg) => to_error_response(NeboError::Validation(msg)),
        None => {
            warn!(error = %e, "share link: hub request failed");
            to_error_response(NeboError::Internal("Could not share this file. Try again.".into()))
        }
    }
}

/// GET /api/v1/neboai/share?artifact= — the file's live link, if it has one.
pub async fn share_link(
    State(state): State<AppState>,
    Query(q): Query<ShareLinkQuery>,
) -> HandlerResult<ShareLinkResponse> {
    let artifact = shareable_artifact(&q.artifact)?;
    let api = build_api_client(&state).map_err(to_error_response)?;
    let share = api.file_shares(artifact).await.map_err(share_error)?.into_iter().next();
    Ok(Json(ShareLinkResponse { share }))
}

/// PUT /api/v1/neboai/share — give the file a link with these settings: the
/// link it has is changed; a file with none is uploaded and gets one.
pub async fn set_share_link(
    State(state): State<AppState>,
    Json(body): Json<SetShareLinkRequest>,
) -> HandlerResult<ShareLinkResponse> {
    let artifact = shareable_artifact(&body.artifact)?;
    let api = build_api_client(&state).map_err(to_error_response)?;
    let settings = comm::api_types::FileShareSettings {
        access: body.access.clone(),
        password: body.password.clone(),
        expires_at: body.expires_at.clone(),
    };
    if let Some(existing) = api.file_shares(artifact).await.map_err(share_error)?.into_iter().next() {
        let share = api.update_file_share(&existing.id, &settings).await.map_err(share_error)?;
        return Ok(Json(ShareLinkResponse { share: Some(share) }));
    }

    if !state.comm_manager.is_connected().await {
        return Err(to_error_response(NeboError::Validation(
            "Connect to NeboAI to share.".into(),
        )));
    }
    let files_dir = config::data_dir()
        .map_err(|e| to_error_response(NeboError::Internal(e.to_string())))?
        .join("files");
    let path = crate::chat_dispatch::artifact_local_path(&files_dir, artifact)
        .ok_or_else(|| to_error_response(NeboError::NotFound))?;
    let uploaded = crate::chat_dispatch::upload_local_file(&state.comm_manager, &path)
        .await
        .map_err(|e| {
            warn!(error = %e, "share link: upload failed");
            to_error_response(NeboError::Internal("Could not upload this file. Try again.".into()))
        })?;
    let share = api
        .create_file_share(&uploaded.file_id, artifact, &settings)
        .await
        .map_err(share_error)?;
    Ok(Json(ShareLinkResponse { share: Some(share) }))
}

/// DELETE /api/v1/neboai/share?artifact= — turn the file's link off. The
/// link stops opening anything, for good.
pub async fn turn_off_share_link(
    State(state): State<AppState>,
    Query(q): Query<ShareLinkQuery>,
) -> HandlerResult<ShareLinkResponse> {
    let artifact = shareable_artifact(&q.artifact)?;
    let api = build_api_client(&state).map_err(to_error_response)?;
    for share in api.file_shares(artifact).await.map_err(share_error)? {
        api.revoke_file_share(&share.id).await.map_err(share_error)?;
    }
    Ok(Json(ShareLinkResponse { share: None }))
}

fn callback_html(_email: &str, err_msg: &str) -> Html<String> {
    let success = err_msg.is_empty();
    let heading = if success { "Signed in" } else { "Sign-in failed" };
    let message = if success {
        "You're connected to your NeboAI account."
    } else {
        err_msg
    };
    super::auth_page::auth_result_page(success, heading, message)
}

// ── Force reconnect (sleep/wake recovery) ─────────────────────────────

/// POST /api/v1/neboai/reconnect — tear down stale connection and reconnect.
/// Called by Tauri on system resume or manually for diagnostics.
pub async fn force_reconnect(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    info!("neboai: force reconnect requested (sleep/wake)");

    state.comm_manager.shutdown().await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    match crate::codes::activate_neboai(&state).await {
        Ok(()) => {
            if let Some(new_token) = state.comm_manager.take_rotated_token().await {
                let _ = state
                    .store
                    .update_auth_profile_token_by_provider("neboai", &new_token);
            }
            Ok(Json(serde_json::json!({"reconnected": true})))
        }
        Err(e) => {
            warn!(error = %e, "neboai: force reconnect failed");
            Ok(Json(
                serde_json::json!({"reconnected": false, "error": e.to_string()}),
            ))
        }
    }
}

// --- Phone binding proxy ("a number is a connected account") ---
//
// Called by the phonecall plugin's `auth login` / `auth logout` over the
// local API. The heavy lifting happens at NeboAI: pick and purchase the
// number, mint the endpoint token, wire the carrier webhook to the
// nebo-phone gateway. This side only adds the owner's identity.

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneBindRequest {
    pub agent_id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub business_name: Option<String>,
    /// The exact owned line to attach — picked in the connect modal.
    /// Empty = the oldest unclaimed line.
    #[serde(default)]
    pub number: String,
}

/// POST /api/v1/phone/bind — provision + bind a number for an employee.
pub async fn phone_bind(
    State(state): State<AppState>,
    Json(req): Json<PhoneBindRequest>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let number = Some(req.number.as_str()).filter(|n| !n.is_empty());
    let resp = api
        .bind_bot_phone(&req.agent_id, &req.label, req.business_name.as_deref(), number)
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("phone bind: {e}"))))?;
    info!(agent = %req.agent_id, number = %req.number, "phone number bound via NeboAI");
    // A line is an outside door: every caller and texter gets a chat of
    // their own (owner rule 09-25).
    state.store.mark_multi_chat(&req.agent_id).map_err(to_error_response)?;
    Ok(Json(resp))
}

/// GET /api/v1/phone/claimable — the owner's attachable lines, for the
/// connect modal's number picker.
pub async fn phone_claimable(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .claimable_bot_phone()
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("phone claimable: {e}"))))?;
    Ok(Json(resp))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneUnbindRequest {
    pub number: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneCallRequest {
    #[serde(default)]
    pub agent_id: String,
    /// The line to call from — picks between an employee's lines; optional
    /// when it holds only one.
    #[serde(default)]
    pub from: String,
    pub to: String,
    pub purpose: String,
}

/// POST /api/v1/phone/call — place one consent-gated outbound call. Called
/// by the phonecall plugin's `dial`; every gate lives at NeboAI.
pub async fn phone_call(
    State(state): State<AppState>,
    Json(req): Json<PhoneCallRequest>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .call_bot_phone(&req.agent_id, &req.from, &req.to, &req.purpose)
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("phone call: {e}"))))?;
    info!(to = %req.to, "outbound call placed via NeboAI");
    Ok(Json(resp))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneOptOutRequest {
    pub number: String,
}

/// POST /api/v1/phone/optout — revoke calling consent for a number (the
/// "stop calling me" honor path).
pub async fn phone_optout(
    State(state): State<AppState>,
    Json(req): Json<PhoneOptOutRequest>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .optout_bot_phone(&req.number)
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("phone optout: {e}"))))?;
    info!(number = %req.number, "phone opt-out recorded via NeboAI");
    Ok(Json(resp))
}

/// GET /api/v1/phone/presence — the hub-minted presence token the bridge
/// registers with. Proof of "this bot is here"; the gateway routes any of
/// the bot's lines to a socket registered with it.
pub async fn phone_presence(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .get_phone_presence()
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("phone presence: {e}"))))?;
    Ok(Json(resp))
}

/// GET /api/v1/phone/lines — the lines this bot answers, greeting included.
/// Read live so /manage/phone edits land on the very next call.
pub async fn phone_lines(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .list_phone_lines()
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("phone lines: {e}"))))?;
    Ok(Json(resp))
}

#[derive(serde::Deserialize)]
pub struct PhoneAnswerRequest {
    #[serde(rename = "agentId")]
    pub agent_id: String,
}

/// POST /api/v1/phone/answer — make one employee able to answer calls on
/// this computer: install the Phone plugin if it is missing, then bind its
/// channel to the employee so the bridge runs. The hub calls this right
/// after assigning a line at /manage/phone; the Phone settings tab offers
/// it as a retry. Without the binding the plugin is just a binary on disk
/// and every call falls to voicemail.
pub async fn phone_answer(
    State(state): State<AppState>,
    Json(req): Json<PhoneAnswerRequest>,
) -> HandlerResult<serde_json::Value> {
    const SLUG: &str = "phonecall";
    if state
        .store
        .get_agent(&req.agent_id)
        .map_err(to_error_response)?
        .is_none()
    {
        return Err(to_error_response(NeboError::NotFound));
    }
    if state.plugin_store.get_channel_def(SLUG).is_none() {
        let api = build_api_client(&state).map_err(to_error_response)?;
        crate::codes::fetch_and_install_plugin(&state, &api, SLUG, "Phonecall", None)
            .await
            .map_err(to_error_response)?;
        state
            .hub
            .broadcast("plugin_installed", serde_json::json!({ "plugin": "Phonecall" }));
        info!(agent = %req.agent_id, "installed the Phone plugin for a hub-assigned line");
    }
    super::agents::bind_channel(&state, &req.agent_id, SLUG)
        .await
        .map_err(to_error_response)?;
    // A line means strangers talk to this employee: every caller gets sealed
    // memory from here on (conversations kept apart; a Confidential employee
    // stays Confidential), and update_agent refuses one conversation while
    // the line is attached.
    if !crate::workflow_manager::agent_memory_mode(&state.store, &req.agent_id).separates_conversations() {
        let fm = state
            .store
            .set_agent_memory_mode(&req.agent_id, "separate")
            .map_err(to_error_response)?;
        if let Ok(Some(agent)) = state.store.get_agent(&req.agent_id) {
            super::agents::write_agent_json_to_fs(&agent.napp_path, &fm);
        }
    }
    state.hub.broadcast("agent_updated", serde_json::json!({ "agentId": req.agent_id }));
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Does the hub have an active line assigned to this employee? Live, not
/// cached: the answer gates un-isolating memory, and a stale yes would lock
/// an owner out of their own setting.
pub(crate) async fn agent_has_phone_line(state: &AppState, agent_id: &str) -> bool {
    let Ok(api) = build_api_client(state) else { return false };
    let Ok(lines) = api.list_phone_lines().await else { return false };
    lines["numbers"]
        .as_array()
        .map(|ns| {
            ns.iter().any(|l| {
                l["agentId"].as_str() == Some(agent_id) && l["status"].as_str() == Some("active")
            })
        })
        .unwrap_or(false)
}

/// POST /api/v1/phone/unbind — release a bound number.
pub async fn phone_unbind(
    State(state): State<AppState>,
    Json(req): Json<PhoneUnbindRequest>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .unbind_bot_phone(&req.number)
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("phone unbind: {e}"))))?;
    info!(number = %req.number, "phone number released via NeboAI");
    Ok(Json(resp))
}

#[cfg(test)]
mod share_link_tests {
    use super::*;

    /// A link makes a file readable by others: only Work-panel files under
    /// `/api/v1/files/` can get one, never a path that climbs out of it.
    #[test]
    fn only_work_panel_files_can_be_shared() {
        assert_eq!(shareable_artifact(" /api/v1/files/Go-Live-Checklist.md ").unwrap(), "/api/v1/files/Go-Live-Checklist.md");
        assert!(shareable_artifact("/api/v1/files/reports/q3.pdf").is_ok());
        for bad in ["", "/api/v1/files/", "/etc/passwd", "/api/v1/files/../settings.json", "/api/v1/files/a//b", "https://example.com/x.md"] {
            assert!(shareable_artifact(bad).is_err(), "{bad} was shareable");
        }
    }

    /// The hub's own sentence reaches the owner for what it refused; anything
    /// else is a plain failure, never a raw status.
    #[test]
    fn hub_refusals_speak_plainly() {
        let refused = comm::CommError::Http { status: 400, body: r#"{"error":"Use at least 6 characters for the password."}"#.into() };
        let (status, Json(body)) = share_error(refused);
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(body.error, "Use at least 6 characters for the password.");
        let (status, Json(body)) = share_error(comm::CommError::Http { status: 502, body: "bad gateway".into() });
        assert_eq!(status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body.error, "Could not share this file. Try again.");
    }
}

#[cfg(test)]
mod usage_tests {
    use super::*;

    /// The plan leaves as a percentage and a reset, never an amount; the
    /// purchased balance stays in cents (the customer's own money).
    #[test]
    fn plan_usage_is_a_percentage() {
        let u = crate::state::JanusUsage {
            plan_included: true,
            plan_used_percent: 42,
            plan_reset_at: "2026-11-01T00:00:00Z".into(),
            budget_credits_cents: 2500,
            ..Default::default()
        };
        let v = janus_usage_response(&u);
        assert_eq!(v["plan"], serde_json::json!({"included": true, "percentUsed": 42, "resetAt": "2026-11-01T00:00:00Z"}));
        assert_eq!(v["budget"]["creditsCents"], 2500);
    }
}
