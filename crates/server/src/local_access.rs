//! Signing a browser in to Nebo's local API.
//!
//! Every caller of the local API proves who it is (`middleware::local_boundary`).
//! A browser can't carry the install key, so it holds a session instead: the
//! `nebo_session` cookie, HttpOnly and SameSite=Strict, set here in exchange
//! for a sign-in ticket. A ticket is made in this process, used once, and
//! lapses after two minutes:
//!
//! - the desktop app asks this server for one with the install key
//!   (`POST /api/v1/local-session/ticket`) and opens its window through it,
//!   each time it attaches to the engine;
//! - a browser the owner opens himself (the Vite dev server's included: the
//!   cookie belongs to `localhost`, whatever the port) signs in through the
//!   link `nebo open` prints, which asks this server for a ticket with the
//!   install key (`POST /api/v1/local-session/ticket`).
//!
//! The session is derived from the install key, so it outlives a restart and
//! ends when the key is replaced. Neither the key nor a session is ever in an
//! employee command's environment or in a file its command can read.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::Query;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The browser session's cookie.
pub const COOKIE: &str = "nebo_session";

/// How long a sign-in ticket may wait to be used.
const TICKET_LIFE: Duration = Duration::from_secs(120);

/// Tickets made and not yet used, with when each was made.
fn tickets() -> &'static Mutex<HashMap<String, Instant>> {
    static TICKETS: std::sync::OnceLock<Mutex<HashMap<String, Instant>>> = std::sync::OnceLock::new();
    TICKETS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The session a browser holds under the install key `key`.
pub fn session_for(key: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"nebo-session:");
    hash.update(key.as_bytes());
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// A new sign-in ticket: used once, within two minutes.
fn ticket() -> String {
    let ticket = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    let mut open = tickets().lock().unwrap_or_else(|e| e.into_inner());
    open.retain(|_, made| made.elapsed() < TICKET_LIFE);
    open.insert(ticket.clone(), Instant::now());
    ticket
}

/// Whether `ticket` was made here, is unused and has not lapsed. Using it
/// spends it.
fn redeem(ticket: &str) -> bool {
    let mut open = tickets().lock().unwrap_or_else(|e| e.into_inner());
    open.remove(ticket).is_some_and(|made| made.elapsed() < TICKET_LIFE)
}

/// The path that signs a browser in with a fresh ticket and opens the app.
/// The desktop app loads it in its window; `nebo open` prints it.
pub fn sign_in_path() -> String {
    format!("/api/v1/local-session?ticket={}", ticket())
}

#[derive(Deserialize)]
pub struct SignIn {
    #[serde(default)]
    ticket: String,
}

/// GET /api/v1/local-session?ticket=… — the ticket for the session cookie,
/// then the app. The one route a browser reaches with no proof yet: the
/// ticket is the proof.
pub async fn sign_in(Query(q): Query<SignIn>) -> Response {
    let key = config::read_install_key();
    match key {
        Some(key) if redeem(&q.ticket) => (
            StatusCode::SEE_OTHER,
            [
                (header::LOCATION, "/".to_string()),
                (header::SET_COOKIE, format!("{COOKIE}={}; Path=/; HttpOnly; SameSite=Strict", session_for(&key))),
            ],
        )
            .into_response(),
        _ => (
            StatusCode::UNAUTHORIZED,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            SIGN_IN_EXPIRED,
        )
            .into_response(),
    }
}

/// POST /api/v1/local-session/ticket — a sign-in path for a browser, for the
/// owner's own client (the boundary admitted it with the install key).
pub async fn new_ticket() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "path": sign_in_path() }))
}

const SIGN_IN_EXPIRED: &str = "<!doctype html><html><head><title>Nebo</title></head><body>\
<p>This sign-in link has expired or was already used.</p>\
<p>Open Nebo from its app, or run <code>nebo open</code> in a terminal on this computer for a new link.</p>\
</body></html>";

/// What a browser page asking for Nebo with no session is shown.
pub const NOT_SIGNED_IN: &str = "<!doctype html><html><head><title>Nebo</title></head><body>\
<p>This window isn't signed in to Nebo.</p>\
<p>Open Nebo from its app, or run <code>nebo open</code> in a terminal on this computer.</p>\
</body></html>";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ticket_is_used_once() {
        let t = ticket();
        assert!(redeem(&t));
        assert!(!redeem(&t), "a used ticket is spent");
        assert!(!redeem("made-up"));
        assert!(!redeem(""));
    }

    #[test]
    fn a_lapsed_ticket_is_refused() {
        let t = ticket();
        tickets()
            .lock()
            .unwrap()
            .insert(t.clone(), Instant::now().checked_sub(TICKET_LIFE + Duration::from_secs(1)).unwrap());
        assert!(!redeem(&t));
    }

    #[test]
    fn the_session_follows_the_key() {
        assert_eq!(session_for("k1"), session_for("k1"));
        assert_ne!(session_for("k1"), session_for("k2"));
        assert!(!session_for("k1").contains("k1"));
    }
}
