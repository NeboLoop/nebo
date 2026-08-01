//! Shared content parsing for the import walkers — memory-file splitting,
//! titles, and timestamp coercion. Both walkers use these so the same file
//! shape imports identically regardless of which system it came from.

/// Split a memory markdown file into discrete entries. Hermes delimits with
/// the section sign (`§`); plain files fall back to blank-line paragraphs.
/// Markdown headings are structure, not memories, and are dropped.
pub(super) fn memory_text_entries(text: &str) -> Vec<String> {
    let raw: Vec<&str> = if text.contains('§') {
        text.split('§').collect()
    } else {
        text.split("\n\n").collect()
    };
    raw.into_iter()
        .map(|chunk| {
            chunk
                .lines()
                .filter(|l| !l.trim_start().starts_with('#'))
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string()
        })
        .filter(|e| !e.is_empty())
        .collect()
}

/// First line of a message, truncated — imported chats' titles.
pub(super) fn truncate_title(content: &str) -> String {
    let line = content.lines().next().unwrap_or("").trim();
    let mut title: String = line.chars().take(60).collect();
    if line.chars().count() > 60 {
        title.push('…');
    }
    title
}

/// Coerce a JSON value that has shipped as int, float, or numeric string into
/// an epoch second. Foreign timestamps are best-effort; `None` means "unknown".
pub(super) fn value_to_epoch(v: &serde_json::Value) -> Option<i64> {
    if let Some(n) = v.as_i64() {
        return Some(n);
    }
    if let Some(f) = v.as_f64() {
        return Some(f as i64);
    }
    if let Some(s) = v.as_str() {
        if let Ok(f) = s.parse::<f64>() {
            return Some(f as i64);
        }
    }
    None
}
