//! The ports Nebo itself listens on, on this computer: its server (the local
//! API), its browser's debugging port, a plugin sign-in's callback. A command
//! an employee runs never connects to them (`tools::confine`): Nebo's own API
//! on loopback would otherwise hand a command everything the owner's app can
//! do, whatever the employee's permissions say.

use std::collections::BTreeSet;
use std::sync::RwLock;

static PORTS: RwLock<BTreeSet<u16>> = RwLock::new(BTreeSet::new());

/// Nebo listens on `port` from now on.
pub fn open(port: u16) {
    if port != 0 {
        PORTS.write().unwrap_or_else(|e| e.into_inner()).insert(port);
    }
}

/// Nebo no longer listens on `port`.
pub fn close(port: u16) {
    PORTS.write().unwrap_or_else(|e| e.into_inner()).remove(&port);
}

/// The ports Nebo listens on now, in order.
pub fn list() -> Vec<u16> {
    PORTS.read().unwrap_or_else(|e| e.into_inner()).iter().copied().collect()
}
