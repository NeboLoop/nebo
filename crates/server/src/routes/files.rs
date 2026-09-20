use axum::extract::DefaultBodyLimit;
use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// The ceiling every upload door wears, from `Runtime.MaxUploadBytes` in
/// `etc/nebo.yaml` (see [`config::DEFAULT_MAX_UPLOAD_BYTES`] for what an unset
/// value means and why the number is what it is). One door is not enough: the
/// limit is per-route in axum, so a door built without this keeps axum's own
/// 2 MiB default and refuses any photo.
pub fn upload_limit(max_upload_bytes: usize) -> DefaultBodyLimit {
    DefaultBodyLimit::max(max_upload_bytes)
}

/// File serving and picker routes.
pub fn routes(max_upload_bytes: usize) -> Router<AppState> {
    Router::new()
        .route(
            "/files/browse",
            axum::routing::post(handlers::files::browse),
        )
        .route(
            "/files/pick",
            axum::routing::post(handlers::files::pick_files),
        )
        .route(
            "/files/pick-folder",
            axum::routing::post(handlers::files::pick_folder),
        )
        .route(
            "/files/upload",
            axum::routing::post(handlers::files::upload_file)
                .layer(upload_limit(max_upload_bytes)),
        )
        .route(
            "/files/{*path}",
            axum::routing::get(handlers::files::serve_file),
        )
        .route(
            "/work/documents",
            axum::routing::get(handlers::files::list_work_documents),
        )
        .route(
            "/comm-files/{id}",
            axum::routing::get(handlers::files::serve_comm_file),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    const BOUNDARY: &str = "nebouploadcap";

    /// One multipart field of `bytes` bytes, shaped like a photo from a phone.
    fn multipart_body(bytes: usize) -> Body {
        let mut body = format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"photo.jpg\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .into_bytes();
        body.extend(std::iter::repeat_n(b'x', bytes));
        body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
        Body::from(body)
    }

    /// Reads the field the way `handlers::files::upload_file` does, including
    /// the sentence it gives a file that is too big.
    async fn read_upload(
        axum::Extension(limit): axum::Extension<usize>,
        mut multipart: axum::extract::Multipart,
    ) -> Result<String, (StatusCode, axum::Json<types::api::ErrorResponse>)> {
        let mut size = 0usize;
        while let Some(field) = multipart
            .next_field()
            .await
            .map_err(|e| crate::handlers::files::too_big_or_bad(limit, e))?
        {
            size += field
                .bytes()
                .await
                .map_err(|e| crate::handlers::files::too_big_or_bad(limit, e))?
                .len();
        }
        Ok(format!("stored {size}"))
    }

    /// An upload door built the way `routes()` and `layers::routes()` build
    /// theirs: the production ceiling over the production multipart reader.
    fn door(limit: usize) -> axum::Router {
        axum::Router::new()
            .route(
                "/upload",
                axum::routing::post(read_upload).layer(upload_limit(limit)),
            )
            .layer(axum::Extension(limit))
    }

    async fn post_upload(limit: usize, bytes: usize) -> (StatusCode, String) {
        let response = door(limit)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/upload")
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={BOUNDARY}"),
                    )
                    .body(multipart_body(bytes))
                    .expect("request builds"),
            )
            .await
            .expect("the door answers");
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body reads");
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    /// Over the cap the door was built with: refused, and the refusal says the
    /// file is too big rather than that the request was malformed.
    #[tokio::test]
    async fn over_the_configured_cap_is_refused_in_plain_words() {
        let limit = 4 * 1024 * 1024;
        let (status, body) = post_upload(limit, limit + 1).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body.contains("larger than 4 MB, which is the most one upload may carry"),
            "the refusal should name the size in plain words, got: {body}"
        );
        assert!(
            !body.contains("multipart/form-data"),
            "the opaque parse error must not come back, got: {body}"
        );
    }

    /// Under the same cap: accepted. This is the case axum's own 2 MiB default
    /// used to refuse, which is why a phone photo looked like a broken camera.
    #[tokio::test]
    async fn under_the_configured_cap_is_accepted() {
        let limit = 4 * 1024 * 1024;
        let sent = 3 * 1024 * 1024;
        let (status, body) = post_upload(limit, sent).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body, format!("stored {sent}"));
    }

    /// The doors are built from one number, and an unset config makes it 100 MB.
    #[tokio::test]
    async fn an_unset_config_caps_uploads_at_100_mb() {
        let limit = config::RuntimeConfig::default().max_upload_bytes();
        assert_eq!(limit, 100 * 1024 * 1024);
        assert_eq!(limit, config::DEFAULT_MAX_UPLOAD_BYTES);

        let (status, body) = post_upload(limit, 8 * 1024 * 1024).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body, format!("stored {}", 8 * 1024 * 1024));
    }
}
