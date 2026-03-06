use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ai::RateLimitMeta;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::{debug, info, warn};

/// Adaptive global concurrency controller for LLM and tool execution.
///
/// Governs all LLM calls and parallel tool execution via dynamic semaphores.
/// Adapts to rate limits (429 backpressure) and system resources (CPU/memory).
pub struct ConcurrencyController {
    /// Dynamic semaphore for LLM calls — initialized at ceiling.
    llm_semaphore: Arc<Semaphore>,
    /// Current effective permits (adjusted dynamically).
    effective_permits: AtomicUsize,
    /// Permits held back to reduce concurrency (acquire-and-hold pattern).
    held_back: Mutex<Vec<OwnedSemaphorePermit>>,
    /// Floor — never go below this.
    min_permits: usize,
    /// Ceiling — never exceed this (set by resource probe).
    ceiling: AtomicUsize,
    /// Backpressure flag — rate limited.
    backpressure: AtomicBool,
    /// Tool-level concurrency per turn.
    tool_semaphore: Arc<Semaphore>,
}

impl ConcurrencyController {
    /// Create a new controller with resource-aware defaults.
    pub fn new() -> Self {
        let cpu_cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let initial = (cpu_cores * 2).min(20);

        Self {
            llm_semaphore: Arc::new(Semaphore::new(initial)),
            effective_permits: AtomicUsize::new(initial),
            held_back: Mutex::new(Vec::new()),
            min_permits: 2,
            ceiling: AtomicUsize::new(initial),
            backpressure: AtomicBool::new(false),
            tool_semaphore: Arc::new(Semaphore::new(8)),
        }
    }

    /// Acquire a permit for an LLM call. Blocks when at capacity.
    pub async fn acquire_llm_permit(&self) -> OwnedSemaphorePermit {
        self.llm_semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("llm semaphore closed")
    }

    /// Acquire a permit for parallel tool execution within a turn.
    pub async fn acquire_tool_permit(&self) -> OwnedSemaphorePermit {
        self.tool_semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("tool semaphore closed")
    }

    /// Report a successful LLM call. May release held permits if headroom exists.
    pub fn report_success(&self, meta: Option<&RateLimitMeta>) {
        // Clear backpressure on success
        if self.backpressure.swap(false, Ordering::SeqCst) {
            debug!("backpressure cleared after successful call");
        }

        // If we have headroom from rate limit headers, release held permits
        if let Some(meta) = meta {
            if let Some(remaining) = meta.remaining_requests {
                if remaining > 5 {
                    self.release_held(2);
                }
            }
        }
    }

    /// Report a 429 rate limit. Acquires permits into held_back to reduce concurrency.
    pub fn report_rate_limit(&self, _retry_after_secs: Option<u64>) {
        self.backpressure.store(true, Ordering::SeqCst);

        let effective = self.effective_permits.load(Ordering::SeqCst);
        let to_hold = (effective / 2).max(1);
        let target = effective.saturating_sub(to_hold).max(self.min_permits);
        let actual_hold = effective - target;

        if actual_hold == 0 {
            return;
        }

        info!(
            effective,
            to_hold = actual_hold,
            target,
            "rate limit backpressure: reducing concurrency"
        );

        // Try to acquire permits synchronously (non-blocking) to hold them back
        let mut held = self.held_back.lock().unwrap();
        for _ in 0..actual_hold {
            match self.llm_semaphore.clone().try_acquire_owned() {
                Ok(permit) => {
                    held.push(permit);
                }
                Err(_) => break, // All permits in use, can't reduce further
            }
        }
        let held_count = held.len();
        drop(held);

        self.effective_permits
            .store(effective - held_count, Ordering::SeqCst);

        // Permits are released on report_success or set_ceiling calls.
        // The backpressure flag + held permits naturally throttle until success.
    }

    /// Release N held permits back to the semaphore.
    fn release_held(&self, count: usize) {
        let mut held = self.held_back.lock().unwrap();
        let to_release = count.min(held.len());
        if to_release == 0 {
            return;
        }

        for _ in 0..to_release {
            // Dropping the OwnedSemaphorePermit returns it to the semaphore
            held.pop();
        }
        let remaining_held = held.len();
        drop(held);

        let old = self.effective_permits.fetch_add(to_release, Ordering::SeqCst);
        debug!(
            released = to_release,
            new_effective = old + to_release,
            remaining_held,
            "released held permits"
        );
    }

    /// Set the ceiling — called by resource monitor. Adjusts permits accordingly.
    pub fn set_ceiling(&self, new_ceiling: usize) {
        let clamped = new_ceiling.max(self.min_permits);
        let old_ceiling = self.ceiling.swap(clamped, Ordering::SeqCst);

        if clamped == old_ceiling {
            return;
        }

        let effective = self.effective_permits.load(Ordering::SeqCst);

        if clamped > effective {
            // Ceiling increased — release held permits up to new ceiling
            let can_release = clamped - effective;
            self.release_held(can_release);
        } else if clamped < effective {
            // Ceiling decreased — acquire more permits into held_back
            let to_hold = effective - clamped;
            let mut held = self.held_back.lock().unwrap();
            for _ in 0..to_hold {
                match self.llm_semaphore.clone().try_acquire_owned() {
                    Ok(permit) => held.push(permit),
                    Err(_) => break,
                }
            }
            let actual_held = held.len();
            drop(held);

            self.effective_permits
                .store(effective.saturating_sub(actual_held), Ordering::SeqCst);
        }

        debug!(
            old_ceiling,
            new_ceiling = clamped,
            effective = self.effective_permits.load(Ordering::SeqCst),
            "ceiling adjusted"
        );
    }

    /// Whether backpressure is active (rate limited).
    pub fn is_backpressured(&self) -> bool {
        self.backpressure.load(Ordering::SeqCst)
    }

    /// Current effective permit count.
    pub fn effective_permits(&self) -> usize {
        self.effective_permits.load(Ordering::SeqCst)
    }

    /// Current ceiling.
    pub fn ceiling(&self) -> usize {
        self.ceiling.load(Ordering::SeqCst)
    }
}

/// Spawn the background resource monitor that adjusts the ceiling every 30s.
pub fn spawn_monitor(controller: Arc<ConcurrencyController>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;

            // sysinfo calls are blocking — use spawn_blocking
            let probe = match tokio::task::spawn_blocking(|| {
                use sysinfo::System;
                let mut sys = System::new();
                sys.refresh_memory();
                let available_mb = sys.available_memory() / 1_048_576;
                let load = System::load_average().one;
                (available_mb, load)
            })
            .await
            {
                Ok(result) => result,
                Err(e) => {
                    warn!("resource probe failed: {}", e);
                    continue;
                }
            };

            let (available_mb, load) = probe;
            let cpu_cores = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4);

            let mem_limit = (available_mb / 200) as usize;
            let cpu_limit = cpu_cores * 2;
            let load_factor = if load > cpu_cores as f64 {
                (cpu_cores as f64 / load).max(0.3)
            } else {
                1.0
            };

            let ceiling = ((mem_limit.min(cpu_limit) as f64) * load_factor) as usize;
            controller.set_ceiling(ceiling.clamp(2, 50));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_controller_creation() {
        let ctrl = ConcurrencyController::new();
        assert!(ctrl.effective_permits() >= 2);
        assert_eq!(ctrl.ceiling(), ctrl.effective_permits());
        assert!(!ctrl.is_backpressured());
    }

    #[tokio::test]
    async fn test_acquire_llm_permit() {
        let ctrl = ConcurrencyController::new();
        let permit = ctrl.acquire_llm_permit().await;
        assert!(ctrl.effective_permits() >= 2);
        drop(permit);
    }

    #[tokio::test]
    async fn test_acquire_tool_permit() {
        let ctrl = ConcurrencyController::new();
        let permit = ctrl.acquire_tool_permit().await;
        drop(permit);
    }

    #[test]
    fn test_report_success_clears_backpressure() {
        let ctrl = ConcurrencyController::new();
        ctrl.backpressure.store(true, Ordering::SeqCst);
        assert!(ctrl.is_backpressured());
        ctrl.report_success(None);
        assert!(!ctrl.is_backpressured());
    }

    #[test]
    fn test_report_rate_limit_sets_backpressure() {
        let ctrl = ConcurrencyController::new();
        ctrl.report_rate_limit(Some(5));
        assert!(ctrl.is_backpressured());
    }

    #[test]
    fn test_set_ceiling_clamps_to_min() {
        let ctrl = ConcurrencyController::new();
        ctrl.set_ceiling(1); // Below min_permits (2)
        assert!(ctrl.ceiling() >= 2);
    }

    #[test]
    fn test_set_ceiling_noop_same_value() {
        let ctrl = ConcurrencyController::new();
        let initial = ctrl.ceiling();
        ctrl.set_ceiling(initial);
        assert_eq!(ctrl.ceiling(), initial);
    }

    #[test]
    fn test_release_held_empty() {
        let ctrl = ConcurrencyController::new();
        let before = ctrl.effective_permits();
        ctrl.release_held(5); // Nothing held
        assert_eq!(ctrl.effective_permits(), before);
    }

    #[test]
    fn test_report_success_with_headroom_releases() {
        let ctrl = ConcurrencyController::new();
        // First, hold back some permits
        ctrl.report_rate_limit(Some(5));
        let after_limit = ctrl.effective_permits();

        // Now report success with headroom
        let meta = RateLimitMeta {
            remaining_requests: Some(100),
            ..Default::default()
        };
        ctrl.report_success(Some(&meta));
        assert!(!ctrl.is_backpressured());
        // Should have released some permits
        assert!(ctrl.effective_permits() >= after_limit);
    }
}
