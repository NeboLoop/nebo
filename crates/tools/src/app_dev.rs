//! Apps an employee builds: where each one's files are served from, and the
//! developer pack (`app_reload`, `app_status`, `app_console`, and publishing
//! with `app_screenshot` / `app_listing` / `app_submit`).
//!
//! Who gets the pack, by one rule ([`withheld`], [`may_work_on`]):
//! - The owner's own app (made on this bot, not installed from the
//!   marketplace: [`is_own_app`]) always builds and publishes ITSELF. No
//!   setting is needed for an app to reload, check, read the console of,
//!   screenshot and publish its own page.
//! - App Developer mode widens it to teammates: every app employee and
//!   whoever works beside one may use the pack on any of the owner's apps,
//!   and the page grows the floating console.
//! - An app installed from the marketplace never gets developer tooling in
//!   its page, mode or not (the server's `serve_app_ui`).
//!
//! `agents.app_ui_path` is the one record of an app's location: it is what
//! the server serves the app from. Every employee is told it for each app
//! coworker (the employees listing), the app employee about itself, and a
//! web file written for an app somewhere else gets a note saying where the
//! app really is.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use serde_json::{Value, json};

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};
use crate::web_tool::Broadcaster;

pub const APP_RELOAD: &str = "app_reload";
pub const APP_STATUS: &str = "app_status";
pub const APP_CONSOLE: &str = "app_console";
/// The developer pack: offered to the owner's own app for itself, and under
/// App Developer mode to app employees and their teammates.
pub const TOOLS: [&str; 6] = [
    APP_RELOAD,
    APP_STATUS,
    APP_CONSOLE,
    // Publishing an app with the owner (`app_publish`).
    crate::app_publish::APP_SCREENSHOT,
    crate::app_publish::APP_LISTING,
    crate::app_publish::APP_SUBMIT,
];

/// The WS event every open view of an app reloads on; its payload is
/// `{"appId": "<the app employee's id>"}`.
pub const RELOAD_EVENT: &str = "app_reload";

/// The line every employee reads about an app: the one place its files go.
pub fn location_line(name: &str, ui_path: &str) -> String {
    format!("{name} is an app; its files are served from `{ui_path}` — edit them there.")
}

/// The folder an app row is served from, when the row is an app that has one.
pub fn served_dir(row: &db::models::Agent) -> Option<&str> {
    if row.is_app != Some(1) {
        return None;
    }
    row.app_ui_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
}

/// The location line for a row, when it is an app.
pub fn app_location(row: &db::models::Agent) -> Option<String> {
    served_dir(row).map(|dir| location_line(&row.name, dir))
}

/// Whether an app is the owner's own: made on this bot (a loose package in
/// `user/agents/`, or a database-only row), never one installed from the
/// marketplace (a sealed `.napp`, or a package under `nebo/agents/`). Only
/// the owner's own apps carry developer tooling and build themselves.
pub fn is_own_app(row: &db::models::Agent) -> bool {
    if row.is_app.unwrap_or(0) == 0 {
        return false;
    }
    let installed_root = config::nebo_dir().ok().map(|d| d.join("agents"));
    !row
        .napp_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(Path::new)
        .is_some_and(|p| {
            p.extension().is_some_and(|e| e == "napp")
                || installed_root.as_deref().is_some_and(|root| p.starts_with(root))
        })
}

/// Whether `caller` may use the developer pack on `app`: only ever one of
/// the owner's own apps, and with App Developer mode off only when the
/// caller is that app.
pub fn may_work_on(store: &db::Store, caller: &str, app: &db::models::Agent) -> bool {
    is_own_app(app) && (store.app_developer_mode() || (!caller.is_empty() && app.id == caller))
}

/// The refusal for a call [`may_work_on`] turns down.
pub fn not_yours(app: &db::models::Agent) -> String {
    if !is_own_app(app) {
        return format!(
            "{} was installed from the marketplace: its page is its maker's, so it is not built or published from here.",
            app.name
        );
    }
    format!(
        "App Developer mode is off, so an app works only on itself here and {} is not you. The owner turns the mode \
         on in Settings → Developer to let employees work on each other's apps.",
        app.name
    )
}

/// What an app employee that builds itself reads about the pack.
pub fn self_build_line(row: &db::models::Agent) -> Option<String> {
    is_own_app(row).then(|| {
        "You build and publish yourself. After changing your files, app_reload refreshes every open view of you and \
         app_console shows what you logged; app_status says what is served. When the owner asks to publish you \
         (\"publish yourself\"), draft the listing with app_listing, add screenshots with app_screenshot, show it, \
         and send it with app_submit once they say so."
            .to_string()
    })
}

/// The apps on this bot: every app row with a served folder.
fn apps(store: &db::Store) -> Vec<db::models::Agent> {
    store
        .list_agents(1000, 0)
        .unwrap_or_default()
        .into_iter()
        .filter(|a| served_dir(a).is_some())
        .collect()
}

/// The developer tools a seat is not offered: none when the seat is one of
/// the owner's own apps (it builds itself), or when App Developer mode is
/// on and the seat works on an app (it is one, it reports to one or one
/// reports to it, or it shares a team with one); all of them otherwise.
pub fn withheld(store: &db::Store, agent_id: &str) -> Vec<String> {
    let builds_itself = !agent_id.is_empty()
        && store
            .get_agent(agent_id)
            .ok()
            .flatten()
            .is_some_and(|a| is_own_app(&a));
    if builds_itself || (store.app_developer_mode() && works_on_apps(store, agent_id)) {
        Vec::new()
    } else {
        TOOLS.iter().map(|t| t.to_string()).collect()
    }
}

fn works_on_apps(store: &db::Store, agent_id: &str) -> bool {
    if agent_id.is_empty() {
        return false;
    }
    let all = store.list_agents(1000, 0).unwrap_or_default();
    let app_ids: HashSet<&str> = all
        .iter()
        .filter(|a| served_dir(a).is_some() && is_own_app(a))
        .map(|a| a.id.as_str())
        .collect();
    if app_ids.is_empty() {
        return false;
    }
    if app_ids.contains(agent_id) {
        return true;
    }
    let reports_to_app = all
        .iter()
        .find(|a| a.id == agent_id)
        .and_then(|a| a.reports_to.as_deref())
        .is_some_and(|boss| app_ids.contains(boss));
    let app_reports_here = all
        .iter()
        .any(|a| app_ids.contains(a.id.as_str()) && a.reports_to.as_deref() == Some(agent_id));
    if reports_to_app || app_reports_here {
        return true;
    }
    store.list_teams().unwrap_or_default().iter().any(|t| {
        let local = || t.members.iter().filter(|m| m.is_local());
        local().any(|m| m.agent_id == agent_id)
            && local().any(|m| app_ids.contains(m.agent_id.as_str()))
    })
}

/// Letters and digits only, lowercased: "Kart Racer" and "kart-racer-mobile"
/// meet as "kartracer" and "kartracermobile".
fn compact(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The note a write of a web file (html, js, css) gets when the file is
/// clearly for an app (the app's name is in its folder path) but lies
/// outside the folder the app is served from. `None` for every other write.
pub fn misplaced_web_file_note(store: &db::Store, path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if !matches!(ext.as_str(), "html" | "htm" | "js" | "mjs" | "css") {
        return None;
    }
    let apps = apps(store);
    if apps
        .iter()
        .any(|a| served_dir(a).is_some_and(|dir| path.starts_with(dir)))
    {
        return None;
    }
    let folder = compact(&path.parent()?.to_string_lossy());
    apps.iter().find_map(|a| {
        let name = compact(&a.name);
        (name.chars().count() >= 4 && folder.contains(&name)).then(|| {
            format!(
                "Note: {} is served from `{}`; this file is outside that folder, so the app will not load it.",
                a.name,
                served_dir(a).unwrap_or_default()
            )
        })
    })
}

/// The developer pack, on one store and the bot's WS broadcaster.
pub fn tools(store: Arc<db::Store>, broadcaster: Option<Broadcaster>) -> Vec<Box<dyn DynTool>> {
    let core = Arc::new(AppDev {
        store: store.clone(),
        broadcaster,
    });
    vec![
        Box::new(AppReloadTool(core.clone())),
        Box::new(AppStatusTool(core)),
        Box::new(crate::app_console::AppConsoleTool::new(store)),
    ]
}

struct AppDev {
    store: Arc<db::Store>,
    broadcaster: Option<Broadcaster>,
}

impl AppDev {
    /// The app a call names, or the error the model reads: no such app, an
    /// employee that is not an app, or (App Developer mode off) an app that
    /// is not the caller itself.
    fn app(&self, ctx: &ToolContext, input: &Value) -> Result<db::models::Agent, String> {
        let caller = types::keyparser::extract_agent_id(&ctx.session_key);
        let row = self.named_app(input, &caller)?;
        if may_work_on(&self.store, &caller, &row) {
            Ok(row)
        } else {
            Err(not_yours(&row))
        }
    }

    /// The app `input` names, else the caller itself.
    fn named_app(&self, input: &Value, caller: &str) -> Result<db::models::Agent, String> {
        let label = input
            .get("app")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .unwrap_or(caller);
        let names = || {
            apps(&self.store)
                .into_iter()
                .map(|a| a.name)
                .collect::<Vec<_>>()
                .join(", ")
        };
        match crate::team::resolve_agent(&self.store, label) {
            Some(row) if served_dir(&row).is_some() => Ok(row),
            Some(row) => Err(format!(
                "{} is not an app. Apps here: {}.",
                row.name,
                names()
            )),
            None => Err(format!("No app named '{label}'. Apps here: {}.", names())),
        }
    }
}

fn app_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "app": { "type": "string", "description": "The app employee's name or id; leave it out when you are the app." }
        }
    })
}

fn app_label(input: &Value) -> &str {
    input
        .get("app")
        .and_then(|v| v.as_str())
        .unwrap_or("the app")
}

type Fut<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>>;

// ── app_reload ─────────────────────────────────────────────────────

struct AppReloadTool(Arc<AppDev>);

impl DynTool for AppReloadTool {
    fn name(&self) -> &str {
        APP_RELOAD
    }

    fn description(&self) -> String {
        "Reloads every open view of an app (the owner's phone, a desktop window, a browser tab) so it runs the files as they are now. \
         Use it after changing an app's files; never rename files to get past a stale copy."
            .to_string()
    }

    fn schema(&self) -> Value {
        app_schema()
    }

    fn search_hint(&self) -> &str {
        "reload refresh open app views"
    }

    fn activity(&self, input: &Value) -> String {
        format!("reloading {}", app_label(input))
    }

    fn outcome(&self, input: &Value) -> String {
        format!("Reloaded {}", app_label(input))
    }

    fn execute_dyn<'a>(&'a self, ctx: &'a ToolContext, input: Value) -> Fut<'a> {
        Box::pin(async move {
            let app = match self.0.app(ctx, &input) {
                Ok(a) => a,
                Err(e) => return ToolResult::error(e),
            };
            let Some(broadcast) = self.0.broadcaster.as_ref() else {
                return ToolResult::error("Reloading open views is not available on this bot.");
            };
            broadcast(RELOAD_EVENT, json!({ "appId": app.id }));
            ToolResult::ok(format!("Sent a reload to every open view of {}.", app.name))
        })
    }
}

// ── app_status ─────────────────────────────────────────────────────

struct AppStatusTool(Arc<AppDev>);

impl DynTool for AppStatusTool {
    fn name(&self) -> &str {
        APP_STATUS
    }

    fn description(&self) -> String {
        "Shows an app as it is served: its folder, every file with size and time changed, the entry page and the scripts and \
         styles it loads, which of those are missing, which files the page does not load, and the console error count. \
         Use it first when an edit does not show."
            .to_string()
    }

    fn schema(&self) -> Value {
        app_schema()
    }

    fn search_hint(&self) -> &str {
        "app files entry scripts not showing"
    }

    fn read_only(&self, _input: &Value) -> bool {
        true
    }

    fn activity(&self, input: &Value) -> String {
        format!("checking {}", app_label(input))
    }

    fn outcome(&self, input: &Value) -> String {
        format!("Checked {}", app_label(input))
    }

    fn execute_dyn<'a>(&'a self, ctx: &'a ToolContext, input: Value) -> Fut<'a> {
        Box::pin(async move {
            let app = match self.0.app(ctx, &input) {
                Ok(a) => a,
                Err(e) => return ToolResult::error(e),
            };
            let dir = PathBuf::from(served_dir(&app).unwrap_or_default());
            ToolResult::ok(status_text(
                &app.name,
                &dir,
                crate::app_console::error_count(&app.id),
            ))
        })
    }
}

/// The most files `app_status` lists.
const MAX_FILES: usize = 200;

/// What `app_status` says about the app served from `dir`.
pub fn status_text(name: &str, dir: &Path, errors: usize) -> String {
    let mut out = vec![format!("{name} is served from `{}`.", dir.display())];
    if !dir.is_dir() {
        out.push("That folder does not exist, so the app has nothing to serve.".into());
        return out.join("\n");
    }
    let files: Vec<(String, u64, String)> = walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .take(MAX_FILES + 1)
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            let rel = e
                .path()
                .strip_prefix(dir)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            let changed = meta
                .modified()
                .ok()
                .map(|t| {
                    chrono::DateTime::<chrono::Local>::from(t)
                        .format("%Y-%m-%d %H:%M:%S")
                        .to_string()
                })
                .unwrap_or_default();
            Some((rel, meta.len(), changed))
        })
        .collect();
    out.push(String::new());
    out.push("Files:".into());
    for (rel, size, changed) in files.iter().take(MAX_FILES) {
        out.push(format!("- {rel} ({size} bytes, changed {changed})"));
    }
    if files.len() > MAX_FILES {
        out.push(format!(
            "- ... more than {MAX_FILES} files; the rest are not listed"
        ));
    }

    let entry = ["index.html", "200.html"]
        .into_iter()
        .find(|f| dir.join(f).is_file());
    out.push(String::new());
    let Some(entry) = entry else {
        out.push("Entry page: none. The app needs an index.html in its folder.".into());
        out.push(errors_line(errors));
        return out.join("\n");
    };
    out.push(format!("Entry page: {entry}"));
    let html = std::fs::read_to_string(dir.join(entry)).unwrap_or_default();
    let loads = loaded_refs(&html);
    let mut loaded_files: HashSet<String> = HashSet::new();
    if loads.is_empty() {
        out.push("It loads no scripts or styles by file.".into());
    } else {
        out.push("It loads:".into());
        for r in &loads {
            match local_ref(r) {
                None => out.push(format!("- {r} (outside the app)")),
                Some(rel) if dir.join(&rel).is_file() => {
                    out.push(format!("- {rel}"));
                    loaded_files.insert(rel);
                }
                Some(rel) => out.push(format!(
                    "- {rel} — MISSING: no such file in the app's folder"
                )),
            }
        }
    }
    // Scripts the loaded ones import (ES modules), followed through.
    let (imported, missing_imports) = followed_imports(dir, &loaded_files);
    if !imported.is_empty() {
        out.push(format!(
            "Imported by those scripts: {}",
            imported.join(", ")
        ));
    }
    for (from, spec) in &missing_imports {
        out.push(format!(
            "- {from} imports {spec} — MISSING: no such file in the app's folder"
        ));
    }
    loaded_files.extend(imported);
    let unloaded: Vec<&str> = files
        .iter()
        .take(MAX_FILES)
        .map(|(rel, _, _)| rel.as_str())
        .filter(|rel| {
            let lower = rel.to_ascii_lowercase();
            (lower.ends_with(".js") || lower.ends_with(".mjs") || lower.ends_with(".css"))
                && !loaded_files.contains(*rel)
        })
        .collect();
    if !unloaded.is_empty() {
        out.push(format!(
            "Not loaded by {entry} or anything it imports: {}",
            unloaded.join(", ")
        ));
    }
    out.push(errors_line(errors));
    out.join("\n")
}

fn errors_line(errors: usize) -> String {
    match errors {
        0 => "Console errors in open views: none.".into(),
        n => format!("Console errors in open views: {n} (read them with app_console)."),
    }
}

/// The `src` of every script tag and the `href` of every stylesheet link,
/// in page order.
fn loaded_refs(html: &str) -> Vec<String> {
    static SCRIPT: OnceLock<regex::Regex> = OnceLock::new();
    static LINK: OnceLock<regex::Regex> = OnceLock::new();
    static ATTR: OnceLock<regex::Regex> = OnceLock::new();
    let script = SCRIPT.get_or_init(|| regex::Regex::new(r"(?is)<script\b[^>]*>").unwrap());
    let link = LINK.get_or_init(|| regex::Regex::new(r"(?is)<link\b[^>]*>").unwrap());
    let attr = ATTR.get_or_init(|| {
        regex::Regex::new(r#"(?is)\b(src|href|rel)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#)
            .unwrap()
    });
    let attrs = |tag: &str| -> Vec<(String, String)> {
        attr.captures_iter(tag)
            .map(|c| {
                let v = c
                    .get(2)
                    .or(c.get(3))
                    .or(c.get(4))
                    .map(|m| m.as_str())
                    .unwrap_or("");
                (c[1].to_ascii_lowercase(), v.trim().to_string())
            })
            .collect()
    };
    let mut found: Vec<(usize, String)> = Vec::new();
    for m in script.find_iter(html) {
        if let Some((_, src)) = attrs(m.as_str())
            .into_iter()
            .find(|(k, v)| k == "src" && !v.is_empty())
        {
            found.push((m.start(), src));
        }
    }
    for m in link.find_iter(html) {
        let a = attrs(m.as_str());
        let stylesheet = a
            .iter()
            .any(|(k, v)| k == "rel" && v.to_ascii_lowercase().contains("stylesheet"));
        if let Some((_, href)) = a.into_iter().find(|(k, v)| k == "href" && !v.is_empty()) {
            if stylesheet
                || href
                    .to_ascii_lowercase()
                    .split(['?', '#'])
                    .next()
                    .is_some_and(|p| p.ends_with(".css"))
            {
                found.push((m.start(), href));
            }
        }
    }
    found.sort_by_key(|(at, _)| *at);
    found.into_iter().map(|(_, r)| r).collect()
}

/// The scripts reached from `loaded` by relative `import` / `export ... from`
/// / `import()` specifiers, in the order found, and the `(importer,
/// specifier)` pairs that name no file. Bounded at [`MAX_FILES`] scripts.
fn followed_imports(dir: &Path, loaded: &HashSet<String>) -> (Vec<String>, Vec<(String, String)>) {
    static IMPORT: OnceLock<regex::Regex> = OnceLock::new();
    let import = IMPORT.get_or_init(|| {
        regex::Regex::new(r#"(?:\bimport\s*\(\s*|\bimport\s+(?:[\w*{}\s,$]+?\s+from\s+)?|\bexport\s+[\w*{}\s,$]+?\s+from\s+)["']([^"']+)["']"#)
            .unwrap()
    });
    let mut seen: HashSet<String> = loaded.clone();
    let mut queue: Vec<String> = loaded
        .iter()
        .filter(|r| r.ends_with(".js") || r.ends_with(".mjs"))
        .cloned()
        .collect();
    queue.sort();
    let (mut imported, mut missing) = (Vec::new(), Vec::new());
    while let Some(from) = queue.pop() {
        if seen.len() > MAX_FILES {
            break;
        }
        let Ok(text) = std::fs::read_to_string(dir.join(&from)) else {
            continue;
        };
        let base = Path::new(&from)
            .parent()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        for c in import.captures_iter(&text) {
            let spec = &c[1];
            if !(spec.starts_with("./") || spec.starts_with("../")) {
                continue;
            }
            let Some(rel) = join_inside(&base, spec.split(['?', '#']).next().unwrap_or("")) else {
                continue;
            };
            if seen.contains(&rel) {
                continue;
            }
            if dir.join(&rel).is_file() {
                seen.insert(rel.clone());
                imported.push(rel.clone());
                queue.push(rel);
            } else {
                missing.push((from.clone(), spec.to_string()));
            }
        }
    }
    (imported, missing)
}

/// `base/spec` with `.` and `..` resolved, or `None` when it climbs out of
/// the app's folder.
fn join_inside(base: &str, spec: &str) -> Option<String> {
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
    for seg in spec.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            seg => parts.push(seg),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// A page reference as a path inside the app's folder, or `None` when it
/// points outside the app (another site, a data URL, the bot's root).
fn local_ref(r: &str) -> Option<String> {
    let lower = r.to_ascii_lowercase();
    if lower.contains("://")
        || lower.starts_with("//")
        || lower.starts_with("data:")
        || r.starts_with('/')
    {
        return None;
    }
    let path = r.split(['?', '#']).next().unwrap_or("");
    let path = path.trim_start_matches("./");
    (!path.is_empty() && !path.split('/').any(|seg| seg == "..")).then(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &Path) -> db::Store {
        db::Store::new(&dir.join("t.db").to_string_lossy()).unwrap()
    }

    fn app_row(store: &db::Store, id: &str, name: &str, ui: &Path) {
        store
            .create_agent(
                id,
                Some("user"),
                name,
                "",
                "---\nname: x\n---\n",
                "{}",
                None,
                None,
            )
            .unwrap();
        store
            .set_agent_app_fields(id, true, Some(&ui.to_string_lossy()), None, None)
            .unwrap();
    }

    /// app_status names the entry page's scripts and styles, flags the one
    /// it loads that is missing, and lists the files it does not load.
    #[test]
    fn status_reports_loaded_missing_and_unloaded_files() {
        let tmp = tempfile::tempdir().unwrap();
        let ui = tmp.path().join("ui");
        std::fs::create_dir_all(ui.join("css")).unwrap();
        std::fs::write(
            ui.join("index.html"),
            r#"<html><head><link rel="stylesheet" href="css/style.css?v=2"><script src="https://cdn.example.com/lib.js"></script></head>
               <body><script type="module" src="./main.js"></script><script src='game.js'></script></body></html>"#,
        )
        .unwrap();
        std::fs::write(ui.join("css").join("style.css"), "body{}").unwrap();
        std::fs::create_dir_all(ui.join("js")).unwrap();
        std::fs::write(
            ui.join("main.js"),
            "import { Track } from './js/track.js';\nimport './js/gone.js';\n",
        )
        .unwrap();
        std::fs::write(
            ui.join("js").join("track.js"),
            "export * from '../js/kart.js';",
        )
        .unwrap();
        std::fs::write(ui.join("js").join("kart.js"), "export const Kart = 1;").unwrap();
        std::fs::write(ui.join("main_v4.js"), "console.log(4)").unwrap();

        let text = status_text("Puzzler", &ui, 2);
        assert!(text.contains("Entry page: index.html"), "{text}");
        assert!(text.contains("- css/style.css\n"), "{text}");
        assert!(text.contains("- main.js\n"), "{text}");
        assert!(
            text.contains("- https://cdn.example.com/lib.js (outside the app)"),
            "{text}"
        );
        assert!(text.contains("- game.js — MISSING"), "{text}");
        assert!(
            text.contains("Imported by those scripts: js/track.js, js/kart.js"),
            "{text}"
        );
        assert!(
            text.contains("- main.js imports ./js/gone.js — MISSING"),
            "{text}"
        );
        assert!(
            text.contains("Not loaded by index.html or anything it imports: main_v4.js\n"),
            "{text}"
        );
        assert!(text.contains("main_v4.js (14 bytes"), "{text}");
        assert!(
            text.contains("Console errors in open views: 2 (read them with app_console)."),
            "{text}"
        );
    }

    /// The owner's own app always has the developer pack for itself; App
    /// Developer mode opens it to its teammate, and nobody else ever gets
    /// it. An app installed from the marketplace never does.
    #[test]
    fn an_app_builds_itself_and_the_mode_opens_the_pack_to_its_team() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());
        app_row(&store, "app-1", "Puzzler", &tmp.path().join("ui"));
        store
            .create_agent(
                "dev-1",
                Some("user"),
                "Level Designer",
                "",
                "---\nname: y\n---\n",
                "{}",
                None,
                None,
            )
            .unwrap();
        store
            .create_agent(
                "other",
                Some("user"),
                "Bookkeeper",
                "",
                "---\nname: z\n---\n",
                "{}",
                None,
                None,
            )
            .unwrap();
        let members = [
            db::TeamMember::local("app-1"),
            db::TeamMember::local("dev-1"),
        ];
        store
            .create_team(
                "team-1",
                "Game team",
                "Ships the puzzle game",
                &members,
                "",
                None,
            )
            .unwrap();

        app_row(&store, "app-9", "Bought Game", &tmp.path().join("ui9"));
        store
            .set_agent_napp_path("app-9", "/data/nebo/agents/bought-game.napp")
            .unwrap();
        assert!(withheld(&store, "app-1").is_empty(), "mode off: the app builds itself");
        assert_eq!(withheld(&store, "dev-1").len(), TOOLS.len(), "mode off: not its teammate");
        assert_eq!(withheld(&store, "app-9").len(), TOOLS.len(), "never an installed app");
        store
            .update_settings(
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(true),
            )
            .unwrap();
        assert!(withheld(&store, "app-1").is_empty(), "the app itself");
        assert!(withheld(&store, "dev-1").is_empty(), "its teammate");
        assert_eq!(
            withheld(&store, "other").len(),
            TOOLS.len(),
            "not on the app's team"
        );
        assert_eq!(withheld(&store, "app-9").len(), TOOLS.len(), "never an installed app, mode on");
    }

    /// A web file written for an app outside its served folder gets the
    /// note; the served folder and unrelated files do not.
    #[test]
    fn a_web_file_outside_the_served_folder_gets_the_location_note() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());
        let ui = tmp.path().join("agents").join("Puzzle Master").join("ui");
        app_row(&store, "app-1", "Puzzle Master", &ui);

        let stray = tmp
            .path()
            .join("files")
            .join("puzzle-master-mobile")
            .join("main.js");
        let note = misplaced_web_file_note(&store, &stray).expect("a note");
        assert!(note.contains(&ui.to_string_lossy().to_string()), "{note}");
        assert!(
            misplaced_web_file_note(&store, &ui.join("main.js")).is_none(),
            "inside the served folder"
        );
        assert!(
            misplaced_web_file_note(
                &store,
                &tmp.path()
                    .join("files")
                    .join("puzzle-master-mobile")
                    .join("notes.md")
            )
            .is_none()
        );
        assert!(
            misplaced_web_file_note(
                &store,
                &tmp.path().join("files").join("site").join("main.js")
            )
            .is_none()
        );
    }
}
