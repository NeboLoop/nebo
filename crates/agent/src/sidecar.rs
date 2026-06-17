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

/// Verify a post-action screenshot using a cheap vision model.
/// Returns a short text description, or None if verification fails.
pub async fn verify_screenshot(
    provider: &dyn Provider,
    screenshot_b64: &str,
    action_context: &str,
) -> Option<String> {
    let data = screenshot_b64
        .strip_prefix("data:image/png;base64,")
        .or_else(|| screenshot_b64.strip_prefix("data:image/jpeg;base64,"))
        .unwrap_or(screenshot_b64);
    let media_type = if screenshot_b64.starts_with("data:image/jpeg") {
        "image/jpeg"
    } else {
        "image/png"
    };

    let req = ChatRequest {
        tool_choice: Default::default(),
        messages: vec![Message {
            role: "user".to_string(),
            content: format!("Action performed: {}", action_context),
            images: Some(vec![ImageContent {
                media_type: media_type.to_string(),
                data: data.to_string(),
            }]),
            ..Default::default()
        }],
        tools: vec![],
        max_tokens: 200,
        temperature: 0.0,
        system: SIDECAR_SYSTEM.to_string(),
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
            debug!("sidecar verification failed: {e}");
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
