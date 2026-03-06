# Nebo Release & Distribution

This document covers how Nebo (Rust) is built, released, and distributed across all platforms.

---

## Architecture

Every platform ships a **desktop build** by default — a native window with system tray powered by Tauri 2. Users can opt into headless (browser-only) mode with `--headless`.

| Platform | Artifact | Desktop Toolkit | Installer |
|----------|----------|-----------------|-----------|
| macOS Apple Silicon | `Nebo-darwin-arm64.dmg` (.app) | WebKit (Tauri) | Homebrew Cask |
| macOS Intel | `Nebo-darwin-amd64.dmg` (.app) | WebKit (Tauri) | Homebrew Cask |
| Linux x86_64 | `nebo-linux-amd64.deb` | WebKitGTK (Tauri) | APT (.deb) |
| Linux ARM64 | `nebo-linux-arm64.deb` | WebKitGTK (Tauri) | APT (.deb) |
| Windows x86_64 | `Nebo-setup.exe` | WebView2 (Tauri) | Direct download / NSIS |

Desktop builds use Tauri 2 with platform-native webviews. Headless builds embed the SPA via `rust-embed` and serve it on `localhost:27895`.

---

## CI/CD Pipeline

Releases are fully automated via GitHub Actions.

### Trigger

Push a version tag:

```bash
git tag v0.2.0
git push origin v0.2.0
```

### Pipeline Flow

```
Tag push (v*)
    |
    +-> frontend        Build SvelteKit app (ubuntu-latest)
    |       |
    |       +-> build-macos     macOS arm64 + amd64 (macos-latest)
    |       +-> build-linux     Linux amd64 (ubuntu-latest) + arm64 (ubuntu-24.04-arm)
    |       +-> build-windows   Windows amd64 (windows-latest)
    |               |
    |               +-> release         Create GitHub Release with all artifacts
    |                       |
    |                       +-> update-homebrew   Push cask to neboloop/homebrew-tap
    |                       +-> update-apt        Push .deb to neboloop/apt
    |
    Done
```

### Build Matrix

| Job | Runner | Build Command | Output |
|-----|--------|--------------|--------|
| `build-macos` (arm64) | `macos-latest` | `cargo tauri build --target aarch64-apple-darwin` | `.dmg` |
| `build-macos` (amd64) | `macos-latest` | `cargo tauri build --target x86_64-apple-darwin` | `.dmg` |
| `build-linux` (amd64) | `ubuntu-latest` | `cargo tauri build` | `.deb`, `.AppImage` |
| `build-linux` (arm64) | `ubuntu-24.04-arm` | `cargo tauri build` | `.deb`, `.AppImage` |
| `build-windows` | `windows-latest` | `cargo tauri build` | `.exe`, `.msi` |

---

## Distribution Channels

### Homebrew (macOS)

```bash
brew install --cask neboloop/tap/nebo
```

- Installs `Nebo.app` to `/Applications` (Spotlight-indexable, proper icon)
- Cask lives in [neboloop/homebrew-tap](https://github.com/neboloop/homebrew-tap)

### APT (Debian / Ubuntu)

```bash
# Add GPG key
curl -fsSL https://neboloop.github.io/apt/key.gpg | sudo gpg --dearmor -o /usr/share/keyrings/nebo.gpg

# Add repository
echo "deb [signed-by=/usr/share/keyrings/nebo.gpg] https://neboloop.github.io/apt stable main" \
  | sudo tee /etc/apt/sources.list.d/nebo.list

# Install
sudo apt update
sudo apt install nebo
```

- Runtime dependencies: `libwebkit2gtk-4.1-0`, `libgtk-3-0`

### Direct Download (all platforms)

Binaries available on the [GitHub Releases](https://github.com/AltMagick/nebo/releases) page.

---

## Required Secrets

The CI pipeline needs these GitHub repository secrets:

### `TAP_GITHUB_TOKEN` (required)

A fine-grained Personal Access Token with write access to the cross-repo distribution channels.

1. Go to https://github.com/settings/tokens?type=beta
2. **Name:** `nebo-release-bot`
3. **Resource owner:** `neboloop`
4. **Repository access:** Select `homebrew-tap` and `apt`
5. **Permissions > Repository permissions > Contents:** Read and write
6. Add to the nebo repo: `gh secret set TAP_GITHUB_TOKEN`

### `APT_GPG_PRIVATE_KEY` (optional, recommended)

GPG private key for signing the APT repository.

```bash
gpg --full-generate-key
# RSA 4096, no expiry, Real name: Nebo, Email: support@neboloop.dev
gpg --export-secret-keys --armor nebo | gh secret set APT_GPG_PRIVATE_KEY
```

---

## Local Builds

### Development

```bash
# Backend + frontend hot reload (desktop)
cd src-tauri && cargo tauri dev

# Backend only (headless)
cargo run -p nebo-server

# Frontend only
cd ../../app && pnpm dev
```

### Release builds

```bash
# Desktop app (current platform)
cargo tauri build

# Headless server binary
cargo build --release -p nebo-server

# With embedded llama.cpp inference
cargo build --release --features local-inference
```

### Feature Flags

| Feature | Effect |
|---------|--------|
| `local-inference` | Compiles llama.cpp FFI for embedded GGUF model inference. Requires llama.cpp C library at build time. Without this flag, `LocalProvider::stream()` returns an error directing users to Ollama. |

---

## Creating a Release

### Automated (recommended)

```bash
# Ensure everything builds and tests pass
cargo build --release
cargo test

# Frontend
cd ../../app && pnpm build && cd ../nebo-rs

# Tag and push
git tag v0.2.0
git push origin v0.2.0
```

### Manual (for testing)

```bash
cd ../../app && pnpm build && cd ../nebo-rs
cargo tauri build

# Upload to existing release
gh release upload v0.2.0 src-tauri/target/release/bundle/*
```

---

## Repository Map

| Repo | Purpose |
|------|---------|
| [AltMagick/nebo](https://github.com/AltMagick/nebo) | Main source code (Rust) + CI pipeline |
| [neboloop/homebrew-tap](https://github.com/neboloop/homebrew-tap) | Homebrew cask (`brew install neboloop/tap/nebo`) |
| [neboloop/apt](https://github.com/neboloop/apt) | APT repository for Debian/Ubuntu |

---

## Troubleshooting

### `cargo tauri build` fails with WebKit errors (Linux)

Install development dependencies:
```bash
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libappindicator3-dev librsvg2-dev
```

### macOS linker warnings about version mismatch

Set deployment target:
```bash
export MACOSX_DEPLOYMENT_TARGET=13.0
```

### `local-inference` build fails

The `local-inference` feature requires llama.cpp C library headers and build tools (CMake, C compiler). Install them first:

```bash
# macOS
brew install cmake

# Ubuntu
sudo apt install cmake build-essential
```
