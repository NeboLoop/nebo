# Update System — Internal Reference

Nebo's self-update system is fully hand-rolled (no third-party libraries). It checks GitHub Releases for new versions, downloads platform-specific binaries, verifies SHA256 checksums, and does an in-place binary swap with rollback on failure.

---

## Version Stamping

The version is set at **build time** via Go ldflags.

- `nebo.go:18` — `var Version = "dev"` (compile-time default)
- CI overrides: `-ldflags="-w -s -X main.Version=${{ github.ref_name }}"` (tag name, e.g. `v1.2.3`)
- Flow: `nebo.go` → `cmd/nebo/vars.go:AppVersion` → `svcCtx.Version`
- Version `"dev"` is **never** considered outdated (hardcoded guard at `updater.go:81`)

---

## Key Files

| File | Role |
|------|------|
| `internal/updater/updater.go` | Core engine: `Check()`, `Download()`, `VerifyChecksum()`, `BackgroundChecker`, `AssetName()`, `DetectInstallMethod()` |
| `internal/updater/apply_unix.go` | `Apply()` for macOS/Linux — backup → copy → `syscall.Exec()` |
| `internal/updater/apply_windows.go` | `Apply()` for Windows — rename → move → spawn → `os.Exit(0)` |
| `internal/svc/servicecontext.go` | `UpdateMgr` struct (in-memory pending state, lines 102-137) |
| `internal/handler/updatecheckhandler.go` | `GET /api/v1/update/check` handler |
| `internal/handler/updateapplyhandler.go` | `POST /api/v1/update/apply` handler |
| `internal/types/types.go` | `UpdateCheckResponse`, `UpdateApplyResponse` (lines 305-319) |
| `internal/server/server.go` | Route registration (lines 142-143) |
| `cmd/nebo/agent.go` | Wires `BackgroundChecker`, sends WebSocket events, auto-downloads (lines 1820-1891) |
| `cmd/nebo/desktop.go` | Tray "Check for Updates" menu item (lines 263-353) |
| `cmd/nebo/root.go` | `RunAll()` creates `UpdateMgr` for headless mode |
| `app/src/lib/stores/update.ts` | Svelte stores: `updateInfo`, `updateDismissed`, `downloadProgress`, `updateReady`, `updateError` |
| `app/src/lib/components/UpdateBanner.svelte` | UI banner with 4 states |
| `app/src/routes/(app)/+layout.svelte` | WebSocket event listeners, calls `checkForUpdate()` on mount |
| `app/src/lib/api/nebo.ts` | `updateCheck()`, `updateApply()` TS API functions |
| `.github/workflows/release.yml` | Full CI/CD pipeline (10 jobs) |

---

## Core Engine (`internal/updater/updater.go`)

### `Check(ctx, currentVersion) (*Result, error)`

1. `GET https://api.github.com/repos/neboloop/nebo/releases/latest` (5s timeout)
2. Headers: `Accept: application/vnd.github.v3+json`, `User-Agent: nebo/{version}`
3. Parses into `githubRelease` struct (tag_name, html_url, body, published_at)
4. Strips `v` prefix, compares semver via `fmt.Sscanf("%d.%d.%d")`
5. Returns `Result{Available, CurrentVersion, LatestVersion, ReleaseURL, ReleaseNotes (truncated 500 chars), PublishedAt}`

### `Download(ctx, tagName, progressFn) (string, error)`

1. URL: `https://github.com/NeboLoop/nebo/releases/download/{tag}/{AssetName()}`
2. 10-minute timeout
3. Streams to OS temp file with 32KB buffer, calls `progressFn(downloaded, total)` periodically
4. On Unix: `chmod 0755`
5. Returns temp file path

### `VerifyChecksum(ctx, binaryPath, tagName) error`

1. Fetches `https://github.com/NeboLoop/nebo/releases/download/{tag}/checksums.txt` (30s timeout)
2. If 404: **skips silently** (older releases without checksums)
3. Parses `{sha256}  {filename}` format
4. Computes SHA256 of downloaded file, compares case-insensitive

### `AssetName() string`

Returns platform-appropriate binary name:
- macOS: `nebo-darwin-arm64` or `nebo-darwin-amd64`
- Linux: `nebo-linux-amd64` or `nebo-linux-arm64`
- Windows: `nebo-windows-amd64.exe`

### `DetectInstallMethod() string`

- `"homebrew"` — resolved binary path contains `/opt/homebrew/` or `/usr/local/Cellar/`
- `"package_manager"` — Linux + `dpkg -S {path}` succeeds
- `"direct"` — everything else

Determines `can_auto_update` (true only for `"direct"`) and what the UI shows.

### `BackgroundChecker`

```go
type BackgroundChecker struct {
    version      string
    interval     time.Duration   // 6 hours (set in agent.go)
    notify       NotifyFunc
    lastNotified string          // deduplication: only notify once per version
    mu           sync.Mutex
}
```

- `Run(ctx)`: 30s initial delay (let app boot), then checks every `interval`
- `check(ctx)`: calls `Check()`, if new version not yet notified → calls `notify`

---

## Platform-Specific Apply

### Unix (`apply_unix.go`, build tag `!windows`)

1. Resolve current executable (follows symlinks)
2. **Health check:** runs `nebo version` on the new binary (5s timeout)
3. **Backup:** copies current binary to `{path}.old`
4. **Replace:** copies new binary over current (preserves permissions)
5. Cleans up temp file
6. **`syscall.Exec(currentExe, os.Args, os.Environ())`** — replaces current process in-place (same PID)
7. On failure at step 4: rolls back by restoring `.old` backup

### Windows (`apply_windows.go`, build tag `windows`)

1. Resolve current executable
2. **Health check:** runs `nebo version` on the new binary (5s timeout)
3. **Rename:** renames running exe to `{path}.old` (Windows allows rename of running exe, not overwrite)
4. **Move:** renames new binary into original path
5. On failure: rolls back the rename
6. **Spawn:** `exec.Command(currentExe, os.Args[1:]...)` with stdout/stderr inherited
7. **`os.Exit(0)`** — terminates old process

Key difference: Unix does seamless process replacement, Windows has a brief two-process overlap.

---

## In-Memory State (`UpdateMgr`)

```go
// internal/svc/servicecontext.go:102-137
type UpdateMgr struct {
    mu          sync.Mutex
    pendingPath string   // path to downloaded+verified binary
    version     string   // version of the pending binary
}
```

- `SetPending(path, version)` — records a ready-to-apply binary
- `PendingPath() string` — returns path or ""
- `PendingVersion() string` — returns version or ""
- `Clear()` — removes pending state

**Purely in-memory.** If the process restarts without applying, the state is lost. The downloaded temp file becomes an orphan on disk.

Initialized in both `RunAll()` (headless) and `RunDesktop()` (desktop):
```go
svcCtx.SetUpdateManager(&svc.UpdateMgr{})
```

---

## HTTP API

Routes registered in `server.go:142-143` — **outside** the JWT-protected group (public, no auth):

```go
r.Get("/api/v1/update/check", handler.UpdateCheckHandler(svcCtx))
r.Post("/api/v1/update/apply", handler.UpdateApplyHandler(svcCtx))
```

### `GET /api/v1/update/check`

1. Calls `DetectInstallMethod()` → sets `canAutoUpdate = (method == "direct")`
2. Calls `Check(ctx, svcCtx.Version)`
3. On error: returns success with `available=false` and current version (**non-fatal**)
4. Returns `UpdateCheckResponse`

### `POST /api/v1/update/apply`

1. Reads `UpdateMgr` from `ServiceContext`
2. No manager or no pending → returns `{status: "no_update"}`
3. If pending exists:
   - Sends `{status: "restarting"}` HTTP response **first**
   - Waits 500ms (let response flush)
   - Calls `Apply(pendingPath)` in a goroutine

**Security:** The pending path is never client-supplied — always read from server-side `UpdateMgr`.

### Response Types (`internal/types/types.go:305-319`)

```go
type UpdateCheckResponse struct {
    Available      bool   `json:"available"`
    CurrentVersion string `json:"current_version"`
    LatestVersion  string `json:"latest_version,omitempty"`
    ReleaseURL     string `json:"release_url,omitempty"`
    ReleaseNotes   string `json:"release_notes,omitempty"`
    PublishedAt    string `json:"published_at,omitempty"`
    InstallMethod  string `json:"install_method"`
    CanAutoUpdate  bool   `json:"can_auto_update"`
}

type UpdateApplyResponse struct {
    Status  string `json:"status"`
    Message string `json:"message,omitempty"`
}
```

---

## Three Update Flows

### Flow A: Background Auto-Update (agent.go:1820-1891)

```
Boot → svcCtx.SetUpdateManager()
  │
  └─ runAgent() starts
       │
       └─ if version != "dev":
            checker = NewBackgroundChecker(version, 6h, notifyFn)
            go checker.Run(ctx)
                │
                ├─ 30s initial delay
                └─ Every 6h: Check() → if newer:
                     │
                     ├─ Send WebSocket "update_available" event
                     │    └─ Frontend sets updateInfo → banner shows
                     │
                     └─ If installMethod == "direct":
                          │
                          ├─ go Download(tag, progressFn)
                          │    └─ progressFn sends "update_progress" events
                          │         └─ Frontend shows progress bar
                          │
                          ├─ VerifyChecksum(tmpPath, tag)
                          ├─ UpdateMgr.SetPending(tmpPath, version)
                          └─ Send "update_ready" event
                               └─ Frontend shows "Restart to Update" button
                                    │
                                    └─ User clicks → POST /api/v1/update/apply
                                         └─ Apply(pendingPath) → restart
```

### Flow B: Desktop Tray (desktop.go:263-353)

Manual "Check for Updates" menu item. Separate from the background checker.

```
User clicks tray item
  │
  ├─ homebrew? → show "Managed by Homebrew" for 3s → done
  ├─ package_manager? → show "Use apt upgrade" for 3s → done
  ├─ Already pending? → Apply() immediately → restart
  │
  └─ Direct install:
       ├─ Tray label: "Checking..." (disabled)
       ├─ Check() → up to date? → "Up to Date (vX.Y.Z)" for 5s → done
       ├─ Download() with tray label progress %
       ├─ VerifyChecksum()
       ├─ UpdateMgr.SetPending()
       ├─ Label: "Restart to Update (vX.Y.Z)"
       └─ On click: Apply() → restart
```

### Flow C: Frontend Page Load (+layout.svelte)

```
User navigates to any (app) route
  │
  ├─ onMount: checkForUpdate()
  │    └─ GET /api/v1/update/check → sets updateInfo store
  │
  └─ Subscribe to WebSocket events:
       ├─ "update_available" → sets updateInfo
       ├─ "update_progress" → sets downloadProgress
       ├─ "update_ready"    → sets updateReady, clears progress
       └─ "update_error"    → sets updateError, clears progress
```

---

## Frontend

### Stores (`app/src/lib/stores/update.ts`)

| Store | Type | Purpose |
|-------|------|---------|
| `updateInfo` | `UpdateCheckResponse \| null` | Full update info from check or WS event |
| `updateDismissed` | `boolean` | User dismissed the banner |
| `downloadProgress` | `{downloaded, total, percent} \| null` | During download |
| `updateReady` | `string \| null` | Version string when binary is verified |
| `updateError` | `string \| null` | Error message on failure |

Functions:
- `checkForUpdate()` — calls `api.updateCheck()`, populates `updateInfo`
- `applyUpdate()` — calls `api.updateApply()` (swallows errors since server may restart mid-response)

### UpdateBanner (`app/src/lib/components/UpdateBanner.svelte`)

DaisyUI `alert-info` banner with 4 states (priority order):

1. **Update Ready** — "Nebo vX.Y.Z is ready to install" + "Restart to Update" button (spinner while restarting)
2. **Downloading** — "Downloading update..." + animated icon + progress bar + percentage
3. **Error** — "Update failed: {message}"
4. **Available (managed)** — "Nebo vX.Y.Z is available" + release notes link + brew/apt instructions

Dismissible via X button (sets `updateDismissed`). Visibility derived from:
```ts
let show = $derived(
    ($updateInfo?.available || $downloadProgress || $updateReady || $updateError) && !$updateDismissed
);
```

---

## Release Pipeline (`.github/workflows/release.yml`)

Triggered on **tag push matching `v*`**. Ten jobs:

| Job | What it builds |
|-----|---------------|
| `frontend` | SvelteKit pnpm build → artifact |
| `build-macos` | arm64 + amd64 desktop binaries, code-signed (Apple Developer ID), Nebo.app bundle, .dmg (create-dmg.sh), notarized |
| `build-linux` | amd64 + arm64 desktop binaries (CGO + WebKitGTK) |
| `build-linux-headless` | amd64 + arm64 headless binaries (no CGO) |
| `build-windows` | amd64 desktop binary |
| `package-windows` | NSIS installer (`scripts/installer.nsi`), optional Authenticode via Azure Trusted Signing |
| `package-deb` | .deb packages via nfpm (`nfpm.yaml.tmpl`) |
| `release` | Stages all artifacts, `sha256sum` → `checksums.txt`, creates GitHub Release via `softprops/action-gh-release` |
| `update-homebrew` | envsubst `scripts/nebo.rb.tmpl` → push to `neboloop/homebrew-tap` |
| `update-apt` | Copy .deb → `dpkg-scanpackages` → GPG sign → push to `neboloop/apt` |

### Signing

- **macOS:** Apple Developer ID certificate (P12) + notarization (Apple ID + app-specific password + team ID)
- **Windows:** Azure Trusted Signing (Authenticode) — optional, controlled by secrets presence
- **APT repo:** GPG-signed Packages/Release files

### Checksums

The `release` job generates `checksums.txt` with SHA256 hashes for all binaries. Uploaded as a GitHub Release asset. Used by `VerifyChecksum()` during auto-update.

---

## Distribution Channels

| Platform | Direct | Installer | Package Manager |
|----------|--------|-----------|----------------|
| macOS | `nebo-darwin-{arch}` (code-signed) | `.dmg` (notarized) | Homebrew Cask (`brew install --cask neboloop/tap/nebo`) |
| Windows | `nebo-windows-amd64.exe` | NSIS `.exe` | — |
| Linux | `nebo-linux-{arch}` (desktop), `nebo-linux-{arch}-headless` | — | `.deb` via APT repo |

---

## Known Design Decisions

1. **No auth on update endpoints** — update check/apply are public routes. This is intentional: the app runs locally, and the apply endpoint only reads from server-side state (no client-supplied paths).
2. **In-memory pending state** — `UpdateMgr` is not persisted. A restart before applying loses the pending update. The temp file becomes an orphan.
3. **Checksum skip on 404** — older releases without `checksums.txt` silently skip verification rather than failing.
4. **Deduplication** — `BackgroundChecker.lastNotified` prevents re-notifying about the same version. Also in-memory only.
5. **Health check before apply** — both platforms run `nebo version` on the new binary before swapping, catching corrupt downloads.
6. **500ms flush delay** — the apply handler sends the HTTP response before restarting, with a 500ms sleep to let it reach the client.
