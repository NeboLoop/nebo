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

/// The `neboai` profile-metadata key the bot's hosted address is kept under.
const BOT_ADDRESS_KEY: &str = "bot_email";

/// The bot's own hosted email address (`nanna-7kq@nebo.bot`) as the hub last
/// gave it, kept with the rest of the NeboAI account info so a turn never
/// asks the hub. `None` = not signed in to NeboAI, or the hub gave none.
pub fn neboai_bot_address(store: &db::Store) -> Option<String> {
    let profiles = store.list_all_active_auth_profiles_by_provider("neboai").ok()?;
    let meta = profiles.first()?.metadata.as_deref()?;
    let meta: serde_json::Map<String, serde_json::Value> = serde_json::from_str(meta).ok()?;
    meta.get(BOT_ADDRESS_KEY)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_string)
}

/// Keep the bot's hosted address with the NeboAI account info; `None` takes
/// it away. Without a NeboAI profile there is nothing to keep it with.
pub fn set_neboai_bot_address(store: &db::Store, address: Option<&str>) {
    let Some(profile) = store
        .list_all_active_auth_profiles_by_provider("neboai")
        .ok()
        .and_then(|p| p.into_iter().next())
    else {
        return;
    };
    let mut meta: serde_json::Map<String, serde_json::Value> =
        profile.metadata.as_deref().and_then(|m| serde_json::from_str(m).ok()).unwrap_or_default();
    let address = address.map(str::trim).filter(|a| !a.is_empty());
    if meta.get(BOT_ADDRESS_KEY).and_then(|v| v.as_str()) == address {
        return;
    }
    match address {
        Some(a) => meta.insert(BOT_ADDRESS_KEY.into(), a.into()),
        None => meta.remove(BOT_ADDRESS_KEY),
    };
    if let Err(e) = store.update_auth_profile_metadata(&profile.id, &serde_json::Value::Object(meta).to_string()) {
        tracing::warn!(error = %e, "the bot's hosted address could not be kept with the account");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The address rides in the profile's metadata beside what is already
    /// there; taking it away leaves the rest, and without a profile there is
    /// no address.
    #[test]
    fn the_bots_address_is_kept_with_the_account() {
        let path = std::env::temp_dir().join(format!("nebo-auth-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = db::Store::new(path.to_str().unwrap()).unwrap();
        set_neboai_bot_address(&store, Some("nanna-7kq@nebo.bot"));
        assert_eq!(neboai_bot_address(&store), None, "no NeboAI account, nothing to keep it with");

        store
            .create_auth_profile("p1", "NeboAI", "neboai", "tok", None, None, 0, 1, Some("token"), Some(r#"{"janus_provider":"true"}"#))
            .unwrap();
        assert_eq!(neboai_bot_address(&store), None);
        set_neboai_bot_address(&store, Some("nanna-7kq@nebo.bot"));
        assert_eq!(neboai_bot_address(&store).as_deref(), Some("nanna-7kq@nebo.bot"));
        let meta = |s: &db::Store| s.list_all_active_auth_profiles_by_provider("neboai").unwrap()[0].metadata.clone().unwrap();
        assert!(meta(&store).contains("janus_provider"), "the rest of the account info stays");

        set_neboai_bot_address(&store, None);
        assert_eq!(neboai_bot_address(&store), None);
        assert!(meta(&store).contains("janus_provider"));
        let _ = std::fs::remove_file(&path);
    }

    /// Every call to Janus or the hub presents this token and nothing else.
    /// A user's own provider keys sit in the same table, ahead of it by
    /// priority; none of them is ever the token, and without a NeboAI
    /// account there is no token at all — never a fallback to theirs.
    #[test]
    fn the_janus_token_is_never_a_users_own_key() {
        const SENTINEL: &str = "sk-SENTINEL-byo-key-0000000000000000";
        let path = std::env::temp_dir().join(format!("nebo-auth-byo-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = db::Store::new(path.to_str().unwrap()).unwrap();
        for provider in ["anthropic", "openai", "google", "xai", "openrouter", "deepseek", "search-brave"] {
            store
                .create_auth_profile(&format!("byo-{provider}"), provider, provider, SENTINEL, None, None, 100, 1, Some("api_key"), None)
                .unwrap();
        }
        assert_eq!(neboai_token(&store), None, "no NeboAI account: no token, never a BYO key");

        store
            .create_auth_profile("p1", "NeboAI", "neboai", "tok", None, None, 0, 1, Some("token"), None)
            .unwrap();
        let token = neboai_token(&store).expect("signed in");
        assert!(!token.contains(SENTINEL), "a BYO key became the Janus token");
        let _ = std::fs::remove_file(&path);
    }
}
