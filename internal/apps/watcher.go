package apps

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sync"
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

	// Debounce map: appDir → timer. A binary rebuild fires Create+Write in quick
	// succession — we coalesce them into a single restart after a short delay.
	var dmu sync.Mutex
	debounce := make(map[string]*time.Timer)

	debouncedRestart := func(appDir string) {
		dmu.Lock()
		defer dmu.Unlock()

		if t, ok := debounce[appDir]; ok {
			t.Stop()
		}
		debounce[appDir] = time.AfterFunc(500*time.Millisecond, func() {
			dmu.Lock()
			delete(debounce, appDir)
			dmu.Unlock()

			appID := filepath.Base(appDir)

			// Skip if a managed restart (supervisor/registry) is already handling this app.
			// Without this check, supervisor restart → binary write → watcher fires → double launch.
			if ar.runtime.IsWatcherSuppressed(appID) {
				fmt.Printf("[apps] Skipping watcher restart for %s (managed restart in progress)\n", appID)
				return
			}

			fmt.Printf("[apps] Restarting app after file change: %s\n", appID)
			ar.runtime.Stop(appID)
			if err := ar.launchAndRegister(ctx, appDir); err != nil {
				fmt.Printf("[apps] Failed to restart app %s: %v\n", appID, err)
			}
		})
	}

	for {
		select {
		case <-ctx.Done():
			// Cancel all pending debounce timers
			dmu.Lock()
			for _, t := range debounce {
				t.Stop()
			}
			dmu.Unlock()
			return nil

		case event, ok := <-watcher.Events:
			if !ok {
				return nil
			}
			ar.handleFSEvent(ctx, watcher, event, debouncedRestart)

		case err, ok := <-watcher.Errors:
			if !ok {
				return nil
			}
			fmt.Printf("[apps] Watcher error: %v\n", err)
		}
	}
}

func (ar *AppRegistry) handleFSEvent(ctx context.Context, watcher *fsnotify.Watcher, event fsnotify.Event, debouncedRestart func(string)) {
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

		// manifest.json created inside a subdirectory — new app
		if name == "manifest.json" {
			appDir := dir
			if !ar.runtime.IsRunning(filepath.Base(appDir)) {
				fmt.Printf("[apps] New app detected: %s\n", filepath.Base(appDir))
				if err := ar.launchAndRegister(ctx, appDir); err != nil {
					fmt.Printf("[apps] Failed to launch new app %s: %v\n", filepath.Base(appDir), err)
				}
			}
		}

		// Binary replaced — debounced restart (coalesces Create+Write events)
		if (name == "binary" || name == "app") && dir != ar.appsDir {
			if ar.runtime.IsRunning(filepath.Base(dir)) {
				debouncedRestart(dir)
			}
		}

	case event.Has(fsnotify.Write):
		if name == "manifest.json" {
			appDir := dir
			if ar.runtime.IsRunning(filepath.Base(appDir)) {
				debouncedRestart(appDir)
			}
		}

		// Binary recompiled in-place — debounced restart
		if (name == "binary" || name == "app") && dir != ar.appsDir {
			if ar.runtime.IsRunning(filepath.Base(dir)) {
				debouncedRestart(dir)
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
