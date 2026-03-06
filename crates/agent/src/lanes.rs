use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{oneshot, Notify};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use types::constants::lanes;

/// A task to be executed within a lane.
pub struct LaneTask {
    pub id: String,
    pub lane: String,
    pub description: String,
    pub task: Pin<Box<dyn Future<Output = Result<(), String>> + Send>>,
    pub enqueued_at: Instant,
    pub warn_after_ms: u64,
    pub completion_tx: Option<oneshot::Sender<Result<(), String>>>,
}

/// Internal state for a single lane.
struct LaneState {
    queue: VecDeque<LaneTask>,
    active: usize,
    max_concurrent: usize, // 0 = unlimited
}

/// Manages per-lane task queuing and concurrency limits.
///
/// Each lane has its own FIFO queue and concurrency cap. Tasks are dispatched
/// through per-lane pump tasks that respect the capacity.
pub struct LaneManager {
    lanes: HashMap<String, (Arc<std::sync::Mutex<LaneState>>, Arc<Notify>)>,
    cancel: CancellationToken,
}

/// Lane configuration: name and max concurrency.
struct LaneConfig {
    name: &'static str,
    max_concurrent: usize,
}

/// Default lane configurations.
/// ALL lanes route through Runner and may make LLM calls. The adaptive
/// ConcurrencyController governs concurrency globally based on available
/// machine resources and LLM rate limits. Lane limits here only exist
/// for physical or design serialization constraints:
/// - desktop: one screen, one mouse, one keyboard
/// - heartbeat: one proactive tick at a time
/// - dev: one developer assistant per project
const LANE_CONFIGS: &[LaneConfig] = &[
    LaneConfig { name: lanes::MAIN, max_concurrent: 0 },
    LaneConfig { name: lanes::EVENTS, max_concurrent: 0 },
    LaneConfig { name: lanes::SUBAGENT, max_concurrent: 0 },
    LaneConfig { name: lanes::NESTED, max_concurrent: 0 },
    LaneConfig { name: lanes::HEARTBEAT, max_concurrent: 1 },    // sequential ticks
    LaneConfig { name: lanes::COMM, max_concurrent: 0 },
    LaneConfig { name: lanes::DEV, max_concurrent: 0 },
    LaneConfig { name: lanes::DESKTOP, max_concurrent: 1 },      // one screen, one mouse
];

impl LaneManager {
    /// Create a new LaneManager with all configured lanes.
    pub fn new() -> Self {
        let mut lanes = HashMap::new();

        for config in LANE_CONFIGS {
            let state = LaneState {
                queue: VecDeque::new(),
                active: 0,
                max_concurrent: config.max_concurrent,
            };
            lanes.insert(
                config.name.to_string(),
                (Arc::new(std::sync::Mutex::new(state)), Arc::new(Notify::new())),
            );
        }

        Self {
            lanes,
            cancel: CancellationToken::new(),
        }
    }

    /// Start per-lane pump tasks. Each pump waits for notifications, dequeues
    /// tasks up to the lane's concurrency limit, and spawns them.
    pub fn start_pumps(&self) {
        for (name, (state, notify)) in &self.lanes {
            let name = name.clone();
            let state = state.clone();
            let notify = notify.clone();
            let cancel = self.cancel.clone();

            tokio::spawn(async move {
                debug!(lane = %name, "lane pump started");
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            debug!(lane = %name, "lane pump shutting down");
                            break;
                        }
                        _ = notify.notified() => {
                            pump_lane(&name, &state, &notify).await;
                        }
                    }
                }
            });
        }
    }

    /// Enqueue a task and return a completion handle.
    pub fn enqueue(&self, lane: &str, task: LaneTask) -> Option<oneshot::Receiver<Result<(), String>>> {
        let (lane_state, notify) = match self.lanes.get(lane) {
            Some(pair) => pair,
            None => {
                warn!(lane = %lane, "unknown lane, falling back to main");
                match self.lanes.get(lanes::MAIN) {
                    Some(pair) => pair,
                    None => return None,
                }
            }
        };

        let (tx, rx) = oneshot::channel();
        let mut task = task;
        task.completion_tx = Some(tx);

        {
            let mut state = lane_state.lock().unwrap();
            state.queue.push_back(task);
        }

        notify.notify_one();
        Some(rx)
    }

    /// Enqueue a task without waiting for completion (fire-and-forget).
    pub fn enqueue_async(&self, lane: &str, task: LaneTask) {
        let (lane_state, notify) = match self.lanes.get(lane) {
            Some(pair) => pair,
            None => {
                warn!(lane = %lane, "unknown lane for async enqueue, falling back to main");
                match self.lanes.get(lanes::MAIN) {
                    Some(pair) => pair,
                    None => return,
                }
            }
        };

        {
            let mut state = lane_state.lock().unwrap();
            state.queue.push_back(task);
        }

        notify.notify_one();
    }

    /// Get status info for all lanes: (name, active, queued, max_concurrent).
    pub fn status(&self) -> Vec<(String, usize, usize, usize)> {
        let mut result = Vec::new();
        for (name, (state, _)) in &self.lanes {
            let s = state.lock().unwrap();
            result.push((name.clone(), s.active, s.queue.len(), s.max_concurrent));
        }
        result.sort_by(|a, b| a.0.cmp(&b.0));
        result
    }

    /// Shutdown all lane pumps.
    pub fn shutdown(&self) {
        self.cancel.cancel();
    }
}

/// Drain ready tasks from a lane's queue, respecting concurrency limits.
async fn pump_lane(
    name: &str,
    state: &Arc<std::sync::Mutex<LaneState>>,
    notify: &Arc<Notify>,
) {
    loop {
        let task = {
            let mut s = state.lock().unwrap();
            let max = s.max_concurrent;
            // Check capacity (0 = unlimited)
            if max > 0 && s.active >= max {
                break;
            }
            match s.queue.pop_front() {
                Some(task) => {
                    // Check for stale tasks
                    let wait_ms = task.enqueued_at.elapsed().as_millis() as u64;
                    if wait_ms > task.warn_after_ms && task.warn_after_ms > 0 {
                        warn!(
                            lane = %name,
                            task_id = %task.id,
                            wait_ms = wait_ms,
                            "task waited longer than {}ms in queue",
                            task.warn_after_ms
                        );
                    }
                    s.active += 1;
                    task
                }
                None => break,
            }
        };

        let task_id = task.id.clone();
        let lane_name = name.to_string();
        let state_clone = state.clone();
        let notify_clone = notify.clone();

        info!(lane = %lane_name, task_id = %task_id, "spawning lane task");

        tokio::spawn(async move {
            let result = task.task.await;

            // Signal completion
            if let Some(tx) = task.completion_tx {
                let _ = tx.send(result.clone());
            }

            // Decrement active count
            {
                let mut s = state_clone.lock().unwrap();
                s.active = s.active.saturating_sub(1);
            }

            // Re-notify pump to drain queued tasks now that a slot is free
            notify_clone.notify_one();

            match &result {
                Ok(()) => debug!(lane = %lane_name, task_id = %task_id, "lane task completed"),
                Err(e) => warn!(lane = %lane_name, task_id = %task_id, error = %e, "lane task failed"),
            }
        });
    }
}

/// Create a LaneTask from an async closure.
pub fn make_task(
    lane: &str,
    description: impl Into<String>,
    future: impl Future<Output = Result<(), String>> + Send + 'static,
) -> LaneTask {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    LaneTask {
        id: format!("{}-{}", lane, now),
        lane: lane.to_string(),
        description: description.into(),
        task: Box::pin(future),
        enqueued_at: Instant::now(),
        warn_after_ms: 2000,
        completion_tx: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lane_manager_creation() {
        let mgr = LaneManager::new();
        assert!(mgr.lanes.contains_key(lanes::MAIN));
        assert!(mgr.lanes.contains_key(lanes::EVENTS));
        assert!(mgr.lanes.contains_key(lanes::SUBAGENT));
        assert!(mgr.lanes.contains_key(lanes::NESTED));
        assert!(mgr.lanes.contains_key(lanes::HEARTBEAT));
        assert!(mgr.lanes.contains_key(lanes::COMM));
        assert!(mgr.lanes.contains_key(lanes::DEV));
        assert!(mgr.lanes.contains_key(lanes::DESKTOP));
        assert_eq!(mgr.lanes.len(), 8);
    }

    #[test]
    fn test_lane_concurrency_limits() {
        let mgr = LaneManager::new();
        let status = mgr.status();
        // LLM-calling lanes unlimited — ConcurrencyController governs
        let main = status.iter().find(|s| s.0 == "main").unwrap();
        assert_eq!(main.3, 0);
        let events = status.iter().find(|s| s.0 == "events").unwrap();
        assert_eq!(events.3, 0);
        let subagent = status.iter().find(|s| s.0 == "subagent").unwrap();
        assert_eq!(subagent.3, 0);
        // Work-nature constraints preserved
        let heartbeat = status.iter().find(|s| s.0 == "heartbeat").unwrap();
        assert_eq!(heartbeat.3, 1);
        let desktop = status.iter().find(|s| s.0 == "desktop").unwrap();
        assert_eq!(desktop.3, 1);
        let comm = status.iter().find(|s| s.0 == "comm").unwrap();
        assert_eq!(comm.3, 0);
    }

    #[tokio::test]
    async fn test_enqueue_and_complete() {
        let mgr = LaneManager::new();
        mgr.start_pumps();

        let task = make_task("main", "test task", async { Ok(()) });
        let rx = mgr.enqueue("main", task).unwrap();

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            rx,
        )
        .await
        .expect("timeout")
        .expect("channel closed");

        assert!(result.is_ok());
        mgr.shutdown();
    }

    #[tokio::test]
    async fn test_enqueue_error_task() {
        let mgr = LaneManager::new();
        mgr.start_pumps();

        let task = make_task("main", "failing task", async {
            Err("test error".to_string())
        });
        let rx = mgr.enqueue("main", task).unwrap();

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            rx,
        )
        .await
        .expect("timeout")
        .expect("channel closed");

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "test error");
        mgr.shutdown();
    }

    #[tokio::test]
    async fn test_enqueue_unknown_lane_falls_back() {
        let mgr = LaneManager::new();
        mgr.start_pumps();

        let task = make_task("nonexistent", "fallback task", async { Ok(()) });
        let rx = mgr.enqueue("nonexistent", task);
        assert!(rx.is_some());

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            rx.unwrap(),
        )
        .await
        .expect("timeout")
        .expect("channel closed");

        assert!(result.is_ok());
        mgr.shutdown();
    }

    #[tokio::test]
    async fn test_enqueue_async_fire_and_forget() {
        let mgr = LaneManager::new();
        mgr.start_pumps();

        let (tx, rx) = oneshot::channel();
        let task = make_task("main", "async task", async move {
            let _ = tx.send(42);
            Ok(())
        });
        mgr.enqueue_async("main", task);

        let val = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            rx,
        )
        .await
        .expect("timeout")
        .expect("channel closed");

        assert_eq!(val, 42);
        mgr.shutdown();
    }

    #[test]
    fn test_make_task() {
        let task = make_task("main", "test", async { Ok(()) });
        assert!(task.id.starts_with("main-"));
        assert_eq!(task.lane, "main");
        assert_eq!(task.description, "test");
        assert_eq!(task.warn_after_ms, 2000);
        assert!(task.completion_tx.is_none());
    }

    #[tokio::test]
    async fn test_multiple_tasks_queue() {
        let mgr = LaneManager::new();
        mgr.start_pumps();

        let mut receivers = Vec::new();
        for i in 0..5 {
            let task = make_task("main", format!("task {}", i), async { Ok(()) });
            receivers.push(mgr.enqueue("main", task).unwrap());
        }

        for rx in receivers {
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                rx,
            )
            .await
            .expect("timeout")
            .expect("channel closed");
            assert!(result.is_ok());
        }

        mgr.shutdown();
    }

    #[test]
    fn test_status_empty() {
        let mgr = LaneManager::new();
        let status = mgr.status();
        for (_, active, queued, _) in &status {
            assert_eq!(*active, 0);
            assert_eq!(*queued, 0);
        }
    }

    #[test]
    fn test_shutdown() {
        let mgr = LaneManager::new();
        mgr.shutdown();
        // Should not panic
    }

    #[tokio::test]
    async fn test_concurrent_lane_tasks() {
        let mgr = LaneManager::new();
        mgr.start_pumps();

        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();

        // Enqueue into different lanes
        let task1 = make_task("main", "main task", async move {
            let _ = tx1.send("main");
            Ok(())
        });
        let task2 = make_task("subagent", "subagent task", async move {
            let _ = tx2.send("subagent");
            Ok(())
        });

        mgr.enqueue_async("main", task1);
        mgr.enqueue_async("subagent", task2);

        let v1 = tokio::time::timeout(std::time::Duration::from_secs(2), rx1)
            .await.expect("timeout").expect("closed");
        let v2 = tokio::time::timeout(std::time::Duration::from_secs(2), rx2)
            .await.expect("timeout").expect("closed");

        assert_eq!(v1, "main");
        assert_eq!(v2, "subagent");
        mgr.shutdown();
    }
}
