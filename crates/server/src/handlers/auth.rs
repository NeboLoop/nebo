use axum::extract::State;
use axum::response::Json;

use crate::handlers::{to_error_response, HandlerResult};
use crate::state::AppState;

/// POST /api/v1/auth/login
pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<types::api::LoginRequest>,
) -> HandlerResult<types::api::LoginResponse> {
    let resp = state
        .auth
        .login(&req.email, &req.password)
        .map_err(to_error_response)?;
    Ok(Json(types::api::LoginResponse {
        token: resp.token,
        refresh_token: resp.refresh_token,
        expires_at: resp.expires_at,
    }))
}

/// POST /api/v1/auth/register
pub async fn register(
    State(state): State<AppState>,
    Json(req): Json<types::api::RegisterRequest>,
) -> HandlerResult<types::api::LoginResponse> {
    let resp = state
        .auth
        .register(&req.email, &req.password, &req.name)
        .map_err(to_error_response)?;
    Ok(Json(types::api::LoginResponse {
        token: resp.token,
        refresh_token: resp.refresh_token,
        expires_at: resp.expires_at,
    }))
}

/// POST /api/v1/auth/refresh
pub async fn refresh(
    State(state): State<AppState>,
    Json(req): Json<types::api::RefreshTokenRequest>,
) -> HandlerResult<types::api::LoginResponse> {
    let resp = state
        .auth
        .refresh_token(&req.refresh_token)
        .map_err(to_error_response)?;
    Ok(Json(types::api::LoginResponse {
        token: resp.token,
        refresh_token: resp.refresh_token,
        expires_at: resp.expires_at,
    }))
}

/// GET /api/v1/auth/config
pub async fn config(State(state): State<AppState>) -> Json<serde_json::Value> {
    let google_enabled = !state.config.oauth.google_client_id.is_empty();
    let github_enabled = !state.config.oauth.github_client_id.is_empty();
    Json(serde_json::json!({
        "requiresSetup": state.store.count_users().unwrap_or(0) == 0,
        "googleEnabled": google_enabled,
        "githubEnabled": github_enabled,
    }))
}

/// POST /api/v1/auth/forgot
pub async fn forgot_password(
    State(state): State<AppState>,
    Json(req): Json<types::api::ForgotPasswordRequest>,
) -> HandlerResult<types::api::MessageResponse> {
    // Always return success to prevent user enumeration
    let _ = state.auth.create_password_reset_token(&req.email);
    Ok(Json(types::api::MessageResponse {
        message: "If an account exists, reset instructions have been sent".to_string(),
    }))
}

/// POST /api/v1/auth/reset
pub async fn reset_password(
    State(state): State<AppState>,
    Json(req): Json<types::api::ResetPasswordRequest>,
) -> HandlerResult<types::api::MessageResponse> {
    state
        .auth
        .reset_password(&req.token, &req.password)
        .map_err(to_error_response)?;
    Ok(Json(types::api::MessageResponse {
        message: "Password reset successfully".to_string(),
    }))
}

/// POST /api/v1/auth/verify
pub async fn verify_email(
    State(_state): State<AppState>,
    Json(_req): Json<types::api::VerifyEmailRequest>,
) -> HandlerResult<types::api::MessageResponse> {
    Ok(Json(types::api::MessageResponse {
        message: "Email verified".to_string(),
    }))
}

/// POST /api/v1/auth/resend
pub async fn resend_verification(
    State(_state): State<AppState>,
    Json(_req): Json<types::api::ResendVerificationRequest>,
) -> HandlerResult<types::api::MessageResponse> {
    Ok(Json(types::api::MessageResponse {
        message: "Verification email sent".to_string(),
    }))
}
