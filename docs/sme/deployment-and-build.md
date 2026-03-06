# Deployment and Build: Complete SME Reference

**Source:** `nebo/docs/sme/DEPLOYMENT.md` | **Target:** `nebo-rs/docs/sme/deployment-and-build.md` | **Status:** Draft

This document is the single source of truth for building, packaging, signing, distributing, and updating Nebo in both the Go and Rust implementations. It covers the full CI/CD pipeline, code signing infrastructure, installer packaging, the in-app updater system, and all distribution channels. Read this file to immediately operate as the deployment SME -- no codebase exploration needed.

---

## Table of Contents

1. [Go Build Matrix (Reference)](#1-go-build-matrix-reference)
2. [Rust Build Matrix](#2-rust-build-matrix)
3. [CI/CD Pipeline Changes](#3-cicd-pipeline-changes)
4. [Code Signing](#4-code-signing)
5. [Packaging](#5-packaging)
6. [In-App Updater](#6-in-app-updater)
7. [Distribution Channels](#7-distribution-channels)
8. [Rust Implementation Status](#8-rust-implementation-status)

---

## 1. Go Build Matrix (Reference)

**File(s):** `nebo/Makefile`, `nebo/.github/workflows/release.yml`

The Go project builds 7 primary targets across 3 platforms. This section is preserved as a reference for understanding the CDN artifact layout and ensuring binary naming compatibility during the transition period.

### 1.1 Build Targets

| Platform | Mode | CGO | Build Tag | Output |
|----------|------|-----|-----------|--------|
| macOS arm64 | Desktop | 1 | `desktop` | `nebo-darwin-arm64` |
| macOS amd64 | Desktop | 1 | `desktop` | `nebo-darwin-amd64` |
| Linux amd64 | Desktop | 1 | `desktop` | `nebo-linux-amd64` |
| Linux arm64 | Desktop | 1 | `desktop` | `nebo-linux-arm64` |
| Linux amd64 | Headless | 0 | none | `nebo-linux-amd64-headless` |
| Linux arm64 | Headless | 0 | none | `nebo-linux-arm64-headless` |
| Windows amd64 | Desktop | 1 | `desktop` | `nebo-windows-amd64.exe` |

### 1.2 Version Injection

```makefile
VERSION ?= $(shell git describe --tags --always --dirty 2>/dev/null || echo "dev")
LDFLAGS = -ldflags "-s -w -X main.Version=$(VERSION)"
LDFLAGS_WIN = -ldflags "-s -w -X main.Version=$(VERSION) -H windowsgui"
```

- `-s -w` strips symbol table and debug info (smaller binary)
- `-X main.Version=...` injects version at compile time
- `-H windowsgui` (Windows only) suppresses the console window
- `main.Version` in `nebo.go` defaults to `"dev"` if NOT injected

### 1.3 Desktop vs Headless

| Aspect | Headless (`make build`) | Desktop (`make desktop`) |
|--------|-------------------------|--------------------------|
| CGO | `CGO_ENABLED=0` | `CGO_ENABLED=1 -tags desktop` |
| Framework | HTTP server, opens browser | Wails v3 native window + system tray |
| Code signing | N/A | Required for macOS Gatekeeper |
| Cross-compile | Yes | No -- must build natively per arch |
| Entry point | `cmd/nebo/desktop_stub.go` | `cmd/nebo/desktop.go` |

### 1.4 macOS CGO Environment

CI uses `MACOSX_DEPLOYMENT_TARGET=13.0` (minimum macOS Ventura). Local Makefile defaults to `15.0` -- override in CI.

```yaml
CGO_CFLAGS: "-mmacosx-version-min=13.0"
CGO_LDFLAGS: "-mmacosx-version-min=13.0"
```

### 1.5 Frontend Build

```bash
cd app && pnpm install --frozen-lockfile && pnpm exec svelte-kit sync && pnpm run build
```

- Output: `app/build/` (SvelteKit static adapter)
- Embedded into Go binary via `embed.FS`
- CI builds frontend once, shares as artifact across all platform jobs

### 1.6 Make Targets (Go)

```bash
make build            # Headless binary (CGO=0), includes frontend build
make desktop          # Desktop binary (CGO=1, -tags desktop)
make release          # All platforms: release-darwin + release-linux + release-windows
make release-darwin   # macOS arm64 + amd64 (CGO=1, desktop)
make release-linux    # Linux amd64 + arm64 (CGO=0, headless)
make release-windows  # Windows amd64 (CGO=0, -H windowsgui)
make app-bundle       # Assemble Nebo.app + code sign
make dmg              # Create .dmg via scripts/create-dmg.sh
make notarize         # Notarize + staple DMG
make install          # app-bundle -> notarize -> copy to /Applications
make installer        # Windows NSIS installer via makensis
```

---

## 2. Rust Build Matrix

**File(s):** `nebo-rs/Cargo.toml`, `nebo-rs/src-tauri/Cargo.toml`, `nebo-rs/src-tauri/tauri.conf.json`, `nebo-rs/Makefile`

### 2.1 Workspace Configuration

The Rust project is a Cargo workspace with 17 crates plus the Tauri desktop shell:

```toml
[workspace]
resolver = "2"
members = [
    "crates/types", "crates/config", "crates/db", "crates/auth",
    "crates/ai", "crates/tools", "crates/agent", "crates/server",
    "crates/mcp", "crates/apps", "crates/browser", "crates/voice",
    "crates/comm", "crates/notify", "crates/updater", "crates/cli",
    "src-tauri",
]
```

The `cli` crate (`nebo`) is the headless binary. The `src-tauri` crate (`nebo-desktop`) is the Tauri 2 desktop binary with system tray, native window, and shell plugin.

### 2.2 Release Profile

```toml
[profile.release]
lto = true          # Link-time optimization (full)
codegen-units = 1   # Single codegen unit for maximum optimization
strip = true        # Strip symbols (equivalent to Go's -s -w)
```

This produces binaries comparable in size to Go's stripped builds. LTO with `codegen-units = 1` trades compilation speed for smaller, faster output -- appropriate for release builds only.

### 2.3 Build Targets

| Platform | Mode | Target Triple | Binary | Output |
|----------|------|---------------|--------|--------|
| macOS arm64 | Desktop | `aarch64-apple-darwin` | `nebo-desktop` | `nebo-darwin-arm64` |
| macOS amd64 | Desktop | `x86_64-apple-darwin` | `nebo-desktop` | `nebo-darwin-amd64` |
| Linux amd64 | Desktop | `x86_64-unknown-linux-gnu` | `nebo-desktop` | `nebo-linux-amd64` |
| Linux arm64 | Desktop | `aarch64-unknown-linux-gnu` | `nebo-desktop` | `nebo-linux-arm64` |
| Linux amd64 | Headless | `x86_64-unknown-linux-gnu` | `nebo` | `nebo-linux-amd64-headless` |
| Linux arm64 | Headless | `aarch64-unknown-linux-gnu` | `nebo` | `nebo-linux-arm64-headless` |
| Windows amd64 | Desktop | `x86_64-pc-windows-msvc` | `nebo-desktop.exe` | `nebo-windows-amd64.exe` |

Key changes from Go:
- `GOOS`/`GOARCH` replaced by Rust target triples
- `CGO_ENABLED` and build tags replaced by separate crate targets (`-p nebo` vs `cargo tauri build`)
- Cross-compilation on macOS uses `--target` flag directly (both architectures available on `macos-latest`)
- Linux builds require native runners per architecture (`ubuntu-latest` for amd64, `ubuntu-24.04-arm` for arm64)

### 2.4 Version Injection

```rust
// Version set via Cargo.toml workspace version:
// [workspace.package]
// version = "0.1.0"
//
// Individual crates inherit: version.workspace = true
```

The `CARGO_PKG_VERSION` environment variable is set automatically during compilation. For git-based version strings, the `Makefile` uses `git describe`:

```makefile
VERSION ?= $(shell git describe --tags --always --dirty 2>/dev/null || echo "dev")
```

The Tauri config (`src-tauri/tauri.conf.json`) also carries the version:

```json
{
  "productName": "Nebo",
  "version": "0.1.0",
  "identifier": "dev.neboloop.nebo"
}
```

### 2.5 Desktop vs Headless

| Aspect | Headless (`cargo build -p nebo`) | Desktop (`cargo tauri build`) |
|--------|----------------------------------|-------------------------------|
| Dependencies | No system UI deps | WebKitGTK (Linux), WebView2 (Windows), WebKit (macOS) |
| Framework | Axum HTTP server, opens browser | Tauri 2 native window + system tray |
| Code signing | N/A | Required for macOS Gatekeeper |
| Cross-compile | Yes (pure Rust + bundled SQLite) | No -- requires native UI framework headers |
| Crate | `crates/cli` (`nebo`) | `src-tauri` (`nebo-desktop`) |
| Tauri plugins | None | `shell`, `window-state`, `single-instance` |

### 2.6 Cross-Compilation

Rust cross-compilation for the headless binary is straightforward because `rusqlite` bundles SQLite (`features = ["bundled"]`) and there are no system UI dependencies:

```bash
# Add target
rustup target add aarch64-unknown-linux-gnu

# Build headless for different arch
cargo build --release -p nebo --target aarch64-unknown-linux-gnu
```

For desktop (Tauri) builds, native compilation per platform is REQUIRED because WebKitGTK / WebView2 headers must match the target architecture. This is why CI uses per-architecture runners for Linux desktop builds.

### 2.7 Rust-Specific Dependencies (Desktop)

Linux desktop builds require these system packages (installed in CI):

```bash
sudo apt-get install -y \
    libwebkit2gtk-4.1-dev \
    libgtk-3-dev \
    libappindicator3-dev \
    librsvg2-dev \
    pkg-config \
    build-essential
```

macOS and Windows do NOT need additional system deps -- macOS uses built-in WebKit, Windows uses WebView2 (auto-downloaded by Tauri if NOT present).

### 2.8 Make Targets (Rust)

```bash
# Development
make dev              # Hot reload headless server (cargo watch)
make build            # Headless CLI binary (cargo build --release -p nebo)
make build-desktop    # Tauri desktop app (cargo tauri build)
make test             # Run all tests (cargo test)
make clean            # Remove target/ and dist/

# Desktop (macOS)
make app-bundle       # Re-sign Tauri .app with Developer ID
make dmg              # Create .dmg installer
make notarize         # Notarize .dmg with Apple
make install          # Notarize + install to /Applications

# Release
make release              # All platforms (clean + darwin + linux + windows)
make release-darwin       # macOS arm64 + amd64 (Tauri)
make release-linux        # Linux desktop + headless
make release-windows      # Windows .exe + .msi (Tauri)
make github-release TAG=v0.1.0  # Create GitHub release with all binaries
```

### 2.9 Frontend Integration

The frontend is the SAME SvelteKit app shared with the Go project (`app/` directory). In the Rust build:

- CI builds frontend to `app/build/`, then copies to `src-tauri/ui/`
- Tauri's `frontendDist` config points to `./ui`
- The headless binary embeds the frontend via `rust-embed` from `../../app/build/` (symlinked from Go project)

---

## 3. CI/CD Pipeline Changes

**File(s):** `nebo-rs/.github/workflows/release.yml`

### 3.1 Trigger

Both Go and Rust pipelines use identical tag-triggered releases:

```yaml
on:
  push:
    tags: ["v*"]  # e.g., git tag v0.1.0 && git push origin v0.1.0
```

### 3.2 Toolchain Versions

| Tool | Go Project | Rust Project |
|------|-----------|--------------|
| Language | Go 1.25 | Rust 1.87 |
| Node.js | 20 | 20 |
| Package manager | pnpm | pnpm |
| Rust toolchain action | N/A | `dtolnay/rust-toolchain@stable` |
| Cargo cache | N/A | `Swatinem/rust-cache@v2` |
| Desktop framework | Wails v3 | Tauri 2 (`cargo install tauri-cli --version "^2"`) |

### 3.3 Job Dependency Graph (Rust)

```
frontend (ubuntu-latest)
  |---> build-macos (macos-latest x [arm64, amd64])
  |       |--> re-sign .app with Developer ID
  |       |--> notarize .dmg
  |       \--> release
  |---> build-linux (ubuntu-latest, ubuntu-24.04-arm x [amd64, arm64])
  |       |--> Tauri desktop build
  |       |--> Headless CLI build
  |       |--> collect .deb (Tauri-generated)
  |       \--> release
  |       \--> update-apt
  \---> build-windows (windows-latest)
          |--> Tauri desktop build
          |--> extract .exe + .msi (Tauri-generated)
          \--> sign-windows (if AZURE_SIGNING_ENABLED)
          \--> release

release (ubuntu-latest)
  \--> update-homebrew
```

### 3.4 Key Differences from Go Pipeline

| Aspect | Go | Rust |
|--------|----|----- |
| Desktop framework | Wails v3 (custom app bundle) | Tauri 2 (auto-generates bundles) |
| DMG creation | Custom `scripts/create-dmg.sh` | Tauri CLI generates DMG automatically |
| .deb packaging | nfpm with yaml template | Tauri CLI generates .deb automatically |
| Windows installer | NSIS (`scripts/installer.nsi`) | Tauri CLI generates MSI automatically |
| macOS signing | Manual `codesign` in Makefile | Tauri signs + CI re-signs with Developer ID |
| Separate sign job | No (inline in build job) | Yes (`sign-windows` job, separate from build) |
| Linux headless | Separate build step (`CGO_ENABLED=0`) | Separate `cargo build --release -p nebo` |
| Build caching | Go module cache | `Swatinem/rust-cache@v2` (Cargo registry + target dir) |

### 3.5 Job Details (Rust)

**frontend:** Node 20, pnpm, builds SvelteKit to `app/build/`. Uploads as `frontend-build` artifact (1-day retention). EXACT same step as Go pipeline.

**build-macos:** Downloads frontend artifact to `src-tauri/ui/`. Installs Rust toolchain with target triple. Imports Apple signing certificate into temporary keychain. Runs `cargo tauri build --target {triple}`. Re-signs the `.app` bundle with Developer ID and entitlements (`--deep` flag). Extracts the signed binary from `Nebo.app/Contents/MacOS/Nebo` as `nebo-darwin-{arch}` for CDN auto-update. Locates the Tauri-generated DMG, renames to `Nebo-{version}-{arch}.dmg`, notarizes and staples. Uploads: bare binary artifact + DMG artifact.

**build-linux:** Downloads frontend artifact. Installs `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `libappindicator3-dev`, `librsvg2-dev`, `pkg-config`, `build-essential`. Runs `cargo tauri build` for desktop. Separately runs `cargo build --release -p nebo` for headless CLI. Collects `.deb` from Tauri's bundle output. Uploads: desktop binary, headless binary, .deb artifacts.

**build-windows:** Downloads frontend artifact. Runs `cargo tauri build` on Windows runner. Extracts `nebo-desktop.exe` as `nebo-windows-amd64.exe`. Collects MSI from Tauri bundle output. Uploads: binary + MSI artifacts.

**sign-windows:** Gated on `vars.AZURE_SIGNING_ENABLED == 'true'`. Downloads Windows binary and MSI. Signs both with `azure/trusted-signing-action@v1`. Re-uploads as `nebo-windows-amd64-signed` artifact. The `release` job prefers signed variants.

**release:** Downloads ALL artifacts. Prefers signed Windows binary over unsigned. Makes binaries executable. Generates `checksums.txt` with SHA256. Uploads all assets to DigitalOcean Spaces CDN (gated on `DO_SPACES_ACCESS_KEY`). Creates GitHub Release via `softprops/action-gh-release@v2`.

**update-homebrew:** Gated on `vars.HAS_TAP_TOKEN == 'true'`. Computes SHA256 of both DMGs. Checks out `neboloop/homebrew-tap`. Templates `scripts/nebo.rb.tmpl` via `envsubst`. Pushes commit. EXACT same logic as Go pipeline.

**update-apt:** Gated on `vars.HAS_TAP_TOKEN == 'true'`. Downloads `.deb` packages. Checks out `neboloop/apt`. Runs `dpkg-scanpackages` for `Packages` index. Creates `Release` file. Optional GPG signing with `APT_GPG_PRIVATE_KEY`. Pushes commit. EXACT same logic as Go pipeline.

### 3.6 CDN Upload (Shared)

Both pipelines upload to the SAME CDN endpoint using the SAME structure:

```bash
# version.json at root (latest pointer)
aws s3 cp version.json s3://neboloop/releases/version.json \
  --endpoint-url https://nyc3.digitaloceanspaces.com --acl public-read

# All assets under releases/{tag}/
aws s3 cp artifacts/ s3://neboloop/releases/${TAG}/ \
  --endpoint-url https://nyc3.digitaloceanspaces.com --acl public-read
```

CRITICAL: During the transition period, both Go and Rust releases write to the SAME CDN paths. The `version.json` manifest at root always points to the latest release, regardless of which implementation produced it. The updater binary naming (`nebo-darwin-arm64`, `nebo-linux-amd64`, etc.) is IDENTICAL between Go and Rust -- the in-app updater does NOT know which implementation it is downloading.

---

## 4. Code Signing

**File(s):** `nebo-rs/.github/workflows/release.yml`, `nebo-rs/Makefile`, `assets/macos/nebo.entitlements`

### 4.1 macOS Code Signing

The Apple code signing infrastructure is SHARED between Go and Rust. Both use the same:
- **Signing identity:** `Developer ID Application: Alma Tuck (7Y2D3KQ2UM)`
- **Bundle identifier:** `dev.neboloop.nebo`
- **Team ID:** `7Y2D3KQ2UM`
- **Entitlements file:** `assets/macos/nebo.entitlements`
- **Minimum macOS:** 13.0 (Ventura)

### 4.2 macOS Signing Process (CI)

```yaml
# 1. Import P12 certificate into temporary keychain
KEYCHAIN_PATH=$RUNNER_TEMP/build.keychain-db
security create-keychain -p "" "$KEYCHAIN_PATH"
security import $RUNNER_TEMP/cert.p12 -k "$KEYCHAIN_PATH" -P "$PASSWORD" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple: -s -k "" "$KEYCHAIN_PATH"

# 2. Build Tauri app (Tauri applies its own codesign pass)
cargo tauri build --target aarch64-apple-darwin

# 3. Re-sign with Developer ID + entitlements (overrides Tauri's ad-hoc signing)
codesign --force --sign "$SIGN_IDENTITY" \
  --identifier dev.neboloop.nebo \
  --entitlements assets/macos/nebo.entitlements \
  --options runtime \
  --deep \
  "src-tauri/target/$TARGET/release/bundle/macos/Nebo.app"
```

The `--deep` flag is used because Tauri bundles may include nested frameworks. The `--force` flag overwrites Tauri's default ad-hoc signature with the Developer ID signature.

### 4.3 macOS Notarization

```bash
# CI (via secrets)
xcrun notarytool submit "dist/Nebo-${VERSION}-${ARCH}.dmg" \
  --apple-id "$APPLE_ID" \
  --password "$APPLE_APP_PASSWORD" \
  --team-id "$APPLE_TEAM_ID" \
  --wait
xcrun stapler staple "dist/Nebo-${VERSION}-${ARCH}.dmg"

# Local (via keychain profile)
xcrun notarytool submit "dist/Nebo-${VERSION}-${ARCH}.dmg" \
  --keychain-profile "nebo-notarize" --wait
xcrun stapler staple "dist/Nebo-${VERSION}-${ARCH}.dmg"
```

First-time local setup:
```bash
xcrun notarytool store-credentials "nebo-notarize"
# Enter: Apple ID, app-specific password (generate at appleid.apple.com), team ID
```

### 4.4 macOS Entitlements

From `assets/macos/nebo.entitlements`:
- File access (user-selected + downloads)
- Network (client + server)
- Microphone, camera
- AppleEvents automation
- Contacts, calendars

### 4.5 Windows Code Signing (Azure Trusted Signing)

The Windows signing infrastructure is SHARED between Go and Rust:

- **Azure service:** Trusted Signing (East US region)
- **Signing account:** `nebosigning`
- **Certificate profile:** `neboloop-public` (Public Trust, linked to NeboLoop LLC identity)
- **Endpoint:** `https://eus.codesigning.azure.net/`
- **Service principal:** `neboloop-ci-signing`
- **Digest:** SHA256
- **Timestamp:** `http://timestamp.acs.microsoft.com` (RFC3161)

```yaml
# CI step (both Go and Rust)
- uses: azure/trusted-signing-action@v1
  with:
    azure-tenant-id: ${{ secrets.AZURE_TENANT_ID }}
    azure-client-id: ${{ secrets.AZURE_CLIENT_ID }}
    azure-client-secret: ${{ secrets.AZURE_CLIENT_SECRET }}
    endpoint: ${{ vars.AZURE_SIGNING_ENDPOINT }}
    trusted-signing-account-name: ${{ vars.AZURE_SIGNING_ACCOUNT }}
    certificate-profile-name: ${{ vars.AZURE_SIGNING_PROFILE }}
    files-folder: ${{ github.workspace }}
    files-folder-filter: exe,msi    # Rust adds MSI (Go used NSIS .exe)
    files-folder-depth: 1
    file-digest: SHA256
    timestamp-rfc3161: http://timestamp.acs.microsoft.com
    timestamp-digest: SHA256
```

Key difference: The Rust pipeline signs MSI files in addition to EXE files. The Go pipeline signed the NSIS installer `.exe` -- the Rust pipeline signs the Tauri-generated `.msi` installer instead.

### 4.6 APT GPG Signing

Optional GPG signing for the APT repository. Only if `APT_GPG_PRIVATE_KEY` secret is set:

```bash
echo "$GPG_KEY" | gpg --import
gpg --armor --export > apt-repo/key.gpg
gpg --default-key nebo -abs -o dists/stable/Release.gpg dists/stable/Release
gpg --default-key nebo --clearsign -o dists/stable/InRelease dists/stable/Release
```

### 4.7 Secrets Reference

| Secret | Purpose | Required By |
|--------|---------|-------------|
| `APPLE_CERTIFICATE_P12` | Base64-encoded Developer ID certificate | macOS signing |
| `APPLE_CERTIFICATE_PASSWORD` | Certificate password | macOS signing |
| `APPLE_ID` | Apple ID email for notarization | macOS notarization |
| `APPLE_APP_PASSWORD` | App-specific password for notarization | macOS notarization |
| `APPLE_TEAM_ID` | Apple Developer Team ID | macOS notarization |
| `APPLE_SIGNING_IDENTITY` | Full signing identity string | macOS signing |
| `AZURE_TENANT_ID` | Azure AD tenant | Windows signing |
| `AZURE_CLIENT_ID` | Azure service principal | Windows signing |
| `AZURE_CLIENT_SECRET` | Azure service principal secret | Windows signing |
| `TAP_GITHUB_TOKEN` | Fine-grained PAT for homebrew-tap + apt repos | Homebrew/APT |
| `APT_GPG_PRIVATE_KEY` | GPG private key for APT signing | APT (optional) |
| `DO_SPACES_ACCESS_KEY` | DigitalOcean Spaces access key | CDN upload |
| `DO_SPACES_SECRET_KEY` | DigitalOcean Spaces secret key | CDN upload |

### 4.8 Variables Reference

| Variable | Purpose | Value |
|----------|---------|-------|
| `HAS_TAP_TOKEN` | Gate Homebrew/APT update jobs | `"true"` to enable |
| `AZURE_SIGNING_ENABLED` | Gate Windows code signing | `"true"` (enabled) |
| `AZURE_SIGNING_ENDPOINT` | Azure Trusted Signing endpoint URL | `https://eus.codesigning.azure.net/` |
| `AZURE_SIGNING_ACCOUNT` | Azure signing account name | `nebosigning` |
| `AZURE_SIGNING_PROFILE` | Code signing certificate profile | `neboloop-public` |

---

## 5. Packaging

**File(s):** `nebo-rs/src-tauri/tauri.conf.json`, `nebo-rs/Makefile`, `scripts/nebo.rb.tmpl`

### 5.1 Tauri 2 Auto-Packaging

The most significant change from Go to Rust is that Tauri 2 generates native installers automatically. The Go project required custom scripts for each platform. Tauri handles this via `bundle.targets: "all"` in `tauri.conf.json`:

| Installer | Go Approach | Rust Approach |
|-----------|------------|---------------|
| macOS DMG | Custom `scripts/create-dmg.sh` | Tauri CLI auto-generates DMG |
| macOS .app | Manual assembly in Makefile | Tauri CLI auto-generates .app |
| Windows MSI | N/A | Tauri CLI auto-generates MSI (WiX) |
| Windows NSIS | Custom `scripts/installer.nsi` | NOT used (replaced by MSI) |
| Linux .deb | nfpm with `nfpm.yaml.tmpl` | Tauri CLI auto-generates .deb |

### 5.2 macOS DMG

Tauri generates the DMG during `cargo tauri build`. CI renames it for consistency:

```bash
# Tauri output:
src-tauri/target/{target}/release/bundle/dmg/Nebo_{version}_{arch}.dmg

# CI renames to match Go convention:
dist/Nebo-{version}-{arch}.dmg
```

For local development, the Makefile provides a `dmg` target that uses `create-dmg` (brew) or falls back to `hdiutil`:

```makefile
dmg: app-bundle
    create-dmg \
        --volname "Nebo" \
        --volicon "src-tauri/icons/icon.icns" \
        --window-pos 200 120 --window-size 600 400 \
        --icon-size 100 --icon "Nebo.app" 175 190 \
        --hide-extension "Nebo.app" --app-drop-link 425 190 \
        "dist/Nebo-$(VERSION)-$(UNAME_M).dmg" "dist/Nebo.app"
```

### 5.3 macOS App Bundle

Tauri generates the `.app` bundle automatically:

```
Nebo.app/
  Contents/
    MacOS/Nebo              <-- Tauri-built desktop binary (nebo-desktop)
    Resources/icon.icns     <-- App icon from src-tauri/icons/
    Info.plist              <-- Generated by Tauri from tauri.conf.json
```

CI re-signs the bundle with Developer ID after Tauri's build:

```bash
# Tauri signs with ad-hoc identity by default
# CI overrides with Developer ID for Gatekeeper compliance
codesign --force --sign "$SIGN_IDENTITY" \
    --identifier dev.neboloop.nebo \
    --entitlements assets/macos/nebo.entitlements \
    --options runtime --deep \
    "src-tauri/target/$TARGET/release/bundle/macos/Nebo.app"
```

### 5.4 Windows MSI Installer (Replaces NSIS)

Tauri 2 generates MSI installers via WiX Toolset instead of NSIS:

```
src-tauri/target/release/bundle/msi/Nebo_{version}_{arch}.msi
```

The MSI installer provides:
- Installation to Program Files
- Start Menu shortcuts
- Add/Remove Programs registration
- File associations (if configured)
- Per-machine or per-user install

The Go project's NSIS installer (`scripts/installer.nsi`) added the install directory to system PATH. The Tauri MSI does NOT do this by default -- PATH management is handled by the CLI crate if needed.

### 5.5 Linux .deb Package

Tauri generates `.deb` packages automatically:

```
src-tauri/target/release/bundle/deb/nebo-desktop_{version}_{arch}.deb
```

Go's .deb required:
- Custom `nfpm.yaml.tmpl` with template substitution
- Separate `nfpm` tool installation
- Manual `scripts/postinstall.sh`

Tauri's .deb bundles:
- The desktop binary
- A `.desktop` file for Linux desktop integration
- Application icons
- Library dependencies metadata

### 5.6 Homebrew Cask

The Homebrew cask template (`scripts/nebo.rb.tmpl`) is SHARED infrastructure:

```ruby
cask "nebo" do
  version "${PKG_VERSION}"

  on_arm do
    url "https://cdn.neboloop.com/releases/v${VERSION}/Nebo-${PKG_VERSION}-arm64.dmg"
    sha256 "${SHA_DARWIN_ARM64}"
  end

  on_intel do
    url "https://cdn.neboloop.com/releases/v${VERSION}/Nebo-${PKG_VERSION}-amd64.dmg"
    sha256 "${SHA_DARWIN_AMD64}"
  end

  name "Nebo"
  homepage "https://neboloop.com"
  app "Nebo.app"
  binary "#{appdir}/Nebo.app/Contents/MacOS/Nebo", target: "nebo"
  zap trash: "~/Library/Application Support/Nebo"
end
```

The binary path inside the `.app` bundle changed:
- Go: `Contents/MacOS/nebo` (lowercase)
- Rust: `Contents/MacOS/Nebo` (capitalized, Tauri convention)

### 5.7 Tauri Configuration

```json
{
  "productName": "Nebo",
  "version": "0.1.0",
  "identifier": "dev.neboloop.nebo",
  "build": {
    "frontendDist": "./ui"
  },
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": [
      "icons/32x32.png",
      "icons/128x128.png",
      "icons/128x128@2x.png",
      "icons/icon.icns",
      "icons/icon.ico"
    ]
  }
}
```

---

## 6. In-App Updater

**File(s):** `nebo-rs/crates/updater/src/lib.rs`, `nebo-rs/crates/updater/src/apply.rs`, `nebo-rs/crates/updater/Cargo.toml`

### 6.1 Architecture Overview

The Rust updater is a standalone crate (`nebo-updater`) that implements the SAME protocol as the Go `internal/updater/` package. Both communicate with the SAME CDN endpoint and use the SAME binary naming convention.

```
+---------------------------------------------------------------+
|  Frontend (Svelte) -- SHARED between Go and Rust              |
|                                                               |
|  update.ts store <-> UpdateBanner.svelte                      |
|    checkForUpdate() -> GET /api/v1/update/check               |
|    applyUpdate()    -> POST /api/v1/update/apply              |
+----------------------------+----------------------------------+
                             |
+----------------------------v----------------------------------+
|  Rust Backend (nebo-updater crate)                            |
|                                                               |
|  pub async fn check(version) -> CheckResult                   |
|  pub async fn download(tag, progress) -> PathBuf              |
|  pub async fn verify_checksum(path, tag) -> ()                |
|  pub fn apply_update(path) -> ()                              |
|  pub fn detect_install_method() -> &str                       |
|  pub fn asset_name() -> String                                |
|  pub struct BackgroundChecker { run(), check_once() }         |
+---------------------------------------------------------------+
```

### 6.2 CDN Protocol (Shared)

The CDN layout and protocol is IDENTICAL between Go and Rust:

```
https://cdn.neboloop.com/releases/
  version.json                         <-- latest version pointer
  v0.1.0/
    nebo-darwin-arm64                  <-- bare binary (macOS Apple Silicon)
    nebo-darwin-amd64                  <-- bare binary (macOS Intel)
    nebo-linux-amd64                   <-- desktop binary
    nebo-linux-arm64                   <-- desktop binary
    nebo-linux-amd64-headless          <-- headless binary
    nebo-linux-arm64-headless          <-- headless binary
    nebo-windows-amd64.exe             <-- bare binary
    checksums.txt                      <-- SHA256 of all binaries
    Nebo-0.1.0-arm64.dmg              <-- macOS DMG (signed + notarized)
    Nebo-0.1.0-amd64.dmg              <-- macOS DMG (signed + notarized)
    *.msi                              <-- Windows installer (Rust only)
    *.deb                              <-- Linux .deb packages
    version.json                       <-- per-tag manifest
```

`version.json` format:
```json
{
  "version": "v0.1.0",
  "release_url": "https://github.com/NeboLoop/nebo-rs/releases/tag/v0.1.0",
  "published_at": "2026-03-04T12:00:00Z"
}
```

### 6.3 Check Flow (Rust)

```rust
/// Check the CDN for a newer version.
pub async fn check(current_version: &str) -> Result<CheckResult, UpdateError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))       // 5s timeout, same as Go
        .user_agent(format!("nebo/{}", current_version))
        .build()?;

    let resp = client.get(RELEASE_URL).send().await?;
    let manifest: VersionManifest = resp.json().await?;
    let latest = normalize_version(&manifest.version);
    let current = normalize_version(current_version);

    // "dev" builds NEVER see updates (same as Go)
    let available = latest != current && current != "dev" && is_newer(&latest, &current);
    let method = detect_install_method();

    Ok(CheckResult {
        available,
        current_version: current_version.to_string(),
        latest_version: manifest.version,
        release_url: manifest.release_url,
        published_at: manifest.published_at,
        install_method: method.to_string(),
        can_auto_update: method == "direct",
    })
}
```

Install method detection logic is IDENTICAL to Go:
- Resolves symlinks on executable path
- `/opt/homebrew/` or `/usr/local/Cellar/` -> `"homebrew"`
- `dpkg -S` succeeds (Linux only, `#[cfg(target_os = "linux")]`) -> `"package_manager"`
- Everything else -> `"direct"` (can auto-update)

### 6.4 Download Flow (Rust)

```rust
pub async fn download(
    tag: &str,
    progress: Option<ProgressFn>,
) -> Result<PathBuf, UpdateError> {
    let asset = asset_name();  // "nebo-{os}-{arch}[.exe]"
    let url = format!("{}/{}/{}", RELEASE_DOWNLOAD_URL, tag, asset);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))   // 10-minute timeout, same as Go
        .build()?;

    let resp = client.get(&url).send().await?;
    let total = resp.content_length().unwrap_or(0);
    let tmp_path = std::env::temp_dir().join(format!("nebo-update-{}", Uuid::new_v4()));

    let mut file = tokio::fs::File::create(&tmp_path).await?;
    let mut stream = resp.bytes_stream();
    // Stream in chunks with progress callbacks
    // ...

    // Set chmod 0755 on Unix
    #[cfg(unix)]
    std::fs::set_permissions(&tmp_path, Permissions::from_mode(0o755))?;

    Ok(tmp_path)
}
```

### 6.5 Checksum Verification (Rust)

```rust
pub async fn verify_checksum(binary_path: &Path, tag: &str) -> Result<(), UpdateError> {
    let url = format!("{}/{}/checksums.txt", RELEASE_DOWNLOAD_URL, tag);
    // 404 = skip verification (graceful for old releases), same as Go
    // Parses "{sha256}  {filename}" format
    // Computes SHA256 via sha2 crate, case-insensitive compare
}
```

### 6.6 Apply Flow (Rust)

**Shared helpers (`apply.rs`):**
- `health_check(binary_path)` -- runs `{binary} --version` (same `--version` flag as Go)
- `copy_file(src, dst)` -- copies file in 64KB chunks preserving permissions
- `set_pre_apply_hook(f)` -- registers a pre-restart callback (for resource cleanup)

**Unix (`#[cfg(unix)]`):**
1. Health check via `health_check()` (runs `--version`)
2. Resolve current exe path + follow symlinks
3. Backup: `copy_file(current, current.old)`
4. Replace: `copy_file(new, current)` -- rollback to `.old` on failure
5. Clean temp file
6. Run pre-apply hook (resource cleanup)
7. `libc::execve(current, args, env)` -- replaces process in-place (same PID)

**Windows (`#[cfg(windows)]`):**
1. Health check via `health_check()` (runs `--version`)
2. Rename running exe: `fs::rename(current, current.exe.old)` -- Windows allows renaming running exe
3. Copy new: `copy_file(new, current)` -- uses copy NOT rename (avoids cross-filesystem failure)
4. Clean temp file
5. Run pre-apply hook
6. `Command::new(current).args(args).spawn()` + `process::exit(0)`

### 6.7 BackgroundChecker (Rust)

```rust
pub struct BackgroundChecker {
    version: String,
    interval: Duration,           // Default: 1 hour (Go used 6 hours)
    notify: Box<dyn Fn(CheckResult) + Send + Sync>,
    last_notified: Mutex<Option<String>>,
}

impl BackgroundChecker {
    pub async fn run(&self, cancel: CancellationToken) {
        // 30-second initial delay (same as Go)
        tokio::time::sleep(Duration::from_secs(30)).await;
        self.check_once().await;

        // Periodic loop
        let mut interval = tokio::time::interval(self.interval);
        loop {
            tokio::select! {
                _ = interval.tick() => self.check_once().await,
                _ = cancel.cancelled() => return,
            }
        }
    }
}
```

Cancellation uses `tokio_util::sync::CancellationToken` instead of Go's `context.Context`. The behavior is equivalent: the loop exits cleanly when the token is cancelled.

Deduplication: the `last_notified` field prevents repeated notifications for the same version (same as Go).

### 6.8 Updater Dependencies

```toml
[dependencies]
reqwest = { workspace = true }        # HTTP client (async)
sha2 = { workspace = true }           # SHA256 computation
hex = { workspace = true }            # Hex encoding for checksum comparison
tokio = { workspace = true }          # Async runtime
tokio-util = { workspace = true }     # CancellationToken
futures = { workspace = true }        # StreamExt for byte streaming
uuid = { workspace = true }           # Temp file naming
libc = "0.2"                          # Unix execve
tracing = { workspace = true }        # Logging
thiserror = { workspace = true }      # Error types
serde = { workspace = true }          # JSON deserialization
serde_json = { workspace = true }     # JSON parsing
```

### 6.9 Known Limitations (Shared)

These limitations apply to BOTH Go and Rust implementations:
- **In-memory pending state** -- pending update path is NOT persisted across restarts
- **No delta updates** -- always downloads full binary
- **No staged rollout** -- all releases immediately available
- **Single `.old` backup** -- no versioned rollback chain
- **No auto-apply** -- always requires user confirmation
- **"dev" builds** never see updates

---

## 7. Distribution Channels

### 7.1 DigitalOcean Spaces CDN (Primary for Auto-Update + Downloads)

- **Public URL:** `https://cdn.neboloop.com/releases/`
- **CNAME:** `cdn.neboloop.com` -> `neboloop.nyc3.cdn.digitaloceanspaces.com`
- **Contents:** `version.json` manifest + all binaries + DMGs + MSIs + .debs + checksums per tag
- **Used by:** in-app updater (version checks + binary downloads) AND neboloop.com download links
- **No rate limits** (unlike GitHub API's 60 req/hr unauthenticated)
- **Populated by:** CI `release` job via `aws s3 cp --endpoint-url`
- **SHARED** between Go and Rust releases (same S3 bucket, same paths)

First-time CDN setup:
```bash
doctl spaces create neboloop --region nyc3
doctl compute cdn create neboloop.nyc3.digitaloceanspaces.com --ttl 3600
doctl spaces keys create --name nebo-ci
# DNS: cdn.neboloop.com -> CNAME -> neboloop.nyc3.cdn.digitaloceanspaces.com
# DO Spaces CDN: add cdn.neboloop.com as custom domain + enable Let's Encrypt SSL
gh secret set DO_SPACES_ACCESS_KEY
gh secret set DO_SPACES_SECRET_KEY
```

### 7.2 GitHub Releases (Source of Truth for Humans)

- **Go URL:** `https://github.com/NeboLoop/nebo/releases`
- **Rust URL:** `https://github.com/NeboLoop/nebo-rs/releases`
- **Artifacts:** raw binaries, DMGs, MSI/NSIS installer, .deb packages, checksums.txt
- **Auto-generated release notes** from commits
- `release_url` in CDN manifest points here for "Release notes" link in UI

NOTE: During the transition, the CDN `version.json` `release_url` will point to whichever repo (Go or Rust) produced the latest release. The frontend "Release notes" link will follow accordingly.

### 7.3 Homebrew (macOS)

```bash
brew install neboloop/tap/nebo
```

- **Tap repo:** `neboloop/homebrew-tap` (Casks/nebo.rb)
- **Auto-updated** by CI on each release (both Go and Rust pipelines)
- **Installs:** Nebo.app + `nebo` CLI symlink
- **SHA256:** per-arch (arm64 + amd64)
- **Upgrade:** `brew upgrade nebo`

### 7.4 APT (Linux)

```bash
# Add repo
echo "deb https://neboloop.github.io/apt stable main" | sudo tee /etc/apt/sources.list.d/nebo.list
sudo apt update
sudo apt install nebo
```

- **Repo:** `neboloop/apt` (GitHub Pages)
- **Auto-updated** by CI on each release
- **Supports:** amd64 + arm64
- **Optional GPG signing**
- **Upgrade:** `sudo apt upgrade nebo`

### 7.5 Direct Download

- Users download binary from GitHub Releases or CDN
- macOS: DMG with drag-to-Applications
- Windows: MSI installer (Rust) or NSIS installer (Go)
- Linux: raw binary or .deb
- **Only direct installs support auto-update** -- Homebrew/APT users use their package manager

### 7.6 Docker (Go Only -- NOT Yet Ported)

The Go project has a multi-stage Dockerfile:

```
Stage 1: development    -- golang:1.25-alpine + Node 20 + pnpm + Air
Stage 2: frontend-builder -- node:20-alpine, builds SvelteKit
Stage 3: builder        -- golang:1.25-alpine, CGO_ENABLED=0, embeds frontend
Stage 4: production     -- alpine:latest + ca-certificates + curl + wget + tzdata
```

The Rust project does NOT yet have a Dockerfile. When implemented, it should use a similar multi-stage approach:

```
Stage 1: frontend-builder -- node:20-alpine, builds SvelteKit
Stage 2: builder          -- rust:{version}-alpine, builds headless binary (cargo build -p nebo)
Stage 3: production       -- alpine:latest + ca-certificates + tzdata
```

The Rust headless binary with bundled SQLite (`rusqlite features=["bundled"]`) should produce a single static binary without external dependencies, making the Docker image even simpler than Go's.

---

## 8. Rust Implementation Status

### 8.1 Build System

| Component | Status | Notes |
|-----------|--------|-------|
| Cargo workspace | Y | 17 crates + src-tauri |
| Release profile (LTO, strip) | Y | `lto = true`, `codegen-units = 1`, `strip = true` |
| Headless binary (`-p nebo`) | Y | `cargo build --release -p nebo` |
| Desktop binary (Tauri 2) | Y | `cargo tauri build` |
| macOS arm64 | Y | `aarch64-apple-darwin` target |
| macOS amd64 | Y | `x86_64-apple-darwin` target |
| Linux amd64 desktop | Y | Native build on `ubuntu-latest` |
| Linux arm64 desktop | Y | Native build on `ubuntu-24.04-arm` |
| Linux amd64 headless | Y | Separate `cargo build -p nebo` step |
| Linux arm64 headless | Y | Separate `cargo build -p nebo` step |
| Windows amd64 | Y | Native build on `windows-latest` |
| Windows arm64 | N | NOT in build matrix (same as Go) |
| Frontend build | Y | Same SvelteKit app, shared between Go and Rust |
| Version injection | P | Uses `CARGO_PKG_VERSION`; git-based version in Makefile only |

### 8.2 CI/CD Pipeline

| Component | Status | Notes |
|-----------|--------|-------|
| Tag-triggered release | Y | `v*` pattern, same as Go |
| Frontend job | Y | Identical to Go pipeline |
| macOS build + sign | Y | Tauri build + Developer ID re-sign |
| macOS notarization | Y | Same `notarytool` + `stapler` |
| Linux desktop build | Y | Per-arch runners with WebKitGTK deps |
| Linux headless build | Y | Separate `cargo build -p nebo` |
| Windows build | Y | Tauri on `windows-latest` |
| Windows signing (Azure) | Y | Separate `sign-windows` job |
| Checksums generation | Y | `sha256sum` on all binaries |
| CDN upload | Y | Same S3 endpoint + bucket + paths |
| GitHub Release | Y | `softprops/action-gh-release@v2` |
| Homebrew update | Y | Same template + cross-repo push |
| APT update | Y | Same `dpkg-scanpackages` + GPG flow |
| Rust caching | Y | `Swatinem/rust-cache@v2` |

### 8.3 Code Signing

| Component | Status | Notes |
|-----------|--------|-------|
| macOS Developer ID | Y | Same cert, same identity |
| macOS entitlements | Y | Same entitlements file |
| macOS notarization | Y | Same notarytool + stapler |
| Windows Authenticode | Y | Same Azure Trusted Signing account |
| APT GPG signing | Y | Same optional GPG flow |

### 8.4 Packaging

| Component | Status | Notes |
|-----------|--------|-------|
| macOS DMG | Y | Tauri auto-generates; Makefile has manual option |
| macOS .app bundle | Y | Tauri auto-generates; CI re-signs |
| Windows MSI | Y | Tauri auto-generates (replaces NSIS) |
| Windows NSIS | N | NOT ported -- replaced by MSI |
| Linux .deb | Y | Tauri auto-generates |
| Homebrew cask | Y | Same template, shared |

### 8.5 In-App Updater

| Component | Status | Notes |
|-----------|--------|-------|
| CDN version check | Y | `check()` in `crates/updater/src/lib.rs` |
| Install method detection | Y | `detect_install_method()` -- homebrew, package_manager, direct |
| Binary download | Y | `download()` with progress callbacks |
| SHA256 verification | Y | `verify_checksum()` against `checksums.txt` |
| Unix apply (execve) | Y | `apply()` in `crates/updater/src/apply.rs` |
| Windows apply (rename+spawn) | Y | `apply()` with `#[cfg(windows)]` |
| Health check | Y | `health_check()` runs `--version` |
| Backup + rollback | Y | `.old` backup, rollback on copy failure |
| BackgroundChecker | Y | Periodic check with CancellationToken |
| Pre-apply hook | Y | `set_pre_apply_hook()` for resource cleanup |
| Asset naming | Y | `nebo-{os}-{arch}[.exe]` -- same as Go |
| Frontend update UI | Y | Shared SvelteKit stores + UpdateBanner component |

### 8.6 Distribution Channels

| Channel | Status | Notes |
|---------|--------|-------|
| CDN (DigitalOcean Spaces) | Y | Same bucket, same paths, same version.json |
| GitHub Releases | Y | `NeboLoop/nebo-rs` repository |
| Homebrew | Y | Same tap repo, same cask template |
| APT | Y | Same apt repo, same update flow |
| Direct download | Y | Binaries on CDN + GitHub |
| Docker | N | NOT yet ported -- Go only |

### 8.7 Outstanding Work

| Item | Priority | Description |
|------|----------|-------------|
| Docker support | Medium | Multi-stage Dockerfile for headless Rust binary |
| Windows arm64 | Low | Add `aarch64-pc-windows-msvc` target to CI matrix |
| Version from git tags | Medium | Inject git version into Rust binary at build time (build.rs or env var) |
| Linux arm-7 (32-bit ARM) | Low | Go had `linux-arm-7` target; NOT planned for Rust |
| Tauri auto-updater | Low | Tauri has built-in updater plugin -- evaluate vs custom updater crate |
| NSIS installer | N | NOT planned -- MSI replaces it in Rust |

---

## Key Files Map

### Build and CI

| File | Purpose |
|------|---------|
| `Cargo.toml` | Workspace root -- all crates, shared deps, release profile |
| `src-tauri/Cargo.toml` | Tauri desktop crate deps (tauri, plugins, server, config) |
| `src-tauri/tauri.conf.json` | Tauri config (product name, version, bundle targets, icons) |
| `Makefile` | All build, release, packaging, install targets |
| `.github/workflows/release.yml` | Multi-platform CI/CD pipeline (tag-triggered) |

### Packaging and Signing

| File | Purpose |
|------|---------|
| `assets/macos/nebo.entitlements` | macOS code signing entitlements |
| `src-tauri/icons/icon.icns` | macOS app icon |
| `src-tauri/icons/icon.ico` | Windows app icon |
| `src-tauri/icons/*.png` | Various resolution icons |
| `scripts/nebo.rb.tmpl` | Homebrew cask template (envsubst) |

### Updater

| File | Purpose |
|------|---------|
| `crates/updater/Cargo.toml` | Updater crate dependencies |
| `crates/updater/src/lib.rs` | Check, Download, VerifyChecksum, BackgroundChecker, DetectInstallMethod, AssetName |
| `crates/updater/src/apply.rs` | Unix apply (execve), Windows apply (rename+spawn), health check, copy_file |

### Frontend (Shared)

| File | Purpose |
|------|---------|
| `app/src/lib/stores/update.ts` | Frontend update state (Svelte writable stores) |
| `app/src/lib/components/UpdateBanner.svelte` | Update notification banner UI |

---

## Runbooks

### Ship a Rust Release

```bash
# 1. Ensure clean state
make build              # Verify headless build succeeds
make test               # Run all tests
make build-desktop      # Verify Tauri desktop build

# 2. Tag and push
git tag v0.1.0
git push origin v0.1.0

# 3. Monitor
# https://github.com/NeboLoop/nebo-rs/actions
# Pipeline: frontend -> builds (parallel) -> sign-windows -> release -> homebrew + apt
```

### Local macOS Install (dev)

```bash
make install    # build-desktop -> app-bundle (re-sign) -> dmg -> notarize -> /Applications/Nebo.app
```

### Local DMG Build

```bash
brew install create-dmg      # one-time
make notarize                # build-desktop -> app-bundle -> dmg -> notarize + staple
```

### Debug Update Issues

```bash
# Check what install method is detected:
curl http://localhost:27895/api/v1/update/check | jq .

# Expected fields: available, current_version, latest_version, install_method, can_auto_update
# install_method: "direct" -> auto-update works
# install_method: "homebrew" or "package_manager" -> shows manual instructions

# Check CDN version manifest directly:
curl -s https://cdn.neboloop.com/releases/version.json | jq .

# Check checksums on CDN:
curl -sL https://cdn.neboloop.com/releases/v0.1.0/checksums.txt

# Check GitHub API directly:
curl -s https://api.github.com/repos/NeboLoop/nebo-rs/releases/latest | jq '{tag_name, published_at}'
```

### Rollback a Bad Release

```bash
# Binary keeps .old backup after update:
# Unix: /path/to/nebo.old -> cp nebo.old nebo
# Windows: C:\...\nebo.exe.old -> rename nebo.exe.old nebo.exe

# Or pin version via package manager:
brew pin nebo                 # prevent auto-upgrade
```

### First-Time Notarization Setup (Local)

```bash
xcrun notarytool store-credentials "nebo-notarize"
# Enter: Apple ID, app-specific password (generate at appleid.apple.com), team ID
```

### First-Time CI Setup

1. Create `TAP_GITHUB_TOKEN` (fine-grained PAT with access to `neboloop/homebrew-tap` and `neboloop/apt`)
2. Enable GitHub Pages on `neboloop/apt` repository
3. Create `APT_GPG_PRIVATE_KEY` (optional, for signed APT repo)
4. Set up DigitalOcean Spaces CDN (see Section 7.1)
5. Import Apple certificate as base64 into `APPLE_CERTIFICATE_P12`
6. Create Azure service principal for Trusted Signing (see Section 4.5)

---

*Generated: 2026-03-04*
