# Nebo Release & Distribution

This document covers how Nebo is built, released, and distributed across all platforms.

---

## Architecture

Every platform ships a **desktop build** by default — a native window with system tray powered by Wails v3. Users can opt into headless (browser-only) mode with `--headless`.

| Platform | Binary | Desktop Toolkit | Installer |
|----------|--------|-----------------|-----------|
| macOS Apple Silicon | `nebo-darwin-arm64` | Cocoa (WebKit) | Homebrew |
| macOS Intel | `nebo-darwin-amd64` | Cocoa (WebKit) | Homebrew |
| Linux x86_64 | `nebo-linux-amd64` | GTK3 + WebKitGTK | APT (.deb) |
| Linux ARM64 | `nebo-linux-arm64` | GTK3 + WebKitGTK | APT (.deb) |
| Windows x86_64 | `nebo-windows-amd64.exe` | WebView2 | Direct download |

All builds use `-tags desktop` with `CGO_ENABLED=1` to include Wails v3 native window support.

---

## CI/CD Pipeline

Releases are fully automated via GitHub Actions (`.github/workflows/release.yml`).

### Trigger

Push a version tag:

```bash
git tag v0.2.0
git push origin v0.2.0
```

### Pipeline Flow

```
Tag push (v*)
    │
    ├─► frontend        Build SvelteKit app (ubuntu-latest)
    │       │
    │       ├─► build-macos     macOS arm64 (macos-latest) + amd64 (macos-13)
    │       ├─► build-linux     Linux amd64 (ubuntu-latest) + arm64 (ubuntu-24.04-arm)
    │       └─► build-windows   Windows amd64 (windows-latest)
    │               │
    │               ├─► package-deb     Build .deb packages via nfpm
    │               │
    │               └─► release         Create GitHub Release with all binaries
    │                       │
    │                       ├─► update-homebrew   Push formula to nebolabs/homebrew-tap
    │                       └─► update-apt        Push .deb to nebolabs/apt
    │
    Done
```

### Build Matrix

| Job | Runner | CGO | Build Tags | Output |
|-----|--------|-----|------------|--------|
| `build-macos` (arm64) | `macos-latest` | 1 | `desktop` | `nebo-darwin-arm64` |
| `build-macos` (amd64) | `macos-13` | 1 | `desktop` | `nebo-darwin-amd64` |
| `build-linux` (amd64) | `ubuntu-latest` | 1 | `desktop` | `nebo-linux-amd64` |
| `build-linux` (arm64) | `ubuntu-24.04-arm` | 1 | `desktop` | `nebo-linux-arm64` |
| `build-windows` | `windows-latest` | 1 | `desktop` | `nebo-windows-amd64.exe` |

### Cross-Repo Updates

After the GitHub Release is created, the pipeline automatically:

1. **Homebrew** — Computes SHA256 checksums, renders `scripts/nebo.rb.tmpl` with `envsubst`, and pushes the updated formula to `nebolabs/homebrew-tap`.

2. **APT** — Copies `.deb` packages into the pool, regenerates `Packages` and `Release` indexes, and pushes to `nebolabs/apt` (served via GitHub Pages).

---

## Distribution Channels

### Homebrew (macOS + Linux)

```bash
brew install nebolabs/tap/nebo
```

- Formula lives in [nebolabs/homebrew-tap](https://github.com/nebolabs/homebrew-tap)
- Template: `scripts/nebo.rb.tmpl` (rendered by CI with checksums)
- Reference copy: `scripts/nebo.rb`

### APT (Debian / Ubuntu)

```bash
# Add GPG key
curl -fsSL https://nebolabs.github.io/apt/key.gpg | sudo gpg --dearmor -o /usr/share/keyrings/nebo.gpg

# Add repository
echo "deb [signed-by=/usr/share/keyrings/nebo.gpg] https://nebolabs.github.io/apt stable main" \
  | sudo tee /etc/apt/sources.list.d/nebo.list

# Install
sudo apt update
sudo apt install nebo
```

- Repo lives in [nebolabs/apt](https://github.com/nebolabs/apt) with GitHub Pages enabled
- Packages built via [nfpm](https://nfpm.goreleaser.com/) using `nfpm.yaml.tmpl`
- Runtime dependencies: `libwebkit2gtk-4.1-0`, `libgtk-3-0`

### Direct Download (all platforms)

Binaries available on the [GitHub Releases](https://github.com/nebolabs/nebo/releases) page.

### Windows

Download `nebo-windows-amd64.exe` from the GitHub Release. Future: winget and/or scoop.

---

## Required Secrets

The CI pipeline needs these GitHub repository secrets configured on `nebolabs/nebo`:

### `TAP_GITHUB_TOKEN` (required)

A fine-grained Personal Access Token with write access to the cross-repo distribution channels.

**Setup:**

1. Go to https://github.com/settings/tokens?type=beta
2. Click **"Generate new token"**
3. Configure:
   - **Name:** `nebo-release-bot`
   - **Expiration:** 1 year (renew annually)
   - **Resource owner:** `nebolabs`
   - **Repository access:** Select repositories → `homebrew-tap` and `apt`
   - **Permissions → Repository permissions → Contents:** Read and write
4. Click **"Generate token"** and copy the value
5. Add to the nebo repo:
   ```bash
   cd /path/to/nebo
   gh secret set TAP_GITHUB_TOKEN
   # Paste the token when prompted
   ```

### `APT_GPG_PRIVATE_KEY` (optional, recommended)

GPG private key for signing the APT repository. Without this, packages are unsigned (users must use `[trusted=yes]` in their sources list).

**Setup:**

```bash
# Generate a GPG key
gpg --full-generate-key
# Choose: RSA and RSA, 4096 bits, 0 (no expiry)
# Real name: Nebo
# Email: support@nebolabs.dev

# Export and set as secret
gpg --export-secret-keys --armor nebo | gh secret set APT_GPG_PRIVATE_KEY

# Export public key for the apt repo
gpg --armor --export nebo > /tmp/key.gpg
# Upload to nebolabs/apt repo root
```

---

## Local Builds

### Desktop build (current platform)

```bash
make desktop
# Output: bin/nebo (with native window + system tray)
```

### Release builds (all platforms, from macOS)

```bash
make release
# Output: dist/nebo-darwin-{arm64,amd64}, dist/nebo-linux-{amd64,arm64}
```

Note: macOS can cross-compile both darwin architectures with CGO (Xcode toolchain supports arm64 ↔ amd64). Linux builds use `CGO_ENABLED=0` locally (headless fallback). Windows builds require a Windows machine or CI.

### Headless build (no Wails, no CGO)

```bash
CGO_ENABLED=0 go build -ldflags="-w -s" -o bin/nebo .
# Output: bin/nebo (headless — falls back to browser mode)
```

---

## Build Tags

| Tag | Effect |
|-----|--------|
| `desktop` | Includes Wails v3 native window + system tray (`cmd/nebo/desktop.go`) |
| (none) | Uses `cmd/nebo/desktop_stub.go` — falls back to headless `RunAll()` |

The default `nebo` command calls `RunDesktop()`. With the `desktop` tag, this launches a native window. Without it, it prints "Desktop mode not available" and falls back to headless mode (opens browser).

---

## Creating a Release

### Automated (recommended)

```bash
# Ensure everything builds
make build && cd app && pnpm build && cd ..

# Tag and push
git tag v0.2.0
git push origin v0.2.0
```

The CI pipeline handles everything: build all platforms, create GitHub Release, update Homebrew, update APT.

### Manual (for testing)

```bash
# Build
cd app && pnpm build && cd ..
make release

# Upload to existing release
gh release upload v0.2.0 dist/*

# Update Homebrew formula manually
# 1. Compute checksums: shasum -a 256 dist/*
# 2. Update scripts/nebo.rb with new version + checksums
# 3. Copy to homebrew-tap repo and push
```

---

## Repository Map

| Repo | Purpose |
|------|---------|
| [nebolabs/nebo](https://github.com/nebolabs/nebo) | Main source code + CI pipeline |
| [nebolabs/homebrew-tap](https://github.com/nebolabs/homebrew-tap) | Homebrew formula (`brew install nebolabs/tap/nebo`) |
| [nebolabs/apt](https://github.com/nebolabs/apt) | APT repository for Debian/Ubuntu (`apt install nebo`) |

---

## Troubleshooting

### `brew install` fails with "formula requires at least a URL"

The formula is missing a binary for your architecture. Check that the release has all platform binaries uploaded.

### "Desktop mode not available in this build"

The binary was built without `-tags desktop` (likely a `CGO_ENABLED=0` build). Reinstall via brew or rebuild with `make desktop`.

### Linux: "Failed to create window" or WebKit errors

Install the WebKitGTK runtime:
```bash
sudo apt install libwebkit2gtk-4.1-0 libgtk-3-0
```

### macOS linker warnings about version mismatch

Set deployment target environment variables before building:
```bash
export MACOSX_DEPLOYMENT_TARGET=13.0
export CGO_CFLAGS="-mmacosx-version-min=13.0"
export CGO_LDFLAGS="-mmacosx-version-min=13.0"
```
These are already configured in the Makefile.
