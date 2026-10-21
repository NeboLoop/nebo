//! The tabs Nebo opened in the owner's Chrome, by the run that opened them.
//!
//! The extension keeps one tab per key it is sent (`session_id` on each
//! request). Each run of a session works under a key of its own,
//! `<session id>#<nonce>`, minted on its first browser call: the run's end
//! closes that key's tab, and a tab the run left open for the owner
//! ([`TabLedger::take`] without a close) is never under a key a later run
//! uses, so nothing navigates or closes it. The owner's own tabs are under no
//! key at all.
//!
//! The keys of tabs still open are kept in a file, so a Nebo that stopped
//! mid-run closes them when a browser next connects ([`TabLedger::orphans`]).

use std::collections::HashMap;
use std::path::PathBuf;

use tokio::sync::Mutex;
use tracing::warn;

pub struct TabLedger {
    /// Where the open keys are kept; `None` keeps them in memory only.
    path: Option<PathBuf>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Session id → the key its current run's tab is under.
    open: HashMap<String, String>,
    /// Keys a Nebo before this one left open, closed in every browser that
    /// connects to this one.
    orphans: Vec<String>,
    /// The orphans are off the file: they were sent once, and the next Nebo
    /// needn't send them again.
    orphans_sent: bool,
}

impl TabLedger {
    /// A ledger kept at `path`: the keys a previous Nebo left there are this
    /// one's orphans.
    pub fn new(path: Option<PathBuf>) -> Self {
        let orphans = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice::<Vec<String>>(&bytes).ok())
            .unwrap_or_default();
        Self { path, state: Mutex::new(State { open: HashMap::new(), orphans, orphans_sent: false }) }
    }

    /// The key `session_id`'s run works under, minted on its first call.
    pub async fn key(&self, session_id: &str) -> String {
        let mut state = self.state.lock().await;
        if let Some(key) = state.open.get(session_id) {
            return key.clone();
        }
        let key = format!("{session_id}#{:016x}", rand::random::<u64>());
        state.open.insert(session_id.to_string(), key.clone());
        self.save(&state).await;
        key
    }

    /// The key `session_id`'s run works under, when it has opened one.
    pub async fn current(&self, session_id: &str) -> Option<String> {
        self.state.lock().await.open.get(session_id).cloned()
    }

    /// The run's key, no longer tracked: its tab is the caller's to close,
    /// or to leave open for the owner. The session's next call mints a new one.
    pub async fn take(&self, session_id: &str) -> Option<String> {
        let mut state = self.state.lock().await;
        let key = state.open.remove(session_id)?;
        self.save(&state).await;
        Some(key)
    }

    /// The keys a previous Nebo left open, for every browser that connects.
    /// The first call takes them off the file.
    pub async fn orphans(&self) -> Vec<String> {
        let mut state = self.state.lock().await;
        if !state.orphans_sent && !state.orphans.is_empty() {
            state.orphans_sent = true;
            self.save(&state).await;
        }
        state.orphans.clone()
    }

    /// Writes the open keys and orphans; called under the lock, so writes
    /// land in the order the changes were made.
    async fn save(&self, state: &State) {
        let Some(path) = &self.path else { return };
        let unsent = if state.orphans_sent { &[][..] } else { &state.orphans[..] };
        let keys: Vec<&String> = state.open.values().chain(unsent.iter()).collect();
        let body = serde_json::to_vec(&keys).unwrap_or_default();
        if let Some(dir) = path.parent() {
            let _ = tokio::fs::create_dir_all(dir).await;
        }
        // Whole or not at all: a crash mid-write never leaves a torn file.
        let temp = path.with_extension("json.tmp");
        let written = match tokio::fs::write(&temp, body).await {
            Ok(()) => tokio::fs::rename(&temp, path).await,
            Err(e) => Err(e),
        };
        if let Err(e) = written {
            warn!(path = %path.display(), error = %e, "could not keep the tabs Nebo opened");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn each_run_gets_its_own_key_and_a_restart_finds_what_was_left_open() {
        let path = std::env::temp_dir().join(format!("nebo-tab-ledger-{}.json", uuid_like()));
        let ledger = TabLedger::new(Some(path.clone()));
        let first = ledger.key("s1").await;
        assert!(first.starts_with("s1#"));
        assert_eq!(ledger.key("s1").await, first, "one key for the whole run");
        assert_eq!(ledger.take("s1").await.as_deref(), Some(first.as_str()));
        assert!(ledger.take("s1").await.is_none(), "taken once");
        let second = ledger.key("s1").await;
        assert_ne!(second, first, "the next run never works in a tab the last one left open");
        let other = ledger.key("s2").await;
        ledger.take("s2").await;
        assert!(ledger.orphans().await.is_empty());

        // Nebo stops with s1's run still open: the next Nebo closes its tab once.
        let after = TabLedger::new(Some(path.clone()));
        assert_eq!(after.orphans().await, vec![second.clone()]);
        assert_eq!(after.orphans().await, vec![second], "every browser that connects gets them");
        assert!(!after.orphans().await.contains(&other));
        assert!(TabLedger::new(Some(path.clone())).orphans().await.is_empty(), "off the file once sent");
        assert!(!path.with_extension("json.tmp").exists(), "written whole, then renamed");
        let _ = std::fs::remove_file(&path);
    }

    fn uuid_like() -> String {
        format!("{}-{:x}", std::process::id(), rand::random::<u64>())
    }
}
