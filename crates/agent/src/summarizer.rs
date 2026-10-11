use std::sync::Arc;

use ai::{ChatRequest, Message, Provider, StreamEventType};
use config::ModelsConfig;
use tokio::sync::RwLock;
use tools::ToolResult;
use tracing::{debug, warn};

use tools::truncate_str;

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
    trace: ai::RequestTrace,
    providers: &[Arc<dyn Provider>],
    tool_calls: &[ai::ToolCall],
    tool_results: &[ToolResult],
    last_assistant_text: &str,
) -> Option<String> {
    if tool_calls.is_empty() {
        return None;
    }

    let (provider, aux_model) = match crate::harness::model_call::resolve_aux(&ModelsConfig::load(), providers) {
        Some(routed) => routed,
        None => (crate::harness::model_call::prefer_non_gateway(providers)?, String::new()),
    };

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
        tool_credential: None,
        chat_id: String::new(),
        ask_channels: None,
        permission_mode: None,
        linked_context: None,
        tool_choice: Default::default(),
        messages: vec![Message {
            role: "user".to_string(),
            content: prompt,
            ..Default::default()
        }],
        tools: vec![],
        max_tokens: 100,
        temperature: 0.0,
        system: SYSTEM_PROMPT.to_string(),
        model: aux_model,
        enable_thinking: false,
        effort: None,
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: None,
        trace,
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
    trace: ai::RequestTrace,
    providers: &Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    user_prompt: &str,
    model: &str,
) -> Option<String> {
    let line = one_line(
        trace,
        providers,
        model,
        "Generate a 3-7 word title for this conversation. \
         Output ONLY the title as plain text: no markdown, no quotes, \
         no punctuation at the end.",
        truncate_str(user_prompt, 300),
        30,
    )
    .await?;
    plain_title(&line)
}

/// A title as plain words. The model sometimes answers in markdown
/// (`**Title**`, often cut before the closing marks by the token cap), and a
/// title is shown as one plain line everywhere, so the marks come off.
/// `None` when nothing but marks was written.
fn plain_title(line: &str) -> Option<String> {
    let mut s = line.trim();
    // A heading, quote or list mark at the start.
    s = s.trim_start_matches('#').trim_start();
    s = s.strip_prefix("> ").unwrap_or(s);
    s = s.strip_prefix("- ").unwrap_or(s);
    let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for (i, &c) in chars.iter().enumerate() {
        let keep = match c {
            '*' | '`' => false,
            '~' => chars.get(i + 1) != Some(&'~') && (i == 0 || chars[i - 1] != '~'),
            // Inside a word (snake_case) an underscore is a letter.
            '_' => word(i.checked_sub(1).map(|p| chars[p])) && word(chars.get(i + 1).copied()),
            _ => true,
        };
        if keep {
            out.push(c);
        }
    }
    let title = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = title.trim_matches(|c| c == '"' || c == '\u{201c}' || c == '\u{201d}').trim();
    (!title.is_empty()).then(|| title.to_string())
}

/// One short line written by the cheap model from `text` under
/// `instruction`: a chat title, a working objective. Non-critical: `None` on
/// any failure, an empty answer, or one over 200 chars.
pub async fn one_line(
    trace: ai::RequestTrace,
    providers: &Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    model: &str,
    instruction: &str,
    text: &str,
    max_tokens: i32,
) -> Option<String> {
    let (provider, model) = {
        let lock = providers.read().await;
        match crate::harness::model_call::resolve_aux(&ModelsConfig::load(), &lock) {
            Some(routed) => routed,
            None => (crate::harness::model_call::prefer_non_gateway(&lock)?, model.to_string()),
        }
    };

    let request = ChatRequest {
        tool_credential: None,
        chat_id: String::new(),
        ask_channels: None,
        permission_mode: None,
        linked_context: None,
        tool_choice: Default::default(),
        model,
        system: instruction.to_string(),
        messages: vec![Message {
            role: "user".to_string(),
            content: text.to_string(),
            ..Default::default()
        }],
        max_tokens,
        temperature: 0.3,
        tools: vec![],
        enable_thinking: false,
        effort: None,
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: None,
        trace,
    };

    let mut rx = match provider.stream(&request).await {
        Ok(rx) => rx,
        Err(e) => {
            debug!(error = %e, "one-line generation failed");
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

    let line = response.trim().to_string();
    if line.is_empty() || line.len() > 200 {
        None
    } else {
        debug!(line = %line, "one line generated");
        Some(line)
    }
}


#[cfg(test)]
mod tests {
    use super::plain_title;

    #[test]
    fn a_title_comes_out_as_plain_words() {
        // Live: bold, cut before its closing stars by the token cap.
        assert_eq!(plain_title("**Xi Jinping's Name in Chinese Characters").as_deref(), Some("Xi Jinping's Name in Chinese Characters"));
        assert_eq!(plain_title("**Quarterly Sales Review**").as_deref(), Some("Quarterly Sales Review"));
        assert_eq!(plain_title("# Trip Planning for Kyoto").as_deref(), Some("Trip Planning for Kyoto"));
        assert_eq!(plain_title("_Draft_ the `README` update").as_deref(), Some("Draft the README update"));
        assert_eq!(plain_title("~~Old~~ New Budget").as_deref(), Some("Old New Budget"));
        assert_eq!(plain_title("\"Weekly Standup Notes\"").as_deref(), Some("Weekly Standup Notes"));
    }

    #[test]
    fn ordinary_titles_stay_as_written() {
        assert_eq!(plain_title("Fix snake_case parser").as_deref(), Some("Fix snake_case parser"));
        assert_eq!(plain_title("Q3 plan: 10% growth (draft)").as_deref(), Some("Q3 plan: 10% growth (draft)"));
        assert_eq!(plain_title("Approx ~5 hours of work").as_deref(), Some("Approx ~5 hours of work"));
        assert_eq!(plain_title("Reunión con el equipo").as_deref(), Some("Reunión con el equipo"));
        assert_eq!(plain_title("****"), None);
    }
}
