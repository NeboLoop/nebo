use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Instant;

/// An element annotated with a role-based ID (e.g. B1, T2, L3).
#[derive(Debug, Clone)]
pub struct AnnotatedElement {
    pub id: String,
    pub role: String,
    pub label: String,
    pub bounds: Option<(i32, i32, i32, i32)>,
    pub actionable: bool,
    pub selector: String,
}

/// A cached accessibility snapshot with annotated elements.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub id: String,
    pub created_at: Instant,
    pub app: String,
    pub window_title: String,
    pub annotated_text: String,
    pub elements: Vec<AnnotatedElement>,
    pub raw_image: Option<Vec<u8>>,
}

/// TTL-based in-memory snapshot cache. Stored in AppState (Rule 8.4: no global state).
/// Uses std::sync::RwLock — read-heavy, never held across .await.
pub struct SnapshotStore {
    snapshots: RwLock<HashMap<String, Snapshot>>,
    ttl_secs: u64,
}

impl SnapshotStore {
    pub fn new() -> Self {
        Self {
            snapshots: RwLock::new(HashMap::new()),
            ttl_secs: 3600, // 1 hour
        }
    }

    /// Store a snapshot.
    pub fn put(&self, snapshot: Snapshot) {
        let mut store = self.snapshots.write().unwrap();
        store.insert(snapshot.id.clone(), snapshot);
    }

    /// Get a snapshot by ID.
    pub fn get(&self, id: &str) -> Option<Snapshot> {
        let store = self.snapshots.read().unwrap();
        store.get(id).cloned()
    }

    /// Get the most recently created snapshot.
    pub fn latest(&self) -> Option<Snapshot> {
        let store = self.snapshots.read().unwrap();
        store.values().max_by_key(|s| s.created_at).cloned()
    }

    /// Look up an element by its role-based ID (e.g. "B1") in a specific snapshot.
    pub fn lookup_element(&self, snapshot_id: &str, element_id: &str) -> Option<AnnotatedElement> {
        let store = self.snapshots.read().unwrap();
        let snapshot = store.get(snapshot_id)?;
        snapshot
            .elements
            .iter()
            .find(|e| e.id == element_id)
            .cloned()
    }

    /// Look up an element by its role-based ID in the latest snapshot.
    pub fn lookup_element_latest(&self, element_id: &str) -> Option<AnnotatedElement> {
        let store = self.snapshots.read().unwrap();
        let latest = store.values().max_by_key(|s| s.created_at)?;
        latest
            .elements
            .iter()
            .find(|e| e.id == element_id)
            .cloned()
    }

    /// Remove snapshots older than the TTL.
    pub fn cleanup(&self) {
        let mut store = self.snapshots.write().unwrap();
        let now = Instant::now();
        store.retain(|_, s| now.duration_since(s.created_at).as_secs() < self.ttl_secs);
    }

    /// Number of cached snapshots.
    pub fn len(&self) -> usize {
        self.snapshots.read().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_snapshot(id: &str, elements: Vec<AnnotatedElement>) -> Snapshot {
        Snapshot {
            id: id.to_string(),
            created_at: Instant::now(),
            app: "TestApp".to_string(),
            window_title: "Test Window".to_string(),
            annotated_text: "test snapshot text".to_string(),
            elements,
            raw_image: None,
        }
    }

    fn make_element(id: &str, role: &str, label: &str) -> AnnotatedElement {
        AnnotatedElement {
            id: id.to_string(),
            role: role.to_string(),
            label: label.to_string(),
            bounds: None,
            actionable: true,
            selector: format!("role={}[name=\"{}\"]", role, label),
        }
    }

    #[test]
    fn test_put_and_get() {
        let store = SnapshotStore::new();
        let snap = make_snapshot("s1", vec![make_element("B1", "button", "Submit")]);
        store.put(snap);
        assert!(store.get("s1").is_some());
        assert!(store.get("nonexistent").is_none());
    }

    #[test]
    fn test_latest() {
        let store = SnapshotStore::new();
        store.put(make_snapshot("s1", vec![]));
        store.put(make_snapshot("s2", vec![]));
        let latest = store.latest().unwrap();
        // Both have similar timestamps, either is valid
        assert!(latest.id == "s1" || latest.id == "s2");
    }

    #[test]
    fn test_lookup_element() {
        let store = SnapshotStore::new();
        let elements = vec![
            make_element("B1", "button", "Submit"),
            make_element("T1", "textbox", "Name"),
            make_element("L1", "link", "Home"),
        ];
        store.put(make_snapshot("s1", elements));

        let elem = store.lookup_element("s1", "B1").unwrap();
        assert_eq!(elem.role, "button");
        assert_eq!(elem.label, "Submit");

        let elem = store.lookup_element("s1", "T1").unwrap();
        assert_eq!(elem.role, "textbox");

        assert!(store.lookup_element("s1", "Z99").is_none());
        assert!(store.lookup_element("nonexistent", "B1").is_none());
    }

    #[test]
    fn test_cleanup_preserves_recent() {
        let store = SnapshotStore::new();
        store.put(make_snapshot("s1", vec![]));
        store.cleanup();
        assert_eq!(store.len(), 1); // recent snapshot preserved
    }
}
