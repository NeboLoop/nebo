# Nebo-rs Makefile
# Rust + Tauri 2 build, sign, package, and release pipeline

VERSION ?= $(shell git describe --tags --always --dirty 2>/dev/null || echo "dev")
SIGN_IDENTITY ?= Developer ID Application: Alma Tuck (7Y2D3KQ2UM)
NOTARIZE_PROFILE ?= nebo-notarize

# Tauri output directories
TAURI_TARGET = src-tauri/target
TAURI_RELEASE = $(TAURI_TARGET)/release
TAURI_BUNDLE = $(TAURI_RELEASE)/bundle

# Platform detection
UNAME_S := $(shell uname -s)
UNAME_M := $(shell uname -m)
ifeq ($(UNAME_M),x86_64)
    ARCH = amd64
else ifeq ($(UNAME_M),aarch64)
    ARCH = arm64
else
    ARCH = $(UNAME_M)
endif

.PHONY: help dev build build-desktop test clean release release-darwin release-linux release-windows app-bundle dmg notarize install github-release

# Default target
help:
	@echo "Nebo — AI Agent Platform (Rust)"
	@echo ""
	@echo "Development:"
	@echo "  make dev            - Hot reload headless server (cargo watch)"
	@echo "  make build          - Build headless CLI binary"
	@echo "  make build-desktop  - Build Tauri desktop app"
	@echo "  make test           - Run all tests"
	@echo "  make clean          - Clean build artifacts"
	@echo ""
	@echo "Desktop (macOS):"
	@echo "  make app-bundle     - Re-sign Tauri .app with Developer ID"
	@echo "  make dmg            - Create .dmg installer"
	@echo "  make notarize       - Notarize .dmg with Apple"
	@echo "  make install        - Notarize + install to /Applications"
	@echo ""
	@echo "Release:"
	@echo "  make release              - Build all platforms"
	@echo "  make release-darwin       - macOS (arm64 + amd64)"
	@echo "  make release-linux        - Linux desktop + headless"
	@echo "  make release-windows      - Windows .exe + .msi"
	@echo "  make github-release TAG=v0.1.0  - Create GitHub release"

# ─── Development ─────────────────────────────────────────────────────────────

dev:
	@echo "Starting Nebo with hot reload..."
	cargo watch -x 'run -p nebo'

build:
	@echo "Building headless CLI binary..."
	cargo build --release -p nebo

build-desktop:
	@echo "Building Tauri desktop app..."
	@cd app && pnpm build
	cargo tauri build

test:
	@echo "Running tests..."
	cargo test

clean:
	@echo "Cleaning build artifacts..."
	rm -rf target/ dist/

# ─── Release Targets ────────────────────────────────────────────────────────

release: clean release-darwin release-linux release-windows
	@echo ""
	@echo "Release binaries built in dist/"
	@ls -la dist/

# macOS: build both architectures via Tauri, extract bare binaries
release-darwin:
	@echo "Building for macOS..."
	@mkdir -p dist
	@cd app && pnpm build
	# arm64
	cargo tauri build --target aarch64-apple-darwin
	cp $(TAURI_TARGET)/aarch64-apple-darwin/release/nebo-desktop dist/nebo-darwin-arm64
	# amd64
	cargo tauri build --target x86_64-apple-darwin
	cp $(TAURI_TARGET)/x86_64-apple-darwin/release/nebo-desktop dist/nebo-darwin-amd64

# Linux: Tauri desktop + headless CLI
release-linux:
	@echo "Building for Linux..."
	@mkdir -p dist
	cargo tauri build
	cp $(TAURI_RELEASE)/nebo-desktop dist/nebo-linux-$(ARCH)
	# Headless binary
	cargo build --release -p nebo
	cp target/release/nebo dist/nebo-linux-$(ARCH)-headless

# Windows: Tauri desktop app
release-windows:
	@echo "Building for Windows..."
	@mkdir -p dist
	cargo tauri build
	cp $(TAURI_RELEASE)/nebo-desktop.exe dist/nebo-windows-amd64.exe

# ─── macOS Desktop Targets ──────────────────────────────────────────────────

# Re-sign the Tauri-produced .app bundle with our Developer ID + entitlements
app-bundle: build-desktop
	@echo "Re-signing Nebo.app with Developer ID..."
	@rm -rf dist/Nebo.app
	@mkdir -p dist
	@cp -R "$(TAURI_BUNDLE)/macos/Nebo.app" dist/Nebo.app
	codesign --force --sign "$(SIGN_IDENTITY)" \
		--identifier dev.neboloop.nebo \
		--entitlements assets/macos/nebo.entitlements \
		--options runtime \
		--deep \
		dist/Nebo.app
	@echo "Built: dist/Nebo.app (Developer ID signed)"

# Create .dmg from signed .app
dmg: app-bundle
	@echo "Creating .dmg installer..."
	@rm -f "dist/Nebo-$(VERSION)-$(UNAME_M).dmg"
	@if command -v create-dmg >/dev/null 2>&1; then \
		create-dmg \
			--volname "Nebo" \
			--volicon "src-tauri/icons/icon.icns" \
			--window-pos 200 120 \
			--window-size 600 400 \
			--icon-size 100 \
			--icon "Nebo.app" 175 190 \
			--hide-extension "Nebo.app" \
			--app-drop-link 425 190 \
			"dist/Nebo-$(VERSION)-$(UNAME_M).dmg" \
			"dist/Nebo.app"; \
	else \
		hdiutil create -volname "Nebo" -srcfolder dist/Nebo.app \
			-ov -format UDZO "dist/Nebo-$(VERSION)-$(UNAME_M).dmg"; \
	fi
	@echo "Built: dist/Nebo-$(VERSION)-$(UNAME_M).dmg"

# Notarize the .dmg with Apple
notarize: dmg
	@echo "Submitting to Apple for notarization..."
	xcrun notarytool submit "dist/Nebo-$(VERSION)-$(UNAME_M).dmg" \
		--keychain-profile "$(NOTARIZE_PROFILE)" --wait
	@echo "Stapling notarization ticket..."
	xcrun stapler staple "dist/Nebo-$(VERSION)-$(UNAME_M).dmg"
	@echo "Done! DMG is signed and notarized."

# Install to /Applications (notarized)
install: notarize
	@echo "Installing Nebo.app to /Applications..."
	@rm -rf /Applications/Nebo.app
	@cp -R dist/Nebo.app /Applications/Nebo.app
	@echo "Installed! Nebo.app is signed, notarized, and in your Applications folder."

# ─── GitHub Release ──────────────────────────────────────────────────────────

github-release:
	@if [ -z "$(TAG)" ]; then echo "Usage: make github-release TAG=v0.1.0"; exit 1; fi
	@echo "Creating GitHub release $(TAG)..."
	gh release create $(TAG) dist/* --title "Nebo $(TAG)" --generate-notes --repo NeboLoop/nebo-rs
