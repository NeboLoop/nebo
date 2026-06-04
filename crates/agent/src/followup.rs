//! Context-aware follow-up suggestion generation.
//!
//! After a chat turn completes, generates 2-3 short follow-up suggestions
//! that the user can click to quickly continue the conversation.

use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::debug;

use ai::{ChatRequest, Message, Provider, StreamEventType};

use crate::runner::prefer_non_gateway;
use types::strutil::floor_char_boundary;

/// Generate 2-3 follow-up suggestions based on the last exchange.
///
/// Uses the cheapest available provider with a minimal prompt.
/// Returns `None` on any failure (non-fatal).
///
/// Skips generation for short conversational replies where follow-up
/// chips would be noise rather than helpful.
pub async fn generate_suggestions(
    providers: &Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    last_exchange: &[db::models::ChatMessage],
    model: &str,
) -> Option<Vec<String>> {
    if last_exchange.len() < 2 {
        return None;
    }

    // Only generate suggestions when the assistant produced substantive output.
    // Short conversational replies (questions, acknowledgments) don't benefit
    // from follow-up chips — they're noise.
    let last_assistant = last_exchange.iter().rev().find(|m| m.role == "assistant")?;
    if last_assistant.content.len() < 200 {
        return None;
    }

    let provider = {
        let lock = providers.read().await;
        prefer_non_gateway(&lock)
    }?;

    let system = "Generate 2-3 follow-up suggestions the USER would naturally type next. \
Output ONLY a JSON array of strings.\n\
RULES:\n\
- 2-8 words each, max 50 characters\n\
- Predict what the user would type, not what the AI would say\n\
- Be specific and actionable: \"Show me the calendar\" beats \"Tell me more\"\n\
- NEVER use questions, multiple sentences, or explanations\n\
- NEVER start with \"Tell me\", \"Can you\", \"What about\", \"How do I\"\n\
- Stay silent (empty array) if the next step isn't obvious\n\
Good: [\"Run the tests\", \"Send it now\", \"Show me alternatives\"]\n\
Bad: [\"Can you tell me more about how this works?\", \"What are the alternatives?\"]";

    // Build context from last few messages, truncated.
    // Use more messages (up to 6) and larger truncation to give the model
    // enough context to generate relevant suggestions.
    let context_msgs: Vec<Message> = last_exchange
        .iter()
        .rev()
        .take(6)
        .rev()
        .map(|m| {
            let content = if m.content.len() > 1000 {
                // Truncate at a char boundary — naive byte-slicing panics when
                // byte 1000 falls inside a multi-byte UTF-8 char (em-dash, emoji,
                // smart quote, etc.). This was crashing whole runs.
                let end = floor_char_boundary(&m.content, 1000);
                format!("{}...", &m.content[..end])
            } else {
                m.content.clone()
            };
            Message {
                role: m.role.clone(),
                content,
                ..Default::default()
            }
        })
        .collect();

    let request = ChatRequest {
        tool_choice: Default::default(),
        model: model.to_string(),
        system: system.to_string(),
        static_system: String::new(),
        messages: context_msgs,
        max_tokens: 200,
        temperature: 0.7,
        tools: vec![],
        enable_thinking: false,
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: None,
    };

    let mut rx = match provider.stream(&request).await {
        Ok(rx) => rx,
        Err(e) => {
            debug!(error = %e, "followup suggestion generation failed");
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

    // Parse JSON array from response
    let response = response.trim();
    // Try to extract JSON array even if wrapped in markdown code fences
    let json_str = if response.starts_with("```") {
        response
            .lines()
            .skip(1)
            .take_while(|l| !l.starts_with("```"))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        response.to_string()
    };

    match serde_json::from_str::<Vec<String>>(&json_str) {
        Ok(suggestions) => {
            let filtered: Vec<String> = suggestions
                .into_iter()
                .filter(|s| {
                    if s.is_empty() || s.len() > 50 {
                        return false;
                    }
                    let words = s.split_whitespace().count();
                    if words < 2 || words > 10 {
                        return false;
                    }
                    // Reject multi-sentence, questions, or AI-voice
                    if s.contains('?') || s.contains('\n') {
                        return false;
                    }
                    let lower = s.to_lowercase();
                    if lower.starts_with("tell me")
                        || lower.starts_with("can you")
                        || lower.starts_with("what ")
                        || lower.starts_with("how ")
                        || lower.starts_with("let me")
                        || lower.starts_with("i'll ")
                    {
                        return false;
                    }
                    true
                })
                .take(3)
                .collect();
            if filtered.is_empty() {
                None
            } else {
                Some(filtered)
            }
        }
        Err(e) => {
            debug!(error = %e, raw = %response, "failed to parse followup suggestions");
            None
        }
    }
}
