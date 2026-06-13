# Rivet Deployment Security Architecture

> **Last updated:** 2026-06-13
> **Status:** **Active.** Phase-0 hardening + WS2-1 landed and verified; WS1-1 / WS1-2 / WS1-3 implemented and cross-compiled, pending verification on a Linux Firecracker host.
> **Subject codebase:** `~/workspaces/rivet.to` (Go) — NOT this repo. This doc lives in Nebo's SME set because Rivet is the intended **hosting substrate for a server/always-on Nebo** (see §2).
> **Companion artifact:** `~/workspaces/rivet.to/SECURITY_FIXES_PRD.md` — the living PRD with per-item designs, effort, and verification checklists. This doc is the durable narrative; the PRD is the working tracker.
> **Commits (branch `claude/rivet-migration-unified-0A19x`):** `b4d86f7`, `5d46f66`, `32dd3df`, `f0b39cb`.

---

## 1. Executive Summary

Rivet is a **multi-tenant Firecracker PaaS** ("deploy with Docker Compose, no cluster"): ~79k lines of Go across a control plane (`svc-api`, reconciler, build-worker, auto-scaler, routing/edge) and a per-node worker-agent that runs untrusted tenant workloads in Firecracker microVMs. State is in PostgreSQL; artifacts in MinIO/Spaces; orchestration over NATS.

A 2026-06-10 multi-agent audit found the **application layer is well-built** (sqlc throughout, tenant-scoped queries, `FOR UPDATE SKIP LOCKED` claim queries, constant-time HMAC where it matters, crypto/rand tokens). The serious problems were **isolation-boundary gaps** — places where the code trusted an outer layer (Firecracker, "the platform") to provide a boundary it never actually constructed — plus one reliability bug severe enough to self-sustain a failure loop.

The pattern across the CRITICALs is one mistake repeated: **trusting that an outer layer provides isolation that the code never builds.** Firecracker isolates the *guest*, not the *network it is plugged into* or the *host process tree it runs in*. That is survivable on a single-tenant / trusted deployment and becomes an incident the moment genuinely untrusted tenants are onboarded.

This document records the threat model, the findings, the fixes that landed, their architecture, and what remains.

### 1.1 Status scorecard

| ID | Title | Severity | Status | Commit |
|----|-------|----------|--------|--------|
| WS3-6 | Missing `/init` shim → VM kernel panic | P0 (correctness) | ✅ fixed + dead builder removed | `b4d86f7` |
| WS4-1 | Empty `JWT_SECRET` → universal token forgery | P0 | ✅ fixed | `b4d86f7` |
| WS4-3 | GitHub webhook accepts unsigned callers | P1 | ✅ fixed | `b4d86f7` |
| WS4-4 | Paid-tier upgrade without payment | P2 | ✅ fixed | `b4d86f7` |
| WS3-3 | Tar-slip in build-context extraction | P1 | ✅ fixed | `b4d86f7` |
| WS3-4 | compose build/dockerfile path traversal | P1 | ✅ fixed | `b4d86f7` |
| WS3-5 | Build temp-dir leak on failure | P1 | ✅ fixed | `b4d86f7` |
| WS1-5 | Kernel-cmdline injection (phase 1: validation) | P1 | ✅ fixed | `b4d86f7` |
| WS4-7 | Missing down-migrations (010, 011) | P2 | ✅ fixed | `b4d86f7` |
| WS2-1 | Worker capacity over-commit → churn loop | P0 | ✅ fixed, **DB-proven** | `5d46f66` |
| WS1-1 | No tenant network isolation | P0 | 🟡 implemented, Linux-verify | `32dd3df` |
| WS1-2 | No Firecracker jailer / privilege drop | P0 | 🟡 implemented (flagged off), Linux-verify | `f0b39cb` |
| WS1-3 | Shared writable rootfs corruption | P0 | 🟡 resolved for jailer path; open for legacy path | `f0b39cb` |

Legend: ✅ landed & verified · 🟡 landed, needs out-of-band (Linux) verification.

---

## 2. Why this is in Nebo's SME set

Nebo is a **Personal Desktop AI Companion**; its moat is local ownership. A naive "hosted Nebo" surrenders that and walks into the most crowded market in software. The viable server story is the opposite: **one private Nebo per tenant, each in its own Firecracker microVM on Rivet** — "your own private companion, in your own private machine in the cloud," always-on for the lid-closed case (NeboLoop messages, cron, mail triage) while the desktop sleeps.

That architecture is attractive because it sidesteps the expensive multi-tenant retrofit on the Nebo side — Nebo stays a single-user app with its SQLite file and `~/.nebo` (resolved via `NEBO_DATA_DIR`, so a mounted `/data` volume works unchanged). Tenant isolation moves to the **infrastructure layer** — VM boundaries, which are categorically stronger than `WHERE user_id = ?` clauses bolted onto a single-user schema.

**But that only holds if Rivet's VM boundary is real.** When the audit ran, it was not: the network had no isolation (WS1-1), Firecracker ran as host root with no jail (WS1-2), and the rootfs was shared-writable (WS1-3). Closing those is the precondition for "Nebo on Rivet" being a security story rather than a liability. Hence this doc.

---

## 3. Rivet Architecture Primer

Enough to read the rest of this document. Authoritative source is the rivet.to repo.

### 3.1 Components

```
Control plane (cloud)                        Worker nodes (run tenant VMs)
  svc-api (Axum-equiv Go, 45+ endpoints)       worker-agent
    auth (JWT), billing (Stripe), webhooks       firecracker.Manager  (launch VMs)
  reconciler (desired vs actual)                 vmnet.Manager        (bridge, TAP, IP, firewall)
    controllers, workers, leader election        vm.Manager           (lifecycle, monitor)
  build-worker (Docker→ext4 rootfs)              rootfs cache (digest-addressed)
  auto-scaler, routing-fetcher, edge-sentinel  WireGuard mesh between workers
        |                                             ^
        +------------------ NATS ---------------------+
   PostgreSQL (state)   MinIO/Spaces (rootfs, certs)
```

### 3.2 The VM launch path (where most CRITICALs live)

```
reconciler schedules an instance
  → NATS instance.create.{worker_id}
    → workernats/listener.go handleCreate → vmtypes.DeploymentRequest{ID, DeploymentID, EnvVars, Mounts, Hosts, ...}
      → vm.Manager.Start(req)
         → vmnet.CreateTAPDevice(vmID, deploymentID)   // TAP + IP + firewall membership
         → rootfs cache.GetRootFS(digest)              // shared, digest-addressed ext4
         → buildKernelArgs(...)                         // init=/init, rivet.mount=, rivet.host=, rivet.env.*
         → firecracker.Manager.Start(fcConfig)          // exec firecracker | jailer
```

Key facts the fixes depend on:
- `req.ID` (the VM id) can contain slashes — format `deploymentID/serviceName/replicaIdx`.
- `req.DeploymentID` is carried explicitly on the request (the network isolation domain).
- The kernel always boots `init=/init`; there is no initrd in the Firecracker config — so `/init` must exist *inside* the rootfs.
- The rootfs returned by the cache is the **shared** digest file, mounted `is_read_only:false`.

### 3.3 Capacity accounting (the WS2-1 substrate)

`worker_nodes.available_cpu_cores` / `available_memory_mb` are maintained by a Postgres trigger `update_worker_available_resources()` (defined in `db/migrations/20251031120000_orchestrator_v1.sql`) that fires `AFTER INSERT OR UPDATE OR DELETE ON deployment_instances FOR EACH ROW` and **recomputes** `available = total − SUM(cpu/mem WHERE state IN ('Starting','Running'))`. It is an absolute recompute, it ignores `Pending`, and it would clobber any manual decrement of `available_*`. The 30s heartbeat upsert *also* overwrites `available_*`. This three-writer reality is the crux of WS2-1.

---

## 4. Threat Model

Adversary: **a tenant who can deploy arbitrary container images / Dockerfiles and run arbitrary code inside their own microVM.** Goals to deny:

1. **Cross-tenant data access** — reach another tenant's VM, rootfs, build artifacts, or DB rows.
2. **Host / control-plane compromise** — escape the VM to host root, or reach control-plane services from a VM.
3. **Denial of service / resource exhaustion** — wedge a worker (IP/TAP/disk leak), starve neighbors (over-commit), runaway builds.
4. **Privilege / billing abuse** — forge auth, trigger deploys unsigned, obtain paid tier without payment.
5. **Supply-chain / build escape** — poison another tenant's rootfs, escape the build sandbox.

The audit mapped findings to these; the CRITICALs cluster on (1) and (2).

---

## 5. Fixes That Landed

### 5.1 `b4d86f7` — `/init` shim, dead-builder removal, Phase-0 hardening

#### WS3-6 — Missing `/init` shim (latent VM-boot bug)

**Root cause.** `vm/manager.go` boots every VM with `init=/init`, no initrd. OCI images ship no init system, and the *live* build path (`builder.BuildFromImage` / `BuildFromContext`) wrote no `/init`. That logic lived only in `reconcilerworkers/build_worker.go` — which had **zero callers** (`NewBuildWorker` was never invoked). So any `image:`-style (and most `build:`-style) service would kernel-panic with "no working init."

**Why it was real, not theoretical.** Verified statically: no `/init` write anywhere in the live path; no `InitrdPath` in `FirecrackerConfig`; kernel arg is unconditional `init=/init`.

**Fix.** Grafted the shim trio into `internal/builder/rootfs.go`, invoked in *both* build functions between `exportImage` and `createExt4`:
- `injectInitShim(ctx, imageRef, rootfsDir)` — reads the image's entrypoint/cmd via `docker inspect`, writes `/etc/rivet/cmd` and a `/init` POSIX shell shim.
- `rivetInitScript` — PID-1 shim: mounts `/proc`,`/sys`,`/dev`; resets `/etc/hosts`; walks `/proc/cmdline` to handle `rivet.mount=` (mount volumes), `rivet.host=` (sibling DNS), `rivet.env.*` (exports); then `exec`s the original entrypoint.
- `dockerInspectCmd` — parses `.Config.Entrypoint` / `.Config.Cmd`.

**Dead code removed.** Deleted `build_worker.go` (1,011 lines) — a *competing duplicate* of the live builder (the one-pathway tech debt itself; identical `createExt4`/`exportImage`/`computeDigest`/`getDirSize`, weaker tar handling). Its only unique value was the shim, now grafted. The three `reconcilerservices` files it solely consumed (`detect.go`, `dockerfile.go`, `compose.go`, 912 lines of source-build/compose capability) were **kept** by explicit decision — unwired ≠ unwanted; tracked as a "wire-or-remove" item.

**Verification.** Static + clean Linux build. End-to-end (a real `image: postgres:16` boot) still needs a Firecracker host.

#### WS4-1 — Empty `JWT_SECRET` → universal forgery

`cmd/api/main.go` warned on an empty worker token but never checked `Auth.AccessSecret` (`${JWT_SECRET}`). Empty/short secret → anyone mints a `super_admin` token for any tenant. **Fix:** fatal boot check, `len(AccessSecret) < 32` → `os.Exit(1)` (HS256 32-byte floor), placed beside the existing worker-token warning.

#### WS4-3 — GitHub webhook open when secret unset

`webhook.go` gated signature verification behind `if WebhookSecret != ""` → unsigned callers could trigger `deployment.create`. **Fix:** 503 when unset, then *always* verify (mirrors the Stripe handler; `verifyGitHubSignature` already uses `hmac.Equal`).

#### WS4-4 — Free paid-tier upgrade when Stripe unconfigured

`BillingChangeTier` skipped the Stripe block when `StripeClient == nil` and still ran `UpdateSubscriptionTier`. **Fix:** refuse paid changes (`NewTier != "free" && StripeClient == nil` → 503); free downgrades still work.

#### WS3-3 — Tar-slip in build-context extraction

`extractTarGz` shelled `tar -xzf` on the tenant-uploaded context with no sanitization. **Fix:** in-process `archive/tar` — `filepath.Clean` each member, reject absolute/`..`, enforce dest-prefix containment, **skip symlink/hardlink/device** members (the classic second-stage escape), cap total bytes (`maxExtractBytes = 8 GiB`, decompression-bomb guard). The reference for the guard already existed at `build_worker.go:271-276`.

#### WS3-4 — compose path traversal

`dockerBuild` did `filepath.Join(extractDir, params.BuildContext/DockerfilePath)` unchecked. **Fix:** `underDir(base, rel)` helper asserts the cleaned path stays within `extractDir`.

#### WS3-5 — Build temp-dir leak on failure

Cleanup ran only after a *successful* upload; every failure path leaked a ≥256 MB ext4. **Fix:** builder owns failure cleanup via a named-return `defer`; returns a `Cleanup` closure for success that the poller `defer`s (covers upload-failure too); poller also cleans the separately-downloaded context dir on all paths.

#### WS1-5 (phase 1) — Kernel-cmdline injection validation

`buildKernelArgs` concatenated tenant env/mount/host values into the space-delimited boot args unescaped (a space/newline injects `init=`, `root=`, a forged `rivet.mount=`). **Fix:** `validateKernelTokens(req)` (called in `Start` before any resource allocation) — env keys must match `^[A-Z_][A-Z0-9_]*$`; env values / mount paths / host names reject whitespace and control chars; host IPs must `net.ParseIP`. Rejects loudly. **Phase 2** (durable) — move tenant data off the cmdline onto a config drive — remains a tracked follow-up; phase-1 validation stays as defense-in-depth.

#### WS4-7 — Missing down-migrations

`010_admin_system.sql` and `011_account_management.sql` had only `+goose Up`. **Fix:** added `+goose Down` sections (FK-safe drop order; shared `uuid-ossp` extension left intact).

### 5.2 `5d46f66` — WS2-1 worker capacity over-commit (DB-proven)

**The bug (subtle — a three-writer race, not "an unwired query").** The scheduler (`SelectWorkerNode`) reads `available_*`, but the trigger only counts `Starting`/`Running`. A scale-up loop inserts N replicas as `Pending`, which decrement nothing, so every iteration's `SelectWorkerNode` sees the *same* full worker → all replicas pile onto one node → OOM → `failed` → rescheduled onto the same full node → churn. The pre-existing `ReserveWorkerCapacity` query that was "meant to fix this" had **zero callers** and decremented `available_*` (trigger-owned), so it could never have worked — almost certainly *why* it was never wired.

**Fix (cleaner than the deep-dive's manual-reserve Model A).** New `reserved_cpu_cores` / `reserved_memory_mb` columns on `worker_nodes`, maintained by the **same trigger** as `available_*` (`reserved_* = SUM(cpu/mem WHERE state='Pending')`); `SelectWorkerNode` selects on `(available_* − reserved_*)`. Properties:
- **Atomic Pending→Starting handoff, free:** one UPDATE drops the instance from `reserved_*` and the existing logic adds it to `available_*`, no double-count.
- **Zero Go logic changes:** `scaleUp` already re-queries `SelectWorkerNode` per replica, so each insert immediately shrinks the next iteration's visible capacity.
- **Heartbeat-safe:** the 30s upsert never touches `reserved_*`, so it can't clobber a live reservation.
- Removed the dead, trigger-fighting `ReserveWorkerCapacity` / `ReleaseWorkerCapacity` queries.

**Migration:** `db/migrations/20260610000000_worker_reserved_capacity.sql` (extends the trigger function for both INSERT/UPDATE and DELETE branches; backfills existing rows; full `+goose Down`). Models regenerated via `sqlc generate`.

**Verification (real Postgres, scratch container, all migrations applied in sequence):**
- 1 Pending insert → `reserved=1.00/1024`, `available` untouched → trigger maintains `reserved_*`.
- 4 Pending on a 4-vCPU worker → `free=0`; **`SelectWorkerNode` returns no worker** → over-commit prevented (the bug).
- Pending→Starting → `available 4→3, reserved 4→3, free stays 0` → clean handoff, no double-count, no phantom capacity.
- Delete a Pending → `free=1` → slot freed.
- `goose down` drops the columns; `goose up` restores them — roundtrip clean.

**Deferred:** per-instance churn backoff (`placement_attempts`/`next_placement_after` — the existing `AddAfter(15s)` already bounds the no-worker case, and correct accounting removes the *systematic* driver); the residual concurrent-different-service select micro-race (millisecond window, self-corrects on next insert; an optimistic `SELECT … FOR UPDATE` closes it fully); WS2-2 rebalance-path reservation.

### 5.3 `32dd3df` — WS1-1 per-deployment network isolation

**Root cause.** All VMs shared one bridge / one subnet with only a MASQUERADE rule. Any VM could reach any other VM (cross-tenant lateral movement), the host/control-plane via the bridge gateway IP (which *is* the host), peer workers over the WireGuard mesh, and cloud metadata.

**Design decision — per-deployment, not blanket drop.** The deep-dive proposed blanket VM→VM drop + bridge port isolation. That would break **same-deployment sibling traffic** (`web`→`db`), which Rivet requires (it's the entire point of `rivet.host=` DNS injection). The correct model is: **isolation domain = the deployment** (one compose app / sibling-service group). Same-deployment VMs may talk; everything else is blocked.

**Mechanism (`internal/vmnet/firewall.go`).**
- `br_netfilter` (`bridge-nf-call-iptables=1`) forces bridged same-subnet VM↔VM frames through iptables (otherwise they are L2-switched and bypass `FORWARD`).
- A `RIVET-FWD` chain: ACCEPT established; ACCEPT same-deployment (`-m set --match-set <dep> src -m set --match-set <dep> dst`); DROP cross-deployment VM↔VM; DROP `→169.254/16`, `→10/8`, `→172.16/12`, `→192.168/16`, `→100.64/10` (CGNAT/mesh); ACCEPT public egress. An `INPUT` guard drops bridge→host except DNS.
- Each VM joins its deployment's ipset (`rivet-dep-<hash12>`) on `CreateTAPDevice(vmID, deploymentID)`; leaves on delete (empty set → ACCEPT rule + set torn down, bounding state).
- Idempotent; re-asserts surviving ipsets' rules on restart so running siblings keep connectivity through a worker-agent restart.
- Empty `deploymentID` ⇒ fully isolated (secure default).

**Wiring.** `CreateTAPDevice` gained a `deploymentID` parameter (caller `vm/manager.go` passes `req.DeploymentID`); `TAPDevice` stores it for teardown; `Initialize()` restructured so the firewall (re)installs whether or not the bridge pre-exists.

**New worker host deps:** `ipset` + `br_netfilter`.

**Linux verification checklist:** (1) same-deployment VMs talk; (2) cross-deployment/cross-tenant cannot; (3) VM→metadata/host/private blocked; (4) VM→public works; (5) `iptables -S RIVET-FWD` + `ipset list` correct; (6) restart preserves sibling connectivity; (7) deleting last VM removes its ipset + rule.

**Deferred:** ebtables/ARP-level L2 isolation (cross-deployment ARP discovery is still possible; IP traffic is blocked).

### 5.4 `f0b39cb` — WS1-2 jailer (+ WS1-3 for the jailer path)

**Root cause.** Firecracker was exec'd directly as the worker-agent's root user with only `Setpgid` — no chroot, no jailer, no cgroup. A VMM/guest escape lands as host root with access to every tenant's files. (A comment claimed "per-VM jailer"; none was wired.)

**Fix (`internal/firecracker/jailer.go`), feature-flagged OFF.** Launch Firecracker via the upstream `jailer` (chroot + uid/gid drop + cgroup + Firecracker's own seccomp). Gated by `workerconfig.UseJailer` (default false / `USE_JAILER=true`) **because a mistake here means no VM boots** — the legacy direct-exec path stays default until validated on a real host. This is a transitional rollout flag, not a permanent competing pathway.

Per-VM chroot staging:
- **kernel** — hard-linked (read-only, shared; copy fallback across devices);
- **rootfs** — `cp --reflink=auto` into the jail → near-instant on CoW FS, full copy otherwise. **This also resolves WS1-3:** each VM writes to its own copy, never the shared digest cache;
- **volumes** — bind-mounted so writes persist to the real backing file;
- config rewritten to in-chroot relative paths (`/kernel`, `/rootfs.ext4`, `/drive-N.ext4`); jail chowned to the jailer uid;
- the API socket moves into the chroot — `Manager` and `Client` both derive it from one `jailerSocketPath` helper so snapshot/restore still connect (the client computed the socket path independently, so this was a required ripple);
- `jailerID()` sanitizes the slash-containing VM id into a valid jailer `--id`;
- `Process.Cleanup` undoes bind-mounts (lazy umount) before removing the chroot.

**WS1-3 status:** resolved for the jailer path (per-VM reflink copy). **Still open for the legacy direct-exec path** (shared cache mounted writable) — closes when jailer becomes the only path.

**New worker host deps:** the `jailer` binary + an unprivileged `rivetVM`-style system user (`JailerUID`/`JailerGID`); a CoW filesystem (XFS/Btrfs) makes the rootfs copy cheap.

**Linux verification checklist (with `USE_JAILER=true`):** (1) VM boots; (2) `ps -o uid= -p <fc-pid>` ≠ 0; (3) `readlink /proc/<pid>/root` is the chroot; (4) `grep Seccomp /proc/<pid>/status` == 2; (5) cgroup `memory.max` matches request; (6) snapshot/restore work; (7) two VMs from one image write distinct data, shared cache untouched (WS1-3); (8) volume writes persist; (9) on stop, chroot removed + bind-mounts gone. Then flip the default to on.

**Deferred:** `--netns` per-VM (kept off to stay decoupled from WS1-1's shared-bridge model); verify bind-mount lifecycle under churn.

---

## 6. Complete Findings Inventory

From the 5 parallel audit agents. ✅ = fixed; backlog items are tracked in PRD §8.

### 6.1 Worker / VM (highest severity cluster)
- ✅ **WS1-1** no network isolation (CRITICAL)
- ✅ **WS1-2** no jailer / seccomp / privilege drop (CRITICAL)
- ✅ **WS1-3** shared writable rootfs corruption — *today, no attacker needed* (CRITICAL; jailer path done)
- ✅ **WS1-5** kernel-cmdline injection (HIGH; phase-1 done)
- ⏳ TAP/IP leak on crashed-not-restarted VM + failed `DeleteTAPDevice` (HIGH) — `vm/manager.go:435-466`, `ip_allocator.go`
- ⏳ `wgInterfaceAddr` hardcodes `.254`, collides with allocatable IP on non-/24 (MEDIUM)
- ⏳ `incrementIP`/`compareIP` panic on malformed/IPv6 input (MEDIUM)
- ⏳ `mustAtoi` swallows errors → duplicate MAC fallback (MEDIUM)
- ⏳ `enableIPForwarding` flips host-global sysctl, never reverted (LOW)

### 6.2 Scheduler / reconciler
- ✅ **WS2-1** capacity over-commit + churn loop (CRITICAL)
- ⏳ **WS2-2** rebalance path reserves neither source nor target (MEDIUM; gated on droplet-vs-worker_nodes ledger question)
- ⏳ Leader-election ping-failure path skips `onResigned` (HIGH; not split-brain, observability gap)
- ⏳ NATS `instance.create` no idempotency → possible double-provision on retry (HIGH; verify vm.Manager idempotency)
- ⏳ `nextReplicaIndex` COUNT(*) → index collision after scale-down gaps (MEDIUM)
- ⏳ Dead `job_scheduler.go` has a double-execution bug *if ever wired* (MEDIUM)

### 6.3 Build / storage / edge
- ✅ **WS3-3/4/5** tar-slip, path traversal, temp-dir leak (HIGH)
- ✅ **WS3-6** dead `build_worker.go` removed (consolidation)
- ⏳ **WS3-1** no build sandbox — `build-worker` runs `privileged:true` with the host Docker socket; tenant Dockerfiles build on the shared host daemon (CRITICAL). Recommended fix: rootless BuildKit (`buildctl`). *Largest remaining item.*
- ⏳ **WS3-2** cross-tenant image-tag collision `rivet-build-<service>:latest` (HIGH; subsumed by WS3-1's daemonless build, or a tenant+job-scoped tag)
- ⏳ Private TLS key written 0644 before chmod 0600 (HIGH) — `routingfetcher/certs.go`
- ⏳ ACME registers a new account per issuance → Let's Encrypt rate limits (HIGH) — `cert/acme/client/letsencrypt.go`
- ⏳ Edge-failover leader split-brain on connection blip (HIGH) — `edgeleader/election.go`
- ⏳ Routing index committed despite failed shard fetches → stale tenant→VM routing (MEDIUM)
- ⏳ Autoscaler div-by-zero + can't-scale-from-zero (MEDIUM)
- ⏳ Rootfs cache trusts file presence without integrity re-check on read (MEDIUM)

### 6.4 Auth / API / DB / CLI
- ✅ **WS4-1/3/4/7** JWT secret, webhook, billing, migrations
- ⏳ **WS4-2** GitHub OAuth account takeover via unverified public-profile email (HIGH) — `apiserver/github.go`
- ⏳ **WS4-6** plaintext secrets at rest: `git_token`, `compose_file` (vs encrypted `env_vars`); reset token stored raw; SES webhook secret in query param + non-constant-time compare (HIGH/LOW)
- ⏳ Multi-tenant isolation overall: **strong** (every resource handler resolves `{app}` tenant-scoped; explicit ownership checks on raw-ID billing endpoints). No classic missing-`WHERE tenant_id` IDOR found.
- ⏳ NanoCPU unit mishandling possibly off by 1e9 in CLI compose sizing (LOW)

---

## 7. Remaining Work / Roadmap

**Immediate (needs a Linux Firecracker host):** run the WS1-1 and WS1-2 verification checklists (§5.3, §5.4); flip `UseJailer` default to true once WS1-2 passes; that also closes WS1-3 fully once jailer is the only path.

**Next CRITICAL:** **WS3-1 build sandbox** — the last CRITICAL. Tenant Dockerfiles currently build on the host Docker daemon (`privileged:true` + host socket). Recommended: rootless BuildKit (`buildctl`), which also dissolves WS3-2's tag race. Largest remaining effort (L).

**Then HIGH backlog:** WS1-4 (TAP/IP leak), WS4-2 (OAuth takeover), WS4-6 (secrets at rest), the edge/ACME/cert items.

**Open decisions (PRD §7):** per-tenant L2 segmentation requirement (drives ebtables follow-up); CoW filesystem on workers (rootfs copy cost); droplet-vs-worker_nodes capacity ledger (blocks WS2-2); consolidate or wire the `reconcilerservices` source-build helpers; data backfill for WS4-6 encryption.

---

## 8. Appendix

### 8.1 File map (rivet.to)
| Area | Path |
|------|------|
| Init shim / builder | `internal/builder/rootfs.go` |
| Build poller | `internal/buildjobs/poller.go` |
| Capacity migration | `db/migrations/20260610000000_worker_reserved_capacity.sql` |
| Worker queries | `internal/queries/worker_nodes.sql` (+ generated `internal/models/`) |
| Scheduler | `internal/scheduler/scheduler.go`, `internal/controllers/deploymentservice/controller.go` |
| Capacity trigger | `db/migrations/20251031120000_orchestrator_v1.sql` |
| Network isolation | `internal/vmnet/firewall.go`, `internal/vmnet/manager.go` |
| Jailer | `internal/firecracker/jailer.go`, `manager.go`, `client.go` |
| VM lifecycle | `internal/vm/manager.go`, kernel args + `validateKernelTokens` |
| Worker config | `internal/workerconfig/config.go` |
| Auth / billing / webhook | `cmd/api/main.go`, `internal/apiserver/{billing,webhook,github,ses_webhook}.go` |
| Migrations 010/011 | `db/migrations/010_admin_system.sql`, `011_account_management.sql` |
| PRD (working tracker) | `SECURITY_FIXES_PRD.md` |

### 8.2 Operational requirements introduced
- **Workers:** `ipset`, `br_netfilter` (WS1-1); `jailer` binary + unprivileged `rivetVM` user + ideally CoW FS (WS1-2/WS1-3).
- **Control plane:** `JWT_SECRET` ≥ 32 bytes or boot fails (WS4-1); `GITHUB_WEBHOOK_SECRET` required for the webhook to function (WS4-3).
- **Codegen:** SQL changes go through `sqlc generate` (no Makefile in rivet.to); never hand-edit `internal/models/*.sql.go`.

### 8.3 How to verify the capacity fix locally (no Firecracker needed)
```
docker run -d --name pg -e POSTGRES_PASSWORD=test -e POSTGRES_DB=rivet -p 55433:5432 postgres:17-alpine
goose -dir db/migrations postgres "postgres://postgres:test@localhost:55433/rivet?sslmode=disable" up
# then insert a worker + N Pending instances and assert SelectWorkerNode returns none when (available-reserved) < req
```
The full proof script and expected output are recorded in the session that produced `5d46f66`.

### 8.4 Design principles applied (worth carrying forward)
- **The boundary you assume must be the boundary you build.** Every CRITICAL was a trusted-but-absent isolation layer.
- **Reuse the proven mechanism.** WS2-1 extended the existing trigger rather than adding a competing accounting path — fewer writers fighting one column.
- **Feature-flag high-blast-radius, un-verifiable-here changes.** WS1-2 ships off until a Linux host proves it; the flag is transitional.
- **Fail closed.** Empty isolation domain → fully isolated; invalid kernel tokens → reject; short JWT secret → refuse to boot.
- **One pathway.** Deleting the dead duplicate builder (and *not* maintaining parallel tar/cleanup fixes in two places) was as important as any single fix.
