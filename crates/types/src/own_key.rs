//! The owner's own key for a paid provider, on its way through Janus.
//!
//! A key the owner brought for one of the providers Nebo knows by name, with
//! no URL of his (Anthropic, OpenAI, Google, OpenRouter, DeepSeek, xAI; his
//! search keys; embeddings and transcription on his OpenAI key; voice on his
//! xAI key), goes to Janus with the key in `X-Provider-API-Key` and the
//! provider it belongs to in `X-Provider` (owner, 2026-10-19): Janus sends
//! the call to that provider on his key, never to another, and charges the
//! platform request fee alone. A connection whose URL he entered himself
//! (any OpenAI- or SystemOne-compatible endpoint, a typed provider URL, Ollama)
//! is reached direct at that URL, never through Janus, and its URL is never
//! sent there (owner, 2026-10-20: "the only way that it doesn't go through
//! Janus is if they plug in their own direct URL").

/// The caller's own provider key.
pub const KEY_HEADER: &str = "X-Provider-API-Key";
/// The provider the key belongs to, by Janus's name for it.
pub const PROVIDER_HEADER: &str = "X-Provider";

/// One own key, with the provider it goes to. Debug never prints the key.
#[derive(Clone, PartialEq, Eq)]
pub struct OwnKey {
    /// Janus's name for the provider (`anthropic`, `openai`, `google`,
    /// `deepseek`, `openrouter`, `xai`, `brave`, `tavily`, `google_cse`,
    /// `serpapi`).
    pub provider: String,
    pub key: String,
}

impl OwnKey {
    /// The headers every Janus request on this key carries, as
    /// (name, value): the key and its provider.
    pub fn headers(&self) -> Vec<(&'static str, String)> {
        vec![(KEY_HEADER, self.key.clone()), (PROVIDER_HEADER, self.provider.clone())]
    }
}

impl std::fmt::Debug for OwnKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnKey").field("provider", &self.provider).field("key", &"[redacted]").finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_carry_the_key_and_its_provider() {
        let k = OwnKey { provider: "anthropic".into(), key: "sk-2".into() };
        assert_eq!(k.headers(), vec![(KEY_HEADER, "sk-2".to_string()), (PROVIDER_HEADER, "anthropic".to_string())]);
    }

    #[test]
    fn debug_never_prints_the_key() {
        let k = OwnKey { provider: "openai".into(), key: "sk-SENTINEL".into() };
        assert!(!format!("{k:?}").contains("SENTINEL"));
    }
}
