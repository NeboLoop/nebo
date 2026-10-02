//! Feedback to the NeboAI team, from the account menu.
//!
//! A message, optional screenshots and, unless the person turns them off,
//! diagnostics. The screenshots go up through the one file path
//! (`POST /api/v1/files/upload` on the hub, via [`NeboAIApi::upload_file`]),
//! straight from here: they are not attachments to any conversation, so they
//! do not land in the uploads folder or announce an arrival a flow could wake
//! on. The feedback is filed with the hub's `POST /api/v1/support/feedback`,
//! which writes a support ticket in the team's inbox.
//!
//! Diagnostics are what this Nebo knows about itself — its version, the
//! platform and OS, its bot id — plus the screen the person was on. Every
//! value passes through [`types::redact`] before it leaves the machine, and a
//! key that names a secret never leaves at all.

use std::collections::BTreeMap;

use axum::extract::{Multipart, State};
use axum::response::Json;
use comm::api::NeboAIApi;
use comm::api_types::SupportFeedback;
use tracing::warn;

use super::{HandlerResult, to_error_response};
use crate::codes::build_api_client;
use crate::state::AppState;
use types::NeboError;

/// The longest message the hub takes.
pub const FEEDBACK_MESSAGE_MAX: usize = 5000;
/// The most screenshots one piece of feedback carries.
pub const FEEDBACK_MAX_ATTACHMENTS: usize = 5;
/// The longest screen name kept in the diagnostics.
const SCREEN_MAX: usize = 200;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendFeedbackResponse {
    /// `received`.
    pub status: String,
}

/// One screenshot, as the form sent it.
pub struct FeedbackImage {
    pub filename: String,
    pub mime_type: String,
    pub data: Vec<u8>,
}

/// What the form sent, parsed.
#[derive(Default)]
pub struct FeedbackForm {
    pub message: String,
    pub include_diagnostics: bool,
    pub screen: String,
    pub images: Vec<FeedbackImage>,
}

/// Why feedback was not sent, in the person's words.
#[derive(Debug, PartialEq)]
pub enum FeedbackError {
    /// Something the person can fix in the form.
    Invalid(String),
    /// A screenshot did not go up.
    Attach,
    /// The hub did not take the feedback.
    Send,
}

impl FeedbackError {
    fn into_response(self) -> (axum::http::StatusCode, Json<types::api::ErrorResponse>) {
        match self {
            FeedbackError::Invalid(msg) => to_error_response(NeboError::Validation(msg)),
            FeedbackError::Attach => to_error_response(NeboError::Upstream(
                "Could not attach a screenshot. Try again.".into(),
            )),
            FeedbackError::Send => to_error_response(NeboError::Upstream(
                "Could not send your feedback. Try again.".into(),
            )),
        }
    }
}

/// POST /api/v1/neboai/feedback — multipart: `message`, `includeDiagnostics`
/// (`true`/`false`, default true), `screen`, and up to five `file` images.
pub async fn send_feedback(
    State(state): State<AppState>,
    multipart: Multipart,
) -> HandlerResult<SendFeedbackResponse> {
    let form = read_form(multipart)
        .await
        .map_err(FeedbackError::into_response)?;
    validate(&form).map_err(FeedbackError::into_response)?;
    let api = build_api_client(&state).map_err(|_| {
        to_error_response(NeboError::Validation(
            "Connect to NeboAI to send feedback.".into(),
        ))
    })?;
    let bot_id = config::read_bot_id();
    file_feedback(&api, form, bot_id.as_deref())
        .await
        .map_err(FeedbackError::into_response)?;
    Ok(Json(SendFeedbackResponse {
        status: "received".into(),
    }))
}

async fn read_form(mut multipart: Multipart) -> Result<FeedbackForm, FeedbackError> {
    let bad = |e: axum::extract::multipart::MultipartError| FeedbackError::Invalid(e.body_text());
    let mut form = FeedbackForm {
        include_diagnostics: true,
        ..Default::default()
    };
    while let Some(field) = multipart.next_field().await.map_err(bad)? {
        match field.name().unwrap_or_default() {
            "message" => form.message = field.text().await.map_err(bad)?,
            "includeDiagnostics" => {
                form.include_diagnostics = field.text().await.map_err(bad)?.trim() != "false"
            }
            "screen" => form.screen = field.text().await.map_err(bad)?,
            "file" => {
                let filename = field.file_name().unwrap_or("screenshot.png").to_string();
                let mime_type = field.content_type().unwrap_or_default().to_string();
                let data = field.bytes().await.map_err(bad)?.to_vec();
                form.images.push(FeedbackImage {
                    filename,
                    mime_type,
                    data,
                });
            }
            _ => {}
        }
    }
    Ok(form)
}

/// The form's own rules: a message, not too long, a few images.
pub fn validate(form: &FeedbackForm) -> Result<(), FeedbackError> {
    let message = form.message.trim();
    if message.is_empty() {
        return Err(FeedbackError::Invalid("Write a message first.".into()));
    }
    if message.chars().count() > FEEDBACK_MESSAGE_MAX {
        return Err(FeedbackError::Invalid(format!(
            "Keep the message under {FEEDBACK_MESSAGE_MAX} characters."
        )));
    }
    if form.images.len() > FEEDBACK_MAX_ATTACHMENTS {
        return Err(FeedbackError::Invalid(format!(
            "Attach at most {FEEDBACK_MAX_ATTACHMENTS} screenshots."
        )));
    }
    if form
        .images
        .iter()
        .any(|i| !i.mime_type.starts_with("image/") || i.data.is_empty())
    {
        return Err(FeedbackError::Invalid(
            "Only images can be attached.".into(),
        ));
    }
    Ok(())
}

/// Uploads the screenshots through the one file path, then files the
/// feedback with the hub.
pub async fn file_feedback(
    api: &NeboAIApi,
    form: FeedbackForm,
    bot_id: Option<&str>,
) -> Result<(), FeedbackError> {
    validate(&form)?;
    let mut attachments = Vec::with_capacity(form.images.len());
    for image in form.images {
        let uploaded = api
            .upload_file(&image.filename, &image.mime_type, image.data, &[])
            .await
            .map_err(|e| {
                warn!(error = %e, "feedback: screenshot upload failed");
                FeedbackError::Attach
            })?;
        attachments.push(uploaded.file_id);
    }
    let feedback = SupportFeedback {
        message: form.message.trim().to_string(),
        attachments,
        diagnostics: form
            .include_diagnostics
            .then(|| diagnostics(&form.screen, bot_id)),
        source: "desktop".into(),
    };
    api.send_support_feedback(&feedback).await.map_err(|e| {
        warn!(error = %e, "feedback: hub did not take it");
        FeedbackError::Send
    })
}

/// What this Nebo knows about itself and where the person was. Redacted:
/// a key that names a secret is dropped, and every value is scanned.
pub fn diagnostics(screen: &str, bot_id: Option<&str>) -> BTreeMap<String, String> {
    let version = env!("CARGO_PKG_VERSION");
    let screen: String = screen.trim().chars().take(SCREEN_MAX).collect();
    let raw = [
        ("appVersion", version.to_string()),
        ("botVersion", version.to_string()),
        (
            "platform",
            format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        ),
        ("os", sysinfo::System::long_os_version().unwrap_or_default()),
        ("botId", bot_id.unwrap_or_default().to_string()),
        ("screen", screen),
    ];
    redact_diagnostics(raw.into_iter().map(|(k, v)| (k.to_string(), v)))
}

/// Drops keys that name a secret and empty values; redacts the rest.
pub fn redact_diagnostics(
    raw: impl IntoIterator<Item = (String, String)>,
) -> BTreeMap<String, String> {
    raw.into_iter()
        .filter(|(k, v)| !types::redact::is_sensitive_key(k) && !v.trim().is_empty())
        .map(|(k, v)| {
            let v = types::redact::redact(&v);
            (k, v)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::post;
    use axum::{Json as AxumJson, Router};
    use std::sync::{Arc, Mutex};

    const FAKE_JWT: &str = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ0ZXN0In0.c2lnbmF0dXJlLWZha2U";
    const BOT: &str = "6f1c2d3e-0000-4000-8000-000000000001";

    #[derive(Default)]
    struct Hub {
        uploads: Vec<String>,
        feedback: Vec<serde_json::Value>,
        refuse: bool,
    }
    type Shared = Arc<Mutex<Hub>>;

    async fn upload(
        axum::extract::State(hub): axum::extract::State<Shared>,
        mut mp: Multipart,
    ) -> AxumJson<serde_json::Value> {
        let mut name = String::new();
        while let Some(f) = mp.next_field().await.unwrap() {
            if f.name() == Some("file") {
                name = f.file_name().unwrap_or_default().to_string();
                let _ = f.bytes().await;
            }
        }
        let id = format!("file-{}", hub.lock().unwrap().uploads.len() + 1);
        hub.lock().unwrap().uploads.push(name.clone());
        AxumJson(serde_json::json!({
            "fileId": id, "filename": name, "mimeType": "image/png", "size": 3,
            "url": format!("https://hub.test/api/v1/files/{id}")
        }))
    }

    async fn feedback(
        axum::extract::State(hub): axum::extract::State<Shared>,
        AxumJson(body): AxumJson<serde_json::Value>,
    ) -> (axum::http::StatusCode, AxumJson<serde_json::Value>) {
        let mut h = hub.lock().unwrap();
        if h.refuse {
            return (
                axum::http::StatusCode::NOT_FOUND,
                AxumJson(serde_json::json!({"error": "not found"})),
            );
        }
        h.feedback.push(body);
        (
            axum::http::StatusCode::CREATED,
            AxumJson(serde_json::json!({"status": "received"})),
        )
    }

    async fn fake_hub(refuse: bool) -> (NeboAIApi, Shared) {
        let hub: Shared = Arc::new(Mutex::new(Hub {
            refuse,
            ..Default::default()
        }));
        let app = Router::new()
            .route("/api/v1/files/upload", post(upload))
            .route("/api/v1/support/feedback", post(feedback))
            .with_state(hub.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (
            NeboAIApi::new(format!("http://{addr}"), BOT.into(), "test-token".into()),
            hub,
        )
    }

    fn form(message: &str) -> FeedbackForm {
        FeedbackForm {
            message: message.into(),
            include_diagnostics: true,
            screen: "/chat/assistant".into(),
            images: vec![],
        }
    }

    fn png(name: &str) -> FeedbackImage {
        FeedbackImage {
            filename: name.into(),
            mime_type: "image/png".into(),
            data: b"png".to_vec(),
        }
    }

    #[test]
    fn an_empty_message_is_refused() {
        assert_eq!(
            validate(&form("   ")),
            Err(FeedbackError::Invalid("Write a message first.".into()))
        );
        let mut f = form("hi");
        f.images = vec![FeedbackImage {
            filename: "a.pdf".into(),
            mime_type: "application/pdf".into(),
            data: b"x".to_vec(),
        }];
        assert!(matches!(validate(&f), Err(FeedbackError::Invalid(_))));
        f.images = (0..6).map(|i| png(&format!("{i}.png"))).collect();
        assert!(matches!(validate(&f), Err(FeedbackError::Invalid(_))));
    }

    #[test]
    fn diagnostics_carry_no_token_shaped_value() {
        let d = diagnostics(
            &format!("/chat?token=abc&x=1 Bearer abc {FAKE_JWT}"),
            Some(BOT),
        );
        assert_eq!(d["appVersion"], env!("CARGO_PKG_VERSION"));
        assert_eq!(d["botId"], BOT);
        assert!(d.contains_key("platform"));
        let all = serde_json::to_string(&d).unwrap();
        assert!(!all.contains("c2lnbmF0dXJlLWZha2U"), "{all}");
        assert!(!all.contains("token=abc"), "{all}");
        assert!(!all.contains("Bearer abc"), "{all}");

        let r = redact_diagnostics([
            ("accessToken".to_string(), "fake-SECRET".to_string()),
            ("screen".to_string(), "home".to_string()),
        ]);
        assert!(!r.contains_key("accessToken"));
        assert_eq!(r["screen"], "home");
    }

    #[tokio::test]
    async fn screenshots_go_up_the_one_path_then_the_feedback_is_filed() {
        let (api, hub) = fake_hub(false).await;
        let mut f = form("  The inbox badge is wrong.  ");
        f.images = vec![png("shot.png")];
        file_feedback(&api, f, Some(BOT)).await.unwrap();

        let h = hub.lock().unwrap();
        assert_eq!(h.uploads, vec!["shot.png"]);
        let body = &h.feedback[0];
        assert_eq!(body["message"], "The inbox badge is wrong.");
        assert_eq!(body["attachments"], serde_json::json!(["file-1"]));
        assert_eq!(body["source"], "desktop");
        assert_eq!(body["diagnostics"]["botId"], BOT);
        assert_eq!(body["diagnostics"]["screen"], "/chat/assistant");
    }

    #[tokio::test]
    async fn diagnostics_turned_off_are_left_out() {
        let (api, hub) = fake_hub(false).await;
        let mut f = form("Love it.");
        f.include_diagnostics = false;
        file_feedback(&api, f, Some(BOT)).await.unwrap();
        let body = &hub.lock().unwrap().feedback[0];
        assert!(body.get("diagnostics").is_none(), "{body}");
        assert!(body.get("attachments").is_none(), "{body}");
    }

    #[tokio::test]
    async fn a_refusal_is_a_plain_failure() {
        let (api, _hub) = fake_hub(true).await;
        assert_eq!(
            file_feedback(&api, form("Keep this."), Some(BOT)).await,
            Err(FeedbackError::Send)
        );
    }
}
