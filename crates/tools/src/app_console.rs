//! An app's console, as its open views saw it (App Developer mode).
//!
//! With App Developer mode on, the developer script the bot injects into an
//! app's entry HTML sends what the page logs, its uncaught errors and its
//! failed requests to `POST /apps/{id}/devlog`. Those entries land here: one
//! bounded ring per app, in memory, newest last. `app_console` reads it for
//! the employee building the app; `error_count` is the one number other
//! tools (`app_status`) ask for.
//!
//! An error a cause can be read for ([`why`]: a file the page could not
//! load, measured against what is in `ui/`; the SDK called the wrong way)
//! carries that cause and its fix, so every reader (`app_console`, the live
//! check, a screenshot's console, "Send to <employee>") gets it.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
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
    /// Its cause and next step ([`why`]), else empty.
    pub why: String,
}

/// An entry's message with its cause, as every reader shows it.
fn shown(message: &str, why: &str) -> String {
    if why.is_empty() { message.to_string() } else { format!("{message}\n  Why: {why}") }
}

#[derive(Default)]
struct Rings {
    next: u64,
    apps: HashMap<String, VecDeque<Entry>>,
    /// Each named view's errors and notes ([`is_note`]), `(seq, note,
    /// message)`, kept apart from the ring: a live check reads its view's
    /// lines whatever else was logged meanwhile. Each app's newest
    /// [`VIEWS_KEPT`] views, [`VIEW_ERRORS`] each.
    views: HashMap<String, Vec<(u64, bool, String)>>,
    /// Each app's views with errors kept, oldest first.
    view_order: HashMap<String, VecDeque<String>>,
}

/// One app's view, as its kept errors are found.
fn view_key(app_id: &str, view: &str) -> String {
    format!("{app_id}\u{0}{view}")
}

/// Views per app whose errors are kept apart, oldest out first.
const VIEWS_KEPT: usize = 32;
/// Errors (and notes) kept per view.
const VIEW_ERRORS: usize = 50;

/// Whether an entry is a note: a failure the page saw that is not the
/// app's own (a request to another site, a file it loads from one, a load
/// the security policy blocked there). The developer script sends those as
/// `warn` from `network`, `resource` or `security`; a live check reports
/// them and never counts them.
fn is_note(e: &Entry) -> bool {
    e.level == "warn" && matches!(e.source.as_str(), "network" | "resource" | "security")
}

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
/// each. `ui` is the folder the app is served from, read for each error's
/// cause ([`why`]).
pub fn record<'a>(
    app_id: &str,
    ui: Option<&Path>,
    entries: impl IntoIterator<Item = (&'a str, &'a str, &'a str, i64, &'a str)>,
) -> usize {
    let mut added = Vec::new();
    for (level, message, source, time_ms, view) in entries {
        let level = level_of(level);
        let why = if level == "error" { why(ui, source, message).unwrap_or_default() } else { String::new() };
        added.push(Entry {
            seq: 0,
            level: level.to_string(),
            message: clip(message, MAX_MESSAGE_CHARS),
            source: clip(source, 40),
            time_ms,
            view: clip(view, 64),
            why,
        });
    }
    let mut rings = rings().lock().unwrap_or_else(|e| e.into_inner());
    let mut seq = rings.next;
    for e in &mut added {
        seq += 1;
        e.seq = seq;
    }
    rings.next = seq;
    let count = added.len();
    for e in added.iter().filter(|e| (e.level == "error" || is_note(e)) && !e.view.is_empty()) {
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
            errors.push((e.seq, is_note(e), shown(&e.message, &e.why)));
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

/// The calls `nebo.storage` has (the SDK's `src/storage.ts`), each async.
const STORAGE_CALLS: [&str; 6] = ["getItem", "setItem", "removeItem", "clear", "keys", "onChange"];

/// Files in `ui/` looked through for one of the same name.
const NAMESAKES_LOOKED: usize = 2000;

/// The cause of an error an app's page reported, and its next step, when
/// one can be read: a file of its own the page could not load (`resource`),
/// measured against `ui/` (the folder it is served from), and the SDK
/// called the wrong way. `None` for anything else.
pub fn why(ui: Option<&Path>, source: &str, message: &str) -> Option<String> {
    if source == "resource" {
        let rest = message.strip_prefix("Failed to load ")?;
        let (asked, written) = match rest.split_once(" (written \"") {
            Some((asked, w)) => (asked, w.strip_suffix("\")")),
            None => (rest, None),
        };
        return ui.and_then(|ui| failed_file(ui, asked, written));
    }
    sdk_misuse(message)
}

/// Why the page could not load `asked` (the address the browser asked for,
/// `written` as the page wrote it), against what `ui/` holds now.
fn failed_file(ui: &Path, asked: &str, written: Option<&str>) -> Option<String> {
    let path = asked.split(['?', '#']).next().unwrap_or(asked);
    let path = urlencoding::decode(path).map(|p| p.into_owned()).unwrap_or_else(|_| path.to_string());
    if path.ends_with("/sdk/nebo.global.js") {
        return Some(format!(
            "the SDK loads from ui/index.html with {} exactly, before your own scripts.",
            crate::agent_tool::PersonaTool::APP_SDK_SCRIPT
        ));
    }
    let as_written = written.map(|w| format!(" (written \"{w}\")")).unwrap_or_default();
    if path.split('/').any(|s| s == "..") {
        return None;
    }
    let Some((_, rel)) = path.split_once("/ui/").filter(|_| path.contains("/apps/")) else {
        let name = path.rsplit('/').next().unwrap_or_default();
        return Some(format!(
            "the page asked for {path}{as_written}, which is outside the app's ui/ folder: a path that starts with / \
             leaves it. {}",
            namesakes(ui, name, "Write it relative to the page")
        ));
    };
    let file = ui.join(rel);
    if file.is_file() {
        if rel.ends_with(".css") {
            let imports = imports(&std::fs::read_to_string(&file).unwrap_or_default());
            if !imports.is_empty() {
                return Some(format!(
                    "ui/{rel} is there, so its path is right; a stylesheet also fails when an @import in it fails, and it \
                     imports {}. Put the rules it needs in ui/ and drop the @import.",
                    imports.join(", ")
                ));
            }
        }
        return Some(format!(
            "ui/{rel} is there now, so its path is right; it was missing or being rewritten when the page asked. Load the \
             page again with app_reload(check: true) before changing anything."
        ));
    }
    let name = rel.rsplit('/').next().unwrap_or(rel);
    Some(format!(
        "the page asked for ui/{rel}{as_written}, and there is no such file. {}",
        namesakes(ui, name, "Point the page at it, relative to the page")
    ))
}

/// Where in `ui/` a file named `name` is, and what to do about it.
fn namesakes(ui: &Path, name: &str, fix: &str) -> String {
    let found: Vec<String> = walkdir::WalkDir::new(ui)
        .into_iter()
        .filter_map(Result::ok)
        .take(NAMESAKES_LOOKED)
        .filter(|e| e.file_type().is_file() && !name.is_empty() && e.file_name().to_string_lossy() == name)
        .filter_map(|e| e.path().strip_prefix(ui).ok().map(|p| format!("ui/{}", p.to_string_lossy().replace('\\', "/"))))
        .take(3)
        .collect();
    if found.is_empty() {
        format!("No file named {name} is anywhere in ui/: write it there, or drop the reference.")
    } else {
        format!("{} is there: {fix}.", found.join(", "))
    }
}

/// The addresses a stylesheet's `@import`s name.
fn imports(css: &str) -> Vec<String> {
    static IMPORT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let import = IMPORT.get_or_init(|| regex::Regex::new(r#"@import\s+(?:url\(\s*)?['"]?([^'")\s;]+)"#).expect("valid"));
    import.captures_iter(css).map(|c| c[1].to_string()).collect()
}

/// The SDK called the wrong way, and the call that is right.
fn sdk_misuse(message: &str) -> Option<String> {
    let import = "`const { nebo } = window.NeboAppSDK;` (the same object as `NeboAppSDK.nebo`)";
    if message.contains("NeboAppSDK is not defined")
        || message.contains("reading 'nebo'")
        || message.contains("property 'nebo' of")
    {
        return Some(format!(
            "the SDK isn't on the page: ui/index.html loads it with {} before your own scripts, then {import}.",
            crate::agent_tool::PersonaTool::APP_SDK_SCRIPT
        ));
    }
    if message.contains("nebo is not defined") {
        return Some(format!("there is no bare `nebo` global: {import}."));
    }
    let (_, after) = message.split_once("storage.")?;
    let call = after.split(|c: char| !c.is_alphanumeric() && c != '_').next()?;
    (after[call.len()..].starts_with(" is not a function") && !STORAGE_CALLS.contains(&call)).then(|| {
        format!(
            "`nebo.storage` has {}, each async (await it). A key never saved reads null.",
            STORAGE_CALLS.join(", ")
        )
    })
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

/// What one view of an app reported: its errors, and its notes
/// (failures from other sites, never counted).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ViewLines {
    pub errors: Vec<String>,
    pub notes: Vec<String>,
}

/// The errors and notes view `view` of the app sent after `since`
/// ([`mark`]), whatever else was logged meanwhile.
pub fn view_lines(app_id: &str, since: u64, view: &str) -> ViewLines {
    let rings = rings().lock().unwrap_or_else(|e| e.into_inner());
    let mut out = ViewLines::default();
    for (_, note, message) in rings.views.get(&view_key(app_id, view)).into_iter().flatten().filter(|(seq, _, _)| *seq > since) {
        if *note { &mut out.notes } else { &mut out.errors }.push(message.clone());
    }
    out
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
    format!("#{} {} [{}] {}: {}", e.seq, time, e.level, e.source, shown(&e.message, &e.why))
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
         Use it after an edit or a reload to see what the app did. An error whose cause \
         Nebo can read ends with `Why:` and the next step: fix that, nothing else. Pass \
         `since` from the last call to read only newer entries.\n"
            .to_string()
            + crate::app_dev::SEE_IT_RUN
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
        record(&id, None, [("error", "from before", "error", 1, "live-1")]);
        let mark = mark();
        record(
            &id,
            None,
            [
                ("log", "ready", "console", 2, "live-1"),
                ("error", "GET /api/items → 500 Internal Server Error", "network", 3, "live-1"),
                ("error", "Blocked by the page's security policy (img-src): https://x", "security", 4, "live-1"),
                ("error", "an owner's view, reloaded meanwhile", "error", 5, ""),
            ],
        );
        let errors = view_lines(&id, mark, "live-1").errors;
        assert_eq!(errors, ["GET /api/items → 500 Internal Server Error", "Blocked by the page's security policy (img-src): https://x"]);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<p>hi</p>").unwrap();
        assert_eq!(crate::app_dev::live_problem(dir.path(), errors.len()), Some("the page has console errors"));
    }

    /// Review 2026-10-21: a failure from another site (analytics, a CDN,
    /// no network) is a note of the view, kept apart from its errors; the
    /// app's own console warnings are neither.
    #[test]
    fn another_sites_failures_are_a_views_notes_never_its_errors() {
        let id = app();
        let mark = mark();
        record(
            &id,
            None,
            [
                ("error", "GET /api/items → 500 Internal Server Error", "network", 1, "live-n"),
                ("warn", "GET https://stats.example/hit → network error (another site's, not counted)", "network", 2, "live-n"),
                ("warn", "Failed to load https://cdn.example/font.css (another site's, not counted)", "resource", 3, "live-n"),
                ("warn", "the cache is cold", "console", 4, "live-n"),
            ],
        );
        let lines = view_lines(&id, mark, "live-n");
        assert_eq!(lines.errors, ["GET /api/items → 500 Internal Server Error"]);
        assert_eq!(
            lines.notes,
            [
                "GET https://stats.example/hit → network error (another site's, not counted)",
                "Failed to load https://cdn.example/font.css (another site's, not counted)"
            ]
        );
    }

    #[test]
    fn the_ring_keeps_the_newest_entries_in_order() {
        let id = app();
        let lines: Vec<String> = (0..RING_CAPACITY + 20).map(|i| format!("line {i}")).collect();
        record(&id, None, lines.iter().map(|l| ("log", l.as_str(), "console", 0, "")));
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
        record(&id, None, [("info", "booted", "console", 1, ""), ("error", "boom", "error", 2, "")]);
        let first = recent(&id, None, 10);
        let mark = first.last().unwrap().seq;
        record(&id, None, [("warning", "slow", "console", 3, ""), ("error", "GET /x → 404", "network", 4, "")]);
        let newer = recent(&id, Some(mark), 10);
        assert_eq!(newer.iter().map(|e| e.level.as_str()).collect::<Vec<_>>(), ["warn", "error"]);
        assert_eq!(error_count(&id), 2);
        assert_eq!(error_count(&app()), 0, "another app's ring is its own");
    }

    #[test]
    fn rendering_says_what_to_pass_next() {
        let id = app();
        record(&id, None, [("error", "Uncaught TypeError: x is undefined (main.js:3:9)", "error", 0, "")]);
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
        record(&id, None, [("error", "Uncaught TypeError: boot failed", "error", 1, "live-x")]);
        let frames: Vec<String> = (0..RING_CAPACITY).map(|i| format!("frame {i}")).collect();
        record(&id, None, frames.iter().map(|f| ("log", f.as_str(), "console", 2, "")));
        assert_eq!(view_lines(&id, mark, "live-x").errors, ["Uncaught TypeError: boot failed"]);
    }


    /// The errors of an app's newest 32 views are kept apart: a 33rd view's
    /// first error puts the oldest view's away, never another's.
    #[test]
    fn the_oldest_views_errors_go_when_a_33rd_view_reports() {
        let id = app();
        let since = mark();
        let views: Vec<String> = (0..=VIEWS_KEPT).map(|i| format!("live-evict-{i}")).collect();
        for v in &views {
            record(&id, None, [("error", "boom", "error", 1, v.as_str())]);
        }
        assert_eq!(view_lines(&id, since, &views[0]), ViewLines::default(), "the oldest view put away");
        for v in &views[1..] {
            assert_eq!(view_lines(&id, since, v).errors, ["boom"], "{v}");
        }
    }


    /// Live 2026-10-22: Football League's console said "Failed to load
    /// …/ui/style.css" while ui/style.css was there, and its employee blamed
    /// caching. A failed load says what the page asked for against what ui/
    /// holds: another path than the file's, a path outside ui/, or the
    /// stylesheet's own @import.
    #[test]
    fn a_file_that_did_not_load_is_measured_against_ui() {
        let dir = tempfile::tempdir().unwrap();
        let ui = dir.path();
        std::fs::create_dir_all(ui.join("css")).unwrap();
        std::fs::write(ui.join("css/style.css"), "body { color: red }").unwrap();

        let moved = why(Some(ui), "resource", "Failed to load /t/bot/apps/a1/ui/style.css (written \"./style.css\")").unwrap();
        assert!(moved.contains("ui/style.css") && moved.contains("\"./style.css\""), "{moved}");
        assert!(moved.contains("ui/css/style.css is there"), "the file that exists: {moved}");

        let outside = why(Some(ui), "resource", "Failed to load /style.css (written \"/style.css\")").unwrap();
        assert!(outside.contains("outside the app's ui/ folder") && outside.contains("ui/css/style.css"), "{outside}");

        std::fs::write(ui.join("style.css"), "/* x */\n@import url('https://esm.sh/css@1');\nbody{}").unwrap();
        let import = why(Some(ui), "resource", "Failed to load /apps/a1/ui/style.css").unwrap();
        assert!(import.contains("ui/style.css is there") && import.contains("https://esm.sh/css@1"), "{import}");

        std::fs::write(ui.join("plain.css"), "body{}").unwrap();
        let there = why(Some(ui), "resource", "Failed to load /apps/a1/ui/plain.css").unwrap();
        assert!(there.contains("app_reload(check: true)"), "{there}");

        let missing = why(Some(ui), "resource", "Failed to load /apps/a1/ui/assets/turf.jpg").unwrap();
        assert!(missing.contains("No file named turf.jpg"), "{missing}");
        assert_eq!(why(Some(ui), "resource", "Failed to load /apps/a1/ui/../../secret.css"), None, "never outside ui/");
    }

    /// The SDK called the wrong way points to the call that is right.
    #[test]
    fn sdk_misuse_points_to_the_right_call() {
        let tag = crate::agent_tool::PersonaTool::APP_SDK_SCRIPT;
        let no_sdk = why(None, "error", "Uncaught TypeError: Cannot read properties of undefined (reading 'nebo')").unwrap();
        assert!(no_sdk.contains(tag) && no_sdk.contains("const { nebo } = window.NeboAppSDK"), "{no_sdk}");
        let bare = why(None, "error", "Uncaught ReferenceError: nebo is not defined").unwrap();
        assert!(bare.contains("const { nebo } = window.NeboAppSDK"), "{bare}");
        let get = why(None, "promise", "Unhandled promise rejection: TypeError: nebo.storage.get is not a function").unwrap();
        assert!(get.contains("getItem") && get.contains("reads null"), "{get}");
        assert_eq!(why(None, "error", "Uncaught TypeError: x.storage.getItem is not a function"), None, "a real call is not misuse");
        assert_eq!(why(None, "error", "Uncaught TypeError: boom"), None);
    }

    /// The cause rides with the error to every reader: the console, the live
    /// check and "Send to <employee>"; warnings get none.
    #[test]
    fn the_cause_rides_with_the_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("style.css"), "@import url('https://esm.sh/css@1');").unwrap();
        let id = app();
        let since = mark();
        record(
            &id,
            Some(dir.path()),
            [
                ("error", "Failed to load /apps/a1/ui/style.css", "resource", 1, "live-why"),
                ("warn", "Failed to load https://cdn.example/x.css (another site's, not counted)", "resource", 2, "live-why"),
            ],
        );
        let entries = recent(&id, None, 10);
        let shown = format_entry(&entries[0]);
        assert!(shown.contains("Failed to load /apps/a1/ui/style.css\n  Why: ui/style.css is there"), "{shown}");
        assert!(entries[1].why.is_empty(), "a note gets no cause");
        let lines = view_lines(&id, since, "live-why");
        assert!(lines.errors[0].contains("Why: ") && lines.errors[0].contains("esm.sh/css@1"), "{:?}", lines.errors);
    }
}
