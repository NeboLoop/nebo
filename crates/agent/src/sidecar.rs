use ai::{ChatRequest, ImageContent, Message, Provider, StreamEventType};
use tracing::debug;

const SIDECAR_SYSTEM: &str = "You are the eyes of an AI agent whose own model cannot see images. \
Describe this screenshot — it may be the user's whole desktop, an app window, or a web page — \
so the agent can answer the user and decide what to do next. Be concrete and concise.\n\n\
WHAT: 2-4 sentences on what is actually visible — the app(s)/window(s) in focus, what the user \
appears to be looking at, and any prominent text, titles, files, or UI you can read. If it is a \
desktop, name the visible apps and any notable windows or files.\n\
BLOCKER: <none | auth-required | captcha | paywall | cookie-banner | age-gate | rate-limited> \
— only relevant for web pages; use 'none' otherwise.\n\
ELEMENTS: up to 5 things the agent might act on, each as '<description> @ (<x>,<y>)' (approximate \
center points). Omit this section entirely if nothing is interactive.\n\n\
Report only what you can actually see. Do not invent content. No preamble.";

/// Resolve the sidecar model — empty string lets Janus pick the model.
fn sidecar_model() -> String {
    config::ModelsConfig::load()
        .sidecar_model()
        .unwrap_or_default()
}

const ATTACHMENT_SYSTEM: &str = "You are the eyes of an AI agent whose own model cannot see \
images. The user attached this image to their message. Describe it so the agent can answer them \
as if it had seen it itself.\n\n\
Cover what the image IS (photo, screenshot, diagram, document scan, chart), what it shows, and \
transcribe any text that is legible — verbatim for short text, faithfully summarised for long \
passages. If it is a document or receipt, give the figures and fields, not an impression. Be \
concrete; omit nothing the user is likely asking about.\n\n\
Report only what you can actually see. Do not invent content. No preamble.";

/// Run one image through the sidecar vision model and return its description.
async fn describe(
    provider: &dyn Provider,
    image: ImageContent,
    system: &str,
    context: String,
    max_tokens: i32,
) -> Option<String> {
    let req = ChatRequest {
        tool_choice: Default::default(),
        messages: vec![Message {
            role: "user".to_string(),
            content: context,
            images: Some(vec![image]),
            ..Default::default()
        }],
        tools: vec![],
        max_tokens,
        temperature: 0.0,
        system: system.to_string(),
        static_system: String::new(),
        model: sidecar_model(),
        enable_thinking: false,
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: None,
        trace: None,
    };

    let mut rx = match provider.stream(&req).await {
        Ok(rx) => rx,
        Err(e) => {
            debug!("sidecar description failed: {e}");
            return None;
        }
    };

    let mut text = String::new();
    while let Some(event) = rx.recv().await {
        match event.event_type {
            StreamEventType::Text => text.push_str(&event.text),
            StreamEventType::Done | StreamEventType::Error => break,
            _ => {}
        }
    }

    if text.is_empty() { None } else { Some(text) }
}

/// Verify a post-action screenshot using a cheap vision model.
/// Returns a short text description, or None if verification fails.
pub async fn verify_screenshot(
    provider: &dyn Provider,
    screenshot_b64: &str,
    action_context: &str,
) -> Option<String> {
    let (media_type, data) = ai::image_source_to_base64(screenshot_b64)?;
    describe(
        provider,
        ImageContent { media_type, data },
        SIDECAR_SYSTEM,
        format!("Action performed: {}", action_context),
        200,
    )
    .await
}

/// Convert images attached to a request into text, for providers that never put
/// `Message::images` on the wire (CLI wrappers, local GGUF). Without this the
/// user attaches a photo, the field is dropped on the floor, and the model
/// answers as though the message arrived empty-handed.
///
/// Every image leaves a mark in the text — a description when the sidecar can
/// produce one, an honest admission when it cannot. The model must never be
/// left able to claim nothing was attached.
pub async fn describe_attached_images(provider: &dyn Provider, req: &mut ChatRequest) {
    for idx in 0..req.messages.len() {
        let Some(images) = req.messages[idx].images.take() else {
            continue;
        };
        let total = images.len();
        for (n, image) in images.into_iter().enumerate() {
            let label = if total > 1 {
                format!("Attached image {} of {}", n + 1, total)
            } else {
                "Attached image".to_string()
            };
            let note = match describe(
                provider,
                image,
                ATTACHMENT_SYSTEM,
                "Describe this attached image.".to_string(),
                600,
            )
            .await
            {
                Some(text) => format!("\n\n[{}]\n{}", label, text),
                None => format!(
                    "\n\n[{}] The user attached an image, but this model cannot view images and \
                     automatic description was unavailable. Say so plainly and ask the user to \
                     describe it — do NOT claim no image was attached.",
                    label
                ),
            };
            req.messages[idx].content.push_str(&note);
        }
    }
}
