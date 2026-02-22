# Deployment SME Reference

> **Purpose:** This file is the single source of truth for Nebo's deployment
> infrastructure. Read this file on launch to immediately operate as the
> deployment SME — no codebase exploration needed.

---

## Table of Contents

1. [Build System](#build-system)
2. [CI/CD Pipeline](#cicd-pipeline)
3. [Packaging & Installers](#packaging--installers)
4. [Code Signing & Notarization](#code-signing--notarization)
5. [In-App Updater](#in-app-updater)
6. [Distribution Channels](#distribution-channels)
7. [Docker](#docker)
8. [Key Files Map](#key-files-map)
9. [Secrets & Variables](#secrets--variables)
10. [Runbooks](#runbooks)

---

## Build System

### Build Matrix

| Platform | Mode | CGO | Build Tag | LDFLAGS extra | Output |
|----------|------|-----|-----------|---------------|--------|
| macOS arm64 | Desktop | 1 | `desktop` | — | `nebo-darwin-arm64` |
| macOS amd64 | Desktop | 1 | `desktop` | — | `nebo-darwin-amd64` |
| Linux amd64 | Desktop | 1 | `desktop` | — | `nebo-linux-amd64` |
| Linux arm64 | Desktop | 1 | `desktop` | — | `nebo-linux-arm64` |
| Linux amd64 | Headless | 0 | none | — | `nebo-linux-amd64-headless` |
| Linux arm64 | Headless | 0 | none | — | `nebo-linux-arm64-headless` |
| Windows amd64 | Desktop | 1 | `desktop` | — | `nebo-windows-amd64.exe` |

### Version Injection

```makefile
VERSION ?= $(shell git describe --tags --always --dirty 2>/dev/null || echo "dev")
LDFLAGS = -ldflags "-s -w -X main.Version=$(VERSION)"
LDFLAGS_WIN = -ldflags "-s -w -X main.Version=$(VERSION) -H windowsgui"
```

- `-s -w` strips symbol table + debug info (smaller binary)
- `-X main.Version=...` injects version at compile time
- `-H windowsgui` (Windows only) suppresses console window
- `main.Version` in `nebo.go` defaults to `"dev"` if not injected
- Propagated to `cmd/nebo/vars.go` as `AppVersion` via `SetupRootCmd(version)`

### Desktop vs Headless

| Aspect | Headless (`make build`) | Desktop (`make desktop`) |
|--------|-------------------------|--------------------------|
| CGO | `CGO_ENABLED=0` | `CGO_ENABLED=1 -tags desktop` |
| Framework | HTTP server, opens browser | Wails v3 native window + system tray |
| Code signing | N/A | Required for macOS Gatekeeper |
| Cross-compile | Yes | No — must build natively per arch |
| Entry point | `cmd/nebo/desktop_stub.go` | `cmd/nebo/desktop.go` |

### macOS CGO Environment

CI uses `MACOSX_DEPLOYMENT_TARGET=13.0` (minimum macOS Ventura).
Local Makefile defaults to `15.0` — override in CI.

```yaml
CGO_CFLAGS: "-mmacosx-version-min=13.0"
CGO_LDFLAGS: "-mmacosx-version-min=13.0"
```

### Frontend Build

```bash
cd app && pnpm install --frozen-lockfile && pnpm exec svelte-kit sync && pnpm run build
```

- Output: `app/build/` (SvelteKit static adapter)
- Embedded into Go binary via `embed.FS`
- CI builds frontend once, shares as artifact across all platform jobs
- `pnpm run build` also runs `scripts/inject-css-fallback.js` post-build

### Make Targets

```bash
# Development
make dev              # Docker compose or parallel air + pnpm dev
make air              # Backend hot reload (desktop mode, CGO=1)
make dev-desktop      # Backend hot reload via .air-desktop.toml
make dev-setup        # First-time: go mod download + pnpm install

# Build
make build            # Headless binary (CGO=0), includes frontend build
make desktop          # Desktop binary (CGO=1, -tags desktop)
make build-cli        # Alias for build

# Package
make app-bundle       # Assemble Nebo.app + code sign
make dmg              # Create .dmg via scripts/create-dmg.sh
make notarize         # Notarize + staple DMG
make install          # app-bundle → notarize → copy to /Applications
make installer        # Windows NSIS installer via makensis

# Release
make release          # All platforms: release-darwin + release-linux + release-windows
make release-darwin   # macOS arm64 + amd64 (CGO=1, desktop)
make release-linux    # Linux amd64 + arm64 (CGO=0, headless)
make release-windows  # Windows amd64 (CGO=0, -H windowsgui)
make github-release TAG=v1.2.3  # Build all + create GitHub release

# Utility
make cli              # Build + install to GOPATH or /usr/local/bin
make clean            # Remove bin/ and tmp/
make test             # go test ./...
make deps             # go mod download + tidy
make gen              # Regenerate TypeScript API client (cmd/genapi)
make sqlc             # Regenerate DB code (sqlc generate)
```

---

## CI/CD Pipeline

### Trigger

```yaml
on:
  push:
    tags: ["v*"]  # e.g., git tag v1.2.3 && git push origin v1.2.3
```

### Job Dependency Graph

```
frontend (ubuntu-latest)
  ├──> build-macos (macos-latest × [arm64, amd64])
  │       └──> release
  ├──> build-linux (ubuntu-latest, ubuntu-24.04-arm × [amd64, arm64])
  │       ├──> package-deb (ubuntu-latest × [amd64, arm64])
  │       │       └──> release
  │       │       └──> update-apt
  │       └──> release
  ├──> build-linux-headless (same runners × [amd64, arm64])
  │       └──> release
  └──> build-windows (windows-latest)
          ├──> package-windows (windows-latest)
          │       └──> release
          └──> release

release
  └──> update-homebrew
```

### Job Details

**frontend:** Node 20, pnpm, builds SvelteKit, uploads `app/build/` artifact (1-day retention).

**build-macos:** Downloads frontend artifact. Builds with `CGO_ENABLED=1 -tags desktop`.
Imports Apple signing certificate from secrets into a temporary keychain. Assembles
`Nebo.app` bundle (binary + Info.plist + icon). Code signs binary and app bundle.
Creates DMG via `create-dmg`. Notarizes DMG with `notarytool submit --wait` + `stapler staple`.
Uploads: signed binary artifact + DMG artifact.

**build-linux:** Downloads frontend artifact. Installs `libgtk-3-dev`, `libwebkit2gtk-4.1-dev`,
`pkg-config`, `build-essential`. Builds with `CGO_ENABLED=1 -tags desktop`.

**build-linux-headless:** Downloads frontend artifact. Builds with `CGO_ENABLED=0` (no system deps).

**build-windows:** Downloads frontend artifact. Builds with `CGO_ENABLED=1 -tags desktop` on Windows runner.

**package-windows:** Downloads Windows binary. Installs NSIS via `choco install nsis -y`.
Runs `makensis` to create `dist/Nebo-{VERSION}-setup.exe`. Optionally signs with Azure
Trusted Signing (if `AZURE_SIGNING_ENABLED == 'true'`). Signs both installer and raw exe.

**package-deb:** Downloads Linux desktop binary. Installs nfpm via `go install`.
Templates `nfpm.yaml.tmpl` → `nfpm.yaml` (version/arch substitution). Builds `.deb`.

**release:** Downloads ALL artifacts. Renames signed macOS binaries (strips `-signed` suffix).
Generates `checksums.txt` with SHA256 of all binaries. **Uploads all release assets to
DigitalOcean Spaces CDN** (`s3://neboloop/releases/{tag}/` + `version.json` manifest) —
gated on `DO_SPACES_ACCESS_KEY` secret. Creates GitHub Release via
`softprops/action-gh-release@v2` with auto-generated release notes.

**update-homebrew:** Gated on `vars.HAS_TAP_TOKEN == 'true'`. Computes SHA256 of both DMGs.
Checks out `neboloop/homebrew-tap`. Templates `scripts/nebo.rb.tmpl` → `Casks/nebo.rb`
via `envsubst`. Pushes commit.

**update-apt:** Gated on `vars.HAS_TAP_TOKEN == 'true'`. Downloads `.deb` packages.
Checks out `neboloop/apt`. Copies debs to `pool/main/`. Runs `dpkg-scanpackages` to
generate `Packages` index. Creates `Release` file. Optionally GPG-signs with
`APT_GPG_PRIVATE_KEY`. Pushes commit. Served via GitHub Pages.

### Toolchain Versions (pinned in workflow)

```yaml
GO_VERSION: "1.25"
NODE_VERSION: "20"
```

---

## Packaging & Installers

### macOS DMG (`scripts/create-dmg.sh`)

```bash
./scripts/create-dmg.sh [version] [arch]  # e.g., v1.2.3 arm64
```

- Expects `dist/Nebo.app` to exist (from `make app-bundle`)
- Prefers `create-dmg` tool (brew): drag-to-Applications layout, volume icon
- Falls back to `hdiutil` if `create-dmg` not installed
- Output: `dist/Nebo-{PKG_VERSION}-{ARCH}.dmg`
- Note: `create-dmg` exits code 2 for cosmetic warning (no background image) — treated as success

### macOS App Bundle (`make app-bundle`)

```
dist/Nebo.app/
  Contents/
    MacOS/nebo          ← signed desktop binary
    Resources/nebo.icns ← app icon
    Info.plist          ← version-injected from assets/macos/Info.plist
```

- Bundle ID: `dev.neboloop.nebo`
- Min macOS: 13.0
- Entitlements (from `assets/macos/nebo.entitlements`):
  - File access (user-selected + downloads)
  - Network (client + server)
  - Microphone, camera
  - AppleEvents automation
  - Contacts, calendars

### Windows NSIS Installer (`scripts/installer.nsi`)

```bash
makensis /DVERSION=1.2.3 /DEXE_PATH=nebo-windows-amd64.exe scripts/installer.nsi
```

- Output: `dist/Nebo-{VERSION}-setup.exe`
- Installs to: `$PROGRAMFILES64\Nebo`
- Features:
  - Admin elevation (UAC required)
  - License page (shows LICENSE)
  - Custom install directory
  - Start Menu shortcuts (`$SMPROGRAMS\Nebo\`)
  - Desktop shortcut
  - Adds install dir to system PATH (registry + WM_WININICHANGE broadcast)
  - Add/Remove Programs registration (publisher: "Nebo Labs")
  - Full uninstaller (removes PATH entry, shortcuts, registry keys)
  - Launch option on finish page

### Linux .deb Package (`nfpm.yaml.tmpl`)

```bash
# Template substitution + build:
sed -e "s/__VERSION__/${PKG_VERSION}/g" -e "s/__ARCH__/${ARCH}/g" -e "s/__GOARCH__/${GOARCH}/g" \
    nfpm.yaml.tmpl > nfpm.yaml
nfpm pkg --packager deb --target nebo_${PKG_VERSION}_${ARCH}.deb
```

- Dependencies: `libwebkit2gtk-4.1-0`, `libgtk-3-0`
- Contents:
  - `/usr/local/bin/nebo` (binary, mode 0755)
  - `/usr/share/applications/nebo.desktop` (desktop integration)
  - `/usr/share/icons/hicolor/256x256/apps/nebo.png`
- Post-install: `scripts/postinstall.sh` (prints usage instructions)
- **Note:** License field says MIT but project is Apache 2.0 — discrepancy

### Homebrew Cask (`scripts/nebo.rb.tmpl`)

- Templated with `envsubst`: `$PKG_VERSION`, `$VERSION`, `$SHA_DARWIN_ARM64`, `$SHA_DARWIN_AMD64`
- Installs `Nebo.app` + symlinks `nebo` binary
- Zap trash: `~/Library/Application Support/Nebo`

---

## Code Signing & Notarization

### macOS

**Signing identity:** `Developer ID Application: Alma Tuck (7Y2D3KQ2UM)`

**Process (CI):**
1. Decode P12 certificate from `APPLE_CERTIFICATE_P12` secret (base64)
2. Create temporary keychain, import certificate
3. `codesign --force --sign "$SIGN_IDENTITY" --identifier dev.neboloop.nebo --entitlements ... --options runtime`
4. Sign both the binary (`Contents/MacOS/nebo`) and the app bundle (`Nebo.app`)
5. Notarize DMG: `xcrun notarytool submit --apple-id --password --team-id --wait`
6. Staple: `xcrun stapler staple`

**Process (local `make install`):**
1. `make app-bundle` signs with `SIGN_IDENTITY` from Makefile
2. Zip app, `notarytool submit` with keychain profile `nebo-notarize`
3. `stapler staple`, copy to `/Applications/`

**First-time local setup:**
```bash
xcrun notarytool store-credentials "nebo-notarize"
# Enter: Apple ID, app-specific password, team ID
```

### Windows

**Authenticode via Azure Trusted Signing:**
- Uses `azure/trusted-signing-action@v1` in CI
- Requires: tenant ID, client ID, client secret, endpoint, account, profile
- Signs ALL `.exe` files in workspace (depth 2): both raw binary and installer
- Digest: SHA256, RFC3161 timestamp: `http://timestamp.acs.microsoft.com`
- Gated on: `vars.AZURE_SIGNING_ENABLED == 'true'`

### APT (GPG)

- Optional: only if `APT_GPG_PRIVATE_KEY` secret is set
- Signs `Release` file → `Release.gpg` (detached) + `InRelease` (clearsigned)
- Public key exported to `key.gpg` in repo root

---

## In-App Updater

### Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│  Frontend (Svelte)                                              │
│                                                                 │
│  update.ts store ←→ UpdateBanner.svelte                         │
│    checkForUpdate() → GET /api/v1/update/check                  │
│    applyUpdate()    → POST /api/v1/update/apply                 │
│                                                                 │
│  Desktop system tray also has "Check for Updates" menu item     │
│  that calls updater package directly (bypasses HTTP endpoints)  │
└──────────────────────────┬──────────────────────────────────────┘
                           │
┌──────────────────────────▼──────────────────────────────────────┐
│  Backend                                                        │
│                                                                 │
│  updatecheckhandler.go   → updater.Check() + DetectInstallMethod│
│  updateapplyhandler.go   → updater.Apply() via UpdateManager    │
│                                                                 │
│  svc.UpdateMgr (in-memory)                                      │
│    .SetPending(path, version)                                   │
│    .PendingPath() → staged binary path                          │
│    .PendingVersion()                                            │
│    .Clear()                                                     │
│                                                                 │
│  updater package:                                               │
│    Check()          → CDN version.json manifest                 │
│    Download()       → Streams binary from CDN to temp file      │
│    VerifyChecksum() → SHA256 against checksums.txt from CDN     │
│    Apply()          → Platform-specific binary replacement      │
│    DetectInstallMethod() → "direct" | "homebrew" | "package_m.."│
│    BackgroundChecker     → Periodic poll (NOT currently active)  │
└─────────────────────────────────────────────────────────────────┘
```

### Check Flow

1. `GET /api/v1/update/check` → `updatecheckhandler.go`
2. Calls `updater.DetectInstallMethod()`:
   - Resolves symlinks on executable path
   - `/opt/homebrew/` or `/usr/local/Cellar/` → `"homebrew"`
   - `dpkg -S` succeeds (Linux) → `"package_manager"`
   - Everything else → `"direct"` (can auto-update)
3. Calls `updater.Check(ctx, svcCtx.Version)`:
   - `GET https://neboloop.nyc3.cdn.digitaloceanspaces.com/releases/version.json` (5s timeout)
   - Parses `versionManifest`: `version`, `release_url`, `published_at`
   - Normalizes versions (strips "v"), compares semver (major.minor.patch)
   - "dev" builds always return `available: false`
   - **No release notes in response** — CDN manifest doesn't carry body, UI links to GitHub instead
4. Returns `UpdateCheckResponse` with `can_auto_update` flag

### Download Flow (desktop tray or future frontend trigger)

1. `updater.Download(ctx, tagName, progressFn)`:
   - Asset name: `nebo-{GOOS}-{GOARCH}[.exe]`
   - URL: `https://neboloop.nyc3.cdn.digitaloceanspaces.com/releases/{tag}/{asset}`
   - Streams to temp file in 32KB chunks with progress callbacks
   - 10-minute timeout
   - Sets chmod 0755 on Unix
2. `updater.VerifyChecksum(ctx, binaryPath, tagName)`:
   - Downloads `checksums.txt` from CDN: `https://neboloop.nyc3.cdn.digitaloceanspaces.com/releases/{tag}/checksums.txt` (30s timeout)
   - 404 = skip verification (graceful for old releases)
   - Parses `{sha256}  {filename}` format
   - Computes SHA256 of downloaded binary, case-insensitive compare
3. On success: `svcCtx.UpdateManager().SetPending(path, version)`

### CDN Layout (DigitalOcean Spaces)

```
s3://neboloop/releases/
  version.json                    ← latest version pointer (updated each release)
  v1.2.3/
    nebo-darwin-arm64
    nebo-darwin-amd64
    nebo-linux-amd64
    nebo-linux-arm64
    nebo-linux-amd64-headless
    nebo-linux-arm64-headless
    nebo-windows-amd64.exe
    checksums.txt
```

CDN URL: `https://neboloop.nyc3.cdn.digitaloceanspaces.com/releases/`

`version.json` format:
```json
{
  "version": "v1.2.3",
  "release_url": "https://github.com/NeboLoop/nebo/releases/tag/v1.2.3",
  "published_at": "2026-02-22T12:00:00Z"
}
```

### Apply Flow

**Trigger:** `POST /api/v1/update/apply` → `updateapplyhandler.go`
- Checks `UpdateManager().PendingPath()` is set
- Responds `{"status": "restarting"}` immediately
- Spawns goroutine with 500ms delay, then calls `updater.Apply(pendingPath)`
- **Errors are logged** (`[updater] apply failed: ...`) — not swallowed

**Shared helpers (`apply.go` — no build tags):**
- `healthCheck(binaryPath)` — runs `{binary} --version` with 5s timeout, kills on timeout
- `copyFile(src, dst)` — copies file preserving permissions

**Unix (`apply_unix.go`):**
1. Health check via shared `healthCheck()` (`--version`, not `version`)
2. Backup: `copyFile(currentExe, currentExe+".old")`
3. Replace: `copyFile(newBinary, currentExe)` — rollback to `.old` on failure
4. Cleanup temp file
5. `syscall.Exec(currentExe, os.Args, os.Environ())` — replaces process in-place (same PID)

**Windows (`apply_windows.go`):**
1. Health check via shared `healthCheck()` (`--version`, not `version`)
2. Rename running exe: `os.Rename(currentExe, currentExe+".old")` — Windows allows renaming
3. Copy new: `copyFile(newBinary, currentExe)` — uses copy not rename (avoids cross-filesystem failure)
4. Cleanup temp file, spawn `exec.Command(currentExe, os.Args[1:]...)` with stdout/stderr
5. `os.Exit(0)` — old process exits, new one takes over

### Frontend Update UI

**Store:** `app/src/lib/stores/update.ts`
- `updateInfo` — check result
- `updateDismissed` — user dismissed banner
- `downloadProgress` — `{downloaded, total, percent}`
- `updateReady` — version string when staged
- `updateError` — error message

**Component:** `app/src/lib/components/UpdateBanner.svelte`
- Shows when: update available OR downloading OR ready OR error, AND not dismissed
- States: notification-only (package manager) → downloading (progress bar) → ready ("Restart to Update" button) → error
- Package manager installs show: `brew upgrade nebo` or `sudo apt upgrade nebo`

### BackgroundChecker (defined but NOT currently active)

The `BackgroundChecker` type exists in `updater.go` but is **not instantiated** anywhere in the
current codebase. Updates are triggered manually via:
- Desktop system tray "Check for Updates" button
- Frontend calling `GET /api/v1/update/check`

```go
// Available for future use:
checker := updater.NewBackgroundChecker(version, 6*time.Hour, notifyFn)
go checker.Run(ctx)
// Initial check: 30s after boot. Then every interval. Deduplicates per version.
```

### Known Limitations
- **No background checking active** — updates are manual only
- **In-memory pending state** — `UpdateMgr` pending path lost on restart
- **No delta updates** — always downloads full binary
- **No staged rollout** — all releases immediately available
- **Single `.old` backup** — no versioned rollback chain
- **No auto-apply** — always requires user confirmation
- **"dev" builds** never see updates

---

## Distribution Channels

### 1. DigitalOcean Spaces CDN (primary for auto-update)

- URL: `https://neboloop.nyc3.cdn.digitaloceanspaces.com/releases/`
- Contains: `version.json` manifest + all binaries + checksums per tag
- Used by in-app updater for both version checks and binary downloads
- No rate limits (unlike GitHub API's 60 req/hr unauthenticated)
- Populated by CI `release` job via `aws s3 cp --endpoint-url`

### 2. GitHub Releases (source of truth for humans)

- URL: `https://github.com/NeboLoop/nebo/releases`
- Artifacts: raw binaries, DMGs, NSIS installer, .deb packages, checksums.txt
- Auto-generated release notes from commits
- `release_url` in CDN manifest points here for "Release notes" link in UI

### 3. Homebrew (macOS)

```bash
brew install neboloop/tap/nebo
```

- Tap repo: `neboloop/homebrew-tap` (Casks/nebo.rb)
- Auto-updated by CI on each release
- Installs Nebo.app + `nebo` CLI symlink
- SHA256 per-arch (arm64 + amd64)

### 4. APT (Linux)

```bash
# Add repo
echo "deb https://neboloop.github.io/apt stable main" | sudo tee /etc/apt/sources.list.d/nebo.list
sudo apt update
sudo apt install nebo
```

- Repo: `neboloop/apt` (GitHub Pages)
- Auto-updated by CI on each release
- Supports amd64 + arm64
- Optional GPG signing

### 5. Direct Download

- Users download binary from GitHub Releases
- macOS: DMG with drag-to-Applications
- Windows: NSIS installer (adds to PATH, Start Menu, etc.)
- Linux: raw binary or .deb
- **Only direct installs support auto-update** — Homebrew/APT users use their package manager

---

## Docker

### Dockerfile (multi-stage)

```
Stage 1: development    — golang:1.25-alpine + Node 20 + pnpm + Air (hot reload)
Stage 2: frontend-builder — node:20-alpine, builds SvelteKit
Stage 3: builder        — golang:1.25-alpine, CGO_ENABLED=0, embeds frontend
Stage 4: production     — alpine:latest, ca-certificates + curl + wget + tzdata
```

- Final image: single static binary + `etc/` config + migrations
- Ports: 80, 443, 8888
- Health check: `wget -q -O /dev/null http://localhost:8888/health` (30s interval, 10s timeout)
- Entry: `./nebo` (headless server mode)

### Commands

```bash
make docker-build   # docker build -t nebo .
make docker-run     # docker run -p 27895:27895 --env-file .env nebo
```

---

## Key Files Map

### Build & CI

| File | Purpose |
|------|---------|
| `Makefile` | All build, release, packaging, install targets |
| `.github/workflows/release.yml` | Multi-platform CI/CD pipeline (tag-triggered) |
| `Dockerfile` | Multi-stage container build |
| `.air.toml` | Hot reload config (desktop mode) |
| `.air-desktop.toml` | Hot reload config (alternative) |

### Packaging

| File | Purpose |
|------|---------|
| `scripts/create-dmg.sh` | macOS DMG creation (create-dmg or hdiutil fallback) |
| `scripts/installer.nsi` | Windows NSIS installer script |
| `scripts/postinstall.sh` | Linux .deb post-install message |
| `scripts/nebo.rb.tmpl` | Homebrew cask template (envsubst) |
| `nfpm.yaml.tmpl` | Linux .deb package config template (sed) |

### macOS Assets

| File | Purpose |
|------|---------|
| `assets/macos/Info.plist` | App bundle metadata (`__VERSION__` placeholder) |
| `assets/macos/nebo.entitlements` | Code signing entitlements |
| `assets/icons/nebo.icns` | macOS app icon |
| `assets/icons/nebo.ico` | Windows app icon |
| `assets/icons/appicon-256.png` | Linux icon (256x256) |
| `assets/nebo.desktop` | Linux desktop integration file |

### Updater

| File | Purpose |
|------|---------|
| `internal/updater/updater.go` | Check (CDN), Download, VerifyChecksum, BackgroundChecker, DetectInstallMethod, AssetName |
| `internal/updater/apply.go` | Shared helpers: `healthCheck()` (`--version`) + `copyFile()` (no build tags) |
| `internal/updater/apply_unix.go` | Unix Apply: backup → copy → syscall.Exec |
| `internal/updater/apply_windows.go` | Windows Apply: rename → copyFile → spawn → exit |
| `internal/handler/updatecheckhandler.go` | `GET /api/v1/update/check` handler |
| `internal/handler/updateapplyhandler.go` | `POST /api/v1/update/apply` handler |
| `app/src/lib/stores/update.ts` | Frontend update state (Svelte writable stores) |
| `app/src/lib/components/UpdateBanner.svelte` | Update notification banner UI |

### Entry Points

| File | Purpose |
|------|---------|
| `nebo.go` | Main entry, `var Version = "dev"` |
| `cmd/nebo/root.go` | Cobra root command, server + agent startup, lock system |
| `cmd/nebo/desktop.go` | Wails v3 desktop mode (build tag: desktop) |
| `cmd/nebo/desktop_stub.go` | Headless fallback (build tag: !desktop) |
| `cmd/nebo/agent.go` | Agent startup, lane wiring, comm plugin |
| `cmd/nebo/vars.go` | `AppVersion` variable |

---

## Secrets & Variables

### GitHub Actions Secrets

| Secret | Purpose | Required |
|--------|---------|----------|
| `APPLE_CERTIFICATE_P12` | Base64-encoded Developer ID certificate | macOS signing |
| `APPLE_CERTIFICATE_PASSWORD` | Certificate password | macOS signing |
| `APPLE_ID` | Apple ID email for notarization | macOS notarization |
| `APPLE_APP_PASSWORD` | App-specific password for notarization | macOS notarization |
| `APPLE_TEAM_ID` | Apple Developer Team ID | macOS notarization |
| `APPLE_SIGNING_IDENTITY` | Signing identity string | macOS signing |
| `AZURE_TENANT_ID` | Azure AD tenant | Windows signing |
| `AZURE_CLIENT_ID` | Azure service principal | Windows signing |
| `AZURE_CLIENT_SECRET` | Azure service principal secret | Windows signing |
| `TAP_GITHUB_TOKEN` | Fine-grained PAT for homebrew-tap + apt repos | Homebrew/APT |
| `APT_GPG_PRIVATE_KEY` | GPG private key for APT signing | APT (optional) |
| `DO_SPACES_ACCESS_KEY` | DigitalOcean Spaces access key for CDN uploads | CDN (required for auto-update) |
| `DO_SPACES_SECRET_KEY` | DigitalOcean Spaces secret key for CDN uploads | CDN (required for auto-update) |

### GitHub Actions Variables

| Variable | Purpose | Values |
|----------|---------|--------|
| `HAS_TAP_TOKEN` | Gate Homebrew/APT update jobs | `"true"` to enable |
| `AZURE_SIGNING_ENABLED` | Gate Windows code signing | `"true"` to enable |
| `AZURE_SIGNING_ENDPOINT` | Azure Trusted Signing endpoint URL | URL |
| `AZURE_SIGNING_ACCOUNT` | Azure signing account name | string |
| `AZURE_SIGNING_PROFILE` | Code signing certificate profile | string |

### Local Makefile Variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `SIGN_IDENTITY` | `Developer ID Application: Alma Tuck (7Y2D3KQ2UM)` | macOS code signing identity |
| `NOTARIZE_PROFILE` | `nebo-notarize` | Keychain profile for notarization |
| `VERSION` | `git describe --tags --always --dirty` | Build version string |

---

## Runbooks

### Ship a Release

```bash
# 1. Ensure clean state
make build                    # Verify build succeeds (includes frontend)
make test                     # Run tests

# 2. Tag and push
git tag v1.2.3
git push origin v1.2.3

# 3. Monitor
# https://github.com/NeboLoop/nebo/actions
# Pipeline: frontend → builds (parallel) → packages → release (+ CDN upload) → homebrew + apt
```

### Local macOS Install (dev)

```bash
make install    # desktop build → app-bundle → notarize → /Applications/Nebo.app
```

### Local DMG Build

```bash
brew install create-dmg      # one-time
make notarize                 # desktop → app-bundle → dmg → notarize + staple
```

### Local Windows Installer

```bash
choco install nsis            # one-time
make release-windows          # build binary
make installer                # create setup.exe
```

### First-Time Notarization Setup (local)

```bash
xcrun notarytool store-credentials "nebo-notarize"
# Enter: Apple ID, app-specific password (generate at appleid.apple.com), team ID
```

### First-Time CI Setup

See `docs/RELEASE_SETUP.md` for:
1. Creating `TAP_GITHUB_TOKEN` (fine-grained PAT)
2. Enabling GitHub Pages on `neboloop/apt`
3. Creating `APT_GPG_PRIVATE_KEY` (optional)
4. Setting up DigitalOcean Spaces CDN (see "First-Time CDN Setup" above)

### Debug Update Issues

```bash
# Check what install method is detected:
curl http://localhost:27895/api/v1/update/check | jq .

# Expected fields: available, current_version, latest_version, install_method, can_auto_update
# install_method: "direct" → auto-update works
# install_method: "homebrew" or "package_manager" → shows manual instructions

# Check CDN version manifest directly:
curl -s https://neboloop.nyc3.cdn.digitaloceanspaces.com/releases/version.json | jq .

# Check checksums on CDN:
curl -sL https://neboloop.nyc3.cdn.digitaloceanspaces.com/releases/v1.2.3/checksums.txt

# Fallback: check GitHub API directly:
curl -s https://api.github.com/repos/neboloop/nebo/releases/latest | jq '{tag_name, published_at}'
```

### First-Time CDN Setup (DigitalOcean Spaces)

```bash
# 1. Create the Space + CDN
doctl spaces create neboloop --region nyc3
doctl compute cdn create neboloop.nyc3.digitaloceanspaces.com --ttl 3600
doctl spaces keys create --name nebo-ci

# 2. Add GitHub repo secrets
gh secret set DO_SPACES_ACCESS_KEY    # paste access key
gh secret set DO_SPACES_SECRET_KEY    # paste secret key

# 3. Verify after first release
curl https://neboloop.nyc3.cdn.digitaloceanspaces.com/releases/version.json
```

### Rollback a Bad Release

```bash
# Binary keeps .old backup after update:
# Unix: /path/to/nebo.old → cp nebo.old nebo
# Windows: C:\...\nebo.exe.old → rename nebo.exe.old nebo.exe

# Or pin version via package manager:
brew pin nebo                 # prevent auto-upgrade
```
