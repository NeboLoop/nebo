# SMB Appliance Scaling — 250+ Employees on One Box

**Goal:** place a Mac mini in a company running only Nebo and have it serve as the
assistant for 250+ employees (one agent per employee, traffic arriving over the
single multiplexed NeboLoop WebSocket).

**Sizing model:** 250 employees ≈ 5–15% concurrently active at peak → the box must
comfortably sustain **15–40 concurrent runs with bursts to ~75** (Monday 9am).
Idle agents are effectively free — a loaded agent is a config struct in
`AgentLoader`, no task, no process, no connection.

## What shipped (2026-06-10)

### 1. Config-driven concurrency ceiling, CPU-core cap removed
`crates/agent/src/concurrency.rs`, `crates/config/src/config.rs`, `etc/nebo.yaml`,
`crates/server/src/lib.rs`

- **Bug found:** a tokio semaphore can never grow past its initial permit count —
  `set_ceiling` only releases previously held-back permits. The old
  `(cores*2).min(20)` initial value meant the "adaptive up to 50" clamp was
  unreachable; a 10-core Mac mini was pinned at 20 permits forever.
- The controller now starts at the max (configured value, or `DEFAULT_MAX_CEILING
  = 100`) and the 30s resource monitor **trims down** from there.
- CPU-core heuristic removed: an LLM permit is spent waiting on a network stream,
  not compute. Memory (`available_mb / 200`) × measured load factor governs the
  ceiling; provider rate limits remain the real throttle via the existing 429
  backpressure machinery.
- Config: `Runtime: MaxConcurrentRuns` in `etc/nebo.yaml` (0 = auto) or env
  `NEBO_MAX_CONCURRENT_RUNS`.
- Global tool semaphore scales with capacity: `(max/4).max(8)` instead of a
  hardcoded 8.

### 2. Per-agent fairness in the lane system
`crates/agent/src/lanes.rs`, `crates/server/src/chat_dispatch.rs`

- `LaneTask.fairness_key` (set to the agent id at both `run_chat` enqueue sites) +
  `MAX_ACTIVE_PER_KEY = 2` on top-level lanes. The pump picks the first
  **eligible** queued task instead of the FIFO head, so one employee's heavy run
  can't starve the other 249.
- `subagent` / `nested` / `desktop` lanes have `per_key_cap = 0` — parents wait on
  children there; capping them could deadlock.

### 3. Concurrent cron dispatch with stagger
`crates/server/src/scheduler.rs`, `crates/db/src/queries/cron_jobs.rs`

- **Bug found:** the tick loop awaited each due job **to completion,
  sequentially**. 250 morning jobs would run one at a time (last one starting
  hours late) and a single long job blocked every other cron and the tick loop.
- Due jobs are now spawned concurrently; `last_run` is consumed **at dispatch**
  so the next tick can't double-fire a job still in flight; starts stagger at
  1 job/sec so the 9:00 herd ramps instead of spiking.
- New `update_cron_job_last_error` records the run outcome without re-bumping
  `run_count`.

**Result:** a 16GB mini lands around ~50 concurrent runs (memory-governed), 32GB
hits the full 100. The binding constraints are now provider rate limits (handled
by backpressure) and token cost — not local caps.

## Follow-ups, in priority order

### 1. Cron crash window — DONE 2026-06-11
Marking `last_run` at dispatch flipped semantics from at-least-once to
**at-most-once**: a process restart between dispatch and completion consumed
the occurrence without running it. **Shipped:** `recover_interrupted_jobs` in
`scheduler.rs` runs once at startup (before the first tick, so no live runs
have open history rows): re-fires enabled jobs whose dangling `cron_history`
row (`finished_at IS NULL`) started within the last 24h, closes ALL dangling
rows as `failed: interrupted by restart`, and does **not** bump `last_run`
(the occurrence was already consumed at original dispatch). Both tick and the
sweep dispatch through one shared `spawn_job_run`.

### 2. Boot window with the semaphore wide open — DONE 2026-06-11
The controller started at 100 permits with the first resource probe ~30s out.
**Shipped:** `ConcurrencyController::new()` now runs the memory probe
synchronously and trims the ceiling immediately, same heuristic as the monitor.
**Bonus latent bug found by this change:** `report_rate_limit` and
`set_ceiling` stored `effective - held.len()` using the *total* held count
instead of the *newly acquired* count — with permits already held (boot trim,
or any monitor trim before a 429), `effective_permits` underflowed (debug
panic / release garbage). Both now count only newly held permits. This bug
predated the scaling work; it was reachable any time the monitor trimmed the
ceiling before a rate limit hit.

### 3. Measure before trusting the constants
Three values are educated guesses, not measurements:

| Constant | Where | Guess |
|---|---|---|
| 200MB per run | monitor `available_mb / 200` | per-run memory footprint |
| `max/4` tool permits | `ConcurrencyController::new` | local tool parallelism |
| Cap of 2 per agent | `MAX_ACTIVE_PER_KEY` | per-employee concurrency |

Build a synthetic load harness — 250 fake agents firing through the comm path
against live Janus (never mock) — and watch permit wait time, lane queue depth,
and actual RSS per active run. Then set the constants from data, one variable
per round, ship/revert before touching the next.

### 4. Per-provider backpressure
The controller is global: one 429 from any provider halves concurrency for
**everyone**, even agents Janus routed to a different model. At company scale,
one rate-limited cheap model can throttle the whole fleet. Right shape:
per-provider permit pools, or at minimum per-provider backpressure flags. This
is a real refactor of the controller — the next structural limit after the ones
already removed.

### 5. Ops story — the appliance is a single point of failure
A Mac mini serving 250 people, with a SQLite file as the company's memory.
Before more concurrency work:

- launchd keep-alive wired to the existing health check
- scheduled backup of `~/.nebo` off the box
- NeboLoop reconnect hardening (ghost connections from non-graceful close, the
  30s reconnect poll instead of `wait_disconnect()`) — at this scale a dropped
  socket is a whole-company outage, not an annoyance
- surface the existing `lanes.status()` and `effective_permits()` stats on an
  admin-visible dashboard. "Which agent is hogging permits at 9am Monday" must
  be answerable in ten seconds.

## Deliberate non-adjustment

The per-agent fairness cap stays a constant, not a config knob. Two concurrent
runs per agent is a behavior decision; every knob on an appliance is a support
ticket.
