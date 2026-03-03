pub mod middleware;
mod state;

use std::net::TcpListener;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{Method, StatusCode};
use axum::response::Json;
use axum::Router;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;

use config::Config;
use middleware::{AuthClaims, JwtSecret};
use state::AppState;
use types::NeboError;
use types::api::HealthResponse;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Run the Nebo HTTP server.
pub async fn run(cfg: Config, quiet: bool) -> Result<(), NeboError> {
    let port = cfg.port;
    let host = cfg.host.clone();
    let bind_addr = format!("{host}:{port}");

    // Check port availability
    TcpListener::bind(&bind_addr).map_err(|_| NeboError::PortInUse(port))?;

    if !quiet {
        println!("Starting server on http://localhost:{port}");
    }

    // Initialize database
    let store = Arc::new(db::Store::new(&cfg.database.sqlite_path)?);

    // Initialize auth service
    let auth_service = Arc::new(auth::AuthService::new(store.clone(), cfg.clone()));

    let jwt_secret = JwtSecret(cfg.auth.access_secret.clone());

    let state = AppState {
        config: cfg.clone(),
        store,
        auth: auth_service,
    };

    // Build router
    let app = Router::new()
        .route("/health", axum::routing::get(health_handler))
        .nest("/api/v1", api_routes(jwt_secret))
        .layer(cors_layer())
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    if !quiet {
        info!("Server ready at http://localhost:{port}");
    }

    // Warn if non-loopback
    if host != "127.0.0.1" && host != "localhost" && host != "::1" {
        eprintln!("WARNING: Server binding to {bind_addr} — Nebo is designed for localhost-only access");
    }

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .map_err(|e| NeboError::Server(format!("failed to bind: {e}")))?;

    axum::serve(listener, app)
        .await
        .map_err(|e| NeboError::Server(format!("server error: {e}")))?;

    Ok(())
}

async fn health_handler() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
        version: VERSION.into(),
    })
}

fn api_routes(jwt_secret: JwtSecret) -> Router<AppState> {
    // Public routes (no auth required)
    let public = Router::new()
        .route("/auth/login", axum::routing::post(auth_login_handler))
        .route("/auth/register", axum::routing::post(auth_register_handler))
        .route("/auth/refresh", axum::routing::post(auth_refresh_handler))
        .route("/setup/status", axum::routing::get(setup_status_handler));

    // Protected routes (JWT required)
    let protected = Router::new()
        .route("/user/me", axum::routing::get(get_current_user_handler))
        .route(
            "/user/me/change-password",
            axum::routing::post(change_password_handler),
        )
        .route(
            "/notifications",
            axum::routing::get(list_notifications_handler),
        )
        .route(
            "/notifications/{id}/read",
            axum::routing::post(mark_notification_read_handler),
        )
        .layer(axum::Extension(jwt_secret))
        .layer(axum::middleware::from_fn(middleware::jwt_auth));

    Router::new().merge(public).merge(protected)
}

// --- Public handlers ---

async fn auth_login_handler(
    State(state): State<AppState>,
    Json(req): Json<types::api::LoginRequest>,
) -> Result<Json<types::api::LoginResponse>, (StatusCode, Json<types::api::ErrorResponse>)> {
    match state.auth.login(&req.email, &req.password) {
        Ok(resp) => Ok(Json(types::api::LoginResponse {
            token: resp.token,
            refresh_token: resp.refresh_token,
            expires_at: resp.expires_at,
        })),
        Err(e) => Err(to_error_response(e)),
    }
}

async fn auth_register_handler(
    State(state): State<AppState>,
    Json(req): Json<types::api::RegisterRequest>,
) -> Result<Json<types::api::LoginResponse>, (StatusCode, Json<types::api::ErrorResponse>)> {
    match state.auth.register(&req.email, &req.password, &req.name) {
        Ok(resp) => Ok(Json(types::api::LoginResponse {
            token: resp.token,
            refresh_token: resp.refresh_token,
            expires_at: resp.expires_at,
        })),
        Err(e) => Err(to_error_response(e)),
    }
}

async fn auth_refresh_handler(
    State(state): State<AppState>,
    Json(req): Json<types::api::RefreshTokenRequest>,
) -> Result<Json<types::api::LoginResponse>, (StatusCode, Json<types::api::ErrorResponse>)> {
    match state.auth.refresh_token(&req.refresh_token) {
        Ok(resp) => Ok(Json(types::api::LoginResponse {
            token: resp.token,
            refresh_token: resp.refresh_token,
            expires_at: resp.expires_at,
        })),
        Err(e) => Err(to_error_response(e)),
    }
}

async fn setup_status_handler() -> Json<serde_json::Value> {
    let complete = config::is_setup_complete().unwrap_or(false);
    Json(serde_json::json!({
        "setupComplete": complete,
    }))
}

// --- Protected handlers ---

async fn get_current_user_handler(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<AuthClaims>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<types::api::ErrorResponse>)> {
    match state.auth.get_user_by_id(&claims.user_id) {
        Ok(Some(user)) => Ok(Json(serde_json::json!({
            "id": user.id,
            "email": user.email,
            "name": user.name,
            "avatarUrl": user.avatar_url,
            "role": user.role,
            "createdAt": user.created_at,
        }))),
        Ok(None) => Err(to_error_response(NeboError::UserNotFound)),
        Err(e) => Err(to_error_response(e)),
    }
}

async fn change_password_handler(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<AuthClaims>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<types::api::ErrorResponse>)> {
    let current = req["currentPassword"]
        .as_str()
        .unwrap_or_default();
    let new_pass = req["newPassword"]
        .as_str()
        .unwrap_or_default();

    match state
        .auth
        .change_password(&claims.user_id, current, new_pass)
    {
        Ok(()) => Ok(Json(serde_json::json!({"success": true}))),
        Err(e) => Err(to_error_response(e)),
    }
}

async fn list_notifications_handler(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<AuthClaims>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<types::api::ErrorResponse>)> {
    match state.store.list_user_notifications(&claims.user_id, 50, 0) {
        Ok(notifs) => Ok(Json(serde_json::json!({
            "notifications": notifs,
        }))),
        Err(e) => Err(to_error_response(e)),
    }
}

async fn mark_notification_read_handler(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<AuthClaims>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<types::api::ErrorResponse>)> {
    match state.store.mark_notification_read(&id, &claims.user_id) {
        Ok(()) => Ok(Json(serde_json::json!({"success": true}))),
        Err(e) => Err(to_error_response(e)),
    }
}

// --- Helpers ---

fn to_error_response(e: NeboError) -> (StatusCode, Json<types::api::ErrorResponse>) {
    (
        StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(types::api::ErrorResponse {
            error: e.to_string(),
        }),
    )
}

fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
            Method::PATCH,
        ])
        .allow_headers(Any)
}
