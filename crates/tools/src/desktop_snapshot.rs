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
}

/// In-memory LRU snapshot store with time-based expiry.
pub struct SnapshotStore {
    snapshots: VecDeque<Snapshot>,
}

impl SnapshotStore {
    pub fn new() -> Self {
        Self {
            snapshots: VecDeque::new(),
        }
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
    if r.contains("button") || r.contains("checkbox") || r.contains("radio") || r.contains("popup") {
        "B"
    } else if r.contains("textfield") || r.contains("textarea") || r.contains("searchfield") || r.contains("combobox") {
        "T"
    } else if r.contains("link") {
        "L"
    } else if r.contains("statictext") || r.contains("heading") || r.contains("label") {
        "S"
    } else if r.contains("image") {
        "I"
    } else if r.contains("group") || r.contains("list") || r.contains("table") || r.contains("outline") {
        "G"
    } else if r.contains("menu") {
        "M"
    } else {
        "X"
    }
}

/// Assign short element IDs (B1, B2, T1, S1, ...) to a list of UI elements.
pub fn assign_element_ids(elements: &mut [UIElement]) {
    use std::collections::HashMap;
    let mut counters: HashMap<&str, usize> = HashMap::new();
    for elem in elements.iter_mut() {
        let prefix = role_prefix(&elem.role);
        let counter = counters.entry(prefix).or_insert(0);
        *counter += 1;
        elem.id = format!("{}{}", prefix, counter);
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

        let actionable = role_prefix(&role) == "B" || role_prefix(&role) == "T" || role_prefix(&role) == "L" || role_prefix(&role) == "M";

        elements.push(UIElement {
            id: String::new(), // assigned later by assign_element_ids
            role,
            label,
            bounds,
            actionable,
            keyboard_shortcut: None,
        });
    }
    elements
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_store_insert_and_retrieve() {
        let mut store = SnapshotStore::new();
        let snap = Snapshot {
            id: "snap_test_001".into(),
            app: Some("Safari".into()),
            created_at: Instant::now(),
            elements: vec![UIElement {
                id: "B1".into(),
                role: "AXButton".into(),
                label: "Submit".into(),
                bounds: Rect { x: 100, y: 200, width: 80, height: 30 },
                actionable: true,
                keyboard_shortcut: None,
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
            elements: vec![],
        });
        store.insert(Snapshot {
            id: "snap_b".into(),
            app: None,
            created_at: Instant::now(),
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
            elements: vec![
                UIElement {
                    id: "B1".into(),
                    role: "AXButton".into(),
                    label: "OK".into(),
                    bounds: Rect { x: 10, y: 20, width: 60, height: 25 },
                    actionable: true,
                    keyboard_shortcut: None,
                },
                UIElement {
                    id: "T1".into(),
                    role: "AXTextField".into(),
                    label: "Name".into(),
                    bounds: Rect { x: 50, y: 100, width: 200, height: 30 },
                    actionable: true,
                    keyboard_shortcut: None,
                },
            ],
        });
        let elem = store.get_element("snap_x", "B1").unwrap();
        assert_eq!(elem.label, "OK");
        assert!(store.get_element("snap_x", "B99").is_none());
    }

    #[test]
    fn test_rect_center() {
        let r = Rect { x: 100, y: 200, width: 80, height: 30 };
        assert_eq!(r.center(), (140, 215));
    }

    #[test]
    fn test_element_id_generation() {
        let mut elements = vec![
            UIElement { id: String::new(), role: "AXButton".into(), label: "OK".into(), bounds: Rect { x: 0, y: 0, width: 0, height: 0 }, actionable: true, keyboard_shortcut: None },
            UIElement { id: String::new(), role: "AXTextField".into(), label: "Name".into(), bounds: Rect { x: 0, y: 0, width: 0, height: 0 }, actionable: true, keyboard_shortcut: None },
            UIElement { id: String::new(), role: "AXButton".into(), label: "Cancel".into(), bounds: Rect { x: 0, y: 0, width: 0, height: 0 }, actionable: true, keyboard_shortcut: None },
            UIElement { id: String::new(), role: "AXStaticText".into(), label: "Help".into(), bounds: Rect { x: 0, y: 0, width: 0, height: 0 }, actionable: false, keyboard_shortcut: None },
        ];
        assign_element_ids(&mut elements);
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
