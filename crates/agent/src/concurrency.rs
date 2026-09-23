use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::{debug, info, warn};

/// Hard upper bound for the adaptive ceiling when no explicit limit is configured.
/// LLM permits are spent waiting on network streams, so the real bound is memory
/// (see MB_PER_LLM_CALL) and 429 backpressure, not CPU cores or a fixed small
/// number — width is ours (auditor Rule 15).
const DEFAULT_MAX_CEILING: usize = 1000;

/// Resident memory one LLM call holds while its permit is out (request body,
/// connection, stream buffers). Measured 2026-09-23 through the real
/// OpenAI-compatible provider (the Janus path), ~100 KB of prompt + 40 tool
/// schemas, streams held open mid-reply: 1.2–1.4 MB per call, flat from 5 to
/// 800 calls in flight (macOS allocator). Rounded up to 4 because a prompt
/// carrying screenshots is several times larger; the 30 s probe re-reads real
/// available memory, so heavier calls shrink the ceiling on their own.
const MB_PER_LLM_CALL: u64 = 4;

/// Floor — never throttle below this.
const MIN_PERMITS: usize = 2;

/// Adaptive global concurrency controller for LLM and tool execution.
///
/// LLM calls take a permit from ONE pool per bot (auditor Rule 15.2). Its size
/// is the smaller of two bounds: the machine's (memory and load, from the
/// resource probe) and the provider's (halved on a 429 wave, +1 per
/// successful call). Shrinking never waits for free permits: permits that are
/// checked out when a cut lands are retired as their calls finish.
pub struct ConcurrencyController {
    llm_semaphore: Arc<Semaphore>,
    llm_pool: Arc<Mutex<LlmPool>>,
    /// Floor — never go below this.
    min_permits: usize,
    /// Tool-level concurrency per turn.
    tool_semaphore: Arc<Semaphore>,
    /// Background LLM work (summaries, objective detection, personality
    /// synthesis) takes permits here, never from the pool user turns wait on:
    /// a burst of housekeeping must not queue a person's reply.
    background_semaphore: Arc<Semaphore>,
    /// Absolute max permits — configured limit, or DEFAULT_MAX_CEILING when auto.
    max_ceiling: usize,
}

/// The LLM pool's books. Every change goes through `rebalance`.
struct LlmPool {
    /// Permits that exist: free in the semaphore plus checked out.
    capacity: usize,
    /// Checked-out permits to retire when they come back instead of freeing
    /// them — the part of a cut larger than the free permits at that moment.
    debt: usize,
    /// The machine's bound (memory and load), set by the resource probe.
    ceiling: usize,
    /// The provider's bound: halved per 429 wave, +1 per successful call.
    rate_target: usize,
    /// Bumped on every 429 cut. A permit carries the round it was granted
    /// in, so the 429s of calls that started before a cut do not cut again —
    /// fifty calls rejected together are one signal, not fifty halvings.
    round: u64,
}

/// A checked-out LLM permit. Dropping it frees the slot, or retires it when
/// the pool owes a cut.
pub struct LlmPermit {
    permit: Option<OwnedSemaphorePermit>,
    pool: Arc<Mutex<LlmPool>>,
    round: u64,
}

impl LlmPermit {
    /// The 429 round this permit was granted in — pass it to
    /// `report_rate_limit` so one wave of rejections cuts once.
    pub fn round(&self) -> u64 {
        self.round
    }
}

impl Drop for LlmPermit {
    fn drop(&mut self) {
        let Some(permit) = self.permit.take() else { return };
        let mut pool = self.pool.lock().unwrap();
        if pool.debt > 0 {
            pool.debt -= 1;
            pool.capacity -= 1;
            permit.forget();
        }
    }
}

impl ConcurrencyController {
    /// Create a new controller. `max_concurrent_runs` is the configured hard cap
    /// on in-flight LLM calls; None = auto (resource monitor trims from
    /// DEFAULT_MAX_CEILING based on available memory and load).
    pub fn new(max_concurrent_runs: Option<usize>) -> Self {
        let max_ceiling = max_concurrent_runs
            .unwrap_or(DEFAULT_MAX_CEILING)
            .max(MIN_PERMITS);

        let controller = Self {
            llm_semaphore: Arc::new(Semaphore::new(max_ceiling)),
            llm_pool: Arc::new(Mutex::new(LlmPool {
                capacity: max_ceiling,
                debt: 0,
                ceiling: max_ceiling,
                rate_target: max_ceiling,
                round: 0,
            })),
            min_permits: MIN_PERMITS,
            // Tools run locally (processes, files, browser) and compete for
            // cores, so their pool is sized from the machine — never derived
            // from the LLM ceiling, whose permits only wait on the network.
            // 4 per core because most tool time is I/O wait, not compute.
            tool_semaphore: Arc::new(Semaphore::new(
                std::thread::available_parallelism()
                    .map(|n| n.get() * 4)
                    .unwrap_or(16)
                    .max(8),
            )),
            background_semaphore: Arc::new(Semaphore::new((max_ceiling / 4).max(1))),
            max_ceiling,
        };

        // Trim to current memory immediately — the first monitor probe is 30s
        // out, and a small machine restarting into a queue of pending work
        // could overcommit before it lands. Same heuristic as the monitor.
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        let mem_limit = (sys.available_memory() / 1_048_576 / MB_PER_LLM_CALL) as usize;
        controller.set_ceiling(mem_limit.min(max_ceiling));

        controller
    }

    /// Absolute max permits (configured cap or default).
    pub fn max_ceiling(&self) -> usize {
        self.max_ceiling
    }

    /// Acquire a permit for an LLM call. Blocks when at capacity.
    pub async fn acquire_llm_permit(&self) -> LlmPermit {
        let permit = self
            .llm_semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("llm semaphore closed");
        let round = self.llm_pool.lock().unwrap().round;
        LlmPermit { permit: Some(permit), pool: self.llm_pool.clone(), round }
    }

    /// Acquire a permit for background LLM work. Separate, smaller pool (a
    /// quarter of the ceiling, at least one) so housekeeping never blocks a
    /// user's turn; callers do not retry on overload.
    pub async fn acquire_background_permit(&self) -> OwnedSemaphorePermit {
        self.background_semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("background semaphore closed")
    }

    /// Acquire a permit for parallel tool execution within a turn.
    pub async fn acquire_tool_permit(&self) -> OwnedSemaphorePermit {
        self.tool_semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("tool semaphore closed")
    }

    /// A call finished without a rate limit: the provider's bound grows by
    /// one, back toward the machine's.
    pub fn report_success(&self) {
        let mut pool = self.llm_pool.lock().unwrap();
        if pool.rate_target < self.max_ceiling {
            pool.rate_target += 1;
            self.rebalance(&mut pool);
        }
    }

    /// A call was rate limited (429). Halve the provider's bound to half the
    /// calls in flight right now — once per wave: a call granted before the
    /// last cut (`granted_round` older than the pool's) reports the same wave
    /// that cut already answered.
    pub fn report_rate_limit(&self, granted_round: u64) {
        let mut pool = self.llm_pool.lock().unwrap();
        if granted_round < pool.round {
            return;
        }
        let in_flight = (pool.capacity - pool.debt)
            .saturating_sub(self.llm_semaphore.available_permits());
        pool.rate_target = (in_flight / 2).max(self.min_permits);
        pool.round += 1;
        info!(
            in_flight,
            rate_target = pool.rate_target,
            round = pool.round,
            "rate limit: halving LLM concurrency"
        );
        self.rebalance(&mut pool);
    }

    /// Set the machine's bound — called by the resource monitor.
    pub fn set_ceiling(&self, new_ceiling: usize) {
        let mut pool = self.llm_pool.lock().unwrap();
        pool.ceiling = new_ceiling.max(self.min_permits);
        self.rebalance(&mut pool);
    }

    /// Bring the pool to min(machine bound, provider bound). Growing pays
    /// down owed retirements before adding permits; shrinking retires free
    /// permits now and owes the rest, collected as busy calls return theirs.
    fn rebalance(&self, pool: &mut LlmPool) {
        let target = pool.ceiling.min(pool.rate_target).max(self.min_permits);
        let effective = pool.capacity - pool.debt;
        if target < effective {
            let mut cut = effective - target;
            while cut > 0 {
                match self.llm_semaphore.clone().try_acquire_owned() {
                    Ok(free) => {
                        free.forget();
                        pool.capacity -= 1;
                        cut -= 1;
                    }
                    Err(_) => break,
                }
            }
            pool.debt += cut;
        } else if target > effective {
            let grow = target - effective;
            let forgiven = grow.min(pool.debt);
            pool.debt -= forgiven;
            let added = grow - forgiven;
            self.llm_semaphore.add_permits(added);
            pool.capacity += added;
        }
        debug!(
            target,
            capacity = pool.capacity,
            debt = pool.debt,
            "LLM pool rebalanced"
        );
    }

    /// Permits the pool is sized to right now (owed retirements excluded).
    pub fn effective_permits(&self) -> usize {
        let pool = self.llm_pool.lock().unwrap();
        pool.capacity - pool.debt
    }

    /// The machine's bound (memory and load).
    pub fn ceiling(&self) -> usize {
        self.llm_pool.lock().unwrap().ceiling
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

            // LLM permits wait on network streams, not compute — memory and
            // *measured* load govern the ceiling; core count is not a cap.
            let mem_limit = (available_mb / MB_PER_LLM_CALL) as usize;
            let load_factor = if load > cpu_cores as f64 {
                (cpu_cores as f64 / load).max(0.3)
            } else {
                1.0
            };

            let ceiling = ((mem_limit as f64) * load_factor) as usize;
            controller.set_ceiling(ceiling.min(controller.max_ceiling()));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_controller_creation() {
        let ctrl = ConcurrencyController::new(None);
        assert!(ctrl.effective_permits() >= 2);
        assert_eq!(ctrl.ceiling(), ctrl.effective_permits());
    }

    #[tokio::test]
    async fn test_acquire_llm_permit() {
        let ctrl = ConcurrencyController::new(None);
        let permit = ctrl.acquire_llm_permit().await;
        assert!(ctrl.effective_permits() >= 2);
        drop(permit);
    }

    #[tokio::test]
    async fn test_acquire_tool_permit() {
        let ctrl = ConcurrencyController::new(None);
        let permit = ctrl.acquire_tool_permit().await;
        drop(permit);
    }

    #[test]
    fn test_configured_cap() {
        let ctrl = ConcurrencyController::new(Some(4));
        assert_eq!(ctrl.max_ceiling(), 4);
        // Boot probe may trim below the cap on a memory-starved machine,
        // but never above it and never below the floor.
        assert!(ctrl.ceiling() <= 4);
        assert!(ctrl.ceiling() >= MIN_PERMITS);
        assert_eq!(ctrl.effective_permits(), ctrl.ceiling());
    }

    #[test]
    fn test_configured_cap_clamps_to_min() {
        let ctrl = ConcurrencyController::new(Some(1));
        assert_eq!(ctrl.max_ceiling(), MIN_PERMITS);
    }

    #[test]
    fn test_auto_defaults_to_max_ceiling() {
        let ctrl = ConcurrencyController::new(None);
        assert_eq!(ctrl.max_ceiling(), DEFAULT_MAX_CEILING);
        // Boot probe trims to available memory; effective tracks the ceiling.
        assert!(ctrl.ceiling() <= DEFAULT_MAX_CEILING);
        assert!(ctrl.ceiling() >= MIN_PERMITS);
        assert_eq!(ctrl.effective_permits(), ctrl.ceiling());
    }

    #[test]
    fn test_set_ceiling_clamps_to_min() {
        let ctrl = ConcurrencyController::new(None);
        ctrl.set_ceiling(1); // Below min_permits (2)
        assert!(ctrl.ceiling() >= 2);
    }

    /// A 429 cut lands while every permit is checked out: nothing is free to
    /// take, so the cut is owed and collected as the busy calls return.
    #[tokio::test]
    async fn test_rate_limit_cut_lands_when_every_permit_is_busy() {
        let ctrl = ConcurrencyController::new(Some(8));
        ctrl.set_ceiling(8);
        let mut busy = Vec::new();
        for _ in 0..8 {
            busy.push(ctrl.acquire_llm_permit().await);
        }
        let round = busy[0].round();
        ctrl.report_rate_limit(round);
        assert_eq!(ctrl.effective_permits(), 4, "halved to half of the 8 in flight");
        drop(busy);
        assert_eq!(
            ctrl.llm_semaphore.available_permits(),
            4,
            "four returning permits were retired, four freed"
        );
    }

    /// Fifty calls rejected together are one signal: the first 429 of a
    /// round cuts; the rest of that round's 429s do not cut again.
    #[tokio::test]
    async fn test_one_wave_of_429s_cuts_once() {
        let ctrl = ConcurrencyController::new(Some(16));
        ctrl.set_ceiling(16);
        let mut wave = Vec::new();
        for _ in 0..16 {
            wave.push(ctrl.acquire_llm_permit().await);
        }
        for p in &wave {
            ctrl.report_rate_limit(p.round());
        }
        assert_eq!(ctrl.effective_permits(), 8, "one cut, not sixteen");
        drop(wave);
        // A call granted after the cut is a new round: it may cut again.
        let next = ctrl.acquire_llm_permit().await;
        ctrl.report_rate_limit(next.round());
        assert_eq!(ctrl.effective_permits(), MIN_PERMITS, "half of the one call in flight, floored");
    }

    /// Recovery needs no header and no probe: every successful call grows
    /// the provider's bound by one, up to the machine's.
    #[tokio::test]
    async fn test_successes_recover_to_the_ceiling() {
        let ctrl = ConcurrencyController::new(Some(8));
        ctrl.set_ceiling(8);
        let mut busy = Vec::new();
        for _ in 0..8 {
            busy.push(ctrl.acquire_llm_permit().await);
        }
        ctrl.report_rate_limit(busy[0].round());
        drop(busy);
        assert_eq!(ctrl.effective_permits(), 4);
        for _ in 0..10 {
            ctrl.report_success();
        }
        assert_eq!(ctrl.effective_permits(), 8, "back to the machine's bound, not past it");
        assert_eq!(ctrl.llm_semaphore.available_permits(), 8);
    }

    /// The memory trim shrinks a busy pool the same way a 429 does.
    #[tokio::test]
    async fn test_ceiling_trim_lands_when_every_permit_is_busy() {
        let ctrl = ConcurrencyController::new(Some(8));
        ctrl.set_ceiling(8);
        let mut busy = Vec::new();
        for _ in 0..8 {
            busy.push(ctrl.acquire_llm_permit().await);
        }
        ctrl.set_ceiling(3);
        assert_eq!(ctrl.effective_permits(), 3);
        drop(busy);
        assert_eq!(ctrl.llm_semaphore.available_permits(), 3);
    }
}
