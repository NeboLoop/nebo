# Check if the environment file exists
ENVFILE := .env
ifneq ("$(wildcard $(ENVFILE))","")
	include $(ENVFILE)
	export $(shell sed 's/=.*//' $(ENVFILE))
endif

# Nebo Makefile
EXECUTABLE=nebo
export MACOSX_DEPLOYMENT_TARGET ?= 15.0
export CGO_CFLAGS += -mmacosx-version-min=15.0
export CGO_LDFLAGS += -mmacosx-version-min=15.0

# macOS code signing — override with your own Developer ID for forks
SIGN_IDENTITY ?= Developer ID Application: Alma Tuck (7Y2D3KQ2UM)
NOTARIZE_PROFILE ?= nebo-notarize

.PHONY: help dev build build-cli run clean test deps gen setup sqlc migrate-status migrate-up migrate-down cli release release-darwin release-linux install desktop package dmg notarize installer

# Default target
help:
	@echo "Nebo - AI Agent Platform"
	@echo ""
	@echo "Quick Start:"
	@echo "  make dev       - Start everything (backend + frontend)"
	@echo "  make build     - Build unified binary (server + agent)"
	@echo ""
	@echo "Development:"
	@echo "  make air       - Backend only with hot reload"
	@echo "  make test      - Run tests"
	@echo "  make gen       - Regenerate API code"
	@echo ""
	@echo "Nebo Commands (after build):"
	@echo "  nebo          - Start server + agent together (default)"
	@echo "  nebo serve    - Start server only"
	@echo "  nebo agent    - Start agent only"
	@echo "  nebo chat     - CLI chat mode"
	@echo "  nebo config   - Show configuration"
	@echo ""
	@echo "Desktop:"
	@echo "  make desktop   - Build desktop app (native window + tray)"
	@echo "  make install   - Build and install Nebo.app to /Applications"
	@echo "  make dmg       - Create macOS .dmg installer"
	@echo "  make notarize  - Sign, notarize, and staple .dmg for Gatekeeper"
	@echo "  make installer - Create Windows NSIS installer"
	@echo ""
	@echo "Installation:"
	@echo "  make cli       - Install nebo binary to PATH (for terminal use)"
	@echo ""
	@echo "Database:"
	@echo "  make migrate-up     - Run pending migrations"
	@echo "  make migrate-down   - Rollback last migration"
	@echo "  make migrate-status - Check migration status"
	@echo ""
	@echo "Other:"
	@echo "  make deps      - Download dependencies"
	@echo "  make clean     - Clean build artifacts"

# Start everything - the easiest way to develop
dev:
	@echo "Starting Nebo development environment..."
	@echo "Backend: http://localhost:27895"
	@echo "Frontend: http://localhost:27458"
	@echo ""
	@if command -v docker >/dev/null 2>&1 && (docker compose version >/dev/null 2>&1 || docker-compose version >/dev/null 2>&1); then \
		echo "Using Docker..."; \
		docker compose up; \
	else \
		echo "Starting without Docker (use two terminals for better experience)..."; \
		echo ""; \
		$(MAKE) air & \
		sleep 2; \
		cd app && pnpm dev; \
	fi

# Build the unified CLI (server + agent in one binary)
build:
	@echo "Building $(EXECUTABLE)..."
	@cd app && pnpm build
	CGO_ENABLED=0 go build $(LDFLAGS) -o bin/$(EXECUTABLE) .

# Build CLI only (for backward compatibility, same as build)
build-cli: build

# Install nebo globally
cli: build
	@echo "Installing nebo..."
	cp bin/nebo $(GOPATH)/bin/nebo 2>/dev/null || cp bin/nebo /usr/local/bin/nebo 2>/dev/null || echo "Copy bin/nebo to your PATH manually"
	@echo "Done! Run 'nebo --help' to get started"

# Run the application
run: build
	@echo "Starting $(EXECUTABLE)..."
	./bin/$(EXECUTABLE)

# Run with air (hot reload, desktop mode with dev routes)
air:
	@echo "Starting with hot reload (desktop)..."
	air

# Desktop dev mode with hot reload (Air)
# Rebuilds Go binary + restarts desktop app on *.go changes
dev-desktop:
	@echo "Starting desktop dev mode with hot reload..."
	air -c .air-desktop.toml

# Clean build artifacts
clean:
	@echo "Cleaning build artifacts..."
	rm -rf bin/
	rm -rf tmp/

# Run tests
test:
	@echo "Running tests..."
	go test ./...

# Download dependencies
deps:
	@echo "Downloading dependencies..."
	go mod download
	go mod tidy

# Code generation - TypeScript API types and client
gen:
	@echo "Generating TypeScript API code..."
	go run ./cmd/genapi
	@echo "Code generation complete!"

# Database migrations and code generation
sqlc:
	@echo "Generating sqlc code..."
	sqlc generate
	@echo "sqlc code generation complete!"

migrate-status:
	@echo "Checking migration status..."
	go run cmd/migrate/main.go status

migrate-up:
	@echo "Running migrations..."
	go run cmd/migrate/main.go up

migrate-down:
	@echo "Rolling back last migration..."
	go run cmd/migrate/main.go down

# Docker commands
docker-build:
	@echo "Building Docker image..."
	docker build -t $(EXECUTABLE) .

docker-run:
	@echo "Running Docker container..."
	docker run -p 27895:27895 --env-file .env $(EXECUTABLE)

# Development environment
dev-setup: deps
	@echo "Setting up development environment..."
	@mkdir -p bin
	@cd app && pnpm install
	@echo "Development setup complete!"
	@echo "Run 'make gen' to generate API code"
	@echo "Run 'make run' to start the backend"
	@echo "Run 'cd app && pnpm dev' to start the frontend"

# =============================================================================
# RELEASE TARGETS
# =============================================================================

VERSION ?= $(shell git describe --tags --always --dirty 2>/dev/null || echo "dev")
LDFLAGS = -ldflags "-s -w -X main.Version=$(VERSION)"

# Build release binaries for all platforms
release: clean release-darwin release-linux
	@echo ""
	@echo "Release binaries built in dist/"
	@ls -la dist/

# macOS builds (desktop — native window + system tray via Wails v3)
# Requires CGO for Wails; must be built natively on each architecture.
# For CI: use GitHub Actions matrix with macos-latest (arm64) and macos-13 (amd64).
release-darwin:
	@echo "Building for macOS (desktop)..."
	@mkdir -p dist
	@cd app && pnpm build
	CGO_ENABLED=1 GOOS=darwin GOARCH=amd64 go build -tags desktop $(LDFLAGS) -o dist/nebo-darwin-amd64 .
	CGO_ENABLED=1 GOOS=darwin GOARCH=arm64 go build -tags desktop $(LDFLAGS) -o dist/nebo-darwin-arm64 .

# Linux builds (headless — no Wails/CGO needed)
release-linux:
	@echo "Building for Linux..."
	@mkdir -p dist
	CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build $(LDFLAGS) -o dist/nebo-linux-amd64 .
	CGO_ENABLED=0 GOOS=linux GOARCH=arm64 go build $(LDFLAGS) -o dist/nebo-linux-arm64 .

# =============================================================================
# DESKTOP TARGETS (Wails v3)
# =============================================================================

# Build desktop app (native window + system tray via Wails v3)
# Requires CGO for Wails — only builds for the current platform.
desktop:
	@echo "Building $(EXECUTABLE) (desktop)..."
	@cd app && pnpm build
	CGO_ENABLED=1 go build -tags desktop $(LDFLAGS) -o bin/$(EXECUTABLE) .

# Assemble Nebo.app bundle from the built binary
# Creates a proper macOS .app that Spotlight can index
app-bundle: desktop
	@echo "Assembling Nebo.app bundle..."
	@rm -rf dist/Nebo.app
	@mkdir -p dist/Nebo.app/Contents/MacOS
	@mkdir -p dist/Nebo.app/Contents/Resources
	@cp bin/nebo dist/Nebo.app/Contents/MacOS/nebo
	@sed "s/__VERSION__/$(VERSION)/g" assets/macos/Info.plist > dist/Nebo.app/Contents/Resources/Info.plist
	@cp assets/macos/Info.plist dist/Nebo.app/Contents/Info.plist
	@sed -i '' "s/__VERSION__/$$(echo $(VERSION) | sed 's/^v//')/g" dist/Nebo.app/Contents/Info.plist
	@cp assets/icons/nebo.icns dist/Nebo.app/Contents/Resources/nebo.icns
	@echo "Signing Nebo.app with Developer ID..."
	@codesign --force --sign "$(SIGN_IDENTITY)" \
		--identifier dev.nebolabs.nebo \
		--entitlements assets/macos/nebo.entitlements \
		--options runtime \
		dist/Nebo.app/Contents/MacOS/nebo
	@codesign --force --sign "$(SIGN_IDENTITY)" \
		--identifier dev.nebolabs.nebo \
		--entitlements assets/macos/nebo.entitlements \
		--options runtime \
		dist/Nebo.app
	@echo "Built: dist/Nebo.app (Developer ID signed)"

# Install Nebo.app to /Applications (signed + notarized)
install: app-bundle
	@echo "Notarizing Nebo.app..."
	@cd dist && zip -qr Nebo.zip Nebo.app
	xcrun notarytool submit dist/Nebo.zip \
		--keychain-profile "$(NOTARIZE_PROFILE)" --wait
	xcrun stapler staple dist/Nebo.app
	@rm dist/Nebo.zip
	@echo "Installing Nebo.app to /Applications..."
	@rm -rf /Applications/Nebo.app
	@cp -R dist/Nebo.app /Applications/Nebo.app
	@echo "Installed! Nebo.app is signed, notarized, and in your Applications folder."

# Create macOS .dmg installer from the app bundle
# Requires: brew install create-dmg (or falls back to hdiutil)
dmg: app-bundle
	@echo "Creating .dmg installer..."
	@./scripts/create-dmg.sh $(VERSION) $(shell uname -m)

# Notarize the DMG with Apple (requires stored keychain credentials)
# First-time setup: xcrun notarytool store-credentials "nebo-notarize"
notarize: dmg
	@echo "Submitting to Apple for notarization..."
	xcrun notarytool submit "dist/Nebo-$$(echo $(VERSION) | sed 's/^v//')-$(shell uname -m).dmg" \
		--keychain-profile "$(NOTARIZE_PROFILE)" --wait
	@echo "Stapling notarization ticket..."
	xcrun stapler staple "dist/Nebo-$$(echo $(VERSION) | sed 's/^v//')-$(shell uname -m).dmg"
	@echo "Done! DMG is signed and notarized."

# Create Windows NSIS installer (requires NSIS: choco install nsis)
# Run on Windows or cross-compile environment with makensis available
installer:
	@echo "Creating Windows installer..."
	@mkdir -p dist
	makensis /DVERSION=$(subst v,,$(VERSION)) /DEXE_PATH=dist/nebo-windows-amd64.exe scripts/installer.nsi

# Create GitHub release (requires gh CLI)
github-release: release
	@if [ -z "$(TAG)" ]; then echo "Usage: make github-release TAG=v1.0.0"; exit 1; fi
	@echo "Creating GitHub release $(TAG)..."
	gh release create $(TAG) dist/* --title "Nebo $(TAG)" --generate-notes
