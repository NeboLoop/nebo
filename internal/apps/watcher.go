package apps

import (
	"context"
	"fmt"
	"os"
	"path/filepath"

	"github.com/fsnotify/fsnotify"
)

// Watch monitors the apps directory for changes (new apps, removed apps, manifest updates).
// It blocks until the context is cancelled.
func (ar *AppRegistry) Watch(ctx context.Context) error {
	watcher, err := fsnotify.NewWatcher()
	if err != nil {
		return fmt.Errorf("create watcher: %w", err)
	}
	defer watcher.Close()

	if err := watcher.Add(ar.appsDir); err != nil {
		return fmt.Errorf("watch apps dir: %w", err)
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
			ar.handleFSEvent(ctx, event)

		case err, ok := <-watcher.Errors:
			if !ok {
				return nil
			}
			fmt.Printf("[apps] Watcher error: %v\n", err)
		}
	}
}

func (ar *AppRegistry) handleFSEvent(ctx context.Context, event fsnotify.Event) {
	// We care about directories being created or manifest.json being written
	name := filepath.Base(event.Name)

	switch {
	case event.Has(fsnotify.Create):
		// New directory or file created — check if it's an app directory
		info, err := os.Stat(event.Name)
		if err != nil {
			return
		}
		if info.IsDir() {
			// New app directory — wait for manifest before launching
			// The manifest write event will trigger the actual launch
			return
		}
		if name == "manifest.json" {
			appDir := filepath.Dir(event.Name)
			fmt.Printf("[apps] New app detected: %s\n", filepath.Base(appDir))
			if err := ar.launchAndRegister(ctx, appDir); err != nil {
				fmt.Printf("[apps] Failed to launch new app %s: %v\n", filepath.Base(appDir), err)
			}
		}

	case event.Has(fsnotify.Write):
		if name == "manifest.json" {
			appDir := filepath.Dir(event.Name)
			appID := filepath.Base(appDir)

			// Check if app is already running — if so, restart it
			if _, ok := ar.runtime.Get(appID); ok {
				fmt.Printf("[apps] Manifest changed, restarting: %s\n", appID)
				ar.runtime.Stop(appID)
				if err := ar.launchAndRegister(ctx, appDir); err != nil {
					fmt.Printf("[apps] Failed to restart app %s: %v\n", appID, err)
				}
			}
		}

	case event.Has(fsnotify.Remove):
		// Directory removed — stop the app if running
		appID := name
		if _, ok := ar.runtime.Get(appID); ok {
			fmt.Printf("[apps] App removed, stopping: %s\n", appID)
			if err := ar.runtime.Stop(appID); err != nil {
				fmt.Printf("[apps] Failed to stop removed app %s: %v\n", appID, err)
			}
		}
	}
}
