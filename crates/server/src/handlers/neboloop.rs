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

use super::{to_error_response, HandlerResult};
use crate::codes::build_api_client;
use crate::state::AppState;
use config;
use types::NeboError;
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
        &user_info.display_name,
        &token_resp.access_token,
        &token_resp.refresh_token,
        janus_provider,
    ) {
        warn!("Failed to store NeboLoop profile: {e}");
    }

    // Reload AI providers so Janus is available immediately
    super::provider::reload_providers(&app_state).await;

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

pub async fn account_status(
    State(state): State<AppState>,
) -> HandlerResult<AccountStatusResponse> {
    let profiles = state
        .store
        .list_all_active_auth_profiles_by_provider("neboloop")
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

    let plan = state.plan_tier.read().await.clone();
    let plan = if plan.is_empty() || plan == "free" { None } else { Some(plan) };

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

// --- Bot connection status (NeboLoop MQTT) ---

/// GET /api/v1/neboloop/status — bot/WebSocket connection status.
pub async fn bot_status(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let profiles = state
        .store
        .list_all_active_auth_profiles_by_provider("neboloop")
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

/// Fetch usage directly from Janus GET /v1/usage and update the in-memory cache.
async fn fetch_janus_usage(state: &AppState) -> Result<crate::state::JanusUsage, NeboError> {
    let janus_url = &state.config.neboloop.janus_url;
    let profiles = state
        .store
        .list_all_active_auth_profiles_by_provider("neboloop")
        .unwrap_or_default();
    let token = profiles
        .first()
        .map(|p| p.api_key.clone())
        .unwrap_or_default();
    if token.is_empty() {
        return Err(NeboError::Internal("no neboloop token for janus usage".into()));
    }
    let bot_id = config::read_bot_id().unwrap_or_default();

    let resp = reqwest::Client::new()
        .get(format!("{janus_url}/v1/usage"))
        .bearer_auth(&token)
        .header("X-Bot-ID", &bot_id)
        .send()
        .await
        .map_err(|e| NeboError::Internal(format!("janus usage fetch: {e}")))?;

    if !resp.status().is_success() {
        return Err(NeboError::Internal(format!("janus usage: HTTP {}", resp.status())));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| NeboError::Internal(format!("janus usage parse: {e}")))?;

    let now = chrono::Utc::now().to_rfc3339();

    // Janus /v1/usage response body structure:
    // all_models.{session_used, session_limit, session_reset_seconds, weekly_*}
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
        budget_free_available: body["grants"]["free_available"].as_u64().unwrap_or(0),
        budget_gift_available: body["grants"]["gift_available"].as_u64().unwrap_or(0),
        budget_credits_cents: body["credits"]["balance_cents"].as_u64().unwrap_or(0),
        budget_active_pool: body["plan"].as_str().unwrap_or("").to_string(),
        updated_at: now,
    };

    // Update in-memory cache
    *state.janus_usage.write().await = Some(usage.clone());

    Ok(usage)
}

/// Build the JSON response from a JanusUsage struct.
fn janus_usage_response(u: &crate::state::JanusUsage) -> serde_json::Value {
    let session_used = u.session_limit_credits.saturating_sub(u.session_remaining_credits);
    let session_pct = if u.session_limit_credits > 0 {
        ((session_used as f64 / u.session_limit_credits as f64) * 100.0).round() as u64
    } else {
        0
    };
    let weekly_used = u.weekly_limit_credits.saturating_sub(u.weekly_remaining_credits);
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
        "budget": {
            "freeAvailable": u.budget_free_available,
            "giftAvailable": u.budget_gift_available,
            "creditsCents": u.budget_credits_cents,
            "activePool": if u.budget_active_pool.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(u.budget_active_pool.clone()) },
        },
        "updatedAt": if u.updated_at.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(u.updated_at.clone()) },
    })
}

/// GET /api/v1/neboloop/janus/usage — Janus usage stats.
/// Returns cached data if available, otherwise fetches directly from Janus.
pub async fn janus_usage(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
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
                "budget": { "freeAvailable": 0, "giftAvailable": 0, "creditsCents": 0 },
            })))
        }
    }
}

/// POST /api/v1/neboloop/janus/usage/refresh — Force-refresh usage from Janus.
pub async fn janus_usage_refresh(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let u = fetch_janus_usage(&state).await.map_err(to_error_response)?;
    Ok(Json(janus_usage_response(&u)))
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
        .list_all_active_auth_profiles_by_provider("neboloop")
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

// --- Billing proxy handlers ---

/// GET /api/v1/neboloop/billing/prices — list billing plans/prices.
pub async fn billing_prices(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api.billing_prices().await
        .map_err(|e| to_error_response(NeboError::Internal(format!("billing_prices: {e}"))))?;
    Ok(Json(resp))
}

/// GET /api/v1/neboloop/billing/subscription — current subscription.
pub async fn billing_subscription(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api.billing_subscription().await
        .map_err(|e| to_error_response(NeboError::Internal(format!("billing_subscription: {e}"))))?;
    Ok(Json(resp))
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

/// POST /api/v1/neboloop/billing/checkout — create Stripe checkout session.
pub async fn billing_checkout(
    State(state): State<AppState>,
    Json(body): Json<CheckoutRequest>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    // Support single priceId or array of priceIds
    let price_ids: Vec<String> = if !body.price_ids.is_empty() {
        body.price_ids.iter().filter(|s| !s.is_empty()).cloned().collect()
    } else if !body.price_id.is_empty() {
        vec![body.price_id.clone()]
    } else {
        vec![]
    };
    if price_ids.is_empty() {
        return Err(to_error_response(NeboError::Validation("priceId or priceIds is required".into())));
    }
    let ui_mode = body.ui_mode.as_deref();
    let resp = api.billing_checkout_multi(&price_ids, ui_mode).await
        .map_err(|e| to_error_response(NeboError::Internal(format!("billing_checkout: {e}"))))?;
    // For redirect-based checkout, open in system browser; embedded mode returns clientSecret instead
    if ui_mode.is_none() || ui_mode == Some("hosted") {
        if let Some(url) = resp.get("checkoutUrl").and_then(|v| v.as_str()) {
            let _ = open::that(url);
        }
    }
    Ok(Json(resp))
}

/// POST /api/v1/neboloop/billing/subscribe — create inline subscription (returns clientSecret for PaymentElement).
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
    let resp = api.billing_subscribe(&price_ids).await
        .map_err(|e| to_error_response(NeboError::Internal(format!("billing_subscribe: {e}"))))?;
    Ok(Json(resp))
}

/// POST /api/v1/neboloop/billing/portal — open Stripe customer portal.
pub async fn billing_portal(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api.billing_portal().await
        .map_err(|e| to_error_response(NeboError::Internal(format!("billing_portal: {e}"))))?;
    // Open portal URL in system browser (Stripe CSP blocks iframe embedding)
    if let Some(url) = resp.get("portalUrl").and_then(|v| v.as_str()) {
        let _ = open::that(url);
    }
    Ok(Json(resp))
}

/// POST /api/v1/neboloop/billing/setup-intent — create Stripe SetupIntent for in-app Elements.
pub async fn billing_setup_intent(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api.billing_setup_intent().await
        .map_err(|e| to_error_response(NeboError::Internal(format!("billing_setup_intent: {e}"))))?;
    Ok(Json(resp))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelRequest {
    pub subscription_id: String,
}

/// POST /api/v1/neboloop/billing/cancel — cancel subscription.
pub async fn billing_cancel(
    State(state): State<AppState>,
    Json(body): Json<CancelRequest>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api.billing_cancel(&body.subscription_id).await
        .map_err(|e| to_error_response(NeboError::Internal(format!("billing_cancel: {e}"))))?;
    Ok(Json(resp))
}

/// GET /api/v1/neboloop/billing/invoices — list invoices.
pub async fn billing_invoices(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api.billing_invoices().await
        .map_err(|e| to_error_response(NeboError::Internal(format!("billing_invoices: {e}"))))?;
    Ok(Json(resp))
}

/// GET /api/v1/neboloop/billing/payment-methods — list payment methods.
pub async fn billing_payment_methods(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api.billing_payment_methods().await
        .map_err(|e| to_error_response(NeboError::Internal(format!("billing_payment_methods: {e}"))))?;
    Ok(Json(resp))
}

/// GET /api/v1/neboloop/referral-code — fetch or create the user's referral/invite code via NeboLoop.
pub async fn referral_code(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp: serde_json::Value = api.referral_code().await
        .map_err(|e| to_error_response(NeboError::Internal(format!("referral_code: {e}"))))?;
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
    display_name: &str,
    token: &str,
    refresh_token: &str,
    janus_provider: bool,
) -> Result<(), String> {
    let profiles = app_state
        .store
        .list_all_active_auth_profiles_by_provider("neboloop")
        .unwrap_or_default();

    // Default to janus enabled — it's the primary reason users connect to NeboLoop.
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

// ── Force reconnect (sleep/wake recovery) ─────────────────────────────

/// POST /api/v1/neboloop/reconnect — tear down stale connection and reconnect.
/// Called by Tauri on system resume or manually for diagnostics.
pub async fn force_reconnect(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    info!("neboloop: force reconnect requested (sleep/wake)");

    state.comm_manager.shutdown().await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    match crate::codes::activate_neboloop(&state).await {
        Ok(()) => {
            if let Some(new_token) = state.comm_manager.take_rotated_token().await {
                let _ = state.store.update_auth_profile_token_by_provider("neboloop", &new_token);
            }
            Ok(Json(serde_json::json!({"reconnected": true})))
        }
        Err(e) => {
            warn!(error = %e, "neboloop: force reconnect failed");
            Ok(Json(serde_json::json!({"reconnected": false, "error": e.to_string()})))
        }
    }
}
