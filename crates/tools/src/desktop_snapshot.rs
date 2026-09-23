use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Maximum number of snapshots in the LRU store.
const MAX_SNAPSHOTS: usize = 25;
/// Snapshots expire after this duration.
const SNAPSHOT_TTL: Duration = Duration::from_secs(600); // 10 minutes

/// Bounding rectangle for a UI element.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rect {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

impl Rect {
    /// Returns the center point (x, y) of this rectangle.
    pub fn center(&self) -> (i64, i64) {
        (self.x + self.width / 2, self.y + self.height / 2)
    }
}

/// A detected UI element from an accessibility tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UIElement {
    /// Short element ID (e.g. "B1", "T2", "S3")
    pub id: String,
    /// Raw accessibility role (e.g. "AXButton", "AXTextField")
    pub role: String,
    /// Human-readable label/name
    pub label: String,
    /// Bounding rectangle in screen coordinates
    pub bounds: Rect,
    /// Whether this element is interactive (clickable/typeable)
    pub actionable: bool,
    /// Keyboard shortcut if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyboard_shortcut: Option<String>,
    /// Accessibility actions the element accepts (`AXPress`, `AXSetValue`, …).
    /// Empty when the walk could not read them (the AppleScript fallback).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
    /// Child-index path from the window, valid for the walk that produced it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    /// Keyboard focus was on this element when the walk ran.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub focused: bool,
    /// The element's current value (a field's contents, a slider's number).
    /// Shown beside the label, never part of the label: a value changes when
    /// typed into and must not change the element's ref. Secure fields never
    /// carry one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// A snapshot combining a screenshot with detected UI elements.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Unique snapshot ID (e.g. "snap_1711900000000_a3f2")
    pub id: String,
    /// Target application name (if any)
    pub app: Option<String>,
    /// When this snapshot was created
    pub created_at: Instant,
    /// Detected UI elements with IDs
    pub elements: Vec<UIElement>,
    /// What the image covers, in screen points: the window's frame, or the
    /// whole screen. `None` only when no frame could be read at all.
    pub frame: Option<Rect>,
    /// Screen points per image pixel (1.0 when the image was not downscaled).
    pub scale: f64,
    /// Which layer produced the elements: `ax`, `ax-shallow`, or `vision`.
    pub via: String,
}

/// Where a pixel of the image the model saw lands on the screen now. The
/// window may have moved since the image was taken (its origin is re-read);
/// a window that changed SIZE invalidates the image, so that is refused.
pub fn image_to_screen(px: (i64, i64), then: &Rect, now: &Rect, scale: f64) -> Result<(i64, i64), String> {
    if (then.width, then.height) != (now.width, now.height) {
        return Err(format!(
            "the window changed size since the last capture ({}×{} → {}×{}); capture it again before acting",
            then.width, then.height, now.width, now.height
        ));
    }
    Ok((
        now.x + (px.0 as f64 * scale).round() as i64,
        now.y + (px.1 as f64 * scale).round() as i64,
    ))
}

/// A screen point as a pixel of the image taken with `frame`/`scale`.
pub fn screen_to_image(pt: (i64, i64), frame: &Rect, scale: f64) -> (i64, i64) {
    (
        ((pt.0 - frame.x) as f64 / scale).round() as i64,
        ((pt.1 - frame.y) as f64 / scale).round() as i64,
    )
}

/// A screen point recorded at capture time, moved with the window.
pub fn screen_point_now(pt: (i64, i64), then: &Rect, now: &Rect) -> Result<(i64, i64), String> {
    if (then.width, then.height) != (now.width, now.height) {
        return Err(format!(
            "the window changed size since the last capture ({}×{} → {}×{}); capture it again before acting",
            then.width, then.height, now.width, now.height
        ));
    }
    Ok((pt.0 + now.x - then.x, pt.1 + now.y - then.y))
}

/// Where a dragged element ended up, from the capture taken after the drop.
/// Refs are reassigned per capture, so the element is found by its label
/// (stated as such); `end` is the drop point in image pixels.
pub fn drop_report(label: &str, end: (i64, i64), after: &Snapshot) -> String {
    if label.is_empty() {
        return String::new();
    }
    let Some(frame) = after.frame.as_ref() else { return String::new() };
    let found: Vec<(&UIElement, (i64, i64))> = after
        .elements
        .iter()
        .filter(|e| e.label == label)
        .map(|e| (e, screen_to_image(e.bounds.center(), frame, after.scale)))
        .collect();
    if found.is_empty() {
        return format!("No element labelled \"{label}\" is visible after the drop; the drag may not have landed.");
    }
    let (e, (cx, cy)) = found
        .iter()
        .min_by_key(|(_, (cx, cy))| (cx - end.0).abs() + (cy - end.1).abs())
        .unwrap();
    let dist = (((cx - end.0).pow(2) + (cy - end.1).pow(2)) as f64).sqrt().round() as i64;
    if dist <= 40 {
        format!("{} \"{label}\" is now at ({cx},{cy}), at the drop point (matched by label).", e.id)
    } else {
        format!("{} \"{label}\" is now at ({cx},{cy}), {dist} px from the drop point ({},{}); the drop may not have landed where intended (matched by label).", e.id, end.0, end.1)
    }
}

/// One sentence on what changed between two captures of the same target,
/// computed from what was measured — never a guess at what the action meant.
pub fn delta_line(before: &Snapshot, after: &Snapshot) -> String {
    let mut parts = Vec::new();
    match (&before.frame, &after.frame) {
        (Some(b), Some(a)) if (b.width, b.height) != (a.width, a.height) => {
            parts.push(format!("The window resized to {}×{}.", a.width, a.height))
        }
        (Some(b), Some(a)) if (b.x, b.y) != (a.x, a.y) => {
            parts.push(format!("The window moved to {},{}.", a.x, a.y))
        }
        _ => {}
    }
    let focus = |s: &Snapshot| s.elements.iter().find(|e| e.focused).map(|e| (e.id.clone(), e.label.clone()));
    let (fb, fa) = (focus(before), focus(after));
    if fa != fb {
        if let Some((id, label)) = &fa {
            parts.push(format!("Focus is now on {id} \"{label}\"."));
        }
    }
    let (nb, na) = (before.elements.len(), after.elements.len());
    if na != nb {
        parts.push(format!("{} elements now ({} before).", na, nb));
    }
    if parts.is_empty() {
        format!("Window unchanged: same frame, same {nb} elements.")
    } else {
        parts.join(" ")
    }
}

/// In-memory LRU snapshot store with time-based expiry.
pub struct SnapshotStore {
    snapshots: VecDeque<Snapshot>,
    // ponytail: one book per app for the process's life; a few hundred
    // labels per app at most. Clear it if that ever shows up in memory.
    books: std::collections::HashMap<String, RefBook>,
}

impl SnapshotStore {
    pub fn new() -> Self {
        Self {
            snapshots: VecDeque::new(),
            books: std::collections::HashMap::new(),
        }
    }

    /// The ref book for `app` (case-insensitive; "" is the whole screen).
    pub fn book(&mut self, app: &str) -> &mut RefBook {
        self.books.entry(app.to_lowercase()).or_default()
    }

    /// Insert a snapshot and return its ID. Evicts expired and over-capacity entries.
    pub fn insert(&mut self, snapshot: Snapshot) -> String {
        self.cleanup();
        // LRU eviction
        while self.snapshots.len() >= MAX_SNAPSHOTS {
            self.snapshots.pop_front();
        }
        let id = snapshot.id.clone();
        self.snapshots.push_back(snapshot);
        id
    }

    /// Get a snapshot by ID (returns None if expired or not found).
    pub fn get(&self, id: &str) -> Option<&Snapshot> {
        self.snapshots
            .iter()
            .rev()
            .find(|s| s.id == id && s.created_at.elapsed() < SNAPSHOT_TTL)
    }

    /// Look up an element within a specific snapshot.
    pub fn get_element(&self, snapshot_id: &str, element_id: &str) -> Option<&UIElement> {
        self.get(snapshot_id)
            .and_then(|snap| snap.elements.iter().find(|e| e.id == element_id))
    }

    /// The most recent non-expired snapshot of `app` (case-insensitive).
    pub fn latest_for(&self, app: &str) -> Option<&Snapshot> {
        self.snapshots.iter().rev().find(|s| {
            s.created_at.elapsed() < SNAPSHOT_TTL
                && s.app.as_deref().map_or(false, |a| a.eq_ignore_ascii_case(app))
        })
    }

    /// Get the most recent non-expired snapshot.
    pub fn latest(&self) -> Option<&Snapshot> {
        self.snapshots
            .iter()
            .rev()
            .find(|s| s.created_at.elapsed() < SNAPSHOT_TTL)
    }

    /// Remove expired snapshots.
    fn cleanup(&mut self) {
        self.snapshots
            .retain(|s| s.created_at.elapsed() < SNAPSHOT_TTL);
    }
}

/// Generate a snapshot ID from the current timestamp.
pub fn generate_snapshot_id() -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let rand: u16 = (ts as u16) ^ (std::process::id() as u16);
    format!("snap_{}_{:04x}", ts, rand)
}

/// Map an accessibility role to a short prefix for element IDs.
fn role_prefix(role: &str) -> &'static str {
    let r = role.to_lowercase();
    if r.contains("button") || r.contains("checkbox") || r.contains("radio") || r.contains("popup")
    {
        "B"
    } else if r.contains("textfield")
        || r.contains("textarea")
        || r.contains("searchfield")
        || r.contains("combobox")
    {
        "T"
    } else if r.contains("link") {
        "L"
    } else if r.contains("statictext") || r.contains("heading") || r.contains("label") || r.contains("ocrtext") {
        "S"
    } else if r.contains("image") {
        "I"
    } else if r.contains("group")
        || r.contains("list")
        || r.contains("table")
        || r.contains("outline")
    {
        "G"
    } else if r.contains("menu") {
        "M"
    } else {
        "X"
    }
}

/// Assign short element IDs (B1, B2, T1, S1, ...) to a list of UI elements.
/// The refs one app's elements have been given so far. A labelled element
/// keeps its ref across captures ("Continue with email" is B6 on every
/// screen it appears on), and a number once used is never handed to a
/// different element. Renumbering per capture made a remembered "B6" press
/// "Save Screen", then the Watch app, in the Simulator (2026-09-22): the model
/// carried refs forward and the tool, correctly, pressed whatever B6 now was.
/// A stale ref is now either the same element or absent, and absent is
/// refused before anything moves.
#[derive(Default)]
pub struct RefBook {
    ids: std::collections::HashMap<String, String>,
    high: std::collections::HashMap<&'static str, usize>,
}

/// Give every element a ref: from `book` when this app has shown the same
/// role and label before, otherwise the next unused number for its role.
/// Unlabelled elements always get a fresh number. Pass a fresh book for
/// refs that start at 1.
pub fn assign_element_ids(elements: &mut [UIElement], book: &mut RefBook) {
    let mut taken = std::collections::HashSet::new();
    for elem in elements.iter_mut() {
        let prefix = role_prefix(&elem.role);
        let key = (!elem.label.is_empty()).then(|| format!("{prefix}|{}", elem.label));
        let known = key.as_ref().and_then(|k| book.ids.get(k)).filter(|id| !taken.contains(*id)).cloned();
        elem.id = known.unwrap_or_else(|| {
            let n = book.high.entry(prefix).or_insert(0);
            *n += 1;
            let id = format!("{prefix}{n}");
            if let Some(k) = &key {
                book.ids.entry(k.clone()).or_insert_with(|| id.clone());
            }
            id
        });
        taken.insert(elem.id.clone());
    }
}

/// Parse macOS AppleScript AX tree output into UIElements.
///
/// Expected format per line: `role||label||x,y,w,h`
/// Lines that don't match are skipped.
pub fn parse_ax_output(output: &str) -> Vec<UIElement> {
    let mut elements = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.splitn(3, "||").collect();
        if parts.len() < 3 {
            continue;
        }
        let role = parts[0].trim().to_string();
        let label = parts[1].trim().to_string();
        let bounds_str = parts[2].trim();

        let coords: Vec<i64> = bounds_str
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        let bounds = if coords.len() == 4 {
            Rect {
                x: coords[0],
                y: coords[1],
                width: coords[2],
                height: coords[3],
            }
        } else {
            Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            }
        };

        let actionable = role_prefix(&role) == "B"
            || role_prefix(&role) == "T"
            || role_prefix(&role) == "L"
            || role_prefix(&role) == "M";

        elements.push(UIElement {
            id: String::new(), // assigned later by assign_element_ids
            role,
            label,
            bounds,
            actionable,
            keyboard_shortcut: None,
            actions: Vec::new(),
            path: String::new(),
            focused: false,
            value: None,
        });
    }
    elements
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i64, y: i64, w: i64, h: i64) -> Rect {
        Rect { x, y, width: w, height: h }
    }

    // The model addresses pixels of the image it saw; the window may have
    // moved since, so the origin is re-read. A downscaled image (scale 2)
    // means one image pixel is two screen points.
    #[test]
    fn image_pixels_land_on_the_moved_window() {
        let then = rect(100, 50, 2000, 1000);
        let now = rect(300, 80, 2000, 1000);
        assert_eq!(image_to_screen((10, 20), &then, &now, 2.0), Ok((320, 120)));
        assert_eq!(screen_to_image((320, 120), &now, 2.0), (10, 20));
        assert_eq!(screen_point_now((110, 70), &then, &now), Ok((310, 100)));
    }

    #[test]
    fn a_resized_window_refuses_stale_coordinates() {
        let then = rect(0, 0, 800, 600);
        let now = rect(0, 0, 900, 600);
        let err = image_to_screen((1, 1), &then, &now, 1.0).unwrap_err();
        assert!(err.contains("800×600 → 900×600"), "{err}");
        assert!(screen_point_now((1, 1), &then, &now).is_err());
    }

    fn snap(frame: Rect, n: usize, focused: Option<usize>) -> Snapshot {
        let elements = (0..n)
            .map(|i| UIElement {
                id: format!("B{}", i + 1),
                role: "AXButton".into(),
                label: format!("b{i}"),
                bounds: rect(0, 0, 1, 1),
                actionable: true,
                keyboard_shortcut: None,
                actions: vec![],
                path: String::new(),
                focused: focused == Some(i),
                value: None,
            })
            .collect();
        Snapshot {
            id: "s".into(),
            app: None,
            created_at: Instant::now(),
            frame: Some(frame),
            scale: 1.0,
            via: "ax".into(),
            elements,
        }
    }

    /// A labelled element keeps its ref from capture to capture, and a ref
    /// that meant one element is never given to another.
    #[test]
    fn refs_are_stable_per_label_and_never_reused() {
        let el = |role: &str, label: &str| UIElement {
            id: String::new(),
            role: role.into(),
            label: label.into(),
            bounds: rect(0, 0, 10, 10),
            actionable: true,
            keyboard_shortcut: None,
            actions: vec![],
            path: String::new(),
            focused: false,
            value: None,
        };
        let mut book = RefBook::default();
        let mut first = vec![el("AXButton", "Home"), el("AXButton", "Continue with email")];
        assign_element_ids(&mut first, &mut book);
        assert_eq!((first[0].id.as_str(), first[1].id.as_str()), ("B1", "B2"));
        // Next screen: the Continue button is gone, other buttons arrive.
        let mut second = vec![el("AXButton", "Watch"), el("AXButton", "Home"), el("AXButton", "")];
        assign_element_ids(&mut second, &mut book);
        assert_eq!(second[1].id, "B1", "Home keeps its ref");
        assert!(second.iter().all(|e| e.id != "B2"), "B2 still means Continue: {:?}", second.iter().map(|e| &e.id).collect::<Vec<_>>());
        // Continue comes back: same ref as before.
        let mut third = vec![el("AXButton", "Continue with email")];
        assign_element_ids(&mut third, &mut book);
        assert_eq!(third[0].id, "B2");
        // Two elements with one label: the second gets its own ref.
        let mut dup = vec![el("AXButton", "Home"), el("AXButton", "Home")];
        assign_element_ids(&mut dup, &mut book);
        assert_ne!(dup[0].id, dup[1].id);
    }

    #[test]
    fn drop_report_finds_the_dragged_label_and_measures_the_miss() {
        let mut after = snap(rect(100, 100, 400, 400), 2, None);
        after.elements[0].label = "Report.pdf".into();
        after.elements[0].bounds = rect(300, 300, 20, 20); // centre (310,310) screen = (210,210) image
        after.elements[1].label = "Other".into();
        assert!(drop_report("", (0, 0), &after).is_empty());
        assert_eq!(
            drop_report("Report.pdf", (215, 205), &after),
            "B1 \"Report.pdf\" is now at (210,210), at the drop point (matched by label)."
        );
        let miss = drop_report("Report.pdf", (10, 10), &after);
        assert!(miss.contains("283 px from the drop point"), "{miss}");
        assert!(drop_report("Missing", (0, 0), &after).starts_with("No element labelled \"Missing\""));
    }

    #[test]
    fn delta_line_reports_only_what_was_measured() {
        let a = snap(rect(0, 0, 100, 100), 3, None);
        assert_eq!(delta_line(&a, &a), "Window unchanged: same frame, same 3 elements.");
        let moved = snap(rect(5, 5, 100, 100), 3, None);
        assert_eq!(delta_line(&a, &moved), "The window moved to 5,5.");
        let resized = snap(rect(0, 0, 120, 100), 3, None);
        assert_eq!(delta_line(&a, &resized), "The window resized to 120×100.");
        let sheet = snap(rect(0, 0, 100, 100), 7, Some(1));
        assert_eq!(delta_line(&a, &sheet), "Focus is now on B2 \"b1\". 7 elements now (3 before).");
    }

    #[test]
    fn test_snapshot_store_insert_and_retrieve() {
        let mut store = SnapshotStore::new();
        let snap = Snapshot {
            id: "snap_test_001".into(),
            app: Some("Safari".into()),
            created_at: Instant::now(),
            frame: None,
            scale: 1.0,
            via: String::new(),
            elements: vec![UIElement {
                id: "B1".into(),
                role: "AXButton".into(),
                label: "Submit".into(),
                bounds: Rect {
                    x: 100,
                    y: 200,
                    width: 80,
                    height: 30,
                },
                actionable: true,
                keyboard_shortcut: None,
                actions: vec![],
                path: String::new(),
                focused: false,
                value: None,
            }],
        };
        store.insert(snap);
        assert!(store.get("snap_test_001").is_some());
        assert!(store.get("nonexistent").is_none());
    }

    #[test]
    fn test_snapshot_store_latest() {
        let mut store = SnapshotStore::new();
        store.insert(Snapshot {
            id: "snap_a".into(),
            app: None,
            created_at: Instant::now(),
            frame: None,
            scale: 1.0,
            via: String::new(),
            elements: vec![],
        });
        store.insert(Snapshot {
            id: "snap_b".into(),
            app: None,
            created_at: Instant::now(),
            frame: None,
            scale: 1.0,
            via: String::new(),
            elements: vec![],
        });
        assert_eq!(store.latest().unwrap().id, "snap_b");
    }

    #[test]
    fn test_snapshot_store_lru_eviction() {
        let mut store = SnapshotStore::new();
        for i in 0..30 {
            store.insert(Snapshot {
                id: format!("snap_{i}"),
                app: None,
                created_at: Instant::now(),
                frame: None,
                scale: 1.0,
                via: String::new(),
                elements: vec![],
            });
        }
        // oldest should be evicted
        assert!(store.get("snap_0").is_none());
        assert!(store.get("snap_29").is_some());
        assert!(store.snapshots.len() <= MAX_SNAPSHOTS);
    }

    #[test]
    fn test_get_element() {
        let mut store = SnapshotStore::new();
        store.insert(Snapshot {
            id: "snap_x".into(),
            app: None,
            created_at: Instant::now(),
            frame: None,
            scale: 1.0,
            via: String::new(),
            elements: vec![
                UIElement {
                    id: "B1".into(),
                    role: "AXButton".into(),
                    label: "OK".into(),
                    bounds: Rect {
                        x: 10,
                        y: 20,
                        width: 60,
                        height: 25,
                    },
                    actionable: true,
                    keyboard_shortcut: None,
                    actions: vec![],
                    path: String::new(),
                    focused: false,
                    value: None,
                },
                UIElement {
                    id: "T1".into(),
                    role: "AXTextField".into(),
                    label: "Name".into(),
                    bounds: Rect {
                        x: 50,
                        y: 100,
                        width: 200,
                        height: 30,
                    },
                    actionable: true,
                    keyboard_shortcut: None,
                    actions: vec![],
                    path: String::new(),
                    focused: false,
                    value: None,
                },
            ],
        });
        let elem = store.get_element("snap_x", "B1").unwrap();
        assert_eq!(elem.label, "OK");
        assert!(store.get_element("snap_x", "B99").is_none());
    }

    #[test]
    fn test_rect_center() {
        let r = Rect {
            x: 100,
            y: 200,
            width: 80,
            height: 30,
        };
        assert_eq!(r.center(), (140, 215));
    }

    #[test]
    fn test_element_id_generation() {
        let mut elements = vec![
            UIElement {
                id: String::new(),
                role: "AXButton".into(),
                label: "OK".into(),
                bounds: Rect {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                },
                actionable: true,
                keyboard_shortcut: None,
                actions: vec![],
                path: String::new(),
                focused: false,
                value: None,
            },
            UIElement {
                id: String::new(),
                role: "AXTextField".into(),
                label: "Name".into(),
                bounds: Rect {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                },
                actionable: true,
                keyboard_shortcut: None,
                actions: vec![],
                path: String::new(),
                focused: false,
                value: None,
            },
            UIElement {
                id: String::new(),
                role: "AXButton".into(),
                label: "Cancel".into(),
                bounds: Rect {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                },
                actionable: true,
                keyboard_shortcut: None,
                actions: vec![],
                path: String::new(),
                focused: false,
                value: None,
            },
            UIElement {
                id: String::new(),
                role: "AXStaticText".into(),
                label: "Help".into(),
                bounds: Rect {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                },
                actionable: false,
                keyboard_shortcut: None,
                actions: vec![],
                path: String::new(),
                focused: false,
                value: None,
            },
        ];
        assign_element_ids(&mut elements, &mut RefBook::default());
        assert_eq!(elements[0].id, "B1");
        assert_eq!(elements[1].id, "T1");
        assert_eq!(elements[2].id, "B2");
        assert_eq!(elements[3].id, "S1");
    }

    #[test]
    fn test_parse_ax_output() {
        let output = "AXButton||Submit||100,200,80,30\nAXTextField||Name||50,100,200,30\n";
        let elems = parse_ax_output(output);
        assert_eq!(elems.len(), 2);
        assert_eq!(elems[0].role, "AXButton");
        assert_eq!(elems[0].label, "Submit");
        assert_eq!(elems[0].bounds.x, 100);
        assert_eq!(elems[1].role, "AXTextField");
        assert_eq!(elems[1].label, "Name");
    }

    #[test]
    fn test_parse_ax_output_malformed() {
        let output = "malformed line\nAXButton||OK||10,20,30,40\n\n";
        let elems = parse_ax_output(output);
        assert_eq!(elems.len(), 1);
        assert_eq!(elems[0].label, "OK");
    }

    #[test]
    fn test_generate_snapshot_id() {
        let id = generate_snapshot_id();
        assert!(id.starts_with("snap_"));
        assert!(id.len() > 15);
    }
}
