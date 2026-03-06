# Concurrency Patterns: Go to Rust Migration

**Source:** `nebo/docs/sme/CONCURRENCY.md` | **Target:** `nebo-rs/crates/` | **Status:** Draft

---

## Table of Contents

1. [Why This Doc Is Minimal](#1-why-this-doc-is-minimal)
2. [Go Primitive to Rust Primitive Mapping](#2-go-primitive-to-rust-primitive-mapping)
3. [Patterns That Disappear in Rust](#3-patterns-that-disappear-in-rust)
4. [Patterns That Need New Approaches](#4-patterns-that-need-new-approaches)
5. [Key Decisions Made in nebo-rs](#5-key-decisions-made-in-nebo-rs)

---

## 1. Why This Doc Is Minimal

The Go CONCURRENCY.md documents ~50 mutexes across 15+ subsystems, a 6-tier lock ordering
hierarchy, 3 critical data races, 5 high-severity lock contention bugs, and 10 medium/low issues.
Two full sprints were spent fixing these.

Rust's ownership system eliminates the majority of these problems at compile time:

- **Data races are impossible.** The borrow checker enforces exclusive mutable access. The Go
  critical findings (C-01 through C-03) -- unprotected fields accessed from multiple goroutines --
  cannot compile in Rust.
- **Forgotten unlocks are impossible.** RAII guards (`MutexGuard`, `RwLockGuard`) release
  automatically when dropped. The Go high findings (H-01: lock held across I/O) require
  deliberate `.lock()` scope management in Rust, but the compiler prevents the "forgot to
  unlock" class entirely.
- **Nil map panics are impossible.** `Option<T>` forces explicit handling. Go's `sync.Map`
  type-assertion panics have no equivalent.
- **Lock ordering is less critical.** Rust's type system does NOT enforce lock ordering, but the
  reduced mutex count (~15 vs ~50) and async architecture make ordering violations far less
  likely.

**Bottom line:** The Go doc is a remediation guide for bugs that already shipped. This doc is a
reference for the primitives chosen and the few concurrency patterns that still require attention.

---

## 2. Go Primitive to Rust Primitive Mapping

### 2.1 Core Mapping Table

| Go Primitive | Rust Equivalent in nebo-rs | When to Use |
|---|---|---|
| `sync.Mutex` | `tokio::sync::Mutex` | Protects state held across `.await` points |
| `sync.Mutex` | `std::sync::Mutex` | Short critical sections, never held across `.await` |
| `sync.RWMutex` | `tokio::sync::RwLock` | Read-heavy async state (registries, session maps) |
| `sync.RWMutex` | `std::sync::RwLock` | Read-heavy sync state (config, hook maps) |
| `sync.Map` | `HashMap` behind `RwLock` | All cases -- no `DashMap` dependency needed |
| `sync.WaitGroup` | `tokio::task::JoinSet` | Awaiting a group of spawned tasks |
| `chan T` (unbounded) | `tokio::sync::mpsc::unbounded_channel` | Fire-and-forget message passing |
| `chan T` (bounded) | `tokio::sync::mpsc::channel(n)` | Backpressure-aware pipelines |
| `chan T` (one-shot) | `tokio::sync::oneshot` | Single request/response correlation |
| N/A | `tokio::sync::broadcast` | Fan-out to multiple subscribers (client hub) |
| N/A | `tokio::sync::Notify` | Wake a pump loop (lane processing) |
| N/A | `tokio::sync::Semaphore` | Concurrency limiting (orchestrator max agents) |
| `atomic.Bool` | `std::sync::atomic::AtomicBool` | Lock-free boolean flags |
| `atomic.Int32` | `std::sync::atomic::AtomicU32` | Lock-free counters / state enums |
| `atomic.Value` | `Arc<T>` + swap, or `ArcSwap` | Lock-free pointer replacement |
| `context.Context` | `tokio_util::sync::CancellationToken` | Cooperative cancellation |
| `context.WithCancel` | `CancellationToken::child_token()` | Scoped cancellation hierarchy |

### 2.2 Tokio vs Std Decision Guide

```
Will the lock be held across an .await point?
  YES -> tokio::sync::Mutex / tokio::sync::RwLock
  NO  -> std::sync::Mutex / std::sync::RwLock (cheaper, no async overhead)
```

**File(s):** Key examples of this decision in nebo-rs:

```rust
// std::sync::Mutex -- short lock, never held across .await
// crates/agent/src/lanes.rs
lanes: HashMap<String, (Arc<std::sync::Mutex<LaneState>>, Arc<Notify>)>,

// tokio::sync::Mutex -- held across .await (process I/O)
// crates/tools/src/process.rs
running: Arc<Mutex<HashMap<String, Arc<BackgroundSession>>>>,

// std::sync::RwLock -- config/hooks, read-heavy, sync-only
// crates/apps/src/hooks.rs
// crates/browser/src/snapshot_store.rs

// tokio::sync::RwLock -- async registry operations
// crates/tools/src/registry.rs
tools: Arc<RwLock<HashMap<String, Box<dyn DynTool>>>>,
```

### 2.3 Performance Characteristics (Rust)

| Mechanism | Contention Model | Can Deadlock | Uncontended Cost |
|---|---|---|---|
| `std::sync::Mutex` | Blocks thread | Yes (if misordered) | ~15ns |
| `std::sync::RwLock` | Blocks writers, readers concurrent | Yes (if misordered) | ~25ns |
| `tokio::sync::Mutex` | Yields task (async) | Yes (if misordered) | ~30ns |
| `tokio::sync::RwLock` | Yields task (async) | Yes (if misordered) | ~40ns |
| `tokio::sync::broadcast` | Lock-free reads | No | ~20ns |
| `tokio::sync::mpsc` | Lock-free send (bounded) | No (send fails) | ~30ns |
| `AtomicBool` / `AtomicU32` | Lock-free | No | ~1ns |
| `CancellationToken` | Lock-free check | No | ~2ns |

---

## 3. Patterns That Disappear in Rust

These are Go concurrency bugs from the source doc that CANNOT occur in Rust.

### 3.1 Data Races on Unprotected Fields

**Go findings eliminated:** C-01 (adapter handler fields), C-02 (dmRelayer field)

In Go, any field can be read/written from any goroutine without synchronization. The race
detector catches some at runtime, but not all. In Rust, shared mutable state requires explicit
synchronization. The compiler rejects code that does NOT use `Mutex`, `RwLock`, `Arc`, or
atomics for cross-task access.

```go
// Go -- compiles, races at runtime
type Plugin struct {
    handler func(Message) // no protection
}

// Called from goroutine A
func (p *Plugin) SetHandler(h func(Message)) { p.handler = h }

// Called from goroutine B
func (p *Plugin) Handle(m Message) { p.handler(m) }
```

```rust
// Rust -- does NOT compile without synchronization
// The compiler enforces Send + Sync bounds on shared state
struct Plugin {
    handler: Arc<RwLock<Option<Box<dyn Fn(Message) + Send + Sync>>>>,
}
```

### 3.2 Forgotten Unlocks and Lock-Held-Across-I/O

**Go findings eliminated:** H-01 (AppProcess.stop held lock 2+ seconds)

Go mutexes require explicit `Unlock()` calls. Forgetting one, or holding a lock across a slow
I/O path, is a logic error that compiles and ships. Rust RAII guards make this structurally
impossible -- the guard drops at scope end.

```go
// Go -- compiles, holds lock for 2+ seconds
func (p *AppProcess) stop() {
    p.mu.Lock()
    defer p.mu.Unlock()
    p.conn.Close()           // gRPC close: slow
    p.cmd.Process.Kill()     // process kill: slow
    <-p.waitDone             // blocks until exit
}
```

```rust
// Rust -- natural scoping forces the right pattern
async fn stop(&self) {
    // Snapshot under lock, release immediately
    let (conn, child) = {
        let mut guard = self.state.lock().await;
        (guard.conn.take(), guard.child.take())
    };
    // I/O happens OUTSIDE the lock scope -- guard already dropped
    if let Some(conn) = conn { conn.close().await; }
    if let Some(mut child) = child { let _ = child.kill().await; }
}
```

### 3.3 Nil Map / Type Assertion Panics

**Go findings eliminated:** Various `sync.Map` type assertion risks

Go's `sync.Map` returns `interface{}`, requiring type assertions that panic on mismatch.
Rust's `HashMap` is generic and type-safe. `Option<T>` replaces nil checks.

### 3.4 TOCTOU on Unprotected Check-Then-Act

**Go findings eliminated:** H-02 (deregisterCapabilities TOCTOU)

In Go, read-under-RLock then write-under-Lock creates a window where state can change.
Rust's borrow checker prevents this within a single scope. Across async boundaries, the same
pattern CAN recur (see Section 4), but it requires explicit `.lock().await` calls that make the
gap visible in code review.

### 3.5 Summary: Go Finding Disposition in Rust

| ID | Go Severity | Go Description | Rust Status |
|---|---|---|---|
| C-01 | Critical | Adapter handler data race | Eliminated -- ownership |
| C-02 | Critical | dmRelayer data race | Eliminated -- ownership |
| C-03 | Critical | contentBlocks use-after-free | Eliminated -- borrowing (was false positive in Go too) |
| H-01 | High | Lock held across I/O | Eliminated -- RAII scoping |
| H-02 | High | TOCTOU in deregister | Eliminated -- single lock scope |
| H-03 | High | Dual locking on same data | Eliminated -- type system (one lock per field) |
| H-04 | High | DB call under lock | Requires discipline (see 4.2) |
| H-05 | High | Voice state contention | Eliminated -- atomics are idiomatic |
| M-01 | Medium | Channel send under lock | Reduced -- broadcast is lock-free |
| M-02 | Medium | Nested locking in loop | Eliminated -- RAII + clone |
| M-03 | Medium | Scheduler call under lock | Reduced -- async scheduler |
| M-04 | Medium | File I/O under write lock | Requires discipline (see 4.2) |
| M-05 | Medium | TOCTOU in supervisor | Reduced -- Arc<T> ownership |
| M-06 | Medium | Handler staleness | Eliminated -- Arc cloning is atomic |
| M-07 | Medium | Multiple independent mutexes | Reduced -- fewer mutexes total |
| L-01 | Low | Unused mutex field | Eliminated -- compiler warns on dead code |
| L-02 | Low | Shared write mutex | Same pattern exists (see 5.4) |
| L-03 | Low | Map aliasing from config | Eliminated -- ownership transfer |

---

## 4. Patterns That Need New Approaches

### 4.1 Async Cancellation

**Go approach:** `context.Context` propagated everywhere, checked with `select` on `ctx.Done()`.

**Rust approach:** `tokio_util::sync::CancellationToken` with structured hierarchy.

**File(s):** `crates/agent/src/orchestrator.rs`, `crates/server/src/handlers/ws.rs`

```rust
// crates/agent/src/orchestrator.rs
let cancel = CancellationToken::new();
let child_cancel = cancel.child_token();

tokio::select! {
    result = run_agent(child_cancel.clone()) => { /* completed */ }
    _ = cancel.cancelled() => { /* parent cancelled */ }
}
```

**Key difference from Go:** Rust tasks do NOT automatically stop when a `CancellationToken` is
cancelled. Every long-running loop MUST check `cancel.is_cancelled()` or use `tokio::select!`
with `cancel.cancelled()`. Forgetting this means orphaned tasks -- the Rust equivalent of a
goroutine leak.

### 4.2 Lock Scope Discipline Across Await

Rust prevents forgotten unlocks but does NOT prevent holding a `tokio::sync::Mutex` across an
`.await` point. This blocks other tasks waiting on the same mutex for the entire duration of the
awaited future.

```rust
// BAD -- lock held across .await (compiles, but starves other tasks)
let mut guard = self.state.lock().await;
guard.data = expensive_db_call().await;  // other tasks blocked here

// GOOD -- snapshot, release, compute, re-acquire
let old_data = {
    let guard = self.state.lock().await;
    guard.data.clone()
};
let new_data = expensive_db_call().await;
{
    let mut guard = self.state.lock().await;
    guard.data = new_data;
}
```

This is the Rust equivalent of Go's H-04 (DB call under lock). The compiler does NOT catch it.
Use `clippy::await_holding_lock` to lint for this.

### 4.3 Deadlock Detection

Rust has no built-in deadlock detector equivalent to Go's `-race` flag. Strategies:

| Approach | When to Use |
|---|---|
| `clippy::await_holding_lock` | CI lint -- catches tokio mutex held across .await |
| `std::sync::Mutex` poisoning | Detects panics inside critical sections at runtime |
| `tokio-console` | Runtime introspection of task blocking (dev only) |
| Minimal lock count | Best defense -- nebo-rs has ~15 mutexes vs Go's ~50 |

### 4.4 Shared Mutable State via Arc

In Go, shared state is implicit -- any goroutine can access any field on a shared struct. In
Rust, `Arc<T>` makes sharing explicit but introduces a new concern: cloning `Arc` references
everywhere can make ownership graphs hard to follow.

**nebo-rs convention:** Shared services are constructed once in `server/src/lib.rs` and passed
as `Arc<T>` fields on `AppState`. Tools and handlers receive owned `Arc` clones. This is a flat,
non-hierarchical ownership model -- intentionally simple.

```rust
// crates/server/src/state.rs -- flat Arc ownership
pub struct AppState {
    pub store: Arc<Store>,
    pub auth: Arc<AuthService>,
    pub hub: Arc<ClientHub>,
    pub runner: Arc<Runner>,
    pub tools: Arc<Registry>,
    pub bridge: Arc<mcp::Bridge>,
    // ...
}
```

---

## 5. Key Decisions Made in nebo-rs

### 5.1 Broadcast for Client Hub (replaces Go RWMutex + map iteration)

**Go pattern:** `Hub.mu.RLock()` -> iterate `clients` map -> send to each `client.send` channel
-> `RUnlock()`. Finding M-01 noted that channel sends under read lock hold the lock longer than
necessary.

**Rust pattern:** `tokio::sync::broadcast::channel(256)`. Subscribers get their own receiver.
No lock needed for fan-out.

**File(s):** `crates/server/src/handlers/ws.rs`

```rust
pub struct ClientHub {
    tx: broadcast::Sender<HubEvent>,
}

impl ClientHub {
    pub fn broadcast(&self, event: HubEvent) {
        let _ = self.tx.send(event); // lock-free, drops if no subscribers
    }

    pub fn subscribe(&self) -> broadcast::Receiver<HubEvent> {
        self.tx.subscribe()
    }
}
```

### 5.2 Semaphore for Orchestrator (replaces Go mutex + counter)

**Go pattern:** `o.mu.Lock()` -> check `len(agents) < maxConcurrent` -> register -> unlock.
Finding H-04 noted DB persistence happened inside the lock.

**Rust pattern:** `tokio::sync::Semaphore` with `acquire()`. Concurrency limiting is built into
the primitive -- no manual counting, no lock-then-check-then-unlock sequence.

**File(s):** `crates/agent/src/orchestrator.rs`

```rust
pub struct Orchestrator {
    semaphore: Arc<Semaphore>,
    active: Arc<RwLock<HashMap<String, ActiveAgent>>>,
    // ...
}

pub async fn spawn(&self, task: Task) -> Result<()> {
    let permit = self.semaphore.acquire().await?; // blocks if at capacity
    // DB persistence happens here -- OUTSIDE any mutex
    let agent = self.start_agent(task).await?;
    self.active.write().await.insert(agent.id.clone(), agent);
    // permit is held until agent completes (moved into spawned task)
    Ok(())
}
```

### 5.3 Notify for Lane Pump (replaces Go channel signaling)

**Go pattern:** Lane pump goroutine wakes on channel receive, processes queue under mutex.

**Rust pattern:** `tokio::sync::Notify` for wake-up, `std::sync::Mutex` for queue state. The
`Notify` primitive is lighter than a channel when the only signal is "something changed."

**File(s):** `crates/agent/src/lanes.rs`

```rust
// Lane state uses std::sync::Mutex (never held across .await)
lanes: HashMap<String, (Arc<std::sync::Mutex<LaneState>>, Arc<Notify>)>,

// Enqueue: push task, notify pump
fn enqueue(&self, lane: &str, task: LaneTask) {
    let (state, notify) = self.lanes.get(lane).unwrap();
    state.lock().unwrap().queue.push_back(task);
    notify.notify_one();
}
```

### 5.4 Oneshot for Request/Response Correlation

**Go pattern:** Pending maps with `sync.RWMutex` protecting `map[string]chan<- Response`.

**Rust pattern:** `tokio::sync::oneshot` channels stored in a `Mutex<HashMap>`. The oneshot is
consumed on send, making double-response impossible at the type level.

**File(s):** `crates/server/src/state.rs`

```rust
pub approval_channels: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
pub ask_channels: Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>,
```

### 5.5 No DashMap

nebo-rs does NOT use `DashMap`. All concurrent maps use `RwLock<HashMap>` (tokio or std).
This avoids an external dependency for a single-user system where contention is minimal. If
profiling shows `RwLock<HashMap>` as a bottleneck, `DashMap` is a drop-in replacement.

### 5.6 Lock Count Comparison

| Subsystem | Go Mutexes | Rust Mutexes | Notes |
|---|---|---|---|
| Agent Hub / Lanes | 8 | 2 | broadcast + Notify replace most |
| Client Hub / Chat | 6 | 1 | broadcast::channel replaces 5 |
| Tools / Registry | 8 | 3 | flatter registry, no sync.Map |
| Process Registry | 3 | 2 | running + finished maps |
| Apps Platform | 8 | 3 | supervisor + registry + hooks |
| Orchestrator | 1 | 1 | Semaphore handles limiting separately |
| MCP / Bridge | 3 | 2 | client + bridge |
| Comm | 2 | 1 | RwLock on manager |
| Browser | 2 | 2 | manager + session (same pattern) |
| Voice / Other | 6 | N/A | NOT yet ported |
| **Total** | **~50** | **~17** | **66% reduction** |

The reduction comes from three sources: broadcast channels replacing fan-out mutexes, Semaphore
replacing mutex + counter patterns, and the elimination of `sync.Map` (which counted as separate
synchronization points in Go).

---

*Generated: 2026-03-04*
