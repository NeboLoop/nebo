use std::fs;
use std::path::PathBuf;

use ai::{ChatRequest, Message, Provider, StreamEventType};
use tracing::{debug, info};

/// Threshold in characters above which input is considered "large" and gets
/// offloaded to a temp file with an LLM-generated summary replacing the inline
/// content.  ~6 000 tokens at the chars/4 heuristic — keeps a single user
/// message well under 20 % of the 40 000-token sliding window.
const LARGE_INPUT_THRESHOLD_CHARS: usize = 24_000;

/// Max tokens to request from the summarization LLM call.
const SUMMARY_MAX_TOKENS: i32 = 1500;

/// Max characters of the original content to feed to the summarization model.
/// Anything beyond this is still saved to the file but not summarised directly.
const SUMMARIZE_CONTENT_CAP: usize = 100_000;

/// Subdirectory under `<data_dir>/files/` where large inputs are persisted.
const STORAGE_SUBDIR: &str = "large_inputs";

/// Rough chars-per-token conversion factor (matches pruning.rs / session.rs).
const CHARS_PER_TOKEN: usize = 4;

// ── Public result type ───────────────────────────────────────────────

/// The replacement content and metadata produced by the large-input pipeline.
pub struct LargeInputResult {
    /// What gets stored in `chat_messages.content` (summary + file reference).
    pub content: String,
    /// JSON string for `chat_messages.metadata`.
    pub metadata_json: String,
    /// Absolute path to the saved file.
    pub file_path: String,
}

// ── Detection ────────────────────────────────────────────────────────

/// Returns `true` when the prompt exceeds the large-input threshold.
pub fn is_large(prompt: &str) -> bool {
    prompt.len() > LARGE_INPUT_THRESHOLD_CHARS
}

// ── Content-type heuristics ──────────────────────────────────────────

/// Best-effort content-type detection for prompt-tuning the summary.
pub fn detect_content_type(content: &str) -> &'static str {
    // Sample the first 4 000 chars to keep the heuristic cheap.
    let sample: &str = if content.len() > 4_000 {
        &content[..4_000]
    } else {
        content
    };
    let lower = sample.to_lowercase();

    // Email chain indicators
    let email_markers = ["from:", "subject:", "sent:", "to:", "re:", "cc:", "date:"];
    let email_hits = email_markers.iter().filter(|m| lower.contains(*m)).count();
    if email_hits >= 3 {
        return "email";
    }

    // Legal document indicators
    let legal_markers = [
        "whereas", "herein", "hereinafter", "party", "agreement",
        "shall", "pursuant", "indemnif", "liability", "governing law",
    ];
    let legal_hits = legal_markers.iter().filter(|m| lower.contains(*m)).count();
    if legal_hits >= 3 {
        return "legal";
    }

    // Code indicators — brace/semicolon density + common patterns
    let brace_count = sample.chars().filter(|c| *c == '{' || *c == '}').count();
    let semicolons = sample.chars().filter(|c| *c == ';').count();
    let has_code_keywords = lower.contains("fn ") || lower.contains("func ")
        || lower.contains("function ") || lower.contains("class ")
        || lower.contains("import ") || lower.contains("def ")
        || lower.contains("#include");
    if (brace_count > 10 && semicolons > 5) || has_code_keywords {
        return "code";
    }

    "prose"
}

// ── File storage ─────────────────────────────────────────────────────

/// Save the full content to `<data_dir>/files/large_inputs/<msg_id>.txt`.
///
/// Creates the directory tree if it doesn't exist.  Returns the absolute
/// path on success.
pub fn save_to_file(content: &str, msg_id: &str) -> Result<PathBuf, String> {
    let data_dir = config::data_dir().map_err(|e| format!("data_dir: {e}"))?;
    let dir = data_dir.join("files").join(STORAGE_SUBDIR);
    fs::create_dir_all(&dir).map_err(|e| format!("create dir {}: {e}", dir.display()))?;

    let path = dir.join(format!("{msg_id}.txt"));
    fs::write(&path, content).map_err(|e| format!("write {}: {e}", path.display()))?;

    info!(path = %path.display(), bytes = content.len(), "large input saved to file");
    Ok(path)
}

// ── Summarisation (isolated LLM call) ────────────────────────────────

/// Summarise the content using an isolated, one-shot provider call.
///
/// Follows the **sidecar.rs** pattern: standalone `ChatRequest`, no session,
/// no DB writes, cheapest available model.  The full document content **never**
/// enters the main chat context.
pub async fn summarize(
    provider: &dyn Provider,
    content: &str,
    content_type: &str,
    model: &str,
) -> Result<String, String> {
    let system = match content_type {
        "email" => "Summarize this email chain concisely. List: participants, key requests, decisions made, action items, and any deadlines.",
        "code" => "Summarize this code concisely. List: what it does, key functions/classes, dependencies, and any issues.",
        "legal" => "Summarize this legal document concisely. List: parties involved, key terms, obligations, dates, and important clauses.",
        _ => "Summarize this document concisely. Capture: main topic, key points, any requests or action items, and important details the reader needs to know.",
    };

    // Cap the content fed to the model — the rest is accessible via file paging.
    let truncated = if content.len() > SUMMARIZE_CONTENT_CAP {
        &content[..SUMMARIZE_CONTENT_CAP]
    } else {
        content
    };

    let req = ChatRequest {
        messages: vec![Message {
            role: "user".to_string(),
            content: truncated.to_string(),
            ..Default::default()
        }],
        tools: vec![],
        max_tokens: SUMMARY_MAX_TOKENS,
        temperature: 0.0,
        system: system.to_string(),
        static_system: String::new(),
        model: model.to_string(),
        enable_thinking: false,
        metadata: None,
        cache_breakpoints: vec![],
    };

    let mut rx = provider.stream(&req).await.map_err(|e| format!("summarize stream: {e}"))?;

    let mut text = String::new();
    while let Some(event) = rx.recv().await {
        match event.event_type {
            StreamEventType::Text => text.push_str(&event.text),
            StreamEventType::Done | StreamEventType::Error => break,
            _ => {}
        }
    }

    if text.is_empty() {
        Err("summarize: empty response from provider".into())
    } else {
        debug!(summary_len = text.len(), "large input summarised");
        Ok(text)
    }
}

// ── Fallback (no LLM) ───────────────────────────────────────────────

/// Produce a best-effort summary when no provider is available or the LLM
/// call fails.  Returns document statistics plus a short preview.
pub fn fallback_summary(content: &str) -> String {
    let line_count = content.lines().count();
    let word_count = content.split_whitespace().count();
    let preview_end = content
        .char_indices()
        .nth(500)
        .map(|(i, _)| i)
        .unwrap_or(content.len());
    let preview = &content[..preview_end];
    let ellipsis = if preview_end < content.len() { "..." } else { "" };

    format!(
        "Document statistics: {} characters, ~{} words, {} lines.\n\nPreview:\n{}{}",
        content.len(),
        word_count,
        line_count,
        preview,
        ellipsis,
    )
}

// ── Build replacement ────────────────────────────────────────────────

/// Construct the replacement message content and metadata that will be stored
/// in the DB (and therefore seen by the agent) instead of the raw input.
pub fn build_replacement(
    original: &str,
    summary: &str,
    file_path: &str,
    content_type: &str,
) -> LargeInputResult {
    let original_chars = original.len();
    let original_tokens_est = original_chars / CHARS_PER_TOKEN;
    let summary_tokens_est = summary.len() / CHARS_PER_TOKEN;

    let content = format!(
        "[This message contained a large {content_type} document ({original_chars} characters, \
         ~{original_tokens_est} tokens). The full content has been saved to {file_path} and can \
         be read with system(resource: \"file\", action: \"read\", path: \"{file_path}\"). \
         Here is a summary:]\n\n{summary}",
    );

    let metadata = serde_json::json!({
        "large_input": {
            "original_chars": original_chars,
            "original_tokens_est": original_tokens_est,
            "file_path": file_path,
            "summary_tokens_est": summary_tokens_est,
            "content_type": content_type,
        }
    });

    LargeInputResult {
        content,
        metadata_json: metadata.to_string(),
        file_path: file_path.to_string(),
    }
}

