//! Short-lived passes: the one credential a page Nebo serves in an origin
//! of its own carries.
//!
//! The local API admits no caller that proves nothing, and a sandboxed page
//! (its origin opaque) holds no session. A pass is minted for one thing and
//! carried as the page's path credential (`/k/<pass>/…`), so everything the
//! page loads or calls from its own address carries it too, and it opens
//! that one thing alone:
//!
//! - [`Scope::App`]: one app's page, routes and socket (`/apps/<id>/…`,
//!   `/api/v1/apps/<id>/…`, `/ws/app/<id>`): every app page, in a browser,
//!   through the tunnel, in the desktop window, in Nebo's own headless
//!   browser (an app's listing screenshots).
//! - [`Scope::Folder`]: reading the owner's files in one folder of the
//!   workspace and below (`/api/v1/files/<folder>/…`): a document opened
//!   from his files, which loads its pictures and styles from beside it.
//!
//! A pass lives while it is used: each request it opens keeps it for
//! another `idle` (what it was minted with), so a page stays open as long
//! as the owner uses it, and a pass left in a history or a log dies once its
//! page is left. It is dropped at once when revoked.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// What a pass opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// One app, by its id.
    App(String),
    /// Reading the owner's files in one folder (a path relative to the
    /// files route, `/`-separated) and below it. The top of the workspace
    /// (an empty folder) is never one: it opens nothing.
    Folder(String),
}

impl Scope {
    /// Whether a pass minted for `self` opens `asked`: the same app, or a
    /// file inside the folder, part by part (`Reports` holds
    /// `Reports/q3/chart.png`, never `ReportsOld/x.png`). A file path with
    /// an empty, `.` or `..` part is inside nothing.
    fn opens(&self, asked: &Scope) -> bool {
        match (self, asked) {
            (Scope::App(minted), Scope::App(asked)) => minted == asked,
            (Scope::Folder(folder), Scope::Folder(file)) => {
                let parts: Vec<&str> = file.split('/').collect();
                if parts.iter().any(|p| p.is_empty() || *p == "." || *p == "..") {
                    return false;
                }
                let folder: Vec<&str> = folder.split('/').filter(|p| !p.is_empty()).collect();
                !folder.is_empty() && parts.len() > folder.len() && parts[..folder.len()] == folder[..]
            }
            _ => false,
        }
    }
}

/// A live pass: what it opens, until when, and how long each use keeps it.
struct Held {
    scope: Scope,
    until: Instant,
    idle: Duration,
}

fn passes() -> &'static Mutex<HashMap<String, Held>> {
    static PASSES: std::sync::OnceLock<Mutex<HashMap<String, Held>>> = std::sync::OnceLock::new();
    PASSES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A pass that opens `scope`, kept while it is used: it dies `idle` after
/// the last request it opened.
pub fn grant(scope: Scope, idle: Duration) -> String {
    let pass = format!(
        "av{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let mut map = passes().lock().unwrap_or_else(|p| p.into_inner());
    let now = Instant::now();
    map.retain(|_, held| held.until > now);
    map.insert(pass.clone(), Held { scope, until: now + idle, idle });
    pass
}

/// Whether `pass` is live and opens `asked`; a pass that does is kept for
/// another of its `idle`.
pub fn admits(pass: &str, asked: &Scope) -> bool {
    let mut map = passes().lock().unwrap_or_else(|p| p.into_inner());
    let now = Instant::now();
    match map.get_mut(pass) {
        Some(held) if held.until > now && held.scope.opens(asked) => {
            held.until = now + held.idle;
            true
        }
        _ => false,
    }
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
    // The page is `/apps/<id>/ui[/…]` alone: nothing else under `/apps/<id>/`
    // is the app's (an unrouted path there is Nebo's own page).
    let (id, rest) = if let Some(r) = path.strip_prefix("/apps/") {
        let (id, rest) = r.split_once('/').unwrap_or((r, ""));
        (id, (rest == "ui" || rest.starts_with("ui/")).then_some(()))
    } else if let Some(r) = path.strip_prefix("/api/v1/apps/") {
        (r.split('/').next().unwrap_or(""), Some(()))
    } else if let Some(r) = path.strip_prefix("/ws/app/") {
        (r, (!r.contains('/')).then_some(()))
    } else {
        return None;
    };
    rest.and(Some(id)).filter(|id| !id.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str) -> Scope {
        Scope::App(id.to_string())
    }

    fn file(rel: &str) -> Scope {
        Scope::Folder(rel.to_string())
    }

    /// A pass opens its own app's page and routes only, until it runs out
    /// or is revoked.
    #[test]
    fn a_pass_opens_one_app_for_a_while() {
        let pass = grant(app("app-1"), Duration::from_secs(60));
        assert!(admits(&pass, &app("app-1")));
        assert!(!admits(&pass, &app("app-2")));
        assert!(!admits(&pass, &file("app-1/x")), "an app's pass reads no files");
        assert!(!admits("made-up", &app("app-1")));
        revoke(&pass);
        assert!(!admits(&pass, &app("app-1")));
        let brief = grant(app("app-1"), Duration::from_millis(0));
        assert!(!admits(&brief, &app("app-1")), "an expired pass admits nothing");

        // Each use keeps it for another idle; left unused, it dies.
        let used = grant(app("app-1"), Duration::from_millis(300));
        for _ in 0..4 {
            std::thread::sleep(Duration::from_millis(150));
            assert!(admits(&used, &app("app-1")), "kept while used");
        }
        std::thread::sleep(Duration::from_millis(400));
        assert!(!admits(&used, &app("app-1")), "left unused, it dies");

        assert_eq!(app_of_path("/apps/app-1/ui/index.html"), Some("app-1"));
        assert_eq!(app_of_path("/api/v1/apps/app-1/storage/k"), Some("app-1"));
        assert_eq!(app_of_path("/ws/app/app-1"), Some("app-1"));
        assert_eq!(app_of_path("/api/v1/agents"), None);
        assert_eq!(app_of_path("/apps/"), None);
        assert_eq!(app_of_path("/apps/app-1/ui"), Some("app-1"));
        assert_eq!(app_of_path("/apps/app-1/chat-embed"), None, "only its page under /apps/<id>/");
        assert_eq!(app_of_path("/apps/app-1/uix"), None);
        assert_eq!(app_of_path("/ws/app/app-1/x"), None);
    }

    /// A document's pass reads the files of its folder and below, part by
    /// part, and nothing beside or above it.
    #[test]
    fn a_folder_pass_reads_its_folder_alone() {
        let pass = grant(Scope::Folder("Reports/q3".into()), Duration::from_secs(60));
        assert!(admits(&pass, &file("Reports/q3/deck.html")));
        assert!(admits(&pass, &file("Reports/q3/img/chart.png")));
        for refused in [
            "Reports/q3",
            "Reports/q4/deck.html",
            "Reports/q3old/x.png",
            "Reports/x.png",
            "Reports/q3/../q4/deck.html",
            "Reports/q3//x.png",
            "Reports/q3/./x.png",
            "finance/taxes.pdf",
        ] {
            assert!(!admits(&pass, &file(refused)), "{refused}");
        }
        assert!(!admits(&pass, &app("Reports")));
        revoke(&pass);
        let top = grant(Scope::Folder(String::new()), Duration::from_secs(60));
        assert!(!admits(&top, &file("deck.html")) && !admits(&top, &file("img/a.png")), "the top opens nothing");
        revoke(&top);
    }
}
