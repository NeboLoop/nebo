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

/// Coerce a JSON value that has shipped as int, float, numeric string, or ISO
/// datetime into an epoch SECOND. Foreign timestamps are best-effort; `None`
/// means "unknown" (import time is used).
pub(super) fn value_to_epoch(v: &serde_json::Value) -> Option<i64> {
    if let Some(n) = v.as_i64() {
        return Some(normalize_epoch(n));
    }
    if let Some(f) = v.as_f64() {
        return Some(normalize_epoch(f as i64));
    }
    if let Some(s) = v.as_str() {
        return epoch_from_str(s);
    }
    None
}

/// Numeric-or-ISO string → epoch second.
pub(super) fn epoch_from_str(s: &str) -> Option<i64> {
    if let Ok(f) = s.parse::<f64>() {
        return Some(normalize_epoch(f as i64));
    }
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp())
}

/// Normalize an epoch that may be in seconds, milliseconds, microseconds, or
/// nanoseconds down to seconds. OpenClaw is Node (`Date.now()` = ms), Hermes
/// has shipped floats — taking the raw number verbatim turns real installs'
/// history into year-55,000 dates. Magnitude bands are unambiguous for any
/// date between 1973 and ~5100.
pub(super) fn normalize_epoch(n: i64) -> i64 {
    match n.unsigned_abs() {
        0..=99_999_999_999 => n,                          // seconds
        100_000_000_000..=99_999_999_999_999 => n / 1_000, // milliseconds
        100_000_000_000_000..=99_999_999_999_999_999 => n / 1_000_000, // microseconds
        _ => n / 1_000_000_000,                            // nanoseconds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epochs_normalize_across_units() {
        let secs = 1_700_000_000_i64;
        assert_eq!(normalize_epoch(secs), secs);
        assert_eq!(normalize_epoch(secs * 1_000), secs); // ms (Date.now())
        assert_eq!(normalize_epoch(secs * 1_000_000), secs); // µs
        assert_eq!(normalize_epoch(secs * 1_000_000_000), secs); // ns
    }

    #[test]
    fn iso_and_numeric_strings_parse() {
        assert_eq!(epoch_from_str("1700000000"), Some(1_700_000_000));
        assert_eq!(epoch_from_str("1700000000000"), Some(1_700_000_000));
        assert_eq!(
            epoch_from_str("2023-11-14T22:13:20Z"),
            Some(1_700_000_000)
        );
        assert_eq!(epoch_from_str("not a date"), None);
    }
}
