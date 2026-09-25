use tracing::debug;

/// The NeboAI token to present right now — the ONE resolver for every call to
/// Janus or the hub. The hub rotates the token on every comms connect and the
/// comms client writes the rotated one to `neboai_token.cache` at once; the
/// `neboai` auth profile's copy is only brought up to date later, so the cache
/// wins when it differs. Resolve per request: a token captured at build time
/// goes stale on the next reconnect. `None` = not signed in to NeboAI.
pub fn neboai_token(store: &db::Store) -> Option<String> {
    let profiles = store
        .list_all_active_auth_profiles_by_provider("neboai")
        .unwrap_or_default();
    let mut token = profiles.first().map(|p| p.api_key.clone())?;
    if token.is_empty() {
        return None;
    }
    if let Ok(dir) = config::data_dir() {
        let cache_path = dir.join("neboai_token.cache");
        if let Ok(cached) = std::fs::read_to_string(&cache_path) {
            let cached = cached.trim().to_string();
            if !cached.is_empty() && cached != token {
                debug!("neboai: using cached rotated token (differs from DB)");
                token = cached;
            }
        }
    }
    Some(token)
}
