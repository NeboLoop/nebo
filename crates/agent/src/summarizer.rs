use std::sync::Arc;

use ai::{ChatRequest, Message, Provider, StreamEventType};
use tokio::sync::RwLock;
use tools::ToolResult;
use tracing::{debug, warn};

use crate::runner::truncate_str;

/// Total prompt cap to keep the summarizer call cheap.
const PROMPT_CAP: usize = 2_000;
/// Max chars of each tool input/output included in the prompt.
const IO_TRUNCATE: usize = 300;
/// Max chars of assistant intent context.
const INTENT_TRUNCATE: usize = 200;

const SYSTEM_PROMPT: &str = "\
Generate a single concise sentence (past tense, max 80 chars) describing what \
these tool operations accomplished. Example: 'Read auth config and fixed token \
validation'. Do not include technical details.";

/// Generate a one-line summary of a tool batch for UI display.
/// Uses a cheap model (Haiku/flash) with truncated inputs/outputs.
/// Non-critical: errors are logged and swallowed.
pub async fn summarize_tool_batch(
    providers: &[Arc<dyn Provider>],
    tool_calls: &[ai::ToolCall],
    tool_results: &[ToolResult],
    last_assistant_text: &str,
) -> Option<String> {
    if tool_calls.is_empty() {
        return None;
    }

    let provider = pick_cheapest(providers)?;

    // Build user prompt content
    let mut prompt = String::with_capacity(PROMPT_CAP);

    // Intent context from last assistant message
    if !last_assistant_text.is_empty() {
        let intent = truncate_str(last_assistant_text, INTENT_TRUNCATE);
        prompt.push_str("Intent: ");
        prompt.push_str(intent);
        prompt.push('\n');
    }

    // Tool call/result pairs
    for (i, tc) in tool_calls.iter().enumerate() {
        if prompt.len() >= PROMPT_CAP {
            break;
        }
        let input_str = tc.input.to_string();
        let input_trunc = truncate_str(&input_str, IO_TRUNCATE);

        let output_trunc = tool_results
            .get(i)
            .map(|r| truncate_str(&r.content, IO_TRUNCATE))
            .unwrap_or("(no result)");

        let entry = format!(
            "- {} | in: {} | out: {}\n",
            tc.name, input_trunc, output_trunc
        );
        let remaining = PROMPT_CAP.saturating_sub(prompt.len());
        if entry.len() > remaining {
            prompt.push_str(truncate_str(&entry, remaining));
            break;
        }
        prompt.push_str(&entry);
    }

    let req = ChatRequest {
        messages: vec![Message {
            role: "user".to_string(),
            content: prompt,
            ..Default::default()
        }],
        tools: vec![],
        max_tokens: 100,
        temperature: 0.0,
        system: SYSTEM_PROMPT.to_string(),
        static_system: String::new(),
        model: String::new(),
        enable_thinking: false,
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: None,
    };

    match provider.stream(&req).await {
        Ok(mut rx) => {
            let mut response = String::new();
            while let Some(event) = rx.recv().await {
                match event.event_type {
                    StreamEventType::Text => response.push_str(&event.text),
                    StreamEventType::Error => {
                        warn!(error = ?event.error, "tool summary generation error");
                        return None;
                    }
                    StreamEventType::Done => break,
                    _ => {}
                }
            }
            let summary = response.trim().to_string();
            if summary.is_empty() {
                None
            } else {
                debug!(summary = %summary, "tool batch summary generated");
                Some(summary)
            }
        }
        Err(e) => {
            warn!(error = %e, "tool summary provider call failed");
            None
        }
    }
}

/// Generate a 3-7 word title for a conversation based on the user's prompt.
///
/// Uses a cheap model with a minimal prompt. Non-critical: returns `None` on
/// any failure so callers can silently skip.
pub async fn generate_session_title(
    providers: &Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    user_prompt: &str,
    model: &str,
) -> Option<String> {
    let provider = {
        let lock = providers.read().await;
        crate::runner::prefer_non_gateway(&lock)
    }?;

    let system = "Generate a 3-7 word title for this conversation. \
                  Output ONLY the title, no quotes, no punctuation at the end.";
    let truncated = truncate_str(user_prompt, 300);

    let request = ChatRequest {
        model: model.to_string(),
        system: system.to_string(),
        static_system: String::new(),
        messages: vec![Message {
            role: "user".to_string(),
            content: truncated.to_string(),
            ..Default::default()
        }],
        max_tokens: 30,
        temperature: 0.3,
        tools: vec![],
        enable_thinking: false,
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: None,
    };

    let mut rx = match provider.stream(&request).await {
        Ok(rx) => rx,
        Err(e) => {
            debug!(error = %e, "session title generation failed");
            return None;
        }
    };

    let mut response = String::new();
    while let Some(event) = rx.recv().await {
        match event.event_type {
            StreamEventType::Text => response.push_str(&event.text),
            StreamEventType::Done | StreamEventType::Error => break,
            _ => {}
        }
    }

    let title = response.trim().to_string();
    if title.is_empty() || title.len() > 100 {
        None
    } else {
        debug!(title = %title, "session title generated");
        Some(title)
    }
}

/// Pick the cheapest available provider. Prefer non-gateway (non-Janus) providers,
/// then fall back to whatever is available.
fn pick_cheapest(providers: &[Arc<dyn Provider>]) -> Option<Arc<dyn Provider>> {
    // Prefer non-gateway providers (local/direct API are cheapest)
    providers
        .iter()
        .find(|p| p.id() != "janus")
        .cloned()
        .or_else(|| providers.first().cloned())
}
