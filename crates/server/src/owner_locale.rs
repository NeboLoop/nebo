//! The owner's home time zone and language, owned by their NeboAI account.
//!
//! The account is the truth. On every connect the bot reads it
//! (GET /owners/me), and on every change the hub tells the bot on its
//! `account` stream (`{"type":"ownerLocale","timezone","language"}`). Both
//! land where every turn, schedule and reminder already reads them: the
//! owner profile's time zone and the owner's language preference. What the
//! owner sets on this bot goes to the account (PUT /owners/me), and from
//! there to every bot they have. A bot with no account keeps what is set on
//! it, as before.

use db::Store;
use tools::owner_clock::OwnerZone;
use tracing::{info, warn};

use crate::state::AppState;

/// The `type` of the hub's account-stream message that carries the
/// account's time zone and language.
pub const EVENT: &str = "ownerLocale";

/// Mirror the account's time zone and language into this bot. An empty or
/// unknown value is not known yet and leaves what is here.
pub fn apply(store: &Store, timezone: &str, language: &str) {
    if let Some(zone) = OwnerZone::parse(Some(timezone)).name() {
        let current = store
            .get_user_profile()
            .ok()
            .flatten()
            .and_then(|p| p.timezone);
        if current.as_deref() != Some(zone) {
            match store.update_user_profile(
                None,
                None,
                None,
                Some(zone),
                None,
                None,
                None,
                None,
                None,
                None,
            ) {
                Ok(()) => info!(timezone = zone, "owner's time zone from the account"),
                Err(e) => warn!(error = %e, "could not keep the account's time zone"),
            }
        }
    }
    if let Some(code) = types::language::app_language(language) {
        let current = store
            .get_user_preferences()
            .ok()
            .flatten()
            .map(|p| p.language);
        if current.as_deref() != Some(code) {
            match store.update_user_preferences(None, Some(code), None, None, None, None) {
                Ok(()) => info!(language = code, "owner's language from the account"),
                Err(e) => warn!(error = %e, "could not keep the account's language"),
            }
        }
    }
}

/// Apply an `account`-stream message when it is the account's locale.
/// Returns whether it was one.
pub fn handle_event(store: &Store, event: &serde_json::Value) -> bool {
    if event.get("type").and_then(|t| t.as_str()) != Some(EVENT) {
        return false;
    }
    apply(store, str_of(event, "timezone"), str_of(event, "language"));
    true
}

/// On connect: read the account's time zone and language and keep them
/// here. An account that has no time zone yet is told the one the owner
/// set on this bot, as a device report (kept only while the account has
/// none).
pub async fn sync(state: &AppState) {
    let Ok(api) = crate::codes::build_api_client(state) else {
        return;
    };
    let me = match api.owner_me().await {
        Ok(me) => me,
        Err(e) => {
            warn!(error = %e, "could not read the account's time zone and language");
            return;
        }
    };
    let (timezone, language) = (str_of(&me, "timezone"), str_of(&me, "language"));
    apply(&state.store, timezone, language);
    if timezone.is_empty()
        && let Some(here) = OwnerZone::of(&state.store).name()
    {
        tell_account(state, serde_json::json!({ "deviceTimezone": here })).await;
    }
}

/// Send the account what the owner set or a client detected (the fields of
/// PUT /owners/me: `timezone`, `language`, `deviceTimezone`,
/// `deviceLanguage`) and keep what it then holds. False when there is no
/// account to tell or it could not be reached.
pub async fn tell_account(state: &AppState, body: serde_json::Value) -> bool {
    let Ok(api) = crate::codes::build_api_client(state) else {
        return false;
    };
    match api.update_owner(&body).await {
        Ok(stored) => {
            apply(
                &state.store,
                str_of(&stored, "timezone"),
                str_of(&stored, "language"),
            );
            true
        }
        Err(e) => {
            warn!(error = %e, "could not tell the account its time zone and language");
            false
        }
    }
}

fn str_of<'a>(v: &'a serde_json::Value, key: &str) -> &'a str {
    v.get(key).and_then(|s| s.as_str()).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let path =
            std::env::temp_dir().join(format!("nebo-owner-locale-{}.db", uuid::Uuid::new_v4()));
        Store::new(&path.to_string_lossy()).expect("store")
    }

    #[test]
    fn the_accounts_locale_lands_where_turns_and_schedules_read_it() {
        let s = store();
        assert_eq!(
            OwnerZone::of(&s),
            OwnerZone::Machine,
            "nothing known: this computer's clock"
        );
        assert!(handle_event(
            &s,
            &serde_json::json!({"type": "ownerLocale", "timezone": "America/Denver", "language": "es-MX"})
        ));
        assert_eq!(OwnerZone::of(&s).name(), Some("America/Denver"));
        assert_eq!(s.get_user_preferences().unwrap().unwrap().language, "es");
    }

    #[test]
    fn unknown_or_empty_values_leave_what_is_here() {
        let s = store();
        apply(&s, "America/Denver", "de");
        apply(&s, "", "");
        apply(&s, "Mars/Olympus_Mons", "fi");
        assert_eq!(OwnerZone::of(&s).name(), Some("America/Denver"));
        assert_eq!(s.get_user_preferences().unwrap().unwrap().language, "de");
        // Another account message is not this one.
        assert!(!handle_event(
            &s,
            &serde_json::json!({"type": "tokenRefresh", "timezone": "Asia/Tokyo"})
        ));
        assert_eq!(OwnerZone::of(&s).name(), Some("America/Denver"));
    }
}
