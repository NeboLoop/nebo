use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use sha2::{Sha256, Digest};

/// Time-based deduplication cache with size limits.
pub struct DedupeCache {
    entries: Mutex<HashMap<String, Instant>>,
    ttl: std::time::Duration,
    max_size: usize,
}

impl Default for DedupeCache {
    fn default() -> Self {
        Self::new(std::time::Duration::from_secs(20 * 60), 5000)
    }
}

impl DedupeCache {
    /// Create a new deduplication cache.
    pub fn new(ttl: std::time::Duration, max_size: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
            max_size,
        }
    }

    /// Returns true if the key is a duplicate (within TTL), false if new.
    /// Also updates the timestamp (touch).
    pub fn check(&self, fingerprint: &str) -> bool {
        if fingerprint.is_empty() {
            return false;
        }

        let mut entries = self.entries.lock().unwrap();
        let now = Instant::now();

        if let Some(ts) = entries.get(fingerprint) {
            if now.duration_since(*ts) < self.ttl {
                // Duplicate — touch
                entries.insert(fingerprint.to_string(), now);
                return true;
            }
        }

        // New entry
        entries.insert(fingerprint.to_string(), now);
        self.prune_locked(&mut entries, now);
        false
    }

    /// Remove expired entries and enforce max size.
    fn prune_locked(&self, entries: &mut HashMap<String, Instant>, now: Instant) {
        // Remove expired
        entries.retain(|_, ts| now.duration_since(*ts) < self.ttl);

        // Enforce max size (LRU: remove oldest)
        if entries.len() > self.max_size {
            let mut sorted: Vec<(String, Instant)> = entries.iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect();
            sorted.sort_by_key(|(_, ts)| *ts);

            let to_remove = entries.len() - self.max_size;
            for (key, _) in sorted.into_iter().take(to_remove) {
                entries.remove(&key);
            }
        }
    }

    /// Clear all entries.
    pub fn clear(&self) {
        self.entries.lock().unwrap().clear();
    }

    /// Current number of entries.
    pub fn size(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
}

/// Create a SHA256 hash of the given text.
pub fn hash_text(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

/// Create a deterministic fingerprint from an API error payload.
pub fn fingerprint_error(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }

    if let Some(payload) = parse_error_payload(raw) {
        let stable = stable_stringify(&payload);
        return hash_text(&stable);
    }

    String::new()
}

/// Structured API error info.
#[derive(Debug, Clone, Default)]
pub struct ApiErrorInfo {
    pub http_code: String,
    pub error_type: String,
    pub message: String,
    pub request_id: String,
}

/// Extract structured error info from raw error text.
pub fn parse_error_info(raw: &str) -> Option<ApiErrorInfo> {
    if raw.is_empty() {
        return None;
    }

    let mut info = ApiErrorInfo::default();
    let lower = raw.to_lowercase();

    // Extract HTTP status code
    let http_codes = ["401", "402", "403", "404", "429", "500", "502", "503"];
    for code in &http_codes {
        if raw.contains(code) {
            info.http_code = code.to_string();
            break;
        }
    }

    // Try to parse JSON payload
    if let Some(payload) = parse_error_payload(raw) {
        // Extract request_id
        if let Some(req_id) = payload.get("request_id").and_then(|v| v.as_str()) {
            info.request_id = req_id.to_string();
        }
        if info.request_id.is_empty() {
            if let Some(req_id) = payload.get("requestId").and_then(|v| v.as_str()) {
                info.request_id = req_id.to_string();
            }
        }

        // Nested error object
        if let Some(err_obj) = payload.get("error").and_then(|v| v.as_object()) {
            if let Some(msg) = err_obj.get("message").and_then(|v| v.as_str()) {
                info.message = msg.to_string();
            }
            if let Some(t) = err_obj.get("type").and_then(|v| v.as_str()) {
                info.error_type = t.to_string();
            }
        }

        // Outer type fallback (skip "error" marker)
        if info.error_type.is_empty() {
            if let Some(t) = payload.get("type").and_then(|v| v.as_str()) {
                if t != "error" {
                    info.error_type = t.to_string();
                }
            }
        }
        if info.message.is_empty() {
            if let Some(msg) = payload.get("message").and_then(|v| v.as_str()) {
                info.message = msg.to_string();
            }
        }
    }

    // Pattern-based type extraction
    if info.error_type.is_empty() {
        let patterns = [
            ("rate_limit", "rate_limit_error"),
            ("authentication", "authentication_error"),
            ("invalid_api_key", "authentication_error"),
            ("insufficient_quota", "billing_error"),
            ("billing", "billing_error"),
            ("overloaded", "overloaded_error"),
        ];
        for (pattern, err_type) in &patterns {
            if lower.contains(pattern) {
                info.error_type = err_type.to_string();
                break;
            }
        }
    }

    if !info.http_code.is_empty() || !info.error_type.is_empty()
        || !info.message.is_empty() || !info.request_id.is_empty()
    {
        Some(info)
    } else {
        None
    }
}

/// Try to parse a JSON error payload from raw error text.
fn parse_error_payload(raw: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    // Direct JSON parse
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(raw) {
        if is_error_payload(&map) {
            return Some(map);
        }
    }

    // Extract JSON from error prefix: "Error: {...}" etc.
    if let Some(start) = raw.find('{') {
        let json_part = &raw[start..];
        let mut depth = 0i32;
        let mut end_idx = None;
        for (i, ch) in json_part.char_indices() {
            if ch == '{' {
                depth += 1;
            } else if ch == '}' {
                depth -= 1;
                if depth == 0 {
                    end_idx = Some(i + 1);
                    break;
                }
            }
        }
        if let Some(end) = end_idx {
            if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(&json_part[..end]) {
                if is_error_payload(&map) {
                    return Some(map);
                }
            }
        }
    }

    None
}

/// Check if parsed JSON looks like an API error.
fn is_error_payload(payload: &serde_json::Map<String, serde_json::Value>) -> bool {
    if let Some(serde_json::Value::String(t)) = payload.get("type") {
        if t == "error" {
            return true;
        }
    }
    if payload.get("request_id").and_then(|v| v.as_str()).is_some() {
        return true;
    }
    if payload.get("requestId").and_then(|v| v.as_str()).is_some() {
        return true;
    }
    if let Some(serde_json::Value::Object(err)) = payload.get("error") {
        if err.contains_key("message") || err.contains_key("type") || err.contains_key("code") {
            return true;
        }
    }
    false
}

/// Deterministic JSON string representation (sorted keys).
fn stable_stringify(value: &serde_json::Map<String, serde_json::Value>) -> String {
    let mut keys: Vec<&String> = value.keys().collect();
    keys.sort();

    let parts: Vec<String> = keys.iter()
        .map(|k| format!("\"{}\":{}", k, stable_value(&value[*k])))
        .collect();
    format!("{{{}}}", parts.join(","))
}

fn stable_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => stable_stringify(map),
        serde_json::Value::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(stable_value).collect();
            format!("[{}]", parts.join(","))
        }
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_dedup_new_entry() {
        let cache = DedupeCache::new(Duration::from_secs(60), 100);
        assert!(!cache.check("abc"));
        assert_eq!(cache.size(), 1);
    }

    #[test]
    fn test_dedup_duplicate() {
        let cache = DedupeCache::new(Duration::from_secs(60), 100);
        assert!(!cache.check("abc"));
        assert!(cache.check("abc")); // duplicate
    }

    #[test]
    fn test_dedup_empty_key() {
        let cache = DedupeCache::new(Duration::from_secs(60), 100);
        assert!(!cache.check(""));
        assert_eq!(cache.size(), 0);
    }

    #[test]
    fn test_dedup_clear() {
        let cache = DedupeCache::new(Duration::from_secs(60), 100);
        cache.check("a");
        cache.check("b");
        assert_eq!(cache.size(), 2);
        cache.clear();
        assert_eq!(cache.size(), 0);
    }

    #[test]
    fn test_dedup_max_size() {
        let cache = DedupeCache::new(Duration::from_secs(60), 3);
        cache.check("a");
        cache.check("b");
        cache.check("c");
        cache.check("d"); // should evict oldest
        assert!(cache.size() <= 3);
    }

    #[test]
    fn test_fingerprint_error() {
        let payload = r#"{"type":"error","error":{"type":"rate_limit_error","message":"Too many requests"}}"#;
        let fp1 = fingerprint_error(payload);
        let fp2 = fingerprint_error(payload);
        assert!(!fp1.is_empty());
        assert_eq!(fp1, fp2); // stable
    }

    #[test]
    fn test_fingerprint_error_empty() {
        assert_eq!(fingerprint_error(""), "");
        assert_eq!(fingerprint_error("not json"), "");
    }

    #[test]
    fn test_parse_error_info_rate_limit() {
        let raw = r#"{"type":"error","error":{"type":"rate_limit_error","message":"Rate limit exceeded"},"request_id":"req-123"}"#;
        let info = parse_error_info(raw).unwrap();
        assert_eq!(info.error_type, "rate_limit_error");
        assert_eq!(info.message, "Rate limit exceeded");
        assert_eq!(info.request_id, "req-123");
    }

    #[test]
    fn test_parse_error_info_from_text() {
        let info = parse_error_info("API returned 429 rate_limit error").unwrap();
        assert_eq!(info.http_code, "429");
        assert_eq!(info.error_type, "rate_limit_error");
    }

    #[test]
    fn test_parse_error_info_none() {
        assert!(parse_error_info("").is_none());
        assert!(parse_error_info("all is fine").is_none());
    }

    #[test]
    fn test_hash_text() {
        let h1 = hash_text("hello");
        let h2 = hash_text("hello");
        assert_eq!(h1, h2);
        assert_ne!(h1, hash_text("world"));
    }

    #[test]
    fn test_stable_stringify() {
        let a: serde_json::Value = serde_json::json!({"b": 1, "a": 2});
        let b: serde_json::Value = serde_json::json!({"a": 2, "b": 1});
        let sa = stable_stringify(a.as_object().unwrap());
        let sb = stable_stringify(b.as_object().unwrap());
        assert_eq!(sa, sb);
    }
}
