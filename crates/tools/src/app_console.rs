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

/// One line an app's page sent, as [`record`] keeps it: `why` is its
/// cause ([`causes`]), empty when none was read.
#[derive(Debug, Clone, Default)]
pub struct Line<'a> {
    pub level: &'a str,
    pub message: &'a str,
    pub source: &'a str,
    pub time_ms: i64,
    pub view: &'a str,
    pub why: String,
}

impl<'a> From<(&'a str, &'a str, &'a str, i64, &'a str)> for Line<'a> {
    fn from((level, message, source, time_ms, view): (&'a str, &'a str, &'a str, i64, &'a str)) -> Self {
        Line { level, message, source, time_ms, view, why: String::new() }
    }
}

/// Keep what an app's page sent: `(level, message, source, time_ms, view)`
/// each, or a [`Line`] with its cause.
pub fn record<'a, L: Into<Line<'a>>>(app_id: &str, entries: impl IntoIterator<Item = L>) -> usize {
    let mut added = Vec::new();
    for line in entries {
        let line = line.into();
        let level = level_of(line.level);
        added.push(Entry {
            seq: 0,
            level: level.to_string(),
            message: clip(line.message, MAX_MESSAGE_CHARS),
            source: clip(line.source, 40),
            time_ms: line.time_ms,
            view: clip(line.view, 64),
            why: if level == "error" { clip(&line.why, MAX_MESSAGE_CHARS) } else { String::new() },
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

/// The calls `nebo.storage` has (the SDK's `src/storage.ts`), each async;
/// the bundle the page loads is checked to export each one.
pub(crate) const STORAGE_CALLS: [&str; 6] = ["getItem", "setItem", "removeItem", "clear", "keys", "onChange"];

/// Entries of `ui/` looked through, once per batch, for files of a name.
const NAMESAKES_LOOKED: usize = 2000;
/// The most of a stylesheet read for its `@import`s.
const CSS_READ: u64 = 256 * 1024;

/// One failure an app's page reported, as its cause is read: what the
/// developer script sent besides the message.
#[derive(Debug, Clone, Copy, Default)]
pub struct Failure<'a> {
    pub level: &'a str,
    pub source: &'a str,
    pub message: &'a str,
    /// The file a failed load asked for: relative to `ui/` (`css/a.css`),
    /// or the page's own path outside `ui/`, from `/` (`/style.css`).
    /// Empty when the load was not the app's own file.
    pub file: &'a str,
    /// The reference as the served page writes it (`./style.css`).
    pub written: &'a str,
}

/// The cause and next step of each of a batch's errors, in order, empty
/// where none can be read: a file of its own the page could not load,
/// measured against `ui/` (the folder it is served from, walked at most
/// once), and the SDK called the wrong way. Blocking: reads the disk.
pub fn causes(ui: Option<&Path>, failures: &[Failure<'_>]) -> Vec<String> {
    let mut names: Option<HashMap<String, Vec<String>>> = None;
    failures
        .iter()
        .map(|f| {
            if level_of(f.level) != "error" {
                return None;
            }
            if f.source == "resource" {
                let ui = ui?;
                return failed_file(ui, f.file, f.written, &mut names);
            }
            sdk_misuse(f.message)
        })
        .map(Option::unwrap_or_default)
        .collect()
}

/// Every file in `ui/` by its name, as `ui/<path>`.
fn files_by_name(ui: &Path) -> HashMap<String, Vec<String>> {
    let mut by_name: HashMap<String, Vec<String>> = HashMap::new();
    for e in walkdir::WalkDir::new(ui).into_iter().filter_map(Result::ok).take(NAMESAKES_LOOKED) {
        if !e.file_type().is_file() {
            continue;
        }
        if let Ok(rel) = e.path().strip_prefix(ui) {
            let rel = format!("ui/{}", rel.to_string_lossy().replace('\\', "/"));
            by_name.entry(e.file_name().to_string_lossy().into_owned()).or_default().push(rel);
        }
    }
    by_name
}

/// Whether `rel` names a file inside `ui/`: plain names only, never `..`,
/// a root, a drive or a backslash.
fn inside(rel: &str) -> bool {
    !rel.is_empty()
        && !rel.contains('\\')
        && !rel.contains(':')
        && Path::new(rel).components().all(|c| matches!(c, std::path::Component::Normal(_)))
}

/// Why the page could not load `file` (as [`Failure::file`] says it), the
/// page writing it `written`, against what `ui/` holds now. `names` is
/// every file of `ui/` by its name, walked the first time one is needed.
fn failed_file(
    ui: &Path,
    file: &str,
    written: &str,
    names: &mut Option<HashMap<String, Vec<String>>>,
) -> Option<String> {
    let as_written = if written.is_empty() { String::new() } else { format!(" (the served page writes \"{written}\")") };
    let namesakes = |names: &mut Option<HashMap<String, Vec<String>>>, name: &str, fix: &str| {
        match names.get_or_insert_with(|| files_by_name(ui)).get(name).filter(|found| !found.is_empty()) {
            Some(found) => format!("{} is there: {fix}.", found.iter().take(3).cloned().collect::<Vec<_>>().join(", ")),
            None => format!("No file named {name} is anywhere in ui/: write it there, or drop the reference."),
        }
    };
    if let Some(outside) = file.strip_prefix('/') {
        if outside == "sdk/nebo.global.js" {
            return Some(format!(
                "the SDK loads from ui/index.html with {} exactly, before your own scripts.",
                crate::agent_tool::PersonaTool::APP_SDK_SCRIPT
            ));
        }
        let name = outside.rsplit('/').next().unwrap_or_default();
        return Some(format!(
            "the page asked for /{outside}{as_written}, which is outside the app's ui/ folder. {}",
            namesakes(names, name, "write the path relative to the page")
        ));
    }
    if !inside(file) {
        return None;
    }
    let path = ui.join(file);
    if path.is_file() {
        if file.ends_with(".css") {
            let mut css = String::new();
            if let Ok(f) = std::fs::File::open(&path) {
                use std::io::Read;
                let _ = f.take(CSS_READ).read_to_string(&mut css);
            }
            let imports = imports(&css);
            if !imports.is_empty() {
                return Some(format!(
                    "ui/{file} is there, so its path is right; a stylesheet also fails when an @import in it fails, and \
                     it imports {}. Put the rules it needs in ui/ and drop the @import.",
                    imports.join(", ")
                ));
            }
        }
        return Some(format!(
            "ui/{file} is there, so its path is right: what failed is in it or behind it (a module it imports, which \
             app_status names when it is missing; content the browser can't read as that type; or the file was being \
             rewritten as the page asked). Check those; the path needs no change."
        ));
    }
    let name = file.rsplit('/').next().unwrap_or(file);
    Some(format!(
        "the page asked for ui/{file}{as_written}, and there is no such file. {}",
        namesakes(names, name, "point the page at it, relative to the page")
    ))
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
         Nebo can read ends with `Why:`: its cause and the next step. Pass \
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
            record(&id, [("error", "boom", "error", 1, v.as_str())]);
        }
        assert_eq!(view_lines(&id, since, &views[0]), ViewLines::default(), "the oldest view put away");
        for v in &views[1..] {
            assert_eq!(view_lines(&id, since, v).errors, ["boom"], "{v}");
        }
    }

    /// The cause of one failure, as a batch of one.
    fn cause(ui: Option<&Path>, f: Failure<'_>) -> String {
        causes(ui, &[f]).remove(0)
    }

    fn load(file: &'static str, written: &'static str) -> Failure<'static> {
        Failure { level: "error", source: "resource", message: "Failed to load …", file, written }
    }

    /// Live 2026-10-22: Football League's console said "Failed to load
    /// …/ui/style.css" while ui/style.css was there, and its employee blamed
    /// caching. A failed load says what the page asked for against what ui/
    /// holds: another path than the file's, a path outside ui/, the
    /// stylesheet's own @import, or what is behind a file that is there.
    #[test]
    fn a_file_that_did_not_load_is_measured_against_ui() {
        let dir = tempfile::tempdir().unwrap();
        let ui = Some(dir.path());
        std::fs::create_dir_all(dir.path().join("css")).unwrap();
        std::fs::write(dir.path().join("css/style.css"), "body { color: red }").unwrap();

        let moved = cause(ui, load("style.css", "./style.css"));
        assert!(moved.contains("asked for ui/style.css (the served page writes \"./style.css\")"), "{moved}");
        assert!(moved.contains("ui/css/style.css is there"), "the file that exists: {moved}");

        let outside = cause(ui, load("/style.css", ""));
        assert!(outside.contains("outside the app's ui/ folder") && outside.contains("ui/css/style.css"), "{outside}");
        assert!(cause(ui, load("/sdk/nebo.global.js", "")).contains("../../../sdk/nebo.global.js"));

        std::fs::write(dir.path().join("style.css"), "/* x */\n@import url('https://esm.sh/css@1');\nbody{}").unwrap();
        let import = cause(ui, load("style.css", ""));
        assert!(import.contains("ui/style.css is there") && import.contains("https://esm.sh/css@1"), "{import}");

        std::fs::write(dir.path().join("app.js"), "import './gone.js';").unwrap();
        let there = cause(ui, load("app.js", ""));
        assert!(there.contains("a module it imports") && there.contains("the path needs no change"), "{there}");

        let missing = cause(ui, load("assets/turf.jpg", ""));
        assert!(missing.contains("No file named turf.jpg"), "{missing}");

        for escape in ["../secret.css", "/etc/x.css/../..", "a/../../b.css", "..\\..\\x.css", "C:/x.css", ""] {
            let shown = cause(ui, load(escape, ""));
            assert!(!shown.contains("is there"), "never read outside ui/: {escape} gave {shown}");
        }
        assert_eq!(cause(None, load("style.css", "")), "", "no folder, no cause");
        let warn = Failure { level: "warn", ..load("style.css", "") };
        assert_eq!(cause(ui, warn), "", "a note gets no cause");
    }

    /// The SDK called the wrong way points to the call that is right.
    #[test]
    fn sdk_misuse_points_to_the_right_call() {
        let tag = crate::agent_tool::PersonaTool::APP_SDK_SCRIPT;
        let err = |source: &'static str, message: &'static str| {
            cause(None, Failure { level: "error", source, message, ..Default::default() })
        };
        let no_sdk = err("error", "Uncaught TypeError: Cannot read properties of undefined (reading 'nebo')");
        assert!(no_sdk.contains(tag) && no_sdk.contains("const { nebo } = window.NeboAppSDK"), "{no_sdk}");
        let bare = err("error", "Uncaught ReferenceError: nebo is not defined");
        assert!(bare.contains("const { nebo } = window.NeboAppSDK"), "{bare}");
        let get = err("promise", "Unhandled promise rejection: TypeError: nebo.storage.get is not a function");
        assert!(get.contains("getItem") && get.contains("reads null"), "{get}");
        assert_eq!(err("error", "Uncaught TypeError: x.storage.getItem is not a function"), "", "a real call is not misuse");
        assert_eq!(err("error", "Uncaught TypeError: boom"), "");
    }

    /// The cause rides with the error to every reader: the console, the live
    /// check and "Send to <employee>".
    #[test]
    fn the_cause_rides_with_the_error() {
        let id = app();
        let since = mark();
        record(
            &id,
            [
                Line {
                    level: "error",
                    message: "Failed to load /apps/a1/ui/style.css",
                    source: "resource",
                    time_ms: 1,
                    view: "live-why",
                    why: "ui/style.css is there; it imports https://esm.sh/css@1".into(),
                },
                ("log", "ready", "console", 2, "live-why").into(),
            ],
        );
        let entries = recent(&id, None, 10);
        let shown = format_entry(&entries[0]);
        assert!(shown.contains("Failed to load /apps/a1/ui/style.css\n  Why: ui/style.css is there"), "{shown}");
        assert!(entries[1].why.is_empty());
        let lines = view_lines(&id, since, "live-why");
        assert!(lines.errors[0].contains("Why: ") && lines.errors[0].contains("esm.sh/css@1"), "{:?}", lines.errors);
    }
}
