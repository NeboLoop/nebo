//! An app's console, as its open views saw it (App Developer mode).
//!
//! With App Developer mode on, the developer script the bot injects into an
//! app's entry HTML sends what the page logs, its uncaught errors and its
//! failed requests to `POST /apps/{id}/devlog`. Those entries land here: one
//! bounded ring per app, in memory, newest last. `app_console` reads it for
//! the employee building the app; `error_count` is the one number other
//! tools (`app_status`) ask for.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// Entries kept per app; the oldest go first.
pub const RING_CAPACITY: usize = 500;
/// The longest message kept, in characters.
const MAX_MESSAGE_CHARS: usize = 2000;
/// Entries `app_console` returns at most.
const MAX_RETURNED: usize = 100;

/// One line of an app's console.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Arrival order across every app, so `since` reads only newer lines.
    pub seq: u64,
    /// `log`, `info`, `warn`, `error` or `debug`.
    pub level: String,
    pub message: String,
    /// What produced it: `console`, `error`, `promise`, `network`, `resource`.
    pub source: String,
    /// When the page saw it, in milliseconds since the epoch.
    pub time_ms: i64,
    /// The view that sent it (set by whoever opened that view), else empty.
    pub view: String,
}

#[derive(Default)]
struct Rings {
    next: u64,
    apps: HashMap<String, VecDeque<Entry>>,
    /// Each named view's errors, `(seq, message)`, kept apart from the
    /// ring: a live check reads its view's errors whatever else was logged
    /// meanwhile. Each app's newest [`VIEWS_KEPT`] views, [`VIEW_ERRORS`]
    /// each.
    views: HashMap<String, Vec<(u64, String)>>,
    /// Each app's views with errors kept, oldest first.
    view_order: HashMap<String, VecDeque<String>>,
}

/// One app's view, as its kept errors are found.
fn view_key(app_id: &str, view: &str) -> String {
    format!("{app_id}\u{0}{view}")
}

/// Views per app whose errors are kept apart, oldest out first.
const VIEWS_KEPT: usize = 32;
/// Errors kept per view.
const VIEW_ERRORS: usize = 50;

fn rings() -> &'static Mutex<Rings> {
    static RINGS: OnceLock<Mutex<Rings>> = OnceLock::new();
    RINGS.get_or_init(Mutex::default)
}

fn level_of(raw: &str) -> &'static str {
    match raw {
        "error" => "error",
        "warn" | "warning" => "warn",
        "info" => "info",
        "debug" => "debug",
        _ => "log",
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// Keep what an app's page sent: `(level, message, source, time_ms, view)`
/// each.
pub fn record<'a>(app_id: &str, entries: impl IntoIterator<Item = (&'a str, &'a str, &'a str, i64, &'a str)>) -> usize {
    let mut rings = rings().lock().unwrap_or_else(|e| e.into_inner());
    let mut seq = rings.next;
    let mut added = Vec::new();
    for (level, message, source, time_ms, view) in entries {
        seq += 1;
        added.push(Entry {
            seq,
            level: level_of(level).to_string(),
            message: clip(message, MAX_MESSAGE_CHARS),
            source: clip(source, 40),
            time_ms,
            view: clip(view, 64),
        });
    }
    rings.next = seq;
    let count = added.len();
    for e in added.iter().filter(|e| e.level == "error" && !e.view.is_empty()) {
        let key = view_key(app_id, &e.view);
        if !rings.views.contains_key(&key) {
            let order = rings.view_order.entry(app_id.to_string()).or_default();
            order.push_back(key.clone());
            let gone = (order.len() > VIEWS_KEPT).then(|| order.pop_front()).flatten();
            if let Some(old) = gone {
                rings.views.remove(&old);
            }
        }
        let errors = rings.views.entry(key).or_default();
        if errors.len() < VIEW_ERRORS {
            errors.push((e.seq, e.message.clone()));
        }
    }
    let ring = rings.apps.entry(app_id.to_string()).or_default();
    for e in added {
        if ring.len() == RING_CAPACITY {
            ring.pop_front();
        }
        ring.push_back(e);
    }
    count
}

/// The app's entries after `since` (a `seq`), newest last, at most `limit`
/// (the newest ones).
pub fn recent(app_id: &str, since: Option<u64>, limit: usize) -> Vec<Entry> {
    let rings = rings().lock().unwrap_or_else(|e| e.into_inner());
    let Some(ring) = rings.apps.get(app_id) else { return Vec::new() };
    let after: Vec<&Entry> = ring.iter().filter(|e| since.is_none_or(|s| e.seq > s)).collect();
    let skip = after.len().saturating_sub(limit);
    after.into_iter().skip(skip).cloned().collect()
}

/// The arrival number of the newest entry of any app: a mark, after which
/// [`recent`] reads only what came since.
pub fn mark() -> u64 {
    rings().lock().unwrap_or_else(|e| e.into_inner()).next
}

/// The errors view `view` of the app sent after `since` ([`mark`]),
/// whatever else was logged meanwhile.
pub fn view_errors(app_id: &str, since: u64, view: &str) -> Vec<String> {
    let rings = rings().lock().unwrap_or_else(|e| e.into_inner());
    rings.views.get(&view_key(app_id, view)).map(|v| v.iter().filter(|(seq, _)| *seq > since).map(|(_, m)| m.clone()).collect()).unwrap_or_default()
}

/// How many errors the app's ring holds.
pub fn error_count(app_id: &str) -> usize {
    let rings = rings().lock().unwrap_or_else(|e| e.into_inner());
    rings.apps.get(app_id).map_or(0, |r| r.iter().filter(|e| e.level == "error").count())
}

/// One entry as the employee reads it.
pub fn format_entry(e: &Entry) -> String {
    let time = chrono::DateTime::from_timestamp_millis(e.time_ms)
        .map(|t| t.format("%H:%M:%S%.3f").to_string())
        .unwrap_or_else(|| "--:--:--".to_string());
    format!("#{} {} [{}] {}: {}", e.seq, time, e.level, e.source, e.message)
}

/// The text `app_console` returns.
pub fn render(app_name: &str, entries: &[Entry], since: Option<u64>) -> String {
    if entries.is_empty() {
        return match since {
            Some(s) => format!("{app_name}: nothing new since #{s}."),
            None => format!(
                "{app_name}: no console output yet. Entries arrive only while a view of the app is open: app_screenshot opens one and reports its console."
            ),
        };
    }
    let mut out = format!("{app_name}: {} entries, newest last.\n", entries.len());
    for e in entries {
        out.push_str(&format_entry(e));
        out.push('\n');
    }
    if let Some(last) = entries.last() {
        out.push_str(&format!("Read only newer entries with since: {}", last.seq));
    }
    out
}

/// `app_console(app, since?)`: the app's recent console output, uncaught
/// errors and failed requests from its open views.
pub struct AppConsoleTool {
    store: Arc<db::Store>,
}

impl AppConsoleTool {
    pub fn new(store: Arc<db::Store>) -> Self {
        Self { store }
    }

    /// The app named `app` (its id or its name).
    fn resolve(&self, app: &str) -> Result<db::models::Agent, String> {
        let app = app.trim();
        if app.is_empty() {
            return Err("Name the app: app_console(app: \"<app name or id>\").".into());
        }
        let agent = match self.store.get_agent(app).ok().flatten() {
            Some(a) => Some(a),
            None => {
                let slug = db::agent_slug(app);
                self.store
                    .list_agents(1000, 0)
                    .unwrap_or_default()
                    .into_iter()
                    .find(|a| db::agent_slug(&a.name) == slug)
            }
        };
        match agent {
            Some(a) if a.is_app.unwrap_or(0) != 0 => Ok(a),
            Some(a) => Err(format!("{} is not an app.", a.name)),
            None => Err(format!("No app named \"{app}\".")),
        }
    }
}

impl DynTool for AppConsoleTool {
    fn name(&self) -> &str {
        "app_console"
    }

    fn description(&self) -> String {
        "Read an app's console from its open views: console output, uncaught errors, \
         unhandled promise rejections and failed network requests, newest last. \
         Use it after an edit or a reload to see what the app did. Pass `since` from \
         the last call to read only newer entries."
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "app": {"type": "string", "description": "The app's name or id; leave it out when you are the app."},
                "since": {"type": "integer", "description": "Only entries after this number (from the last call)."}
            }
        })
    }

    fn search_hint(&self) -> &str {
        "app console logs errors network debug"
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            // The app named, else the caller itself; with App Developer mode
            // off an app reads only its own console.
            let caller = types::keyparser::extract_agent_id(&ctx.session_key);
            let named = input["app"].as_str().map(str::trim).filter(|a| !a.is_empty()).unwrap_or(&caller);
            let app = match self.resolve(named) {
                Ok(v) => v,
                Err(e) => return ToolResult::error(e),
            };
            if !crate::app_dev::may_work_on(&self.store, &caller, &app) {
                return ToolResult::error(crate::app_dev::not_yours(&app));
            }
            let (id, name) = (app.id, app.name);
            let since = input["since"].as_u64();
            ToolResult::ok(render(&name, &recent(&id, since, MAX_RETURNED), since))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> String {
        format!("app-{}", uuid::Uuid::new_v4().simple())
    }

    /// Review round 6: a live check counts only its own view's lines since
    /// its mark, failed requests and blocked loads included: an app whose
    /// API answers 500 fails it, and another open view's errors never count.
    #[test]
    fn a_live_check_counts_its_own_views_errors_and_a_failed_request_fails_it() {
        let id = app();
        record(&id, [("error", "from before", "error", 1, "live-1")]);
        let mark = mark();
        record(
            &id,
            [
                ("log", "ready", "console", 2, "live-1"),
                ("error", "GET /api/items → 500 Internal Server Error", "network", 3, "live-1"),
                ("error", "Blocked by the page's security policy (img-src): https://x", "security", 4, "live-1"),
                ("error", "an owner's view, reloaded meanwhile", "error", 5, ""),
            ],
        );
        let errors = view_errors(&id, mark, "live-1");
        assert_eq!(errors, ["GET /api/items → 500 Internal Server Error", "Blocked by the page's security policy (img-src): https://x"]);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<p>hi</p>").unwrap();
        assert_eq!(crate::app_dev::live_problem(dir.path(), errors.len()), Some("the page has console errors"));
    }

    #[test]
    fn the_ring_keeps_the_newest_entries_in_order() {
        let id = app();
        let lines: Vec<String> = (0..RING_CAPACITY + 20).map(|i| format!("line {i}")).collect();
        record(&id, lines.iter().map(|l| ("log", l.as_str(), "console", 0, "")));
        let all = recent(&id, None, usize::MAX);
        assert_eq!(all.len(), RING_CAPACITY, "bounded");
        assert_eq!(all.first().unwrap().message, "line 20", "the oldest went first");
        assert_eq!(all.last().unwrap().message, format!("line {}", RING_CAPACITY + 19), "newest last");
        let tail = recent(&id, None, 3);
        assert_eq!(tail.iter().map(|e| e.message.as_str()).collect::<Vec<_>>(), [
            format!("line {}", RING_CAPACITY + 17),
            format!("line {}", RING_CAPACITY + 18),
            format!("line {}", RING_CAPACITY + 19),
        ]);
    }

    #[test]
    fn since_reads_only_newer_entries_and_errors_are_counted() {
        let id = app();
        record(&id, [("info", "booted", "console", 1, ""), ("error", "boom", "error", 2, "")]);
        let first = recent(&id, None, 10);
        let mark = first.last().unwrap().seq;
        record(&id, [("warning", "slow", "console", 3, ""), ("error", "GET /x → 404", "network", 4, "")]);
        let newer = recent(&id, Some(mark), 10);
        assert_eq!(newer.iter().map(|e| e.level.as_str()).collect::<Vec<_>>(), ["warn", "error"]);
        assert_eq!(error_count(&id), 2);
        assert_eq!(error_count(&app()), 0, "another app's ring is its own");
    }

    #[test]
    fn rendering_says_what_to_pass_next() {
        let id = app();
        record(&id, [("error", "Uncaught TypeError: x is undefined (main.js:3:9)", "error", 0, "")]);
        let entries = recent(&id, None, 10);
        let text = render("Racer", &entries, None);
        assert!(text.contains("[error] error: Uncaught TypeError"), "{text}");
        assert!(text.ends_with(&format!("since: {}", entries[0].seq)), "{text}");
        assert!(render("Racer", &[], Some(7)).contains("nothing new since #7"));
    }

    /// A live view's boot error, then per-frame logs (a game, its own view
    /// plus the owner's views reloaded at the same time): the error is still
    /// there when the check reads it, though the ring of 500 dropped it.
    #[test]
    fn a_live_views_error_survives_log_spam() {
        let id = app();
        let mark = mark();
        record(&id, [("error", "Uncaught TypeError: boot failed", "error", 1, "live-x")]);
        let frames: Vec<String> = (0..RING_CAPACITY).map(|i| format!("frame {i}")).collect();
        record(&id, frames.iter().map(|f| ("log", f.as_str(), "console", 2, "")));
        assert_eq!(view_errors(&id, mark, "live-x"), ["Uncaught TypeError: boot failed"]);
    }


    /// The errors of an app's newest 32 views are kept apart: a 33rd view's
    /// first error puts the oldest view's away, never another's.
    #[test]
    fn the_oldest_views_errors_go_when_a_33rd_view_reports() {
        let id = app();
        let since = mark();
        let views: Vec<String> = (0..=VIEWS_KEPT).map(|i| format!("live-evict-{i}")).collect();
        for v in &views {
            record(&id, [("error", "boom", "error", 1, v.as_str())]);
        }
        assert!(view_errors(&id, since, &views[0]).is_empty(), "the oldest view put away");
        for v in &views[1..] {
            assert_eq!(view_errors(&id, since, v), ["boom"], "{v}");
        }
    }

}
