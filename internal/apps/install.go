package apps

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"

	"github.com/neboloop/nebo/internal/neboloop/sdk"
)

// HandleInstallEvent processes an install event from the NeboLoop comms SDK.
// Routes by event type to install, update, uninstall, or revoke handlers.
func (ar *AppRegistry) HandleInstallEvent(ctx context.Context, evt sdk.InstallEvent) {
	if evt.AppID == "" {
		fmt.Printf("[apps:install] Event missing app_id, ignoring\n")
		return
	}

	fmt.Printf("[apps:install] Event: %s app=%s version=%s\n", evt.Type, evt.AppID, evt.Version)

	switch evt.Type {
	case "installed":
		ar.handleInstall(ctx, evt)
	case "updated":
		ar.handleUpdate(ctx, evt)
	case "uninstalled":
		ar.handleUninstall(evt)
	case "revoked":
		ar.handleRevoke(evt)
	default:
		fmt.Printf("[apps:install] Unknown event type: %s\n", evt.Type)
	}
}

// handleInstall downloads and installs a new app.
func (ar *AppRegistry) handleInstall(ctx context.Context, evt sdk.InstallEvent) {
	downloadURL := evt.DownloadURL
	if downloadURL == "" {
		fmt.Printf("[apps:install] No download URL for %s\n", evt.AppID)
		return
	}

	appDir := filepath.Join(ar.appsDir, evt.AppID)

	// Check if already installed
	if _, err := os.Stat(filepath.Join(appDir, "manifest.json")); err == nil {
		fmt.Printf("[apps:install] App %s already installed, skipping\n", evt.AppID)
		return
	}

	if err := DownloadAndExtractNapp(downloadURL, appDir); err != nil {
		fmt.Printf("[apps:install] Failed to install %s: %v\n", evt.AppID, err)
		os.RemoveAll(appDir)
		return
	}

	if err := ar.launchAndRegister(ctx, appDir); err != nil {
		fmt.Printf("[apps:install] Failed to launch %s: %v\n", evt.AppID, err)
		return
	}

	fmt.Printf("[apps:install] Installed and launched %s v%s\n", evt.AppID, evt.Version)
}

// handleUpdate stops the running app, replaces the binary/manifest (preserving data), and relaunches.
// If the new version adds permissions, the update is staged but not launched until user approves.
func (ar *AppRegistry) handleUpdate(ctx context.Context, evt sdk.InstallEvent) {
	downloadURL := evt.DownloadURL
	if downloadURL == "" {
		fmt.Printf("[apps:install] No download URL for %s\n", evt.AppID)
		return
	}

	appDir := filepath.Join(ar.appsDir, evt.AppID)

	// Load the old manifest before stopping (for permission diff)
	var oldPermissions []string
	if oldManifest, err := LoadManifest(appDir); err == nil {
		oldPermissions = oldManifest.Permissions
	}

	// Stop the running app if it exists
	if _, ok := ar.runtime.Get(evt.AppID); ok {
		if err := ar.runtime.Stop(evt.AppID); err != nil {
			fmt.Printf("[apps:install] Warning: failed to stop %s for update: %v\n", evt.AppID, err)
		}
	}

	// Preserve the data directory across updates
	dataDir := filepath.Join(appDir, "data")
	logsDir := filepath.Join(appDir, "logs")
	hasData := dirExists(dataDir)
	hasLogs := dirExists(logsDir)

	// Extract to a temp directory first, then swap
	tmpDir := appDir + ".updating"
	os.RemoveAll(tmpDir)

	if err := DownloadAndExtractNapp(downloadURL, tmpDir); err != nil {
		fmt.Printf("[apps:install] Failed to download update for %s: %v\n", evt.AppID, err)
		os.RemoveAll(tmpDir)
		if _, err := os.Stat(filepath.Join(appDir, "manifest.json")); err == nil {
			_ = ar.launchAndRegister(ctx, appDir)
		}
		return
	}

	// Permission diff: check if new version adds permissions
	newManifest, err := LoadManifest(tmpDir)
	if err != nil {
		fmt.Printf("[apps:install] Failed to load new manifest for %s: %v\n", evt.AppID, err)
		os.RemoveAll(tmpDir)
		_ = ar.launchAndRegister(ctx, appDir)
		return
	}

	added := permissionDiff(oldPermissions, newManifest.Permissions)
	if len(added) > 0 {
		fmt.Printf("[apps:install] Update for %s adds new permissions: %v — requires user approval\n", evt.AppID, added)
		pendingDir := appDir + ".pending"
		os.RemoveAll(pendingDir)
		os.Rename(tmpDir, pendingDir)
		if _, err := os.Stat(filepath.Join(appDir, "manifest.json")); err == nil {
			_ = ar.launchAndRegister(ctx, appDir)
		}
		return
	}

	// No new permissions — safe to auto-update
	if hasData {
		os.RemoveAll(filepath.Join(tmpDir, "data"))
		os.Rename(dataDir, filepath.Join(tmpDir, "data"))
	}
	if hasLogs {
		os.RemoveAll(filepath.Join(tmpDir, "logs"))
		os.Rename(logsDir, filepath.Join(tmpDir, "logs"))
	}

	os.RemoveAll(appDir)
	if err := os.Rename(tmpDir, appDir); err != nil {
		fmt.Printf("[apps:install] Failed to swap directories for %s: %v\n", evt.AppID, err)
		os.RemoveAll(tmpDir)
		return
	}

	if err := ar.launchAndRegister(ctx, appDir); err != nil {
		fmt.Printf("[apps:install] Failed to relaunch %s after update: %v\n", evt.AppID, err)
		return
	}

	fmt.Printf("[apps:install] Updated and relaunched %s v%s\n", evt.AppID, evt.Version)
}

// permissionDiff returns permissions present in newPerms but not in oldPerms.
func permissionDiff(oldPerms, newPerms []string) []string {
	old := make(map[string]bool, len(oldPerms))
	for _, p := range oldPerms {
		old[p] = true
	}
	var added []string
	for _, p := range newPerms {
		if !old[p] {
			added = append(added, p)
		}
	}
	return added
}

// handleUninstall stops and removes an app.
func (ar *AppRegistry) handleUninstall(evt sdk.InstallEvent) {
	if _, ok := ar.runtime.Get(evt.AppID); ok {
		if err := ar.runtime.Stop(evt.AppID); err != nil {
			fmt.Printf("[apps:install] Warning: failed to stop %s: %v\n", evt.AppID, err)
		}
	}

	appDir := filepath.Join(ar.appsDir, evt.AppID)
	if err := os.RemoveAll(appDir); err != nil {
		fmt.Printf("[apps:install] Warning: failed to remove %s: %v\n", evt.AppID, err)
	}

	fmt.Printf("[apps:install] Uninstalled %s\n", evt.AppID)
}

// handleRevoke quarantines a revoked app — stops it immediately but preserves
// data/ for forensic analysis.
func (ar *AppRegistry) handleRevoke(evt sdk.InstallEvent) {
	if err := ar.Quarantine(evt.AppID); err != nil {
		fmt.Printf("[apps:install] Warning: quarantine failed for %s: %v\n", evt.AppID, err)
	}
}

// DownloadAndExtractNapp downloads a .napp from the URL and extracts it to destDir.
func DownloadAndExtractNapp(downloadURL, destDir string) error {
	resp, err := http.Get(downloadURL)
	if err != nil {
		return fmt.Errorf("download: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("download returned HTTP %d", resp.StatusCode)
	}

	maxDownloadSize := int64(600 * 1024 * 1024)

	tmpFile, err := os.CreateTemp("", "nebo-app-*.napp")
	if err != nil {
		return fmt.Errorf("create temp file: %w", err)
	}
	tmpPath := tmpFile.Name()
	defer os.Remove(tmpPath)

	written, err := io.Copy(tmpFile, io.LimitReader(resp.Body, maxDownloadSize+1))
	tmpFile.Close()
	if err != nil {
		return fmt.Errorf("download write: %w", err)
	}
	if written > maxDownloadSize {
		return fmt.Errorf("download too large (%d bytes, max %d)", written, maxDownloadSize)
	}

	if err := os.MkdirAll(destDir, 0700); err != nil {
		return fmt.Errorf("create app dir: %w", err)
	}

	if err := ExtractNapp(tmpPath, destDir); err != nil {
		return fmt.Errorf("extract: %w", err)
	}

	return nil
}

// dirExists returns true if the path exists and is a directory.
func dirExists(path string) bool {
	info, err := os.Stat(path)
	return err == nil && info.IsDir()
}

