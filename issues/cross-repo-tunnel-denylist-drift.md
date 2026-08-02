# Tunnel Denylist Is Hand-Mirrored Across a Language Boundary

**Repos:** `nebo` + `neboloop` (cross-repo)
**Found:** 2026-07-28
**Priority:** High — currently correct, but a single forgotten edit silently exposes a
local-trust-only endpoint through the public tunnel.

## Issue

The reverse management tunnel (Plane B) is the auth boundary for Nebo's local HTTP surface — ~250
endpoints and `/ws` are unauthenticated by design because they trust localhost
(`docs/sme/TUNNEL.md` §1). Some of those endpoints must **never** be reachable through the tunnel,
so both sides carry a denylist.

The list exists twice, in two languages, in two repos, with nothing enforcing that they agree:

| Side | File | Function |
|------|------|----------|
| Nebo (Rust) | `crates/comm/src/tunnel.rs:110` | `is_blocked_path` |
| NeboLoop (Go) | `internal/api/tunnel.go:334` | `isBlockedTunnelPath` |

`docs/sme/TUNNEL.md` §3 already flags this ("keep both sides in sync… add it to BOTH") — the
instruction exists, the enforcement does not.

## Current State (verified 2026-07-28)

Diffed both implementations. **They are identical in behavior.** No live exposure.

```rust
// crates/comm/src/tunnel.rs:110
fn is_blocked_path(path: &str) -> bool {
    let p = path.split('?').next().unwrap_or(path);
    p == "/ws/extension" || p.starts_with("/ws/extension/") || p.starts_with("/api/v1/update/")
}
```

```go
// internal/api/tunnel.go:334
func isBlockedTunnelPath(path string) bool {
    if i := strings.IndexByte(path, '?'); i >= 0 {
        path = path[:i]
    }
    return path == "/ws/extension" ||
        strings.HasPrefix(path, "/ws/extension/") ||
        strings.HasPrefix(path, "/api/v1/update/")
}
```

Both strip the query string before matching, both block the same three patterns. This issue is about
the **absence of a mechanism**, not a present defect.

## Failure Mode

1. A developer adds a new local-trust-only endpoint — say `/api/v1/keychain/` or a new sidecar socket.
2. They add it to `tunnel.rs::is_blocked_path` (the repo they're working in).
3. They do not touch `neboloop`, a different repo in a different language.
4. Nebo's bot-side gate blocks it — so **nothing appears broken**, and no test fails.
5. The hub-side gate is now the weaker of the two. Defense-in-depth is gone: the layered model in
   `TUNNEL.md` §3 assumes both gates hold, so the endpoint is one bot-side regression away from being
   publicly reachable at `neboai.com/t/<botID>/...`.

The reverse ordering is worse: added to Go only, and the bot — the side that actually protects
localhost — never gates it at all.

There is no test, no shared fixture, and no CI check that compares the two lists. The only thing
keeping them aligned is that the same person happened to edit both.

## Expected

Adding a path to one side and not the other should fail a build or a test, not pass silently.

## Suggested Fix

A shared fixture both repos assert against — the cheapest thing that actually enforces it:

1. Create a checked-in list of blocked path patterns as data (e.g. `tunnel-denylist.json`), owned by
   one repo and vendored into the other.
2. Both `is_blocked_path` and `isBlockedTunnelPath` read from / are tested against that fixture.
3. Add a table test on **both** sides that runs the same set of `(path, expected_blocked)` cases,
   including the query-string, trailing-slash, and prefix-vs-exact cases already handled today.

A drift test alone (assert the two lists are equal) is enough to catch the failure mode and is
smaller than refactoring both functions to be data-driven — start there if the fixture plumbing is
not worth it.

## Notes

- Do **not** "fix" this by dropping one of the two gates. The layered model is deliberate: the bot
  must gate independently because it cannot trust a compromised hub (`TUNNEL.md` §3.4 — the bot
  re-gates every request on a stream and forces `Connection: close` on non-upgrade requests
  specifically to defend against a hostile hub).
- `TUNNEL.md` §4 flags a related Phase-4 change (the hub must strip `Origin` when the Nebo SPA is
  served from the loop). That is a separate item; it does not affect this list.

## Related

- `docs/sme/TUNNEL.md` §3 — the layered security model and the existing "keep both sides in sync" note
- `issues/cross-repo-wire-contract-no-source-of-truth.md` — same root cause (two repos agreeing by
  convention rather than construction), lower stakes
