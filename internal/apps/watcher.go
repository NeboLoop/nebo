package apps

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"time"

	"github.com/fsnotify/fsnotify"
)

// Watch monitors the apps directory for changes (new apps, removed apps, manifest updates).
// It watches both the top-level apps/ directory and all app subdirectories so that
// new manifest.json files inside new subdirectories are detected.
// Blocks until the context is cancelled.
func (ar *AppRegistry) Watch(ctx context.Context) error {
	watcher, err := fsnotify.NewWatcher()
	if err != nil {
		return fmt.Errorf("create watcher: %w", err)
	}
	defer watcher.Close()

	// Watch the top-level apps directory for new/removed app directories
	if err := watcher.Add(ar.appsDir); err != nil {
		return fmt.Errorf("watch apps dir: %w", err)
	}

	// Watch all existing app subdirectories for manifest/binary changes
	// Use os.Stat (not entry.IsDir) to follow symlinks — sideloaded apps are symlinks
	entries, _ := os.ReadDir(ar.appsDir)
	for _, entry := range entries {
		subDir := filepath.Join(ar.appsDir, entry.Name())
		info, err := os.Stat(subDir)
		if err != nil || !info.IsDir() {
			continue
		}
		watcher.Add(subDir) // ignore error — best effort
	}

	fmt.Printf("[apps] Watching %s for changes\n", ar.appsDir)

	for {
		select {
		case <-ctx.Done():
			return nil

		case event, ok := <-watcher.Events:
			if !ok {
				return nil
			}
			ar.handleFSEvent(ctx, watcher, event)

		case err, ok := <-watcher.Errors:
			if !ok {
				return nil
			}
			fmt.Printf("[apps] Watcher error: %v\n", err)
		}
	}
}

func (ar *AppRegistry) handleFSEvent(ctx context.Context, watcher *fsnotify.Watcher, event fsnotify.Event) {
	name := filepath.Base(event.Name)
	dir := filepath.Dir(event.Name)

	switch {
	case event.Has(fsnotify.Create):
		info, err := os.Stat(event.Name)
		if err != nil {
			return
		}

		if info.IsDir() && dir == ar.appsDir {
			// New app directory created — start watching it for manifest.json
			watcher.Add(event.Name)

			// Check if the directory already has a manifest (e.g., copied as a whole)
			// Use a short delay to let all files finish writing
			go func(appDir string) {
				time.Sleep(500 * time.Millisecond)
				manifestPath := filepath.Join(appDir, "manifest.json")
				if _, err := os.Stat(manifestPath); err == nil {
					fmt.Printf("[apps] New app detected: %s\n", filepath.Base(appDir))
					if err := ar.launchAndRegister(ctx, appDir); err != nil {
						fmt.Printf("[apps] Failed to launch new app %s: %v\n", filepath.Base(appDir), err)
					}
				}
			}(event.Name)
			return
		}

		// manifest.json created inside a subdirectory
		if name == "manifest.json" {
			appDir := dir
			fmt.Printf("[apps] New app detected: %s\n", filepath.Base(appDir))
			if err := ar.launchAndRegister(ctx, appDir); err != nil {
				fmt.Printf("[apps] Failed to launch new app %s: %v\n", filepath.Base(appDir), err)
			}
		}

		// Binary replaced — restart app if already running
		if (name == "binary" || name == "app") && dir != ar.appsDir {
			appID := filepath.Base(dir)
			if _, ok := ar.runtime.Get(appID); ok {
				fmt.Printf("[apps] Binary changed, restarting: %s\n", appID)
				ar.runtime.Stop(appID)
				if err := ar.launchAndRegister(ctx, dir); err != nil {
					fmt.Printf("[apps] Failed to restart app %s: %v\n", appID, err)
				}
			}
		}

	case event.Has(fsnotify.Write):
		if name == "manifest.json" {
			appDir := dir
			appID := filepath.Base(appDir)

			if _, ok := ar.runtime.Get(appID); ok {
				fmt.Printf("[apps] Manifest changed, restarting: %s\n", appID)
				ar.runtime.Stop(appID)
				if err := ar.launchAndRegister(ctx, appDir); err != nil {
					fmt.Printf("[apps] Failed to restart app %s: %v\n", appID, err)
				}
			}
		}

		// Binary recompiled in-place — restart
		if (name == "binary" || name == "app") && dir != ar.appsDir {
			appID := filepath.Base(dir)
			if _, ok := ar.runtime.Get(appID); ok {
				fmt.Printf("[apps] Binary changed, restarting: %s\n", appID)
				ar.runtime.Stop(appID)
				if err := ar.launchAndRegister(ctx, dir); err != nil {
					fmt.Printf("[apps] Failed to restart app %s: %v\n", appID, err)
				}
			}
		}

	case event.Has(fsnotify.Remove):
		if dir == ar.appsDir {
			// Top-level entry removed — stop app if running
			appID := name
			if _, ok := ar.runtime.Get(appID); ok {
				fmt.Printf("[apps] App removed, stopping: %s\n", appID)
				if err := ar.runtime.Stop(appID); err != nil {
					fmt.Printf("[apps] Failed to stop removed app %s: %v\n", appID, err)
				}
			}
			watcher.Remove(event.Name)
		}
	}
}
