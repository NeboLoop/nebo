# Release SME

> Authoritative reference for shipping Nebo releases. If it's not here, it doesn't happen.

---

## Release Checklist

### Pre-Release

- [ ] **Confirm Windows signing is healthy FIRST** — a lapsed Azure bill silently blocks
      Trusted Signing for hours/days even while everything reads "Active" (see
      **⚠️ Azure Trusted Signing Billing Hold**). Cheap 40-sec check: `bash scripts/sign-probe.sh`
      (or the ad-hoc probe in that section) must return `Succeeded` before you tag.
- [ ] **Bump version** in both files (they must match):
  - `Cargo.toml` → `[workspace.package] version = "X.Y.Z"`
  - `src-tauri/tauri.conf.json` → `"version": "X.Y.Z"`
- [ ] **Build locally** — `cargo build` must succeed with no errors
- [ ] **Commit** the version bump: `git commit -m "Bump vX.Y.Z: <summary>"`
- [ ] **Push commits** to `main`: `git push origin main`
- [ ] **Tag** the release: `git tag vX.Y.Z`
- [ ] **Push tag** to trigger pipeline: `git push origin vX.Y.Z`
- [ ] **Monitor pipeline** — `gh run list --limit 1` (CI runs Linux + Windows only; mac is local)
- [ ] **Build + publish macOS locally** (CI skips mac — runners cost ~10x; see "Local macOS Build"):
  - `make release-macos` — signed + notarized **arm64** DMG + bare binary
  - `make release-macos-amd64` — signed + notarized **Intel (amd64)** DMG + bare binary (cross-compiled on Apple Silicon; no Intel Mac needed)
  - `make publish-macos TAG=vX.Y.Z` — attaches the 4 mac assets to the GH release + uploads to Spaces + merges mac checksums + flushes CDN
- [ ] **Verify CDN pointer**: `curl -s https://cdn.neboai.com/releases/version.json` → `vX.Y.Z`
- [ ] **Verify GitHub Release** — `gh release view vX.Y.Z` lists ALL assets (mac ×4, linux ×6, windows ×2, both AppImages, both debs, checksums)
- [ ] **Verify every download URL resolves** (incl. Windows — the one most likely to 404):
      `for f in ...; do curl -sI https://cdn.neboai.com/releases/vX.Y.Z/$f | head -1; done`
- [ ] **Verify the download site** — `curl -s https://neboai.com/api/v1/nebo/version` reports `X.Y.Z` and lists all platforms (the site reads this API, see "Download Site")
- [ ] **Smoke test** — install on ≥1 platform: About shows the version, marketplace loads, update check says "up to date"

### Common Mistakes (Things We've Broken Before)

| Mistake | Consequence | Prevention |
|---------|-------------|------------|
| **Tagged while Azure bill was lapsed** | Windows leg fails at signing (`0x80004005`), no installer for hours/days | Run the sign-probe BEFORE tagging; keep the Azure card valid |
| Forgot to bump `Cargo.toml` + `tauri.conf.json` | Binary reports old version, auto-update loops forever | Always bump BOTH files before tagging |
| Tagged before pushing commits | Pipeline builds stale code | Push commits first, then tag |
| Assumed `.msi` ships | It doesn't — Windows is **NSIS `setup.exe` only** (`--bundles nsis`) | Reference `Nebo-X.Y.Z-setup.exe`, never `.msi` |
| Forgot mac assets (built locally) | Release "done" but no Mac downloads / broken auto-update on Mac | `make publish-macos` after CI; verify all 4 mac assets present |
| Snake_case JSON from Rust structs | Frontend gets `undefined` for fields like `currentVersion` | `#[serde(rename_all = "camelCase")]` on API-facing structs |
| Cooldown-filtered DB queries in API clients | Marketplace/NeboAI features stop working intermittently | Use `list_all_active_auth_profiles_by_provider` for client building |

---

## ⚠️ Azure Trusted Signing Billing Hold (CRITICAL GOTCHA)

**This bit us hard (v0.12.4, June 2026 — cost ~2 days). Read it.**

### Symptom
Windows CI fails at **`Sign Obscura sidecars (Authenticode)`** (or `sign-windows`) with:
```
Azure.RequestFailedException: Service request failed.
{"operationId":"…","status":"Failed","signature":null}
Error: SignerSign() failed. (-2147467259 / 0x80004005)
```
The `:sign` call is **accepted (HTTP 202)** and returns an operation ID, then the operation resolves to **`Failed` / `signature: null`**. It reproduces across **all signature algorithms** and **all identities**.

### Root cause
A **lapsed payment** on the Azure subscription (e.g. an expired card → unpaid monthly Trusted Signing invoice) disables the subscription. Paying the past-due invoice reactivates the **control plane** (subscription reads `Enabled`), **but the Trusted Signing *data plane* stays in a billing/entitlement hold** — and it does **NOT** reliably auto-clear.

> **MYTH-BUSTED:** Azure docs say reactivation takes "up to ~24h." For the Trusted Signing **data plane** that is WRONG — ours stayed held **>36h** after payment. Do not wait it out; escalate (below).

### Why it's so confusing — every status page reads healthy
During the hold, ALL of these are green and are **red herrings**:
subscription `Enabled` (PayAsYouGo, spending-limit Off) · billing `$0 due / paid` · RP `Microsoft.CodeSigning` Registered · signing account `Succeeded` · cert profile `neboloop-public` Active w/ a live cert · identity validation (NeboLoop LLC) Completed → 2028 · signer SP secret valid · Service Health clean. **The block is data-plane-only and is exposed on NO portal/ARM/CLI status page** — the only way to see it is an actual `:sign` call.

### How to diagnose (the probe)
The subscription **Owner** role has NO data-plane sign permission, so the probe temporarily grants the signer role, signs a throwaway digest, reads the result, and revokes:
```bash
SCOPE="/subscriptions/<sub>/resourceGroups/nebo/providers/Microsoft.CodeSigning/codeSigningAccounts/nebosigning"
ROLE="Artifact Signing Certificate Profile Signer"
OID=$(az ad signed-in-user show --query id -o tsv)
az role assignment create --assignee-object-id "$OID" --assignee-principal-type User --role "$ROLE" --scope "$SCOPE" -o none
sleep 25   # RBAC propagation
TOKEN=$(az account get-access-token --scope "https://codesigning.azure.net/.default" --query accessToken -o tsv)
DIGEST=$(printf x | openssl dgst -sha256 -binary | base64)
BASE="https://eus.codesigning.azure.net/codesigningaccounts/nebosigning/certificateprofiles/neboloop-public"
API="api-version=2023-06-15-preview"
ID=$(curl -s -X POST "$BASE:sign?$API" -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
     -d "{\"signatureAlgorithm\":\"RS256\",\"digest\":\"$DIGEST\"}" | python3 -c "import sys,json;print(json.load(sys.stdin).get('id',''))")
sleep 8; curl -s "$BASE/sign/$ID?$API" -H "Authorization: Bearer $TOKEN"   # look for "status":"Succeeded"
az role assignment delete --assignee "$OID" --role "$ROLE" --scope "$SCOPE" -o none   # ALWAYS revoke
```
- `Succeeded` → signing is healthy, ship.
- `Failed` / `signature: null` → still held.

**Discriminator that proves it's account-level (not profile/identity):** create a *new* PublicTrust profile reusing the same Completed identity validation — under a hold it **fails provisioning** (`provisioningState: Failed`), and there is **no new-profile workaround**.

### Fix
1. **Pay the past-due invoice** (update the card first). Check: `az billing invoice list --account-name <ba> --period-start-date … --period-end-date …` — all invoices must be `Paid`.
2. If signing doesn't return within an hour or two, **open a Microsoft support request** — that's what actually released ours (backend reset), ~1h after filing.
   - Trusted Signing is **public preview** + a PayAsYouGo **Basic** support plan blocks *Technical* tickets. **File it as `Billing` / `Subscription management`** (free on every plan) framed as "subscription reactivated after payment but a paid service wasn't restored." Also post to **Microsoft Q&A** tag `azure-trusted-signing` (free; the precedent case was MS-staff-reset there).
   - Include: subscription ID, account `nebosigning`, profile `neboloop-public`, region `eastus`, the failed `:sign` operation IDs, and the failed new-profile provisioning timestamp.
3. **NEVER ship unsigned** to "work around" it (`AZURE_SIGNING_ENABLED=false`) — Windows Defender/SmartScreen flags unsigned installers as malware. That is never acceptable.
4. Once the probe returns `Succeeded`, re-run the Windows leg: `gh run rerun <run-id> --failed`.

---

## Version System

### Source of Truth

```
Cargo.toml [workspace.package]
  └── version = "X.Y.Z"
        ├── Most crates inherit via `version.workspace = true`
        ├── Injected at compile time: env!("CARGO_PKG_VERSION")
        └── Used by: server (const VERSION), cli (--version), updater
```

### Files That Contain Version

| File | Field | Must Match? |
|------|-------|-------------|
| `Cargo.toml` | `[workspace.package] version` | Source of truth |
| `src-tauri/tauri.conf.json` | `"version"` | YES — must match Cargo.toml |
| `app/package.json` | `"version"` | No — stays `0.0.1`, not used for app versioning |

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

**macOS is built LOCALLY, not in CI** — `macos-latest` runners bill ~10x Linux. `build-macos` +
`notarize-macos` are gated `if: ${{ vars.BUILD_MACOS_IN_CI == 'true' }}` and skipped by default.
CI runs the Linux + Windows legs; the mac arm64 + amd64 assets are produced on a dev Mac
(see "Local macOS Build") and attached afterward via `make publish-macos`.

```
CI (Linux + Windows):
frontend ─┬─→ build-linux (amd64,arm64) ───────────────┬─→ release ─→ update-homebrew
          └─→ build-windows ─→ sign-windows ────────────┘     │
                              update-apt ←── build-linux ──────┘

Local (dev Mac): make release-macos + make release-macos-amd64 → make publish-macos TAG=vX.Y.Z
```

`release` has **`needs: [build-linux, sign-windows]`** — it now *does* depend on `sign-windows`
(via a uploaded **artifact**, not an OS-scoped cache) so the signed Windows installer reliably
attaches. When Windows is disabled, `sign-windows` is skipped and the artifact download is
`continue-on-error`. `update-apt` keys off `build-linux`.

**The `release` job also uploads Linux + Windows assets + `version.json` to Spaces** (gated on
`DO_SPACES_ACCESS_KEY`, endpoint `nyc3.digitaloceanspaces.com`, bucket `neboloop`). So CI handles
Linux + Windows → CDN automatically; only the **mac** assets are pushed to Spaces locally by
`make publish-macos`.

| Job | Runner | Default | Output |
|-----|--------|---------|--------|
| frontend | ubuntu-latest | runs | SvelteKit build artifact |
| build-linux | ubuntu-latest / ubuntu-24.04-arm | runs (amd64, arm64) | `nebo-linux-{arch}` + headless + `.deb` + `.AppImage` |
| build-windows | windows-latest | runs | signs obscura sidecars → builds NSIS → `Nebo-X.Y.Z-setup.exe` + `nebo-windows-amd64.exe` |
| sign-windows | windows-latest | runs | Authenticode-signs the installer + bare exe |
| release | ubuntu-latest | runs | GitHub Release + Spaces upload (Linux + Windows + version.json) |
| update-homebrew | ubuntu-latest | runs | `neboloop/homebrew-tap` push |
| update-apt | ubuntu-latest | runs | `neboloop/apt` push |
| build-macos | macos-latest | **SKIPPED** (`BUILD_MACOS_IN_CI`) | (local instead) |
| notarize-macos | macos-latest | **SKIPPED** | (local instead) |

> **Two Windows Authenticode signing steps, both Trusted Signing:** (1) `Sign Obscura sidecars
> (Authenticode)` inside `build-windows` — the obscura/obscura-worker sidecars must be signed
> BEFORE NSIS bundles them in (can't sign after); (2) `Sign binaries (Authenticode)` in
> `sign-windows` — the installer + bare exe. Under a billing hold, #1 fails first.

> **macOS ships both arm64 + amd64.** Not complete until both mac arches are in the GH Release,
> Spaces, and `checksums.txt`.

**CI wall-clock (Linux + Windows): ~35–40 min** (Windows build ~27m + sign ~6m is the critical
path). Local mac build runs in parallel (~15–20 min per arch).

### Release Assets

```
checksums.txt                  # SHA256 of every asset (CI writes Linux+Windows; mac merged locally)
Nebo-X.Y.Z-arm64.dmg           # macOS Apple Silicon installer (signed + notarized, built locally)
Nebo-X.Y.Z-amd64.dmg           # macOS Intel installer (signed + notarized, cross-built locally)
nebo-darwin-arm64              # macOS Apple Silicon bare binary (built locally)
nebo-darwin-amd64              # macOS Intel bare binary (built locally)
Nebo-X.Y.Z-setup.exe           # Windows NSIS installer (Authenticode-signed) — NO .msi
nebo-windows-amd64.exe         # Windows bare binary (Authenticode-signed)
nebo-linux-amd64               # Linux x86_64 desktop (Tauri)
nebo-linux-arm64               # Linux ARM64 desktop (Tauri)
nebo-linux-amd64-headless      # Linux x86_64 CLI-only
nebo-linux-arm64-headless      # Linux ARM64 CLI-only
Nebo-X.Y.Z-amd64.AppImage      # Linux x86_64 AppImage
Nebo-X.Y.Z-arm64.AppImage      # Linux ARM64 AppImage
Nebo_X.Y.Z_amd64.deb           # Debian/Ubuntu x86_64 package
Nebo_X.Y.Z_arm64.deb           # Debian/Ubuntu ARM64 package
```

---

## Local macOS Build

CI skips mac (cost). Build + publish from a dev Mac (Apple Silicon builds BOTH arches):

```bash
make release-macos          # arm64: build → re-sign Developer ID → DMG → notarize → staple
make release-macos-amd64    # Intel: cross-compile x86_64-apple-darwin → sign → DMG → notarize → staple
make publish-macos TAG=vX.Y.Z
```

- **`make publish-macos`** uploads the 4 mac assets (`Nebo-X.Y.Z-{arch}.dmg`, `nebo-darwin-{arch}`)
  to the GH release + `s3://neboloop/releases/{vX.Y.Z}/` + merges mac hashes into `checksums.txt`
  + writes/updates `version.json` (latest pointer) + flushes the DO CDN.
- **DO Spaces creds** come from the local `digitalocean` AWS profile
  (`aws configure get aws_access_key_id --profile digitalocean`); the script reads
  `DO_SPACES_ACCESS_KEY`/`AWS_ACCESS_KEY_ID`. Never echo these.
- Signing identity `Developer ID Application: Alma Tuck (7Y2D3KQ2UM)`, notary profile `nebo-notarize`.
- Notarization requires ALL bundled binaries (incl. the obscura sidecars) to be Developer-ID-signed
  with the **hardened runtime** — Tauri leaves `externalBin` sidecars ad-hoc, so they're re-signed
  inside-out with `assets/macos/obscura.entitlements` (JIT entitlements) before the DMG. If
  notarization rejects, an unsigned/un-hardened obscura sidecar is the usual culprit.

---

## CDN Structure

**Provider:** DigitalOcean Spaces (`s3://neboloop`, endpoint `nyc3.digitaloceanspaces.com`) → `https://cdn.neboai.com`

```
releases/
├── version.json                          ← latest pointer (updater + site read this)
├── vX.Y.Z/
│   ├── version.json                      ← per-tag snapshot
│   ├── checksums.txt
│   ├── nebo-darwin-arm64 / nebo-darwin-amd64
│   ├── Nebo-X.Y.Z-arm64.dmg / Nebo-X.Y.Z-amd64.dmg
│   ├── nebo-linux-amd64 / -amd64-headless / -arm64 / -arm64-headless
│   ├── Nebo-X.Y.Z-amd64.AppImage / Nebo-X.Y.Z-arm64.AppImage
│   ├── Nebo_X.Y.Z_amd64.deb / Nebo_X.Y.Z_arm64.deb
│   ├── Nebo-X.Y.Z-setup.exe
│   └── nebo-windows-amd64.exe
```
Uploaded by: CI `release` job (Linux + Windows + version.json) · `make publish-macos` (mac).

**version.json format:**
```json
{ "version": "vX.Y.Z", "release_url": "https://github.com/NeboLoop/nebo/releases/tag/vX.Y.Z", "published_at": "…Z" }
```

> **CDN caching:** a 404 (e.g. Windows before it was published) can get cached. `make publish-macos`
> flushes the pointer + mutables; if a just-uploaded asset still 404s, flush it explicitly with
> `doctl compute cdn flush <cdn-id> --files "releases/vX.Y.Z/<file>"`.

---

## Download Site (neboai.com/download)

The public download page is **dynamic**, not hardcoded:
- `neboloop/internal/version/poller.go` polls `cdn.neboai.com/releases/version.json` **every 60s**
  and serves it (with per-platform download entries) at **`GET /api/v1/nebo/version`**.
- `neboloop/app/src/routes/(www)/download/+page.svelte` fetches that API `onMount` and builds the
  download links; `neboloop/app/src/lib/config.ts` holds only a **fallback** version used pre-fetch.
- **Implication:** once `version.json` flips to `vX.Y.Z`, the site self-updates within ~60s — no
  neboloop deploy needed. Keep the `config.ts` fallback roughly current so the no-JS/SSR paint
  isn't wildly stale. The site lists Windows unconditionally, so its Windows button 404s until the
  `setup.exe` is actually in Spaces — publish Windows before advertising the version.

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
| `app_bundle` | `.app/Contents/MacOS/` or `\Nebo\` | YES — downloads DMG/installer | Click "Update Now" or auto |
| `direct` | Default (bare binary) | YES — replaces in-place | Click "Update Now" or auto |
| `homebrew` | `/opt/homebrew/` or `/usr/local/Cellar/` | NO | `brew upgrade nebo` |
| `package_manager` | `dpkg -S` succeeds | NO | `sudo apt upgrade nebo` |

### Restart Behavior (Per Platform)

| Platform | Method | Restart Strategy |
|----------|--------|-----------------|
| macOS app_bundle | DMG mount + cp -R | Background shell waits for PID to die, then `open` |
| macOS direct | Binary copy | `execve` (in-place process replacement) |
| Windows app_bundle | NSIS | Spawn installer + relaunch |
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

- **Identity:** `Developer ID Application: Alma Tuck (7Y2D3KQ2UM)` · **Bundle ID:** `dev.neboai.nebo`
- **Entitlements:** `assets/macos/nebo.entitlements` (app) + `assets/macos/obscura.entitlements` (sidecars, JIT)
- **Flow:** Tauri builds → re-sign app + obscura sidecars (hardened runtime) → DMG → notarize → staple
- Both arm64 and amd64 are signed + notarized locally (`make release-macos` / `release-macos-amd64`).

### Windows (Azure Trusted Signing)

- **Account:** `nebosigning` · **Cert profile:** `neboloop-public` · **Region:** `eastus`
- **Endpoint:** `https://eus.codesigning.azure.net/` · **Type:** Authenticode + RFC 3161 timestamp
- **Two steps:** obscura sidecars (in `build-windows`, pre-NSIS) + installer/exe (in `sign-windows`)
- **Gated:** `vars.AZURE_SIGNING_ENABLED == 'true'`
- ⚠️ **Depends on the Azure subscription being paid** — see the Billing Hold section. The cert's
  private key lives in Azure's HSM (not exportable), so there is no "sign it locally" fallback for
  this cert; any non-Azure path needs a *different* cert from another CA.

---

## CI Secrets & Variables

### Secrets

| Secret | Purpose |
|--------|---------|
| `APPLE_CERTIFICATE_P12` / `APPLE_CERTIFICATE_PASSWORD` | Developer ID cert (base64) + password |
| `APPLE_SIGNING_IDENTITY` / `APPLE_ID` / `APPLE_APP_PASSWORD` / `APPLE_TEAM_ID` | Notarization |
| `TAP_GITHUB_TOKEN` | homebrew-tap + apt repos |
| `DO_SPACES_ACCESS_KEY` / `DO_SPACES_SECRET_KEY` | DigitalOcean Spaces (CDN upload) |
| `AZURE_TENANT_ID` / `AZURE_CLIENT_ID` / `AZURE_CLIENT_SECRET` | Trusted Signing SP (`neboloop-ci-signing`) |
| `APT_GPG_PRIVATE_KEY` | GPG key for APT signing (optional) |
| `NEBOAI_CDN_URL` | CDN base for bundled .napp downloads during build |

### Variables

| Variable | Value | Purpose |
|----------|-------|---------|
| `HAS_TAP_TOKEN` | `"true"` | Enable Homebrew + APT updates |
| `AZURE_SIGNING_ENABLED` | `"true"` | Enable Windows code signing (NEVER set false to "ship") |
| `AZURE_SIGNING_ENDPOINT` | `https://eus.codesigning.azure.net/` | Signing endpoint |
| `AZURE_SIGNING_ACCOUNT` | `nebosigning` | Signing account |
| `AZURE_SIGNING_PROFILE` | `neboloop-public` | Certificate profile |
| `BUILD_MACOS_IN_CI` | unset/`false` | Set `true` to build mac in CI (expensive) |

---

## Key Files

| File | Role |
|------|------|
| `Cargo.toml` / `src-tauri/tauri.conf.json` | Workspace + Tauri version (must match) |
| `.github/workflows/release.yml` | Full CI/CD pipeline |
| `Makefile` | `release-macos`, `release-macos-amd64`, `publish-macos` (+ linux/windows local targets) |
| `scripts/publish-macos.sh` | Mac asset → GH release + Spaces + version.json + CDN flush |
| `assets/macos/nebo.entitlements` / `obscura.entitlements` | macOS signing entitlements |
| `crates/updater/src/lib.rs` / `apply.rs` | Version check/download/verify + platform apply/restart |
| `crates/server/src/lib.rs` | BackgroundChecker wiring + WS events |
| `neboloop/internal/version/poller.go` | Download-site version poller (`/api/v1/nebo/version`) |
| `neboloop/app/src/routes/(www)/download/+page.svelte` + `lib/config.ts` | Download page + fallback version |
| `app/src/lib/stores/update.ts` / `websocket/listeners.ts` / `routes/settings/about/+page.svelte` | Update UI + events |

---

## Troubleshooting

### "Windows leg fails at signing — `SignerSign() 0x80004005`, operation `Failed`, `signature: null`"
**Cause:** Azure Trusted Signing **data-plane billing hold** (lapsed/late subscription payment).
Everything else reads healthy — this is invisible on status pages.
**Fix:** See **⚠️ Azure Trusted Signing Billing Hold**. Pay the invoice, run the sign-probe, and if
it doesn't clear fast, file a free **Billing** support ticket. Never ship unsigned.

### "App keeps updating and restarting in a loop"
**Cause:** Compiled version doesn't match the tag. **Fix:** Bump both version files to match the tag, rebuild, re-release.

### "Version shows — on About page" / "Update button missing"
**Cause:** Backend JSON snake_case vs frontend camelCase (or install method not `direct`/`app_bundle`).
**Fix:** `#[serde(rename_all = "camelCase")]` on `CheckResult`.

### "Download site shows the old version / Windows button 404s"
**Cause:** `version.json` not flipped yet (poller reads it every 60s), or the Windows `.exe` isn't in
Spaces (or a cached 404). **Fix:** confirm `cdn.../version.json` = the tag; confirm the asset is in
Spaces + resolves; flush the CDN for that path.

### "Marketplace is empty"
**Cause:** NeboAI auth profile in cooldown. **Fix:** Use `list_all_active_auth_profiles_by_provider` for API client building.

### "App doesn't restart after update"
**Cause:** Race between relaunch and `exit(0)`. **Fix:** macOS uses a background shell that waits for the PID to die before relaunching.

### "Notarization failed"
**Cause:** Entitlements / signing identity / an unsigned obscura sidecar without hardened runtime.
**Debug:** `xcrun notarytool log <submission-id>`.
