//! Short-lived passes that let Nebo's own headless browser open one app as
//! the owner sees it (an app's listing screenshots).
//!
//! The local API admits no caller that proves nothing, and the built-in
//! browser holds no session. A pass is minted for one app and carried as
//! the page's path credential (`/k/<pass>/apps/<id>/ui/index.html`), so the
//! page's own files and the app's own routes (`/api/v1/apps/<id>/…`) load
//! with it, and nothing else: no other app, no other route. It lasts
//! minutes and is dropped when the screenshot is taken.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

fn passes() -> &'static Mutex<HashMap<String, (String, Instant)>> {
    static PASSES: std::sync::OnceLock<Mutex<HashMap<String, (String, Instant)>>> =
        std::sync::OnceLock::new();
    PASSES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A pass to view `app_id` for `ttl`.
pub fn grant(app_id: &str, ttl: Duration) -> String {
    let pass = format!(
        "av{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let mut map = passes().lock().unwrap_or_else(|p| p.into_inner());
    let now = Instant::now();
    map.retain(|_, (_, until)| *until > now);
    map.insert(pass.clone(), (app_id.to_string(), now + ttl));
    pass
}

/// Whether `pass` is a live pass for `app_id`.
pub fn admits(pass: &str, app_id: &str) -> bool {
    let map = passes().lock().unwrap_or_else(|p| p.into_inner());
    map.get(pass)
        .is_some_and(|(app, until)| app == app_id && *until > Instant::now())
}

/// Drop a pass before it runs out.
pub fn revoke(pass: &str) {
    passes()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(pass);
}

/// The app a request path belongs to, for a pass: the app's page
/// (`/apps/<id>/…`), its own routes (`/api/v1/apps/<id>/…`) and its socket
/// (`/ws/app/<id>`).
pub fn app_of_path(path: &str) -> Option<&str> {
    path.strip_prefix("/apps/")
        .or_else(|| path.strip_prefix("/api/v1/apps/"))
        .or_else(|| path.strip_prefix("/ws/app/"))?
        .split('/')
        .next()
        .filter(|id| !id.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pass opens its own app's page and routes only, until it runs out
    /// or is revoked.
    #[test]
    fn a_pass_opens_one_app_for_a_while() {
        let pass = grant("app-1", Duration::from_secs(60));
        assert!(admits(&pass, "app-1"));
        assert!(!admits(&pass, "app-2"));
        assert!(!admits("made-up", "app-1"));
        revoke(&pass);
        assert!(!admits(&pass, "app-1"));
        let brief = grant("app-1", Duration::from_millis(0));
        assert!(!admits(&brief, "app-1"), "an expired pass admits nothing");

        assert_eq!(app_of_path("/apps/app-1/ui/index.html"), Some("app-1"));
        assert_eq!(app_of_path("/api/v1/apps/app-1/storage/k"), Some("app-1"));
        assert_eq!(app_of_path("/ws/app/app-1"), Some("app-1"));
        assert_eq!(app_of_path("/api/v1/agents"), None);
        assert_eq!(app_of_path("/apps/"), None);
    }
}
