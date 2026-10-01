//! How a tool reaches Janus, the inference gateway: the one bearer every
//! tool call sends with its `X-Bot-ID`. Search, extract and media generation
//! all authenticate this way, so the construction can never drift between
//! them.

/// Bearer token for Janus calls. Parity with the LLM provider
/// (build_providers): the Janus token is the NeboAI token, resolved per
/// call through `auth::neboai_token` (it rotates on every comms connect) —
/// a `janus` provider row never exists, so looking one up sent a bare
/// bot_id and Janus replied 401 on every call. The bool is whether the
/// token came from a real `neboai` profile — the bare bot_id fallback is a
/// known 401 cause, so callers surface it in their failure reasons.
pub fn bearer(store: Option<&db::Store>, bot_id: &str) -> (String, bool) {
    match store.and_then(auth::neboai_token) {
        Some(key) => (key, true),
        None => (bot_id.to_string(), false),
    }
}

/// What a caller adds to a Janus failure when the bot is not signed in.
pub const NOT_SIGNED_IN: &str = "Nebo is not signed in to NeboAI; ask the owner to sign in under Settings > Account, then retry.";
