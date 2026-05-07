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

.PHONY: help dev run build build-desktop test clean seed-plugins bundle-napps plugin-status release release-darwin release-linux release-windows app-bundle dmg notarize install github-release gen

# Default target
help:
	@echo "Nebo — AI Agent Platform (Rust)"
	@echo ""
	@echo "Code Generation:"
	@echo "  make gen            - Generate TS API client from Rust routes"
	@echo ""
	@echo "Development:"
	@echo "  make dev            - Hot reload desktop (cargo tauri dev)"
	@echo "  make run            - Build + run CLI once (no file watching)"
	@echo "  make build          - Build headless CLI binary"
	@echo "  make build-desktop  - Build Tauri desktop app"
	@echo "  make test           - Run all tests"
	@echo "  make clean          - Clean build artifacts"
	@echo "  make seed-plugins   - Copy plugin binaries from sibling repos"
	@echo "  make plugin-status  - Show build/bundle status of all 14 plugins"
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

# ─── Code Generation ────────────────────────────────────────────────────────
gen:
	@echo "Generating API client from Rust routes..."
	@cd app && pnpm run gen:api

# ─── Development ─────────────────────────────────────────────────────────────

dev:
	@echo "Starting Nebo (Tauri + Vite)..."
	@echo "  Vite HMR for frontend, Tauri watch for backend"
	@echo "  Proxy errors during Rust build are normal — Tauri window waits for build."
	@echo "  NOTE: File changes trigger restart — use 'make run' to test workflows."
	@echo "  Ctrl-C to stop all processes."
	@echo ""
	@cargo tauri dev; \
		lsof -ti :5173 -ti :27895 2>/dev/null | xargs kill -9 2>/dev/null; \
		pkill -9 -f "cargo-tauri" 2>/dev/null; \
		pkill -9 -f "target/debug/nebo" 2>/dev/null; \
		pkill -9 -f "target/release/nebo" 2>/dev/null; \
		true

# Full Tauri app + Vite HMR, but NO Rust file watching — safe for workflow testing
run:
	@echo "Starting Nebo (Tauri + Vite, no Rust hot-reload)..."
	@echo "  Frontend HMR works. Rust backend won't restart on file changes."
	@echo "  Use this when testing workflows/agents."
	@echo "  Ctrl-C to stop all processes."
	@echo ""
	@cargo tauri dev --no-watch; \
		lsof -ti :5173 -ti :27895 2>/dev/null | xargs kill -9 2>/dev/null; \
		pkill -9 -f "cargo-tauri" 2>/dev/null; \
		pkill -9 -f "target/debug/nebo" 2>/dev/null; \
		pkill -9 -f "target/release/nebo" 2>/dev/null; \
		true

build:
	@echo "Building headless CLI binary..."
	cargo build --release -p nebo-cli

build-desktop: bundle-napps
	@echo "Building Tauri desktop app..."
	@cd app && pnpm build
	cargo tauri build

test:
	@echo "Running tests..."
	cargo test

clean:
	@echo "Cleaning build artifacts..."
	rm -rf target/ dist/

# Copy plugin binaries + manifests from sibling repos into the local plugin store.
# Prerequisites: build each plugin first (cargo build --release in each repo).
REPOS_DIR ?= $(shell cd .. && pwd)/repos/plugins
PLUGIN_DIR ?= $(HOME)/.nebo/nebo/plugins
PLUGINS = gws nebo-pdf nebo-office ffmpeg outreach sfdc social warm-market nuskin

seed-plugins:
	@echo "Seeding plugin binaries from $(REPOS_DIR)..."
	@for slug in $(PLUGINS); do \
		src="$(REPOS_DIR)/$$slug"; \
		if [ ! -d "$$src" ]; then \
			echo "  SKIP $$slug (repo not found)"; \
			continue; \
		fi; \
		manifest="$$src/plugin.json"; \
		if [ ! -f "$$manifest" ]; then \
			echo "  SKIP $$slug (no plugin.json)"; \
			continue; \
		fi; \
		version=$$(python3 -c "import json; print(json.load(open('$$manifest'))['version'])" 2>/dev/null || echo "0.1.0"); \
		dst="$(PLUGIN_DIR)/$$slug/$$version"; \
		binary_name=$$(python3 -c "import json; p=json.load(open('$$manifest')).get('platforms',{}).get('darwin-arm64',{}); print(p.get('binaryName','$$slug'))" 2>/dev/null || echo "$$slug"); \
		binary="$$src/target/release/$$binary_name"; \
		if [ ! -f "$$binary" ]; then \
			echo "  SKIP $$slug (binary not built at $$binary)"; \
			continue; \
		fi; \
		mkdir -p "$$dst"; \
		cp "$$manifest" "$$dst/plugin.json"; \
		cp "$$binary" "$$dst/$$binary_name"; \
		chmod +x "$$dst/$$binary_name"; \
		if [ -d "$$src/skills" ]; then \
			rm -rf "$$dst/skills"; \
			cp -R "$$src/skills" "$$dst/skills"; \
			skill_count=$$(ls "$$dst/skills" 2>/dev/null | wc -l | tr -d ' '); \
			echo "  OK   $$slug $$version → $$dst ($$skill_count skills)"; \
		else \
			echo "  OK   $$slug $$version → $$dst"; \
		fi; \
	done
	@echo "Done. Restart Nebo to pick up plugins."

# Show build/bundle status of all 14 bundled plugins.
plugin-status:
	@scripts/publish-plugins.sh status

# ─── Bundled .napp Files ─────────────────────────────────────────────────────

BUNDLED_NAPPS_DIR = src-tauri/bundled-napps

# Download signed .napp files from NeboLoop CDN into the Tauri bundle.
# Skills and agents are platform-agnostic. Plugins are per-platform.
# Override NEBOLOOP_CDN_URL if using a staging environment.
NEBOLOOP_CDN_URL ?= https://cdn.neboloop.com

bundle-napps:
	@echo "Preparing bundled .napp directory..."
	@mkdir -p $(BUNDLED_NAPPS_DIR)/{skills,agents,plugins}
	@echo "Place signed .napp files in $(BUNDLED_NAPPS_DIR)/{skills,agents,plugins}/"
	@echo "  Skills/agents: platform-agnostic (one .napp per artifact)"
	@echo "  Plugins: platform-specific (download for target arch)"
	@echo ""
	@echo "Current contents:"
	@find $(BUNDLED_NAPPS_DIR) -name "*.napp" -type f 2>/dev/null | sort || echo "  (none)"

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
	cp $(TAURI_TARGET)/aarch64-apple-darwin/release/nebo dist/nebo-darwin-arm64
	# amd64
	cargo tauri build --target x86_64-apple-darwin
	cp $(TAURI_TARGET)/x86_64-apple-darwin/release/nebo dist/nebo-darwin-amd64

# Linux: Tauri desktop + headless CLI
release-linux:
	@echo "Building for Linux..."
	@mkdir -p dist
	cargo tauri build
	cp $(TAURI_RELEASE)/nebo dist/nebo-linux-$(ARCH)
	# Headless binary
	cargo build --release -p nebo-cli
	cp target/release/nebo-cli dist/nebo-linux-$(ARCH)-headless

# Windows: Tauri desktop app
release-windows:
	@echo "Building for Windows..."
	@mkdir -p dist
	cargo tauri build
	cp $(TAURI_RELEASE)/nebo.exe dist/nebo-windows-amd64.exe

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
	gh release create $(TAG) dist/* --title "Nebo $(TAG)" --generate-notes --repo NeboLoop/nebo
