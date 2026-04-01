use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Marketplace / store routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/store/products", axum::routing::get(handlers::store::list_store_products))
        .route("/store/products/top", axum::routing::get(handlers::store::list_store_products_top))
        .route("/store/featured", axum::routing::get(handlers::store::list_store_featured))
        .route("/store/categories", axum::routing::get(handlers::store::list_store_categories))
        .route("/store/screenshots/{type}", axum::routing::get(handlers::store::get_store_screenshots))
        .route("/store/products/{id}", axum::routing::get(handlers::store::get_store_product))
        .route("/store/products/{id}/reviews", axum::routing::get(handlers::store::get_store_product_reviews).post(handlers::store::submit_store_product_review))
        .route("/store/products/{id}/similar", axum::routing::get(handlers::store::get_store_product_similar))
        .route("/store/products/{id}/media", axum::routing::get(handlers::store::get_store_product_media))
        .route("/store/products/{id}/feedback", axum::routing::get(handlers::store::get_store_product_feedback).post(handlers::store::submit_store_product_feedback))
        .route("/store/products/{id}/install", axum::routing::post(handlers::store::install_store_product).delete(handlers::store::uninstall_store_product))
}
