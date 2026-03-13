use ai::{ChatRequest, ImageContent, Message, Provider, StreamEventType};
use tracing::debug;

const SIDECAR_SYSTEM: &str = "You verify browser automation actions. \
Given a screenshot taken immediately after an action, describe what you see in 1-2 SHORT sentences. \
Focus on: Was the action successful? What is the current page state? \
Be concise. Do not describe the screenshot in detail.";

/// Resolve the sidecar model from models.yaml (cheapest fallback model).
fn sidecar_model() -> String {
    config::ModelsConfig::load()
        .sidecar_model()
        .unwrap_or_else(|| "claude-haiku-4-5-20251001".into())
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
        .unwrap_or(screenshot_b64);

    let req = ChatRequest {
        messages: vec![Message {
            role: "user".to_string(),
            content: format!("Action performed: {}", action_context),
            images: Some(vec![ImageContent {
                media_type: "image/png".to_string(),
                data: data.to_string(),
            }]),
            ..Default::default()
        }],
        tools: vec![],
        max_tokens: 150,
        temperature: 0.0,
        system: SIDECAR_SYSTEM.to_string(),
        static_system: String::new(),
        model: sidecar_model(),
        enable_thinking: false,
        metadata: None,
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

    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}
