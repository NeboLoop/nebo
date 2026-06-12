//! Question-driven answer options.
//!
//! When a chat turn ends with the assistant asking the user a question,
//! generates 2-4 tappable answer options. Turns that don't ask anything get
//! NO chips — suggestions on every turn are noise users learn to ignore.

use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::debug;

use ai::{ChatRequest, Message, Provider, StreamEventType};

use crate::runner::prefer_non_gateway;
use types::strutil::floor_char_boundary;

/// Generate 2-4 answer options when the assistant's reply asks a question.
///
/// Uses the cheapest available provider with a minimal prompt.
/// Returns `None` on any failure (non-fatal) and for replies that don't
/// ask the user anything — ordinary turns get no chips.
pub async fn generate_suggestions(
    providers: &Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    last_exchange: &[db::models::ChatMessage],
    model: &str,
) -> Option<Vec<String>> {
    if last_exchange.len() < 2 {
        return None;
    }

    // Suggestions are ANSWER OPTIONS for a question the assistant actually
    // asked — never generic "what next" filler (product decision: chips on
    // every turn train users to ignore them). Only generate when the reply's
    // closing poses a question; otherwise stay silent.
    let last_assistant = last_exchange.iter().rev().find(|m| m.role == "assistant")?;
    let content = &last_assistant.content;
    let tail_start = floor_char_boundary(content, content.len().saturating_sub(400));
    if !content[tail_start..].contains('?') {
        return None;
    }

    let provider = {
        let lock = providers.read().await;
        prefer_non_gateway(&lock)
    }?;

    let system = "The assistant's reply ends by asking the user a question. \
Generate 2-4 tappable ANSWER OPTIONS for that question. \
Output ONLY a JSON array of strings.\n\
RULES:\n\
- Each option is a direct answer to the question the assistant asked\n\
- Options must be meaningfully different directions, not rephrasings\n\
- 1-8 words each, max 50 characters\n\
- Only offer options the assistant's reply explicitly raised or clearly implied\n\
- If the question is open-ended with no enumerable answers, output []\n\
- If the assistant wasn't really asking the user to decide anything, output []\n\
- NEVER use questions, multiple sentences, or explanations\n\
Good: [\"3% pool\", \"5% pool\", \"Start drafting the articles\"]\n\
Bad: [\"Tell me more\", \"Sounds good\", \"What are the alternatives?\"]";

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
                    // Single-word answers ("Yes", "5%") are valid option chips.
                    let words = s.split_whitespace().count();
                    if words < 1 || words > 10 {
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
