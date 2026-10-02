//! Log redaction: the ONE way a frame, body, header set or URL reaches a log
//! line.
//!
//! The owner's session token rode the `auth` WebSocket frame straight into
//! `ws client message: {...}` on every connect, so every bot log and every
//! desktop log carried it. Log sites now print a frame's type and size; when
//! a payload genuinely helps debugging it goes through [`redact`] first —
//! never `%text`, `?body` or `{}` on the raw bytes.
//!
//! What is redacted:
//! - the value of any key that names a secret, case-insensitively and with
//!   `_`/`-` ignored: anything ending in `token`, `authorization`, `cookie`,
//!   `secret`, `password`, `apikey`, `jwt` or `privatekey` (`access_token`,
//!   `accessToken`, `refresh_token`, `set-cookie`, `x-api-key`,
//!   `client_secret`…), plus the bare keys `session`, `auth` and `bearer`.
//!   Counts and ids stay: `max_tokens`, `session_id`, `session_key`.
//! - `Bearer <x>` anywhere.
//! - anything shaped like a JWT (three base64url segments, the first one
//!   `eyJ…` — the encoding of `{"`, which every JWT header starts with).
//!
//! JSON is walked structurally (nested objects, arrays, and strings that are
//! themselves JSON); anything else — headers, query strings, Debug output,
//! truncated JSON — is scanned as text.

use std::sync::LazyLock;

use regex::{Captures, Regex};
use serde_json::Value;

/// What a redacted value is replaced with.
pub const REDACTED: &str = "[redacted]";

/// Redact every secret in `s` (JSON or free text). Redact BEFORE truncating:
/// a cut-off JSON frame is only scanned as text.
pub fn redact(s: &str) -> String {
    let t = s.trim_start();
    if (t.starts_with('{') || t.starts_with('['))
        && let Ok(mut v) = serde_json::from_str::<Value>(s)
    {
        redact_value(&mut v);
        return v.to_string();
    }
    redact_text(s)
}

/// Whether `key` names a secret (see the module docs for the rule).
pub fn is_sensitive_key(key: &str) -> bool {
    let k: String = key
        .chars()
        .filter(|c| *c != '_' && *c != '-')
        .flat_map(char::to_lowercase)
        .collect();
    matches!(k.as_str(), "session" | "auth" | "bearer")
        || ["token", "authorization", "cookie", "secret", "password", "apikey", "jwt", "privatekey"]
            .iter()
            .any(|s| k.ends_with(s))
}

fn redact_value(v: &mut Value) {
    match v {
        Value::Object(map) => {
            for (k, val) in map.iter_mut() {
                if is_sensitive_key(k) {
                    if !val.is_null() {
                        *val = Value::String(REDACTED.into());
                    }
                } else {
                    redact_value(val);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact_value),
        Value::String(s) => *s = redact(s),
        _ => {}
    }
}

/// `Cookie: …`, `Set-Cookie: …`, `Authorization: …` header lines: the whole
/// rest of the line is the secret (a cookie header holds many `k=v;` pairs).
static HEADER_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)^(\s*(?:set-cookie|cookie|proxy-authorization|authorization)\s*:)[^\r\n]*").unwrap()
});

/// `key=value`, `key: value`, `"key":"value"`, `\"key\":\"value\"` — the
/// closure decides whether the key is a secret.
static KEY_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?P<key>[A-Za-z][A-Za-z0-9_-]*)(?P<q>\\?["']?)(?P<sep>\s*[:=]\s*)(?P<val>\\"(?:[^\\]|\\[^"])*(?:\\"|$)|"(?:[^"\\]|\\.)*(?:"|$)|'[^']*(?:'|$)|[^\s&;,}\])"'\\<>]+)"#,
    )
    .unwrap()
});

static BEARER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(bearer)\s+[A-Za-z0-9._~+/=-]+").unwrap());

static JWT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]*").unwrap());

fn redact_text(s: &str) -> String {
    let s = HEADER_LINE.replace_all(s, |c: &Captures| format!("{} {REDACTED}", &c[1]));
    let s = redact_pairs(&s);
    let s = BEARER.replace_all(&s, |c: &Captures| format!("{} {REDACTED}", &c[1]));
    JWT.replace_all(&s, REDACTED).into_owned()
}

fn redact_pairs(s: &str) -> String {
    KEY_VALUE
        .replace_all(s, |c: &Captures| {
            let (key, q, sep, val) = (&c["key"], &c["q"], &c["sep"], &c["val"]);
            if is_sensitive_key(key) {
                let quoted = if val.starts_with("\\\"") {
                    format!("\\\"{REDACTED}\\\"")
                } else if let Some(open) = val.chars().next().filter(|c| *c == '"' || *c == '\'') {
                    format!("{open}{REDACTED}{open}")
                } else {
                    REDACTED.to_string()
                };
                format!("{key}{q}{sep}{quoted}")
            } else {
                // The value can hold pairs of its own (`https://h/p?token=…`).
                format!("{key}{q}{sep}{}", redact_pairs(val))
            }
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fake credentials only — nothing here is a real token.
    const FAKE: &str = "fake-SECRET-value-123";
    const FAKE_JWT: &str = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ0ZXN0In0.c2lnbmF0dXJlLWZha2U";

    fn clean(out: &str) {
        assert!(!out.contains(FAKE), "secret survived: {out}");
        assert!(!out.contains("c2lnbmF0dXJlLWZha2U"), "jwt survived: {out}");
    }

    #[test]
    fn sensitive_keys() {
        for k in [
            "token", "TOKEN", "access_token", "accessToken", "refresh_token", "Authorization",
            "cookie", "Set-Cookie", "secret", "client_secret", "password", "api_key", "apiKey",
            "X-Api-Key", "session", "jwt", "bearer", "nebo_token", "idToken", "auth",
        ] {
            assert!(is_sensitive_key(k), "{k}");
        }
        for k in ["type", "max_tokens", "input_tokens", "session_id", "sessionKey", "token_type", "id", "authorized"] {
            assert!(!is_sensitive_key(k), "{k}");
        }
    }

    #[test]
    fn ws_auth_frame() {
        let frame = format!(
            r#"{{"type":"auth","data":{{"token":"{FAKE}","client_id":"c-1"}},"timestamp":"t"}}"#
        );
        let out = redact(&frame);
        clean(&out);
        assert!(out.contains(r#""client_id":"c-1""#), "{out}");
        assert!(out.contains(r#""type":"auth""#), "{out}");
    }

    #[test]
    fn nested_json() {
        let frame = serde_json::json!({
            "type": "chat",
            "session_id": "s-9",
            "usage": {"max_tokens": 1024},
            "data": {
                "headers": {"Authorization": format!("Bearer {FAKE}"), "Cookie": format!("sid={FAKE}")},
                "items": [{"refreshToken": FAKE}, {"note": format!("here is {FAKE_JWT}")}],
                "creds": {"apiKey": FAKE, "password": FAKE, "set-cookie": FAKE},
                "content": format!(r#"{{"accessToken":"{FAKE}","id":7}}"#),
                "session": {"anything": FAKE},
            }
        });
        let out = redact(&frame.to_string());
        clean(&out);
        assert!(out.contains(r#""session_id":"s-9""#), "{out}");
        assert!(out.contains(r#""max_tokens":1024"#), "{out}");
        assert!(out.contains(r#"\"id\":7"#), "{out}");
    }

    #[test]
    fn headers_text() {
        let raw = format!(
            "GET /ws HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {FAKE}\r\nCookie: a=1; sid={FAKE}\r\nSet-Cookie: s={FAKE}; Path=/\r\nX-Api-Key: {FAKE}\r\nAccept: */*\r\n\r\n"
        );
        let out = redact(&raw);
        clean(&out);
        assert!(out.contains("Host: localhost"), "{out}");
        assert!(out.contains("Accept: */*"), "{out}");
        // Debug output of a header map
        let dbg = format!(r#"{{"authorization": "Bearer {FAKE}", "content-type": "application/json"}}"#);
        let out = redact(&dbg);
        clean(&out);
        assert!(out.contains("application/json"), "{out}");
    }

    #[test]
    fn urls_bearer_jwt_and_truncated_json() {
        for raw in [
            format!("https://hub.example/ws?bot=1&token={FAKE}&x=2"),
            format!("/t/abc/api?access_token={FAKE}"),
            format!("calling with bearer {FAKE} now"),
            format!("the token is {FAKE_JWT} ok"),
            format!(r#"{{"type":"auth","data":{{"token":"{FAKE}","cli"#),
            format!(r#"Frame {{ token: "{FAKE}", id: 3 }}"#),
            format!(r#"payload={{\"token\":\"{FAKE}\"}}"#),
        ] {
            let out = redact(&raw);
            clean(&out);
        }
        assert_eq!(redact("version 0.16.7 at a.b.c"), "version 0.16.7 at a.b.c");
        assert_eq!(redact("plain text, nothing secret"), "plain text, nothing secret");
    }
}
