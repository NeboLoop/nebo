//! Global agent run registry — tracks ALL active runs across the entire system.
//!
//! Every chat entry point (WebSocket, REST, cron, heartbeat, comm) registers its
//! run here. The registry is the single source of truth for "who is running right now",
//! enabling visibility, cancellation, and progress tracking for all agents.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::handlers::chat::PendingAsk;

/// The question a run is parked on, shared between the registry entry and
/// the run's handle. It lives exactly as long as the run: an entry that goes
/// (Done, error, cancel, stale cleanup) takes its question with it.
type ParkedAsk = Arc<std::sync::Mutex<Option<PendingAsk>>>;

/// A live run entry in the registry. Contains both identity info and live counters.
pub struct RunEntry {
    pub run_id: String,
    pub session_key: String,
    pub entity_id: String,
    pub entity_name: String,
    pub origin: String,
    pub channel: String,
    pub cancel_token: CancellationToken,
    pub started_at: Instant,
    pub last_activity: Arc<AtomicU64>,
    pub iteration_count: Arc<AtomicU32>,
    pub tool_call_count: Arc<AtomicU32>,
    pub current_tool: Arc<std::sync::Mutex<String>>,
    pub parent_run_id: Option<String>,
    pending_ask: ParkedAsk,
}

/// Serializable snapshot of a run — safe to send over WS/REST.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSnapshot {
    pub run_id: String,
    pub session_key: String,
    pub entity_id: String,
    pub entity_name: String,
    pub origin: String,
    pub channel: String,
    pub iteration_count: u32,
    pub tool_call_count: u32,
    pub current_tool: String,
    pub elapsed_secs: u64,
    pub parent_run_id: Option<String>,
    pub child_count: usize,
}

impl RunEntry {
    fn snapshot(&self, child_count: usize) -> RunSnapshot {
        RunSnapshot {
            run_id: self.run_id.clone(),
            session_key: self.session_key.clone(),
            entity_id: self.entity_id.clone(),
            entity_name: self.entity_name.clone(),
            origin: self.origin.clone(),
            channel: self.channel.clone(),
            iteration_count: self.iteration_count.load(Ordering::Relaxed),
            tool_call_count: self.tool_call_count.load(Ordering::Relaxed),
            current_tool: self
                .current_tool
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            elapsed_secs: self.started_at.elapsed().as_secs(),
            parent_run_id: self.parent_run_id.clone(),
            child_count,
        }
    }

    /// Record activity to prevent stale detection.
    pub fn touch(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.last_activity.store(now, Ordering::Relaxed);
    }

    /// Seconds since last activity.
    pub fn idle_secs(&self) -> u64 {
        let last = self.last_activity.load(Ordering::Relaxed);
        if last == 0 {
            return self.started_at.elapsed().as_secs();
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now.saturating_sub(last)
    }

    fn pending_ask(&self) -> Option<PendingAsk> {
        self.pending_ask
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

/// Handle returned from `register()` — holds Arc refs to counters so the runner
/// can update them. Auto-unregisters the run on drop (panic-safe).
pub struct RunHandle {
    registry: Arc<RunRegistryInner>,
    pub run_id: String,
    pub last_activity: Arc<AtomicU64>,
    pub iteration_count: Arc<AtomicU32>,
    pub tool_call_count: Arc<AtomicU32>,
    pub current_tool: Arc<std::sync::Mutex<String>>,
    pub cancel_token: CancellationToken,
    pending_ask: ParkedAsk,
}

impl RunHandle {
    /// Release the question this run was parked on, if any. Called when the
    /// run ends so the answer channel it registered can be dropped with it.
    pub fn take_pending_ask(&self) -> Option<PendingAsk> {
        self.pending_ask
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    /// Record activity on this run.
    pub fn touch(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.last_activity.store(now, Ordering::Relaxed);
    }

    /// Increment iteration count.
    pub fn inc_iteration(&self) {
        self.iteration_count.fetch_add(1, Ordering::Relaxed);
        self.touch();
    }

    /// Increment tool call count and set current tool name.
    pub fn start_tool(&self, tool_name: &str) {
        self.tool_call_count.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut ct) = self.current_tool.lock() {
            ct.clear();
            ct.push_str(tool_name);
        }
        self.touch();
    }

    /// Clear the current tool (call after tool completes).
    pub fn finish_tool(&self) {
        if let Ok(mut ct) = self.current_tool.lock() {
            ct.clear();
        }
        self.touch();
    }
}

impl Drop for RunHandle {
    fn drop(&mut self) {
        // Best-effort removal — don't panic if lock is poisoned
        if let Ok(mut runs) = self.registry.runs.try_write() {
            runs.remove(&self.run_id);
        } else {
            // If we can't get the lock synchronously, spawn a task to clean up
            let registry = self.registry.clone();
            let run_id = self.run_id.clone();
            tokio::spawn(async move {
                registry.runs.write().await.remove(&run_id);
            });
        }
    }
}

/// Parameters for registering a new run.
pub struct RegisterParams {
    pub session_key: String,
    pub entity_id: String,
    pub entity_name: String,
    pub origin: String,
    pub channel: String,
    pub cancel_token: CancellationToken,
    pub parent_run_id: Option<String>,
}

struct RunRegistryInner {
    runs: RwLock<HashMap<String, RunEntry>>,
}

/// Global registry of all active agent runs.
#[derive(Clone)]
pub struct RunRegistry {
    inner: Arc<RunRegistryInner>,
}

impl RunRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RunRegistryInner {
                runs: RwLock::new(HashMap::new()),
            }),
        }
    }

    /// Register a new run and return a handle with live counters.
    pub async fn register(&self, params: RegisterParams) -> RunHandle {
        let run_id = uuid::Uuid::new_v4().to_string();
        let last_activity = Arc::new(AtomicU64::new(0));
        let iteration_count = Arc::new(AtomicU32::new(0));
        let tool_call_count = Arc::new(AtomicU32::new(0));
        let current_tool = Arc::new(std::sync::Mutex::new(String::new()));
        let pending_ask: ParkedAsk = Arc::new(std::sync::Mutex::new(None));

        let entry = RunEntry {
            run_id: run_id.clone(),
            session_key: params.session_key,
            entity_id: params.entity_id,
            entity_name: params.entity_name,
            origin: params.origin,
            channel: params.channel,
            cancel_token: params.cancel_token.clone(),
            started_at: Instant::now(),
            last_activity: last_activity.clone(),
            iteration_count: iteration_count.clone(),
            tool_call_count: tool_call_count.clone(),
            current_tool: current_tool.clone(),
            parent_run_id: params.parent_run_id,
            pending_ask: pending_ask.clone(),
        };

        self.inner.runs.write().await.insert(run_id.clone(), entry);

        RunHandle {
            registry: self.inner.clone(),
            run_id,
            last_activity,
            iteration_count,
            tool_call_count,
            current_tool,
            cancel_token: params.cancel_token,
            pending_ask,
        }
    }

    /// Record the question the session's run is now parked on. Returns false
    /// when no run is registered under that session (nothing to park on).
    pub async fn park_ask(&self, session_key: &str, ask: PendingAsk) -> bool {
        let runs = self.inner.runs.read().await;
        match runs.values().find(|e| e.session_key == session_key) {
            Some(entry) => {
                *entry.pending_ask.lock().unwrap_or_else(|e| e.into_inner()) = Some(ask);
                true
            }
            None => false,
        }
    }

    /// The question the session's run is parked on, if any.
    pub async fn pending_ask_for_session(&self, session_key: &str) -> Option<PendingAsk> {
        let runs = self.inner.runs.read().await;
        runs.values()
            .find(|e| e.session_key == session_key)
            .and_then(RunEntry::pending_ask)
    }

    /// Every parked question, with the session it belongs to (a client that
    /// connects late is shown all of them).
    pub async fn pending_asks(&self) -> Vec<(String, PendingAsk)> {
        let runs = self.inner.runs.read().await;
        runs.values()
            .filter_map(|e| e.pending_ask().map(|ask| (e.session_key.clone(), ask)))
            .collect()
    }

    /// The question `request_id` was answered (or dropped): clear it from
    /// whichever run held it. Returns that run's session key.
    pub async fn resolve_ask(&self, request_id: &str) -> Option<String> {
        let runs = self.inner.runs.read().await;
        for entry in runs.values() {
            let mut slot = entry.pending_ask.lock().unwrap_or_else(|e| e.into_inner());
            if slot.as_ref().is_some_and(|a| a.request_id == request_id) {
                *slot = None;
                return Some(entry.session_key.clone());
            }
        }
        None
    }

    /// Remove a run explicitly (also happens automatically on RunHandle drop).
    pub async fn unregister(&self, run_id: &str) {
        self.inner.runs.write().await.remove(run_id);
    }

    /// All runs (user/frontend — full visibility).
    pub async fn list_all(&self) -> Vec<RunSnapshot> {
        let runs = self.inner.runs.read().await;
        let child_counts = count_children(&runs);
        runs.values()
            .map(|e| e.snapshot(*child_counts.get(&e.run_id).unwrap_or(&0)))
            .collect()
    }

    /// Top-level runs only (parent_run_id is None).
    pub async fn list_top_level(&self) -> Vec<RunSnapshot> {
        let runs = self.inner.runs.read().await;
        let child_counts = count_children(&runs);
        runs.values()
            .filter(|e| e.parent_run_id.is_none())
            .map(|e| e.snapshot(*child_counts.get(&e.run_id).unwrap_or(&0)))
            .collect()
    }

    /// Direct children of a specific run.
    pub async fn list_children(&self, parent_run_id: &str) -> Vec<RunSnapshot> {
        let runs = self.inner.runs.read().await;
        let child_counts = count_children(&runs);
        runs.values()
            .filter(|e| e.parent_run_id.as_deref() == Some(parent_run_id))
            .map(|e| e.snapshot(*child_counts.get(&e.run_id).unwrap_or(&0)))
            .collect()
    }

    /// Single run lookup.
    pub async fn get(&self, run_id: &str) -> Option<RunSnapshot> {
        let runs = self.inner.runs.read().await;
        let child_counts = count_children(&runs);
        runs.get(run_id)
            .map(|e| e.snapshot(*child_counts.get(&e.run_id).unwrap_or(&0)))
    }

    /// Find a run by session key.
    pub async fn find_by_session(&self, session_key: &str) -> Option<RunSnapshot> {
        let runs = self.inner.runs.read().await;
        let child_counts = count_children(&runs);
        runs.values()
            .find(|e| e.session_key == session_key)
            .map(|e| e.snapshot(*child_counts.get(&e.run_id).unwrap_or(&0)))
    }

    /// All runs for a specific entity (self + own sub-agents).
    pub async fn find_by_entity(&self, entity_id: &str) -> Vec<RunSnapshot> {
        let runs = self.inner.runs.read().await;
        let child_counts = count_children(&runs);

        // Find all run_ids that belong to this entity (top-level runs for this entity)
        let entity_run_ids: Vec<String> = runs
            .values()
            .filter(|e| e.entity_id == entity_id && e.parent_run_id.is_none())
            .map(|e| e.run_id.clone())
            .collect();

        // Collect entity's own runs + all descendants
        let mut result = Vec::new();
        for entry in runs.values() {
            if entry.entity_id == entity_id {
                result.push(entry.snapshot(*child_counts.get(&entry.run_id).unwrap_or(&0)));
            } else if let Some(ref parent_id) = entry.parent_run_id {
                // Check if this sub-agent's parent chain leads back to one of our entity's runs
                if is_descendant_of(&runs, parent_id, &entity_run_ids) {
                    result.push(entry.snapshot(*child_counts.get(&entry.run_id).unwrap_or(&0)));
                }
            }
        }
        result
    }

    /// Cancel a specific run (cascades to children via CancellationToken).
    pub async fn cancel(&self, run_id: &str) -> bool {
        let runs = self.inner.runs.read().await;
        if let Some(entry) = runs.get(run_id) {
            entry.cancel_token.cancel();
            true
        } else {
            false
        }
    }

    /// Cancel a run by session key.
    pub async fn cancel_by_session(&self, session_key: &str) -> bool {
        let runs = self.inner.runs.read().await;
        if let Some(entry) = runs.values().find(|e| e.session_key == session_key) {
            entry.cancel_token.cancel();
            true
        } else {
            false
        }
    }

    /// Cancel all runs for an entity (its run + sub-agents).
    pub async fn cancel_by_entity(&self, entity_id: &str) -> usize {
        let runs = self.inner.runs.read().await;
        let mut count = 0;
        for entry in runs.values() {
            if entry.entity_id == entity_id {
                entry.cancel_token.cancel();
                count += 1;
            }
        }
        count
    }

    /// Emergency kill — cancel every active run.
    pub async fn cancel_all(&self) -> usize {
        let runs = self.inner.runs.read().await;
        let count = runs.len();
        for entry in runs.values() {
            entry.cancel_token.cancel();
        }
        count
    }

    /// Check if a session has an active run.
    pub async fn is_session_active(&self, session_key: &str) -> bool {
        let runs = self.inner.runs.read().await;
        runs.values().any(|e| e.session_key == session_key)
    }

    /// Number of active runs.
    pub async fn active_count(&self) -> usize {
        self.inner.runs.read().await.len()
    }

    /// Clean up stale runs that have been idle for too long.
    /// Returns the session keys of cleaned-up runs (for browser tab cleanup).
    pub async fn cleanup_stale(&self, max_idle_secs: u64) -> Vec<String> {
        let runs = self.inner.runs.read().await;
        let stale: Vec<(String, String)> = runs
            .values()
            .filter(|e| e.idle_secs() > max_idle_secs)
            .map(|e| {
                e.cancel_token.cancel();
                (e.run_id.clone(), e.session_key.clone())
            })
            .collect();
        drop(runs);

        if stale.is_empty() {
            return vec![];
        }

        let session_keys: Vec<String> = stale.iter().map(|(_, sk)| sk.clone()).collect();
        let mut runs = self.inner.runs.write().await;
        for (id, _) in &stale {
            runs.remove(id);
        }
        session_keys
    }
}

impl Default for RunRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Count how many direct children each run has.
fn count_children(runs: &HashMap<String, RunEntry>) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for entry in runs.values() {
        if let Some(ref parent_id) = entry.parent_run_id {
            *counts.entry(parent_id.clone()).or_insert(0) += 1;
        }
    }
    counts
}

/// Check if a run_id is a descendant of any of the target run IDs.
fn is_descendant_of(runs: &HashMap<String, RunEntry>, run_id: &str, target_ids: &[String]) -> bool {
    let mut current = run_id.to_string();
    // Walk up the parent chain (with cycle protection)
    for _ in 0..10 {
        if target_ids.iter().any(|t| t == &current) {
            return true;
        }
        match runs.get(&current).and_then(|e| e.parent_run_id.clone()) {
            Some(parent) => current = parent,
            None => return false,
        }
    }
    false
}

// ---------- RunQuerier trait impl (cross-crate bridge to tools) ----------

impl tools::run_querier::RunQuerier for RunRegistry {
    fn list_runs(
        &self,
        caller_entity_id: &str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Vec<tools::run_querier::RunInfo>> + Send + '_>,
    > {
        let caller = caller_entity_id.to_string();
        Box::pin(async move {
            let snapshots = if caller == "main" {
                self.list_all().await
            } else {
                self.find_by_entity(&caller).await
            };
            snapshots
                .into_iter()
                .map(|s| tools::run_querier::RunInfo {
                    run_id: s.run_id,
                    entity_id: s.entity_id,
                    entity_name: s.entity_name,
                    origin: s.origin,
                    tool_call_count: s.tool_call_count,
                    current_tool: s.current_tool,
                    elapsed_secs: s.elapsed_secs,
                    parent_run_id: s.parent_run_id,
                })
                .collect()
        })
    }

    fn cancel_run(
        &self,
        run_id: &str,
        caller_entity_id: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + Send + '_>>
    {
        let run_id = run_id.to_string();
        let caller = caller_entity_id.to_string();
        Box::pin(async move {
            // Primary agent can cancel anything
            if caller == "main" {
                return Ok(self.cancel(&run_id).await);
            }

            // Persona agents: verify the target run belongs to them
            let allowed = {
                let runs = self.inner.runs.read().await;
                match runs.get(&run_id) {
                    Some(entry) => {
                        if entry.entity_id == caller {
                            true
                        } else {
                            // Check if it's a descendant of one of the caller's runs
                            let caller_run_ids: Vec<String> = runs
                                .values()
                                .filter(|e| e.entity_id == caller)
                                .map(|e| e.run_id.clone())
                                .collect();
                            entry
                                .parent_run_id
                                .as_ref()
                                .map_or(false, |pid| is_descendant_of(&runs, pid, &caller_run_ids))
                        }
                    }
                    None => return Ok(false),
                }
            };

            if allowed {
                Ok(self.cancel(&run_id).await)
            } else {
                Err("Not authorized to cancel this run".to_string())
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(id: &str) -> PendingAsk {
        PendingAsk {
            request_id: id.into(),
            prompt: format!("question {id}"),
            widgets: None,
            created_at: 1,
        }
    }

    async fn register(registry: &RunRegistry, session_key: &str) -> RunHandle {
        registry
            .register(RegisterParams {
                session_key: session_key.into(),
                entity_id: "e".into(),
                entity_name: "E".into(),
                origin: "user".into(),
                channel: "web".into(),
                cancel_token: CancellationToken::new(),
                parent_run_id: None,
            })
            .await
    }

    /// A parked question is readable by session and listed globally; it can
    /// only be parked on a registered run.
    #[tokio::test]
    async fn park_ask_is_keyed_by_session_and_needs_a_run() {
        let registry = RunRegistry::new();
        let _h = register(&registry, "agent:a:thread:1").await;
        assert!(!registry.park_ask("agent:b:thread:9", ask("r0")).await);
        assert!(registry.park_ask("agent:a:thread:1", ask("r1")).await);
        let parked = registry.pending_ask_for_session("agent:a:thread:1").await.unwrap();
        assert_eq!(parked.request_id, "r1");
        assert_eq!(parked.prompt, "question r1");
        assert!(registry.pending_ask_for_session("agent:b:thread:9").await.is_none());
        let all = registry.pending_asks().await;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].0, "agent:a:thread:1");
    }

    /// Answering clears the question by request id and names the session it
    /// belonged to; an unknown id clears nothing.
    #[tokio::test]
    async fn resolve_ask_clears_by_request_id() {
        let registry = RunRegistry::new();
        let _h = register(&registry, "s1").await;
        let _h2 = register(&registry, "s2").await;
        registry.park_ask("s1", ask("r1")).await;
        registry.park_ask("s2", ask("r2")).await;
        assert_eq!(registry.resolve_ask("nope").await, None);
        assert_eq!(registry.pending_asks().await.len(), 2);
        assert_eq!(registry.resolve_ask("r2").await.as_deref(), Some("s2"));
        assert!(registry.pending_ask_for_session("s2").await.is_none());
        assert_eq!(registry.pending_ask_for_session("s1").await.unwrap().request_id, "r1");
    }

    /// The handle releases the question (for dropping its answer channel) and
    /// the run's end takes the question with it: no card outlives its run.
    #[tokio::test]
    async fn pending_ask_dies_with_the_run() {
        let registry = RunRegistry::new();
        let h = register(&registry, "s1").await;
        registry.park_ask("s1", ask("r1")).await;
        assert_eq!(h.take_pending_ask().unwrap().request_id, "r1");
        assert!(h.take_pending_ask().is_none());
        assert!(registry.pending_ask_for_session("s1").await.is_none());

        registry.park_ask("s1", ask("r2")).await;
        drop(h);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(registry.pending_asks().await.is_empty());
    }

    #[tokio::test]
    async fn test_register_and_list() {
        let registry = RunRegistry::new();
        let token = CancellationToken::new();

        let handle = registry
            .register(RegisterParams {
                session_key: "agent:test:main".to_string(),
                entity_id: "test-agent".to_string(),
                entity_name: "Test Agent".to_string(),
                origin: "ws".to_string(),
                channel: "main".to_string(),
                cancel_token: token.clone(),
                parent_run_id: None,
            })
            .await;

        // Should appear in listings
        let all = registry.list_all().await;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].entity_name, "Test Agent");
        assert_eq!(all[0].parent_run_id, None);

        let top = registry.list_top_level().await;
        assert_eq!(top.len(), 1);

        // Drop handle should unregister
        drop(handle);
        // Give the async cleanup a moment
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let all = registry.list_all().await;
        assert_eq!(all.len(), 0);
    }

    #[tokio::test]
    async fn test_parent_child_relationship() {
        let registry = RunRegistry::new();
        let parent_token = CancellationToken::new();

        let parent_handle = registry
            .register(RegisterParams {
                session_key: "agent:parent:main".to_string(),
                entity_id: "parent".to_string(),
                entity_name: "Parent Agent".to_string(),
                origin: "ws".to_string(),
                channel: "main".to_string(),
                cancel_token: parent_token.clone(),
                parent_run_id: None,
            })
            .await;

        let child_token = parent_token.child_token();
        let _child_handle = registry
            .register(RegisterParams {
                session_key: "subagent:parent:child1".to_string(),
                entity_id: "parent".to_string(),
                entity_name: "Child Explorer".to_string(),
                origin: "ws".to_string(),
                channel: "main".to_string(),
                cancel_token: child_token,
                parent_run_id: Some(parent_handle.run_id.clone()),
            })
            .await;

        // Top-level should only show parent
        let top = registry.list_top_level().await;
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].child_count, 1);

        // Children query should show the child
        let children = registry.list_children(&parent_handle.run_id).await;
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].entity_name, "Child Explorer");

        // Entity query should show both
        let entity_runs = registry.find_by_entity("parent").await;
        assert_eq!(entity_runs.len(), 2);
    }

    #[tokio::test]
    async fn test_cancel_cascades() {
        let registry = RunRegistry::new();
        let parent_token = CancellationToken::new();
        let child_token = parent_token.child_token();

        let _parent = registry
            .register(RegisterParams {
                session_key: "agent:p:main".to_string(),
                entity_id: "p".to_string(),
                entity_name: "Parent".to_string(),
                origin: "ws".to_string(),
                channel: "main".to_string(),
                cancel_token: parent_token.clone(),
                parent_run_id: None,
            })
            .await;

        let _child = registry
            .register(RegisterParams {
                session_key: "subagent:p:c".to_string(),
                entity_id: "p".to_string(),
                entity_name: "Child".to_string(),
                origin: "ws".to_string(),
                channel: "main".to_string(),
                cancel_token: child_token.clone(),
                parent_run_id: Some(_parent.run_id.clone()),
            })
            .await;

        // Cancel parent should cascade to child
        registry.cancel(&_parent.run_id).await;
        assert!(parent_token.is_cancelled());
        assert!(child_token.is_cancelled());
    }

    #[tokio::test]
    async fn test_cancel_by_session() {
        let registry = RunRegistry::new();
        let token = CancellationToken::new();

        let _handle = registry
            .register(RegisterParams {
                session_key: "agent:test:main".to_string(),
                entity_id: "test".to_string(),
                entity_name: "Test".to_string(),
                origin: "ws".to_string(),
                channel: "main".to_string(),
                cancel_token: token.clone(),
                parent_run_id: None,
            })
            .await;

        assert!(registry.cancel_by_session("agent:test:main").await);
        assert!(token.is_cancelled());
        assert!(!registry.cancel_by_session("nonexistent").await);
    }

    #[tokio::test]
    async fn test_isolation_between_entities() {
        let registry = RunRegistry::new();

        let _agent_a = registry
            .register(RegisterParams {
                session_key: "agent:a:main".to_string(),
                entity_id: "agent-a".to_string(),
                entity_name: "Agent A".to_string(),
                origin: "ws".to_string(),
                channel: "main".to_string(),
                cancel_token: CancellationToken::new(),
                parent_run_id: None,
            })
            .await;

        let _agent_b = registry
            .register(RegisterParams {
                session_key: "agent:b:main".to_string(),
                entity_id: "agent-b".to_string(),
                entity_name: "Agent B".to_string(),
                origin: "ws".to_string(),
                channel: "main".to_string(),
                cancel_token: CancellationToken::new(),
                parent_run_id: None,
            })
            .await;

        // Entity A should only see its own runs
        let a_runs = registry.find_by_entity("agent-a").await;
        assert_eq!(a_runs.len(), 1);
        assert_eq!(a_runs[0].entity_name, "Agent A");

        // Entity B should only see its own runs
        let b_runs = registry.find_by_entity("agent-b").await;
        assert_eq!(b_runs.len(), 1);
        assert_eq!(b_runs[0].entity_name, "Agent B");

        // Full listing sees both
        let all = registry.list_all().await;
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_counter_updates() {
        let registry = RunRegistry::new();

        let handle = registry
            .register(RegisterParams {
                session_key: "test".to_string(),
                entity_id: "test".to_string(),
                entity_name: "Test".to_string(),
                origin: "ws".to_string(),
                channel: "main".to_string(),
                cancel_token: CancellationToken::new(),
                parent_run_id: None,
            })
            .await;

        handle.inc_iteration();
        handle.inc_iteration();
        handle.start_tool("web");
        handle.finish_tool();
        handle.start_tool("system");

        let snap = registry.get(&handle.run_id).await.unwrap();
        assert_eq!(snap.iteration_count, 2);
        assert_eq!(snap.tool_call_count, 2);
        assert_eq!(snap.current_tool, "system");
    }

    #[tokio::test]
    async fn test_cleanup_stale() {
        let registry = RunRegistry::new();

        let _handle = registry
            .register(RegisterParams {
                session_key: "stale".to_string(),
                entity_id: "test".to_string(),
                entity_name: "Stale".to_string(),
                origin: "cron".to_string(),
                channel: "cron".to_string(),
                cancel_token: CancellationToken::new(),
                parent_run_id: None,
            })
            .await;

        // With 0 max_idle, everything is stale
        // But the run just started, so idle_secs is 0 — it won't be cleaned up with max_idle=1
        // This just tests the mechanism works without actually waiting
        let cleaned = registry.cleanup_stale(999999).await;
        assert!(cleaned.is_empty());
    }
}

#[cfg(test)]
mod cancel_semantics_tests {
    //! Locks the registry semantics the ws.rs "cancel" handler's precedence
    //! chain (run_id > entity_id > session_id > cancel-ALL fallback) is built
    //! on. The chain itself is inline in handle_client_ws; these pin the
    //! building blocks it calls.
    use super::*;

    async fn reg(
        registry: &RunRegistry,
        session_key: &str,
        entity_id: &str,
        token: &CancellationToken,
    ) -> RunHandle {
        registry
            .register(RegisterParams {
                session_key: session_key.to_string(),
                entity_id: entity_id.to_string(),
                entity_name: entity_id.to_string(),
                origin: "ws".to_string(),
                channel: "main".to_string(),
                cancel_token: token.clone(),
                parent_run_id: None,
            })
            .await
    }

    /// cancel(run_id) on an unknown id returns false and cancels nothing —
    /// the ws handler relies on this to fall through to the next precedence
    /// tier instead of silently "succeeding".
    #[tokio::test]
    async fn cancel_unknown_run_id_is_a_miss() {
        let registry = RunRegistry::new();
        let token = CancellationToken::new();
        let _h = reg(&registry, "agent:a:main", "a", &token).await;
        assert!(!registry.cancel("no-such-run").await);
        assert!(!token.is_cancelled());
    }

    /// cancel_by_entity cancels EVERY run of that entity (returning the
    /// count) and leaves other entities' runs alive.
    #[tokio::test]
    async fn cancel_by_entity_scopes_to_that_entity_only() {
        let registry = RunRegistry::new();
        let (ta, tb, tc) = (
            CancellationToken::new(),
            CancellationToken::new(),
            CancellationToken::new(),
        );
        let _h1 = reg(&registry, "agent:x:main", "x", &ta).await;
        let _h2 = reg(&registry, "agent:x:thread:c1", "x", &tb).await;
        let _h3 = reg(&registry, "agent:y:main", "y", &tc).await;

        assert_eq!(registry.cancel_by_entity("x").await, 2);
        assert!(ta.is_cancelled());
        assert!(tb.is_cancelled());
        assert!(!tc.is_cancelled());
        assert_eq!(registry.cancel_by_entity("nobody").await, 0);
    }

    /// cancel_all (the ws fallback when no key matches — "stop means stop")
    /// cancels every active run and reports how many.
    #[tokio::test]
    async fn cancel_all_stops_everything_and_counts() {
        let registry = RunRegistry::new();
        let tokens: Vec<CancellationToken> =
            (0..3).map(|_| CancellationToken::new()).collect();
        let _h1 = reg(&registry, "agent:a:main", "a", &tokens[0]).await;
        let _h2 = reg(&registry, "agent:b:main", "b", &tokens[1]).await;
        let _h3 = reg(&registry, "neboai:group:g1", "main", &tokens[2]).await;

        assert_eq!(registry.cancel_all().await, 3);
        assert!(tokens.iter().all(|t| t.is_cancelled()));
        assert_eq!(registry.cancel_all().await, 3); // idempotent: entries remain until handles drop
    }

    /// Session activity queries answer by exact session key — the predicate
    /// the session-tier cancel and stream-status checks depend on.
    #[tokio::test]
    async fn session_activity_is_keyed_exactly() {
        let registry = RunRegistry::new();
        let token = CancellationToken::new();
        let _h = reg(&registry, "agent:a:thread:c9", "a", &token).await;
        assert!(registry.is_session_active("agent:a:thread:c9").await);
        assert!(!registry.is_session_active("agent:a:thread").await);
        assert!(!registry.is_session_active("agent:a").await);
        assert!(registry.find_by_session("agent:a:thread:c9").await.is_some());
        assert!(registry.find_by_session("agent:a:main").await.is_none());
    }
}
