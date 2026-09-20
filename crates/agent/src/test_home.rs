//! One temporary Nebo root for tests that write where the product writes.
//!
//! Tests share the process environment, so `NEBO_HOME` is mutated under a
//! single lock here rather than in each module that needs a root of its own —
//! two locks would not serialize against each other.

use std::path::Path;

pub(crate) fn with_home<T>(f: impl FnOnce(&Path) -> T) -> T {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    // SAFETY: the lock above serializes env mutation across these tests.
    unsafe { std::env::set_var("NEBO_HOME", home.path()) };
    let out = f(home.path());
    unsafe { std::env::remove_var("NEBO_HOME") };
    out
}
