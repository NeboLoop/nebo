//! Monotonic ULID generator (16 bytes each).
//!
//! Layout (Crockford ULID spec):
//! - [0-5]   48-bit Unix millisecond timestamp (big-endian)
//! - [6-15]  80-bit random, monotonically incrementing within same ms

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Thread-safe monotonic ULID generator.
pub struct UlidGen {
    inner: Mutex<[u8; 16]>,
}

impl UlidGen {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new([0u8; 16]),
        }
    }

    /// Generate the next monotonic ULID.
    pub fn next(&self) -> [u8; 16] {
        let mut last = self.inner.lock().unwrap_or_else(|p| p.into_inner());

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let mut id = [0u8; 16];
        // Timestamp: 6 bytes big-endian
        id[0] = (now >> 40) as u8;
        id[1] = (now >> 32) as u8;
        id[2] = (now >> 24) as u8;
        id[3] = (now >> 16) as u8;
        id[4] = (now >> 8) as u8;
        id[5] = now as u8;

        // Check if same millisecond
        let same_ms = id[..6] == last[..6];

        if same_ms {
            // Copy previous random part and increment
            id[6..].copy_from_slice(&last[6..]);
            for i in (6..16).rev() {
                id[i] = id[i].wrapping_add(1);
                if id[i] != 0 {
                    break;
                }
            }
        } else {
            // New millisecond: fresh random bytes
            getrandom::getrandom(&mut id[6..]).expect("getrandom failed");
        }

        *last = id;
        id
    }
}

impl Default for UlidGen {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract the millisecond timestamp from a ULID.
pub fn timestamp_ms(id: &[u8; 16]) -> u64 {
    (id[0] as u64) << 40
        | (id[1] as u64) << 32
        | (id[2] as u64) << 24
        | (id[3] as u64) << 16
        | (id[4] as u64) << 8
        | (id[5] as u64)
}

/// Extract a u64 from the first 8 bytes for comparison/sorting.
pub fn to_u64(id: &[u8; 16]) -> u64 {
    u64::from_be_bytes([id[0], id[1], id[2], id[3], id[4], id[5], id[6], id[7]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_monotonic() {
        let ulid_gen = UlidGen::new();
        let a = ulid_gen.next();
        let b = ulid_gen.next();
        let c = ulid_gen.next();
        assert!(to_u64(&a) <= to_u64(&b));
        assert!(to_u64(&b) <= to_u64(&c));
        // All unique
        assert_ne!(a, b);
        assert_ne!(b, c);
    }

    #[test]
    fn test_timestamp() {
        let ulid_gen = UlidGen::new();
        let id = ulid_gen.next();
        let ts = timestamp_ms(&id);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        // Within 1 second
        assert!(now.abs_diff(ts) < 1000);
    }
}
