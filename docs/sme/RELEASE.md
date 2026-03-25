# Release SME

> Authoritative reference for shipping Nebo releases. If it's not here, it doesn't happen.

---

## Release Checklist

### Pre-Release

- [ ] **Bump version** in both files (they must match):
  - `Cargo.toml` → `[workspace.package] version = "X.Y.Z"`
  - `src-tauri/tauri.conf.json` → `"version": "X.Y.Z"`
- [ ] **Build locally** — `cargo build` must succeed with no errors
- [ ] **Commit** the version bump: `git commit -m "Bump vX.Y.Z: <summary>"`
- [ ] **Push commits** to `main`: `git push origin main`
- [ ] **Tag** the release: `git tag vX.Y.Z`
- [ ] **Push tag** to trigger pipeline: `git push origin vX.Y.Z`
- [ ] **Monitor pipeline** — `gh run list --limit 1` should show the release run
- [ ] **Verify CDN** after pipeline completes: `curl -s https://cdn.neboloop.com/releases/version.json`
- [ ] **Verify GitHub Release** — `gh release view vX.Y.Z` should list all 14 assets
- [ ] **Smoke test** — install on at least one platform and confirm:
  - About page shows correct version
  - Marketplace loads
  - Update check returns "up to date"

### Common Mistakes (Things We've Broken Before)

| Mistake | Consequence | Prevention |
|---------|-------------|------------|
| Forgot to bump `Cargo.toml` + `tauri.conf.json` | Binary reports old version, auto-update loops forever | Always bump BOTH files before tagging |
| Tagged before pushing commits | Pipeline builds stale code | Push commits first, then tag |
| Snake_case JSON from Rust structs | Frontend gets `undefined` for fields like `currentVersion` | Use `#[serde(rename_all = "camelCase")]` on API-facing structs |
| Cooldown-filtered DB queries in API clients | Marketplace/NeboLoop features stop working intermittently | Use `list_all_active_auth_profiles_by_provider` for client building |

---

## Version System

### Source of Truth

```
Cargo.toml [workspace.package]
  └── version = "0.4.1"
        ├── All crates inherit via `version.workspace = true`
        ├── Injected at compile time: env!("CARGO_PKG_VERSION")
        └── Used by: server (const VERSION), cli (--version), updater
```

### Files That Contain Version

| File | Field | Must Match? |
|------|-------|-------------|
| `Cargo.toml` | `[workspace.package] version` | Source of truth |
| `src-tauri/tauri.conf.json` | `"version"` | YES — must match Cargo.toml |
| `app/package.json` | `"version"` | No — stays at `0.0.1`, not used for app versioning |

### How Version Flows

1. Developer sets version in `Cargo.toml` + `tauri.conf.json`
2. `cargo build` injects `CARGO_PKG_VERSION` at compile time
3. `const VERSION: &str = env!("CARGO_PKG_VERSION")` in server + CLI
4. CI reads tag name `vX.Y.Z` for artifact naming and `version.json`
5. CDN `version.json` gets `"version": "vX.Y.Z"` from the tag
6. Running app compares its compiled version against CDN version

---

## CI/CD Pipeline

**Trigger:** Push a `v*` tag → `.github/workflows/release.yml`

### 10 Jobs (in dependency order)

```
frontend ─────────┬─→ build-macos (arm64 + amd64)  ─┐
                   ├─→ build-linux (arm64 + amd64)   ├─→ release ──→ update-homebrew
                   └─→ build-windows ─→ sign-windows ┘       │
                                                              └──→ update-apt (from build-linux)
```

| Job | Runner | Duration | Output |
|-----|--------|----------|--------|
| frontend | ubuntu-latest | ~1m | SvelteKit build artifact |
| build-macos (arm64) | macos-latest | ~22m | `nebo-darwin-arm64` + `Nebo-X.Y.Z-arm64.dmg` |
| build-macos (amd64) | macos-latest | ~20m | `nebo-darwin-amd64` + `Nebo-X.Y.Z-amd64.dmg` |
| build-linux (amd64) | ubuntu-latest | ~23m | `nebo-linux-amd64` + headless + `.deb` |
| build-linux (arm64) | ubuntu-24.04-arm | ~23m | `nebo-linux-arm64` + headless + `.deb` |
| build-windows | windows-latest | ~27m | `Nebo-X.Y.Z-setup.exe` + `.msi` |
| sign-windows | windows-latest | ~6m | Signed `.exe` + `.msi` |
| release | ubuntu-latest | ~1m | GitHub Release + CDN upload |
| update-homebrew | ubuntu-latest | ~30s | `neboloop/homebrew-tap` push |
| update-apt | ubuntu-latest | ~40s | `neboloop/apt` push |

**Total pipeline time: ~37 minutes**

### Release Assets (14 files)

```
checksums.txt
Nebo-X.Y.Z-amd64.dmg          # macOS Intel installer
Nebo-X.Y.Z-arm64.dmg          # macOS Apple Silicon installer
Nebo-X.Y.Z-amd64.msi          # Windows MSI installer
Nebo-X.Y.Z-setup.exe          # Windows EXE installer
nebo-darwin-amd64              # macOS Intel bare binary
nebo-darwin-arm64              # macOS Apple Silicon bare binary
nebo-linux-amd64               # Linux x86_64 desktop (Tauri)
nebo-linux-arm64               # Linux ARM64 desktop (Tauri)
nebo-linux-amd64-headless      # Linux x86_64 CLI-only
nebo-linux-arm64-headless      # Linux ARM64 CLI-only
Nebo_X.Y.Z_amd64.deb          # Debian/Ubuntu x86_64 package
Nebo_X.Y.Z_arm64.deb          # Debian/Ubuntu ARM64 package
```

---

## CDN Structure

**Provider:** DigitalOcean Spaces → `https://cdn.neboloop.com`

```
releases/
├── version.json                          ← latest pointer (updater reads this)
├── vX.Y.Z/
│   ├── version.json                      ← per-tag snapshot
│   ├── checksums.txt                     ← SHA256 for all assets
│   ├── nebo-darwin-arm64
│   ├── nebo-darwin-amd64
│   ├── nebo-linux-amd64
│   ├── nebo-linux-amd64-headless
│   ├── nebo-linux-arm64
│   ├── nebo-linux-arm64-headless
│   ├── nebo-windows-amd64.exe
│   ├── Nebo-X.Y.Z-arm64.dmg
│   ├── Nebo-X.Y.Z-amd64.dmg
│   └── Nebo-X.Y.Z-amd64.msi
```

**version.json format:**
```json
{
  "version": "vX.Y.Z",
  "release_url": "https://github.com/NeboLoop/nebo/releases/tag/vX.Y.Z",
  "published_at": "2026-03-25T12:34:56Z"
}
```

---

## Auto-Update System

### How It Works

1. **Background checker** polls CDN every 1 hour (30s initial delay)
2. Compares `CARGO_PKG_VERSION` against `version.json`
3. If newer: broadcasts `update_available` WebSocket event
4. If `can_auto_update && user_pref_enabled`: auto-downloads binary
5. Reports download progress via `update_progress` events
6. Verifies SHA256 against `checksums.txt`
7. Stages binary in `state.update_pending` (in-memory)
8. Broadcasts `update_ready` — frontend can trigger apply
9. On apply: replaces binary/app, restarts process

### Install Methods & Update Behavior

| Method | Detection | Auto-Update? | User Action |
|--------|-----------|-------------|-------------|
| `app_bundle` | `.app/Contents/MacOS/` or `\Nebo\` | YES — downloads DMG/MSI | Click "Update Now" or auto |
| `direct` | Default (bare binary) | YES — replaces in-place | Click "Update Now" or auto |
| `homebrew` | `/opt/homebrew/` or `/usr/local/Cellar/` | NO | Run `brew upgrade nebo` |
| `package_manager` | `dpkg -S` succeeds | NO | Run `sudo apt upgrade nebo` |

### Restart Behavior (Per Platform)

| Platform | Method | Restart Strategy |
|----------|--------|-----------------|
| macOS app_bundle | DMG mount + cp -R | Background shell waits for PID to die, then `open` |
| macOS direct | Binary copy | `execve` (in-place process replacement) |
| Windows app_bundle | MSI | `cmd /C "msiexec ... && start exe"` |
| Windows direct | Binary rename + copy | Spawn new process, exit old |
| Linux app_bundle | AppImage copy | Spawn new, exit old |
| Linux direct | Binary copy | `execve` (in-place process replacement) |

### WebSocket Events

| Event | When | Payload |
|-------|------|---------|
| `update_available` | Background check finds newer | `latestVersion, currentVersion, installMethod, canAutoUpdate` |
| `update_progress` | During download | `downloaded, total, percent` |
| `update_ready` | Download + verify complete | `version` |
| `update_error` | Any failure | `error` (string) |

### API Endpoints

| Endpoint | Auth | Purpose |
|----------|------|---------|
| `GET /api/v1/update/check` | None | Check CDN for newer version |
| `POST /api/v1/update/apply` | None | Apply staged or fresh update |

---

## Code Signing

### macOS (Developer ID + Notarization)

- **Identity:** `Developer ID Application: Alma Tuck (7Y2D3KQ2UM)`
- **Bundle ID:** `dev.neboloop.nebo`
- **Entitlements:** `assets/macos/nebo.entitlements`
- **Flow:** Tauri builds → re-sign with Developer ID → create DMG → notarize → staple

### Windows (Azure Trusted Signing)

- **Account:** `nebosigning` / profile `neboloop-public`
- **Endpoint:** `https://eus.codesigning.azure.net/`
- **Type:** Authenticode + RFC 3161 timestamp
- **Gated:** `vars.AZURE_SIGNING_ENABLED == 'true'`

---

## CI Secrets & Variables

### Secrets

| Secret | Purpose |
|--------|---------|
| `APPLE_CERTIFICATE_P12` | Base64 Developer ID certificate |
| `APPLE_CERTIFICATE_PASSWORD` | Certificate password |
| `APPLE_SIGNING_IDENTITY` | Full signing identity string |
| `APPLE_ID` | Apple ID for notarization |
| `APPLE_APP_PASSWORD` | App-specific password for notary |
| `APPLE_TEAM_ID` | Apple Team ID |
| `TAP_GITHUB_TOKEN` | Token for homebrew-tap + apt repos |
| `DO_SPACES_ACCESS_KEY` | DigitalOcean Spaces access |
| `DO_SPACES_SECRET_KEY` | DigitalOcean Spaces secret |
| `AZURE_TENANT_ID` | Azure AD tenant |
| `AZURE_CLIENT_ID` | Azure signing client |
| `AZURE_CLIENT_SECRET` | Azure signing secret |
| `APT_GPG_PRIVATE_KEY` | GPG key for APT signing (optional) |

### Variables

| Variable | Value | Purpose |
|----------|-------|---------|
| `HAS_TAP_TOKEN` | `"true"` | Enable Homebrew + APT updates |
| `AZURE_SIGNING_ENABLED` | `"true"` | Enable Windows code signing |
| `AZURE_SIGNING_ENDPOINT` | `https://eus.codesigning.azure.net/` | Signing endpoint |
| `AZURE_SIGNING_ACCOUNT` | `nebosigning` | Signing account |
| `AZURE_SIGNING_PROFILE` | `neboloop-public` | Certificate profile |

---

## Key Files

| File | Role |
|------|------|
| `Cargo.toml` | Workspace version (bump here first) |
| `src-tauri/tauri.conf.json` | Tauri version (must match) |
| `.github/workflows/release.yml` | Full CI/CD pipeline |
| `crates/updater/src/lib.rs` | Version check, download, verify, background checker |
| `crates/updater/src/apply.rs` | Platform-specific binary replacement + restart |
| `crates/server/src/lib.rs:913-998` | BackgroundChecker wiring + WS events |
| `crates/server/src/handlers/agent.rs:477-519` | `/update/check` + `/update/apply` handlers |
| `app/src/lib/stores/update.ts` | Frontend update store |
| `app/src/routes/(app)/+layout.svelte` | WS event listeners for updates |
| `app/src/routes/(app)/settings/about/+page.svelte` | Update UI |
| `app/src/lib/components/UpdateBanner.svelte` | In-app update notification banner |
| `scripts/nebo.rb.tmpl` | Homebrew cask template |
| `assets/macos/nebo.entitlements` | macOS code signing entitlements |

---

## Troubleshooting

### "App keeps updating and restarting in a loop"
**Cause:** Compiled version doesn't match the tag. CDN says vX.Y.Z but binary reports an older version.
**Fix:** Bump `Cargo.toml` + `tauri.conf.json` to match the tag, rebuild, re-release.

### "Version shows — on About page"
**Cause:** Backend JSON uses snake_case but frontend expects camelCase.
**Fix:** Ensure `CheckResult` has `#[serde(rename_all = "camelCase")]`.

### "Update button missing"
**Cause:** `canAutoUpdate` is undefined/false on the frontend.
**Fix:** Same camelCase issue, or install method isn't `direct`/`app_bundle`.

### "Marketplace is empty"
**Cause:** NeboLoop auth profile is in cooldown, `build_api_client` can't find it.
**Fix:** Use `list_all_active_auth_profiles_by_provider` (ignores cooldown) for API client building.

### "App doesn't restart after update"
**Cause:** Race condition between `open`/`spawn` and `exit(0)`.
**Fix:** macOS uses a background shell that waits for PID to die before relaunching. Windows chains `msiexec && start`.

### "Notarization failed"
**Cause:** Entitlements, signing identity, or binary structure issue.
**Debug:** Check `xcrun notarytool log <submission-id>` for detailed rejection reasons.
