//! Sliding-window message deduplication.

use std::sync::Mutex;
use std::time::{Duration, Instant};

const WINDOW_SIZE: usize = 1000;
const WINDOW_TTL: Duration = Duration::from_secs(5 * 60);

struct Entry {
    id: [u8; 16],
    seen: Instant,
}

/// Per-conversation sliding window deduplicator.
/// Tracks up to 1000 message IDs or 5 minutes, whichever is reached first.
pub struct DedupWindow {
    inner: Mutex<Vec<Entry>>,
}

impl DedupWindow {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Vec::with_capacity(WINDOW_SIZE)),
        }
    }

    /// Returns true if the msg_id has already been seen.
    /// If not a duplicate, records the ID.
    pub fn is_duplicate(&self, msg_id: [u8; 16]) -> bool {
        let mut entries = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let now = Instant::now();

        // Evict expired entries
        let cutoff = now - WINDOW_TTL;
        entries.retain(|e| e.seen >= cutoff);

        // Check for duplicate
        if entries.iter().any(|e| e.id == msg_id) {
            return true;
        }

        // Evict oldest if at capacity
        if entries.len() >= WINDOW_SIZE {
            entries.remove(0);
        }

        entries.push(Entry { id: msg_id, seen: now });
        false
    }

    /// Current number of tracked IDs.
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for DedupWindow {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_not_duplicate_first_time() {
        let dw = DedupWindow::new();
        assert!(!dw.is_duplicate([1; 16]));
    }

    #[test]
    fn test_duplicate_second_time() {
        let dw = DedupWindow::new();
        assert!(!dw.is_duplicate([1; 16]));
        assert!(dw.is_duplicate([1; 16]));
    }

    #[test]
    fn test_different_ids() {
        let dw = DedupWindow::new();
        assert!(!dw.is_duplicate([1; 16]));
        assert!(!dw.is_duplicate([2; 16]));
        assert_eq!(dw.len(), 2);
    }

    #[test]
    fn test_capacity_eviction() {
        let dw = DedupWindow::new();
        // Fill to capacity + 1
        for i in 0..=WINDOW_SIZE {
            let mut id = [0u8; 16];
            id[0] = (i >> 8) as u8;
            id[1] = i as u8;
            dw.is_duplicate(id);
        }
        assert_eq!(dw.len(), WINDOW_SIZE);
    }
}
