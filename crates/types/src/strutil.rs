//! String utilities shared across crates. Tiny, no deps.

/// Find the largest valid char boundary `<= max_bytes`. Safe replacement for
/// `&s[..max_bytes]` when `s` may contain multi-byte UTF-8 (em-dash, emoji,
/// smart quotes). The std `Index<Range>` impl panics if the end isn't on a
/// char boundary — and that panic crashes whole agent runs.
///
/// Use this whenever you'd otherwise write `&s[..n]` on user-provided or
/// model-generated text.
pub fn floor_char_boundary(s: &str, max_bytes: usize) -> usize {
    if max_bytes >= s.len() {
        return s.len();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// Truncate `s` in place to at most `max_bytes`, on a char boundary.
/// Replacement for `String::truncate(n)` which panics on bad boundary.
pub fn safe_truncate(s: &mut String, max_bytes: usize) {
    if s.len() > max_bytes {
        let end = floor_char_boundary(s, max_bytes);
        s.truncate(end);
    }
}

/// Borrow a prefix of `s` up to at most `max_bytes`, on a char boundary.
pub fn safe_prefix(s: &str, max_bytes: usize) -> &str {
    &s[..floor_char_boundary(s, max_bytes)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_multibyte_at_boundary() {
        let s = "abcd—efgh"; // — is 3 bytes
        // bytes: 'a','b','c','d', e2,80,94, 'e','f','g','h'
        // requesting 6 falls inside the em-dash → should back off to 4
        assert_eq!(floor_char_boundary(s, 6), 4);
        assert_eq!(safe_prefix(s, 6), "abcd");
    }

    #[test]
    fn keeps_full_string_when_max_exceeds_len() {
        let s = "hello";
        assert_eq!(floor_char_boundary(s, 100), 5);
        assert_eq!(safe_prefix(s, 100), "hello");
    }

    #[test]
    fn safe_truncate_mutates_in_place() {
        let mut s = String::from("héllo world"); // é is 2 bytes (indices 1–2)
        safe_truncate(&mut s, 3); // byte 3 is a char boundary (start of 'l') → keep "hé"
        assert_eq!(s, "hé");
    }
}
