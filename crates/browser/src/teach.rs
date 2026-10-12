//! Teach recording, the engine's half of element capture.
//!
//! While a teach session records, the Chrome extension sends one
//! `teach_event` per thing the person did: the element it was done to (role
//! and accessible name in the browser tool's own vocabulary, plus a ranked
//! locator chain), the page, and, as separate `after` events, what the page
//! showed once it settled. Element crops come with their event and are
//! saved next to the keyframes, under `elements/`.
//!
//! At stop, [`build_actions`] turns the raw events into the action list:
//! each action paired with its after-state, double-clicks folded, page
//! changes chained from-to, and desktop-app clicks (from the AT-SPI watcher,
//! `desktop-elements.jsonl`) kept only where the browser saw nothing. The
//! list is written twice: `elements.jsonl` (one action per line, which the
//! timeline merge reads) and `actions.json` (the whole record, for the
//! learner and replay).
//!
//! Redaction is enforced here as well as in the page: an element the page
//! marked sensitive never has a value other than "[hidden]" and never a
//! crop.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use base64::Engine;
use serde_json::{Map, Value, json};

/// The value a sensitive field is recorded as.
pub const HIDDEN: &str = "[hidden]";
/// Two input events this close (ms) are the same physical action.
const SAME_ACTION_MS: i64 = 400;
/// ...and this close on screen (px).
const SAME_ACTION_PX: f64 = 40.0;
/// A submit this soon (ms) after a click or key press is that action's effect.
const SUBMIT_FOLD_MS: i64 = 600;
/// The most crops one session keeps.
const MAX_CROPS: usize = 2000;
/// The largest crop accepted (bytes, decoded).
const MAX_CROP_BYTES: usize = 512 * 1024;

/// One teach session's element capture, from start to stop.
pub struct TeachCapture {
    pub session: String,
    pub dir: PathBuf,
    pub started_ms: u64,
    events: Vec<Value>,
    crops: usize,
}

impl TeachCapture {
    pub fn new(session: String, dir: PathBuf, started_ms: u64) -> Self {
        Self {
            session,
            dir,
            started_ms,
            events: Vec::new(),
            crops: 0,
        }
    }

    /// Take one event from the extension. Its crop is saved as
    /// `elements/NNNN.jpg` and the event names it; a sensitive element's
    /// crop is dropped and its value forced to "[hidden]".
    pub fn add(&mut self, mut event: Value, crop_b64: Option<&str>) {
        if !event.is_object() {
            return;
        }
        let sensitive = redact(&mut event);
        if let Some(b64) = crop_b64.filter(|_| !sensitive && self.crops < MAX_CROPS)
            && let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64)
            && bytes.len() <= MAX_CROP_BYTES
            && bytes.starts_with(&[0xFF, 0xD8])
        {
            self.crops += 1;
            let rel = format!("elements/{:04}.jpg", self.crops);
            let path = self.dir.join(&rel);
            let saved = path
                .parent()
                .is_some_and(|p| std::fs::create_dir_all(p).is_ok())
                && std::fs::write(&path, &bytes).is_ok();
            if saved {
                event["crop"] = Value::String(rel);
            }
        }
        self.events.push(event);
    }

    /// Write `elements.jsonl` and `actions.json` into the session dir, folding
    /// in the desktop watcher's clicks. Returns how many actions were written.
    pub fn finish(self) -> Result<usize, String> {
        let desktop = read_jsonl(&self.dir.join("desktop-elements.jsonl"));
        let actions = build_actions(self.events, desktop, self.started_ms);
        write_actions(&self.dir, &self.session, self.started_ms, &actions)?;
        Ok(actions.len())
    }
}

/// Force the redaction rules onto one event. True when its element is sensitive.
fn redact(event: &mut Value) -> bool {
    let sensitive = event["element"]["sensitive"].as_bool().unwrap_or(false);
    if sensitive {
        if event.get("value").is_some_and(|v| !v.is_null()) {
            event["value"] = Value::String(HIDDEN.into());
        }
        if let Some(el) = event.get_mut("element").and_then(Value::as_object_mut) {
            el.insert("text".into(), Value::String(String::new()));
        }
        if let Some(obj) = event.as_object_mut() {
            obj.remove("crop");
        }
    }
    sensitive
}

fn read_jsonl(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .map(|s| {
            s.lines()
                .filter_map(|l| serde_json::from_str(l.trim()).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn write_actions(
    dir: &Path,
    session: &str,
    started_ms: u64,
    actions: &[Value],
) -> Result<(), String> {
    let mut lines = String::new();
    for a in actions {
        lines.push_str(&serde_json::to_string(a).map_err(|e| e.to_string())?);
        lines.push('\n');
    }
    std::fs::write(dir.join("elements.jsonl"), lines)
        .map_err(|e| format!("elements.jsonl: {e}"))?;
    let doc =
        json!({ "version": 1, "session": session, "started_ms": started_ms, "actions": actions });
    let pretty = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("actions.json"), pretty).map_err(|e| format!("actions.json: {e}"))
}

fn i(v: &Value, k: &str) -> Option<i64> {
    v.get(k)
        .and_then(|x| x.as_i64().or_else(|| x.as_f64().map(|f| f as i64)))
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// The page's own count in an event id (`<page>-<n>`).
fn seq(a: &Value) -> u64 {
    s(a, "id")
        .rsplit('-')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

const POINTER_ACTIONS: &[&str] = &["click", "right_click", "double_click", "check"];

/// The action list from the raw events of one recording. Pure: the unit
/// tests drive it directly.
pub fn build_actions(events: Vec<Value>, desktop: Vec<Value>, started_ms: u64) -> Vec<Value> {
    let started = started_ms as i64;

    // After-states by the action they belong to, and the clicks a
    // double-click replaces.
    let mut afters: HashMap<String, Value> = HashMap::new();
    let mut replaced: HashSet<String> = HashSet::new();
    let mut acts: Vec<Value> = Vec::new();
    for ev in events {
        match s(&ev, "kind") {
            "after" => {
                afters.insert(s(&ev, "of").to_string(), ev);
            }
            "" => {}
            _ => {
                if let Some(r) = ev.get("replaces").and_then(Value::as_array) {
                    replaced.extend(r.iter().filter_map(Value::as_str).map(String::from));
                }
                acts.push(ev);
            }
        }
    }
    acts.retain(|a| !replaced.contains(s(a, "id")));
    // Same-millisecond events keep the order the page made them in (a crop
    // can make an earlier event arrive later).
    acts.sort_by_key(|a| (i(a, "at_ms").unwrap_or(0), seq(a)));

    // A form submitted by the click or key just before it is that step's
    // effect, not a step of its own: the step says it submits the form.
    let mut folded: Vec<Value> = Vec::with_capacity(acts.len());
    for a in acts {
        if s(&a, "kind") == "submit"
            && let Some(prev) = folded.last_mut()
            && matches!(s(prev, "kind"), "click" | "press")
            && i(prev, "tab") == i(&a, "tab")
            && i(&a, "at_ms").unwrap_or(0) - i(prev, "at_ms").unwrap_or(0) <= SUBMIT_FOLD_MS
        {
            let form = s(&a["element"], "name");
            prev["submits"] = Value::String(if form.is_empty() {
                "the form".into()
            } else {
                form.to_string()
            });
            // What the submit led to is what the step led to.
            if let Some(after) = afters.remove(s(&a, "id")) {
                afters.insert(s(prev, "id").to_string(), after);
            }
            continue;
        }
        folded.push(a);
    }
    let acts = folded;

    // Page changes: chain each tab's pages from-to, and drop a repeat of the
    // page the tab is already on.
    let mut page_of_tab: HashMap<i64, String> = HashMap::new();
    let mut keep = Vec::with_capacity(acts.len());
    for mut a in acts {
        let tab = i(&a, "tab").unwrap_or(0);
        let url = s(&a, "url").to_string();
        if s(&a, "kind") == "navigate" {
            match page_of_tab.get(&tab) {
                Some(prev) if *prev == url => continue,
                Some(prev) => a["from"] = Value::String(prev.clone()),
                None => {}
            }
        }
        if !url.is_empty() {
            page_of_tab.insert(tab, url);
        }
        keep.push(a);
    }
    let acts = keep;

    // Each action's after-state: the page it led to when a page change
    // follows before the next action in the tab, else what the page showed
    // once it settled. The next step's element, when it is on that page, is
    // expected to be visible.
    let mut out: Vec<Value> = Vec::with_capacity(acts.len());
    for (n, a) in acts.iter().enumerate() {
        let kind = s(a, "kind");
        let tab = i(a, "tab").unwrap_or(0);
        let mut after = Map::new();
        if kind != "navigate" && kind != "person_check" {
            let later: Vec<&Value> = acts[n + 1..]
                .iter()
                .filter(|b| i(b, "tab").unwrap_or(0) == tab)
                .collect();
            let nav = later
                .iter()
                .take_while(|b| s(b, "kind") == "navigate")
                .last();
            let src = match (nav, afters.get(s(a, "id"))) {
                (Some(nav), _) => Some(*nav),
                (None, Some(af)) => Some(af),
                (None, None) => None,
            };
            if let Some(src) = src {
                for k in ["url", "title", "heading", "appeared"] {
                    if !s(src, k).is_empty() {
                        after.insert(k.into(), Value::String(s(src, k).into()));
                    }
                }
            }
            if let Some(next) = later.iter().find(|b| s(b, "kind") != "navigate")
                && after
                    .get("url")
                    .and_then(Value::as_str)
                    .is_some_and(|u| u == s(next, "url"))
                && !s(&next["element"], "name").is_empty()
            {
                after.insert(
                    "visible".into(),
                    json!({ "role": s(&next["element"], "role"), "name": s(&next["element"], "name") }),
                );
            }
        }
        out.push(action_record(a, started, after));
    }

    // Desktop clicks (AT-SPI): kept where no browser action is the same click.
    for d in desktop {
        let t = i(&d, "t_ms").unwrap_or(0);
        let (x, y) = (d["x"].as_f64(), d["y"].as_f64());
        let same = out.iter().any(|b| {
            POINTER_ACTIONS.contains(&s(b, "action"))
                && (i(b, "t_ms").unwrap_or(i64::MIN) - t).abs() <= SAME_ACTION_MS
                && match (x, y, b["x"].as_f64(), b["y"].as_f64()) {
                    (Some(x), Some(y), Some(bx), Some(by)) => {
                        ((x - bx).powi(2) + (y - by).powi(2)).sqrt() <= SAME_ACTION_PX
                    }
                    _ => true,
                }
        });
        if !same {
            out.push(desktop_record(&d, started));
        }
    }
    out.sort_by_key(|a| i(a, "t_ms").unwrap_or(0));
    out
}

/// One browser action in the shape elements.jsonl and actions.json carry.
fn action_record(a: &Value, started: i64, after: Map<String, Value>) -> Value {
    let at = i(a, "at_ms").unwrap_or(started);
    let el = &a["element"];
    let mut r = Map::new();
    r.insert("t_ms".into(), json!((at - started).max(0)));
    r.insert("at_ms".into(), json!(at));
    if let Some(end) = i(a, "end_ms") {
        r.insert("t_end_ms".into(), json!((end - started).max(0)));
    }
    r.insert("action".into(), json!(s(a, "kind")));
    r.insert("source".into(), json!("browser"));
    r.insert("role".into(), json!(s(el, "role")));
    r.insert("name".into(), json!(s(el, "name")));
    r.insert(
        "locators".into(),
        el.get("locators").cloned().unwrap_or_else(|| json!([])),
    );
    r.insert("url".into(), json!(s(a, "url")));
    r.insert("title".into(), json!(s(a, "title")));
    if !after.is_empty() {
        r.insert("after".into(), Value::Object(after));
    }
    if let Some(screen) = a.get("screen").filter(|v| v.is_object()) {
        r.insert("x".into(), screen["x"].clone());
        r.insert("y".into(), screen["y"].clone());
    }
    for k in [
        "value", "key", "checked", "submits", "crop", "from", "heading", "viewport", "dpr", "tab",
    ] {
        if let Some(v) = a.get(k).filter(|v| !v.is_null()) {
            r.insert(k.into(), v.clone());
        }
    }
    if s(a, "kind") == "person_check" {
        // What check it was, never anything inside it.
        r.insert(
            "note".into(),
            json!("the page asked to confirm a person is using it; the owner completed it"),
        );
    }
    if el.is_object() {
        r.insert("element".into(), el.clone());
    }
    Value::Object(r)
}

/// One desktop-app click from the AT-SPI watcher.
fn desktop_record(d: &Value, started: i64) -> Value {
    let t = i(d, "t_ms").unwrap_or(0);
    let (role, name, app, window) = (s(d, "role"), s(d, "name"), s(d, "app"), s(d, "window"));
    let mut locators = Vec::new();
    if !role.is_empty() && !name.is_empty() {
        locators.push(format!(
            "app={app:?} window={window:?} role={role}[name={name:?}]"
        ));
    }
    if let (Some(x), Some(y)) = (d["x"].as_f64(), d["y"].as_f64()) {
        locators.push(format!(
            "coords=({},{})",
            x.round() as i64,
            y.round() as i64
        ));
    }
    json!({
        "t_ms": t,
        "at_ms": started + t,
        "action": "click",
        "source": "desktop",
        "role": role,
        "name": name,
        "app": app,
        "window": window,
        "locators": locators,
        "x": d["x"],
        "y": d["y"],
        "frame": d["frame"],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_760_000_000_000;

    /// A fresh scratch dir for one test (removed when it drops).
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let p = std::env::temp_dir()
                .join(format!("nebo-teach-{tag}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn click(id: &str, at: u64, role: &str, name: &str, url: &str) -> Value {
        json!({
            "id": id, "kind": "click", "at_ms": at, "tab": 1, "url": url, "title": "Shop",
            "screen": {"x": 400, "y": 300},
            "element": {"role": role, "name": name, "sensitive": false,
                "locators": [format!("role={role}[name=\"{name}\"]"), "coords=(390,210)@1280x800"]},
        })
    }

    #[test]
    fn an_action_takes_the_page_it_led_to_as_its_after_state() {
        let events = vec![
            json!({"id": "n1", "kind": "navigate", "at_ms": T0 + 500, "tab": 1, "url": "https://shop.test/dashboard", "title": "Dashboard"}),
            click(
                "c1",
                T0 + 14_000,
                "link",
                "Orders",
                "https://shop.test/dashboard",
            ),
            json!({"id": "n2", "kind": "navigate", "at_ms": T0 + 14_700, "tab": 1, "url": "https://shop.test/orders", "title": "Orders", "heading": "Orders", "spa": true}),
            click(
                "c2",
                T0 + 16_000,
                "button",
                "New order",
                "https://shop.test/orders",
            ),
            json!({"id": "a2", "kind": "after", "of": "c2", "at_ms": T0 + 17_500, "url": "https://shop.test/orders/new", "title": "New order", "heading": "New order", "appeared": "Customer · Items"}),
        ];
        let acts = build_actions(events, vec![], T0);
        assert_eq!(acts.len(), 4);
        let c1 = &acts[1];
        assert_eq!(c1["t_ms"], 14_000);
        assert_eq!(c1["action"], "click");
        assert_eq!(c1["after"]["url"], "https://shop.test/orders");
        assert_eq!(c1["after"]["heading"], "Orders");
        // The next step's element is on the page it led to: it must show.
        assert_eq!(
            c1["after"]["visible"],
            json!({"role": "button", "name": "New order"})
        );
        assert_eq!(
            acts[2]["from"], "https://shop.test/dashboard",
            "page changes chain from-to"
        );
        assert_eq!(acts[3]["after"]["appeared"], "Customer · Items");
        assert_eq!(
            acts[3]["x"], 400,
            "screen coordinates ride along for the input-log match"
        );
    }

    #[test]
    fn a_submit_right_after_a_click_is_that_clicks_effect() {
        let submit = |at: u64| {
            json!({"id": format!("s{at}"), "kind": "submit", "at_ms": at, "tab": 1, "url": "https://a.test/",
            "element": {"role": "form", "name": "New order", "locators": []}})
        };
        let mut early = submit(T0 + 1000);
        early["id"] = json!("p-8");
        let mut save = click("p-7", T0 + 1000, "button", "Save order", "https://a.test/");
        save["tab"] = json!(1);
        let events = vec![
            // The submit arrived first (the click waited for its crop), same ms.
            early,
            save,
            json!({"id": "x", "kind": "after", "of": "p-8", "at_ms": T0 + 2000, "url": "https://a.test/", "heading": "Order saved"}),
            // A submit with no action just before it stays a step.
            submit(T0 + 9000),
        ];
        let acts = build_actions(events, vec![], T0);
        assert_eq!(acts.len(), 2);
        assert_eq!(acts[0]["submits"], "New order");
        assert_eq!(
            acts[0]["after"]["heading"], "Order saved",
            "the submit's result is the click's"
        );
        assert_eq!(acts[1]["action"], "submit");
    }

    #[test]
    fn a_double_click_replaces_its_two_clicks() {
        let mut dbl = click("d", T0 + 1300, "row", "Invoice 4471", "https://shop.test/");
        dbl["kind"] = json!("double_click");
        dbl["replaces"] = json!(["c1", "c2"]);
        let events = vec![
            click("c1", T0 + 1000, "row", "Invoice 4471", "https://shop.test/"),
            click("c2", T0 + 1200, "row", "Invoice 4471", "https://shop.test/"),
            dbl,
        ];
        let acts = build_actions(events, vec![], T0);
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0]["action"], "double_click");
    }

    #[test]
    fn a_sensitive_field_never_keeps_its_value_or_crop() {
        let dir = Scratch::new("redact");
        let mut cap = TeachCapture::new("s".into(), dir.path().to_path_buf(), T0);
        let jpeg =
            base64::engine::general_purpose::STANDARD.encode([0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3]);
        cap.add(
            json!({"id": "t1", "kind": "type", "at_ms": T0 + 3000, "end_ms": T0 + 5000, "tab": 1,
                "value": "hunter2", "url": "https://shop.test/login",
                "element": {"role": "textbox", "name": "Password", "text": "hunter2", "sensitive": true, "locators": []}}),
            Some(&jpeg),
        );
        cap.add(
            json!({"id": "t2", "kind": "type", "at_ms": T0 + 6000, "end_ms": T0 + 7000, "tab": 1,
                "value": "[pasted text]", "url": "https://shop.test/login",
                "element": {"role": "textbox", "name": "Note", "sensitive": false, "locators": []}}),
            Some(&jpeg),
        );
        assert_eq!(cap.finish().unwrap(), 2);
        let jsonl = std::fs::read_to_string(dir.path().join("elements.jsonl")).unwrap();
        assert!(
            !jsonl.contains("hunter2"),
            "a password must never be written: {jsonl}"
        );
        let lines: Vec<Value> = jsonl
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines[0]["value"], HIDDEN);
        assert_eq!(lines[0]["t_ms"], 3000);
        assert_eq!(lines[0]["t_end_ms"], 5000);
        assert!(
            lines[0].get("crop").is_none(),
            "a sensitive field is never cropped"
        );
        assert_eq!(lines[1]["value"], "[pasted text]");
        assert_eq!(lines[1]["crop"], "elements/0001.jpg");
        assert!(dir.path().join("elements/0001.jpg").exists());
        let doc: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("actions.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(doc["actions"].as_array().unwrap().len(), 2);
        assert_eq!(doc["started_ms"], T0);
    }

    #[test]
    fn a_crop_that_is_not_a_jpeg_is_not_saved() {
        let dir = Scratch::new("png");
        let mut cap = TeachCapture::new("s".into(), dir.path().to_path_buf(), T0);
        let png = base64::engine::general_purpose::STANDARD.encode([0x89, b'P', b'N', b'G']);
        cap.add(
            click("c1", T0 + 10, "button", "Go", "https://a.test/"),
            Some(&png),
        );
        cap.finish().unwrap();
        assert!(!dir.path().join("elements").exists());
    }

    #[test]
    fn desktop_clicks_stay_only_where_the_browser_saw_nothing() {
        let events = vec![click("c1", T0 + 2000, "button", "Save", "https://a.test/")];
        let desktop = vec![
            // The same click, seen by AT-SPI on Chromium's window: dropped.
            json!({"t_ms": 2100, "role": "push button", "name": "Save", "app": "Chromium", "window": "Shop", "x": 410, "y": 305}),
            // A click in the file manager: kept, with its own locator.
            json!({"t_ms": 9000, "role": "push button", "name": "New Folder", "app": "Thunar", "window": "Home", "x": 90, "y": 60, "frame": [80, 50, 30, 20]}),
        ];
        let acts = build_actions(events, desktop, T0);
        assert_eq!(acts.len(), 2);
        assert_eq!(acts[1]["source"], "desktop");
        assert_eq!(
            acts[1]["locators"][0],
            "app=\"Thunar\" window=\"Home\" role=push button[name=\"New Folder\"]"
        );
        assert_eq!(acts[1]["locators"][1], "coords=(90,60)");
    }

    #[test]
    fn a_reload_of_the_same_page_is_not_a_page_change() {
        let events = vec![
            json!({"id": "n1", "kind": "navigate", "at_ms": T0 + 100, "tab": 1, "url": "https://a.test/x"}),
            json!({"id": "n2", "kind": "navigate", "at_ms": T0 + 900, "tab": 1, "url": "https://a.test/x"}),
            json!({"id": "n3", "kind": "navigate", "at_ms": T0 + 1900, "tab": 2, "url": "https://b.test/"}),
        ];
        let acts = build_actions(events, vec![], T0);
        assert_eq!(acts.len(), 2);
        assert!(
            acts[1].get("from").is_none(),
            "another tab's first page has no from"
        );
    }
}
