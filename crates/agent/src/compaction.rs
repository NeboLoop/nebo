use db::models::ChatMessage;

/// A failed tool execution preserved across compaction.
#[derive(Debug, Clone)]
pub struct ToolFailure {
    pub tool_call_id: String,
    pub tool_name: String,
    pub summary: String,
    pub meta: String,
}

/// Max failures included in compaction summary.
const MAX_TOOL_FAILURES: usize = 8;
/// Max characters per failure message.
const MAX_TOOL_FAILURE_CHARS: usize = 240;

/// Scan messages for tool errors and collect failures.
pub fn collect_tool_failures(messages: &[ChatMessage]) -> Vec<ToolFailure> {
    let mut failures = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    for msg in messages {
        if msg.role != "tool" {
            continue;
        }
        let tr_json = match &msg.tool_results {
            Some(tr) if !tr.is_empty() => tr,
            _ => continue,
        };

        let results: Vec<serde_json::Value> = match serde_json::from_str(tr_json) {
            Ok(r) => r,
            Err(_) => continue,
        };

        for result in &results {
            let is_error = result
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !is_error {
                continue;
            }

            let tool_call_id = result
                .get("tool_call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if tool_call_id.is_empty() || seen_ids.contains(&tool_call_id) {
                continue;
            }
            seen_ids.insert(tool_call_id.clone());

            let content = result
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let tool_name = extract_tool_name(messages, &tool_call_id);
            let summary = truncate_text(&normalize_text(&sanitize_for_summary(content)), MAX_TOOL_FAILURE_CHARS);
            let meta = extract_failure_meta(content);

            failures.push(ToolFailure {
                tool_call_id,
                tool_name,
                summary,
                meta,
            });
        }
    }

    failures
}

/// Format tool failures as a markdown section.
pub fn format_tool_failures_section(failures: &[ToolFailure]) -> String {
    if failures.is_empty() {
        return String::new();
    }

    let mut section = String::from("## Tool Failures\n");
    let show_count = std::cmp::min(failures.len(), MAX_TOOL_FAILURES);

    for failure in failures.iter().take(show_count) {
        let meta_suffix = if failure.meta.is_empty() {
            String::new()
        } else {
            format!(" ({})", failure.meta)
        };
        section.push_str(&format!(
            "- {}{}: {}\n",
            failure.tool_name, meta_suffix, failure.summary
        ));
    }

    if failures.len() > MAX_TOOL_FAILURES {
        section.push_str(&format!("- ...and {} more\n", failures.len() - MAX_TOOL_FAILURES));
    }

    section
}

/// Combine a base compaction summary with tool failure details.
pub fn enhanced_summary(messages: &[ChatMessage], base_summary: &str) -> String {
    let failures = collect_tool_failures(messages);
    let failures_section = format_tool_failures_section(&failures);

    if failures_section.is_empty() {
        return base_summary.to_string();
    }

    format!("{}\n\n{}", base_summary, failures_section)
}

/// Look up tool name from assistant messages by tool call ID.
fn extract_tool_name(messages: &[ChatMessage], tool_call_id: &str) -> String {
    for msg in messages {
        if msg.role != "assistant" {
            continue;
        }
        if let Some(ref tc_json) = msg.tool_calls {
            if let Ok(calls) = serde_json::from_str::<Vec<serde_json::Value>>(tc_json) {
                for call in &calls {
                    if call.get("id").and_then(|v| v.as_str()) == Some(tool_call_id) {
                        return call
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                    }
                }
            }
        }
    }
    "unknown".to_string()
}

/// Collapse whitespace to single spaces.
fn normalize_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<&str>>().join(" ")
}

/// Truncate with "..." suffix.
fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        text.to_string()
    } else {
        format!("{}...", &text[..max_chars.saturating_sub(3)])
    }
}

/// Extract failure metadata (exit codes, status patterns).
fn extract_failure_meta(content: &str) -> String {
    let lower = content.to_lowercase();
    let mut parts = Vec::new();

    // Exit code detection
    for pattern in ["exit code ", "exited with code "] {
        if let Some(idx) = lower.find(pattern) {
            let after = &content[idx + pattern.len()..];
            if let Some(num) = extract_number(after) {
                parts.push(format!("exitCode={}", num));
                break;
            }
        }
    }

    // Status patterns
    if lower.contains("command timed out") || lower.contains("timeout") {
        parts.push("status=timeout".to_string());
    } else if lower.contains("permission denied") {
        parts.push("status=permission_denied".to_string());
    } else if lower.contains("not found") || lower.contains("enoent") {
        parts.push("status=not_found".to_string());
    }

    parts.join(" ")
}

/// Extract first numeric substring.
fn extract_number(s: &str) -> Option<String> {
    let start = s.find(|c: char| c.is_ascii_digit())?;
    let end = s[start..]
        .find(|c: char| !c.is_ascii_digit())
        .map(|i| start + i)
        .unwrap_or(s.len());
    Some(s[start..end].to_string())
}

/// Strip non-printable characters.
fn sanitize_for_summary(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_graphic() || c.is_ascii_whitespace())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_failure_meta() {
        assert!(extract_failure_meta("exit code 1").contains("exitCode=1"));
        assert!(extract_failure_meta("command timed out").contains("status=timeout"));
        assert!(extract_failure_meta("permission denied").contains("status=permission_denied"));
        assert!(extract_failure_meta("all good").is_empty());
    }

    #[test]
    fn test_truncate_text() {
        assert_eq!(truncate_text("short", 10), "short");
        assert_eq!(truncate_text("a very long text here", 10), "a very ...");
    }
}
