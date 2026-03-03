/// SSE (Server-Sent Events) line parser.
///
/// Handles the standard SSE format used by OpenAI and Anthropic APIs:
/// - Lines starting with "data: " contain event data
/// - "data: [DONE]" signals end of stream
/// - Empty lines separate events
/// - Lines starting with "event: " carry event type (used by Anthropic)

/// Parsed SSE event.
#[derive(Debug)]
pub enum SseEvent {
    /// A data line with the JSON payload.
    Data(String),
    /// The [DONE] sentinel — stream is complete.
    Done,
    /// An event type line (e.g., "message_start", "content_block_delta").
    Event(String),
    /// Empty line or non-data line — skip.
    Skip,
}

/// Parse a single SSE line.
pub fn parse_sse_line(line: &str) -> SseEvent {
    let trimmed = line.trim();

    if trimmed.is_empty() {
        return SseEvent::Skip;
    }

    if let Some(data) = trimmed.strip_prefix("data: ") {
        if data == "[DONE]" {
            return SseEvent::Done;
        }
        return SseEvent::Data(data.to_string());
    }

    if let Some(event_type) = trimmed.strip_prefix("event: ") {
        return SseEvent::Event(event_type.to_string());
    }

    SseEvent::Skip
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_data_line() {
        match parse_sse_line("data: {\"id\":\"123\"}") {
            SseEvent::Data(d) => assert_eq!(d, "{\"id\":\"123\"}"),
            _ => panic!("expected Data"),
        }
    }

    #[test]
    fn test_parse_done() {
        assert!(matches!(parse_sse_line("data: [DONE]"), SseEvent::Done));
    }

    #[test]
    fn test_parse_event_type() {
        match parse_sse_line("event: message_start") {
            SseEvent::Event(e) => assert_eq!(e, "message_start"),
            _ => panic!("expected Event"),
        }
    }

    #[test]
    fn test_parse_empty() {
        assert!(matches!(parse_sse_line(""), SseEvent::Skip));
        assert!(matches!(parse_sse_line("  "), SseEvent::Skip));
    }

    #[test]
    fn test_parse_comment() {
        assert!(matches!(parse_sse_line(": comment"), SseEvent::Skip));
    }
}
