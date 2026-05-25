//! Pre-write secret scanner for memory persistence.
//! Scans fact values for common credential patterns before storage.

use regex::Regex;
use std::sync::OnceLock;

/// Compiled regex patterns for common secrets.
fn patterns() -> &'static [(&'static str, Regex)] {
    static PATTERNS: OnceLock<Vec<(&str, Regex)>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let raw: [(&str, &str); 15] = [
            ("AWS Access Key", r#"AKIA[0-9A-Z]{16}"#),
            ("AWS Secret Key", r#"(?i)aws_secret_access_key\s*=\s*\S{20,}"#),
            ("OpenAI API Key", r#"sk-[A-Za-z0-9]{32,}"#),
            ("Anthropic API Key", r#"sk-ant-[A-Za-z0-9\-]{40,}"#),
            ("GitHub Token", r#"gh[pousr]_[A-Za-z0-9]{36,}"#),
            ("Generic API Key", r#"(?i)(api[_-]?key|apikey)\s*[:=]\s*['"]?[A-Za-z0-9\-_.]{20,}"#),
            ("Bearer Token", r#"(?i)bearer\s+[A-Za-z0-9\-_.]{20,}"#),
            ("Private Key", r#"-----BEGIN (RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----"#),
            ("Slack Token", r#"xox[bprs]-[A-Za-z0-9\-]{10,}"#),
            ("Google API Key", r#"AIza[A-Za-z0-9\-_]{35}"#),
            ("Stripe Key", r#"(?:sk|pk)_(?:live|test)_[A-Za-z0-9]{20,}"#),
            ("Twilio Auth Token", r#"(?i)twilio.*[0-9a-f]{32}"#),
            ("SendGrid Key", r#"SG\.[A-Za-z0-9\-_.]{22,}\.[A-Za-z0-9\-_.]{43}"#),
            ("npm Token", r#"npm_[A-Za-z0-9]{36}"#),
            ("Heroku API Key", r#"(?i)heroku.*[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"#),
        ];
        raw.iter()
            .filter_map(|(name, pat)| {
                Regex::new(pat).ok().map(|r| (*name, r))
            })
            .collect()
    })
}

/// Returns true if the text contains any known secret patterns.
pub fn contains_secret(text: &str) -> bool {
    patterns().iter().any(|(_, re)| re.is_match(text))
}

/// Returns the name of the first secret pattern found, if any.
pub fn detect_secret(text: &str) -> Option<&'static str> {
    patterns().iter().find_map(|(name, re)| {
        if re.is_match(text) { Some(*name) } else { None }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detects_aws_key() {
        assert!(contains_secret("my key is AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn test_detects_openai_key() {
        assert!(contains_secret(
            "sk-abcdefghijklmnopqrstuvwxyz123456"
        ));
    }

    #[test]
    fn test_detects_anthropic_key() {
        assert!(contains_secret(
            "sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOP"
        ));
    }

    #[test]
    fn test_detects_private_key() {
        assert!(contains_secret("-----BEGIN RSA PRIVATE KEY-----"));
    }

    #[test]
    fn test_clean_text() {
        assert!(!contains_secret(
            "User prefers dark mode in all editors"
        ));
    }

    #[test]
    fn test_clean_code() {
        assert!(!contains_secret(
            "let x = get_api_key(); // retrieves from env"
        ));
    }
}
