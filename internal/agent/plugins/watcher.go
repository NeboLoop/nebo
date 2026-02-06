package plugins

import (
	"context"
	"log"
	"path/filepath"
	"strings"

	"github.com/fsnotify/fsnotify"
)

// Watcher monitors the plugin directory for changes and hot-reloads plugins
type Watcher struct {
	dir       string
	loader    *Loader
	watcher   *fsnotify.Watcher
	cancelCtx context.CancelFunc
}

// NewWatcher creates a new plugin watcher
func NewWatcher(dir string, loader *Loader) (*Watcher, error) {
	fsWatcher, err := fsnotify.NewWatcher()
	if err != nil {
		return nil, err
	}

	return &Watcher{
		dir:     dir,
		loader:  loader,
		watcher: fsWatcher,
	}, nil
}

// Watch starts watching for plugin changes
func (w *Watcher) Watch(ctx context.Context) error {
	ctx, cancel := context.WithCancel(ctx)
	w.cancelCtx = cancel

	// Add directories to watch
	dirs := []string{
		filepath.Join(w.dir, "tools"),
		filepath.Join(w.dir, "channels"),
		filepath.Join(w.dir, "comm"),
	}

	for _, dir := range dirs {
		if err := w.watcher.Add(dir); err != nil {
			// Directory might not exist, that's okay
			log.Printf("[plugins] Warning: could not watch %s: %v", dir, err)
		}
	}

	// Start watch loop
	go w.watchLoop(ctx)

	log.Printf("[plugins] Watching for plugin changes in %s", w.dir)
	return nil
}

// watchLoop handles file system events
func (w *Watcher) watchLoop(ctx context.Context) {
	for {
		select {
		case <-ctx.Done():
			return
		case event, ok := <-w.watcher.Events:
			if !ok {
				return
			}
			w.handleEvent(event)
		case err, ok := <-w.watcher.Errors:
			if !ok {
				return
			}
			log.Printf("[plugins] Watch error: %v", err)
		}
	}
}

// handleEvent processes file system events
func (w *Watcher) handleEvent(event fsnotify.Event) {
	// Skip non-executable events (like .swp files)
	if strings.HasSuffix(event.Name, "~") || strings.HasPrefix(filepath.Base(event.Name), ".") {
		return
	}

	log.Printf("[plugins] File event: %s %s", event.Op, event.Name)

	switch {
	case event.Op&fsnotify.Create == fsnotify.Create:
		// New plugin added - load it
		if err := w.loader.Load(event.Name); err != nil {
			log.Printf("[plugins] Failed to load new plugin %s: %v", event.Name, err)
		}

	case event.Op&fsnotify.Write == fsnotify.Write:
		// Plugin modified - reload it
		// First unload if already loaded
		_ = w.loader.Unload(event.Name)
		if err := w.loader.Load(event.Name); err != nil {
			log.Printf("[plugins] Failed to reload plugin %s: %v", event.Name, err)
		}

	case event.Op&fsnotify.Remove == fsnotify.Remove,
		event.Op&fsnotify.Rename == fsnotify.Rename:
		// Plugin removed - unload it
		if err := w.loader.Unload(event.Name); err != nil {
			log.Printf("[plugins] Failed to unload plugin %s: %v", event.Name, err)
		}
	}
}

// Stop stops watching for changes
func (w *Watcher) Stop() {
	if w.cancelCtx != nil {
		w.cancelCtx()
	}
	if w.watcher != nil {
		w.watcher.Close()
	}
}
