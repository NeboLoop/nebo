use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// NeboLoop OAuth, account, billing, and status routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/neboloop/oauth/start", axum::routing::get(handlers::neboloop::oauth_start))
        .route("/neboloop/oauth/status", axum::routing::get(handlers::neboloop::oauth_status))
        .route("/neboloop/account", axum::routing::get(handlers::neboloop::account_status))
        .route("/neboloop/account", axum::routing::delete(handlers::neboloop::account_disconnect))
        .route("/neboloop/status", axum::routing::get(handlers::neboloop::bot_status))
        .route("/neboloop/janus/usage", axum::routing::get(handlers::neboloop::janus_usage))
        .route("/neboloop/janus/usage/refresh", axum::routing::post(handlers::neboloop::janus_usage_refresh))
        .route("/neboloop/open", axum::routing::get(handlers::neboloop::open_neboloop))
        .route("/neboloop/connect", axum::routing::post(handlers::neboloop::connect_handler))
        .route("/neboloop/billing/prices", axum::routing::get(handlers::neboloop::billing_prices))
        .route("/neboloop/billing/subscription", axum::routing::get(handlers::neboloop::billing_subscription))
        .route("/neboloop/billing/checkout", axum::routing::post(handlers::neboloop::billing_checkout))
        .route("/neboloop/billing/subscribe", axum::routing::post(handlers::neboloop::billing_subscribe))
        .route("/neboloop/billing/portal", axum::routing::post(handlers::neboloop::billing_portal))
        .route("/neboloop/billing/setup-intent", axum::routing::post(handlers::neboloop::billing_setup_intent))
        .route("/neboloop/billing/cancel", axum::routing::post(handlers::neboloop::billing_cancel))
        .route("/neboloop/billing/invoices", axum::routing::get(handlers::neboloop::billing_invoices))
        .route("/neboloop/billing/payment-methods", axum::routing::get(handlers::neboloop::billing_payment_methods))
        .route("/neboloop/referral-code", axum::routing::get(handlers::neboloop::referral_code))
        .route("/neboloop/reconnect", axum::routing::post(handlers::neboloop::force_reconnect))
}
