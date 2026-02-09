# Plan: Consolidate Duplicate SPA Handlers

**Status:** Complete
**Created:** 2026-02-07
**Completed:** 2026-02-07  
**Context:** Two separate SPA serving codepaths exist — only one is used.

---

## Problem

There are **two independent SPA handler implementations** serving the same purpose:

| # | Location | Function | Used? |
|---|----------|----------|-------|
| 1 | `app/app.go:76` | `SPAHandler()` | ❌ Dead code |
| 2 | `internal/server/server.go:474` | `spaHandler()` | ✅ Registered as `r.NotFound()` |

Additionally, `app.NotFoundHandler()` (line 49 in `app/app.go`) is also dead code — never called anywhere.

The server imports `app.FileSystem()` to get the embedded `fs.FS`, then passes it to its own local `spaHandler()` — completely bypassing the more feature-rich `app.SPAHandler()` that already exists.

### Why this is bad

- Two implementations of the same logic that can drift apart
- `app.SPAHandler()` has more features (`.html` suffix resolution, redirect logic) that the server's version lacks
- Dead code creates confusion about which path is canonical

---

## Root Cause

The server's `spaHandler()` was likely written before (or independently of) `app.SPAHandler()`. Nobody wired them together, so both survived.

---

## Solution

### Step 1: Replace server.go's local handler with app.SPAHandler

**File:** `internal/server/server.go`

```go
// BEFORE (line 239)
r.NotFound(spaHandler(spaFS))

// AFTER
r.NotFound(app.SPAHandler(spaFS).ServeHTTP)
```

Ensure `app` is imported (it likely already is since `app.FileSystem()` is called earlier).

### Step 2: Delete the local spaHandler function

**File:** `internal/server/server.go`

Remove lines 474–508 (the entire `spaHandler()` function).

### Step 3: Delete dead code from app/app.go

**File:** `app/app.go`

Remove `NotFoundHandler()` (line 49) — it is never referenced anywhere.

### Step 4: Verify app.SPAHandler signature compatibility

Confirm that `app.SPAHandler(spaFS fs.FS)` returns an `http.Handler` so `.ServeHTTP` works as a `http.HandlerFunc` for `r.NotFound()`.

If the signature returns `http.HandlerFunc` directly, simplify to:
```go
r.NotFound(app.SPAHandler(spaFS))
```

---

## Files Changed

| File | Change |
|------|--------|
| `internal/server/server.go` | Replace `spaHandler(spaFS)` call with `app.SPAHandler(spaFS)`, delete `spaHandler()` func |
| `app/app.go` | Delete unused `NotFoundHandler()` |

---

## Verification

1. `make build` — must compile clean
2. `cd app && pnpm build` — frontend build unaffected
3. Manual test: navigate to `/app/agent`, `/app/settings`, and a bogus route like `/app/nonexistent` — all should load the SPA shell and let client-side routing handle it
4. Static assets (`/assets/...`, `/_app/...`) should still serve directly without hitting the SPA fallback
5. `200.html` fallback should still work for SPA client-side routing

---

## Risk

**Low.** This is a straight consolidation — the `app.SPAHandler()` implementation is a superset of `server.go:spaHandler()`. No behavior change expected; if anything, routes get the `.html` suffix resolution they were missing.
