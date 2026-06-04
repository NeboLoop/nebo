use ai::{ChatRequest, ImageContent, Message, Provider, StreamEventType};
use tracing::debug;

const SIDECAR_SYSTEM: &str = "You analyze browser screenshots to help an AI agent make browsing decisions. \
Return a compact structured assessment in this exact format:\n\
PAGE: <type> (search-results | article | form | login | dashboard | error | landing | list | map | media | other)\n\
STATUS: <what happened> (1 short sentence)\n\
BLOCKER: <none | auth-required | captcha | paywall | cookie-banner | age-gate | geo-blocked | rate-limited>\n\
CONTENT: <available | partial | gated | empty | error> — is the main content visible or hidden behind a wall?\n\
ACTION: <continue | stop | try-different-source> — should the agent keep working on this page?\n\n\
Rules:\n\
- If you see a login form, sign-in prompt, or 'Join to see' overlay → BLOCKER: auth-required, CONTENT: gated\n\
- If content is visible but truncated with 'sign in to see more' → CONTENT: partial\n\
- If a CAPTCHA or challenge page appears → BLOCKER: captcha\n\
- Keep STATUS under 15 words.\n\
After the structured fields, list up to 5 KEY ELEMENTS with approximate positions:\n\
ELEMENTS:\n\
- <description> @ (<x>,<y>)\n\
Only list elements the agent would need to interact with (search boxes, submit buttons, login forms, main navigation). \
Coordinates are approximate center points relative to viewport. No other text.";

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
