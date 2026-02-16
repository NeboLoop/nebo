package advisors

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"

	"github.com/fsnotify/fsnotify"
	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/logging"
)

// Loader manages loading and hot-reloading of advisor definitions
type Loader struct {
	mu        sync.RWMutex
	advisors  map[string]*Advisor // name -> advisor
	dir       string
	watcher   *fsnotify.Watcher
	onChange  func([]*Advisor) // callback when advisors change
	cancelCtx context.CancelFunc
}

// NewLoader creates a new advisor loader for the given directory
func NewLoader(dir string) *Loader {
	return &Loader{
		advisors: make(map[string]*Advisor),
		dir:      dir,
	}
}

// LoadAll loads all advisor files from the configured directory.
// Advisors are expected to be in subdirectories with an ADVISOR.md file:
//
//	advisors/
//	├── skeptic/
//	│   └── ADVISOR.md
//	├── pragmatist/
//	│   └── ADVISOR.md
//	└── historian/
//	    └── ADVISOR.md
func (l *Loader) LoadAll() error {
	l.mu.Lock()
	defer l.mu.Unlock()

	// Clear existing advisors
	l.advisors = make(map[string]*Advisor)

	// Ensure directory exists
	if _, err := os.Stat(l.dir); os.IsNotExist(err) {
		// Directory doesn't exist, that's okay - no advisors loaded
		return nil
	}

	// Walk directory looking for ADVISOR.md files
	err := filepath.Walk(l.dir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		if info.IsDir() {
			return nil
		}

		// Only load ADVISOR.md files (case-insensitive check)
		if !strings.EqualFold(filepath.Base(path), AdvisorFileName) {
			return nil
		}

		return l.loadFile(path)
	})

	if err != nil {
		return fmt.Errorf("failed to load advisors: %w", err)
	}

	logging.Infof("[advisors] Loaded %d advisors from %s", len(l.advisors), l.dir)
	return nil
}

// loadFile loads a single ADVISOR.md file (must hold lock)
func (l *Loader) loadFile(path string) error {
	data, err := os.ReadFile(path)
	if err != nil {
		return fmt.Errorf("failed to read %s: %w", path, err)
	}

	advisor, err := ParseAdvisorMD(data)
	if err != nil {
		return fmt.Errorf("failed to parse %s: %w", path, err)
	}

	// Set defaults
	if advisor.Role == "" {
		advisor.Role = "general"
	}
	advisor.Enabled = true // Default to enabled
	advisor.FilePath = path

	// Validate
	if err := advisor.Validate(); err != nil {
		return fmt.Errorf("invalid advisor %s: %w", path, err)
	}

	l.advisors[advisor.Name] = advisor
	logging.Debugf("[advisors] Loaded advisor: %s (role: %s)", advisor.Name, advisor.Role)
	return nil
}

// Watch starts watching the advisors directory for changes
func (l *Loader) Watch(ctx context.Context) error {
	watcher, err := fsnotify.NewWatcher()
	if err != nil {
		return fmt.Errorf("failed to create watcher: %w", err)
	}

	l.watcher = watcher

	// Create cancellable context
	ctx, cancel := context.WithCancel(ctx)
	l.cancelCtx = cancel

	// Start watching goroutine
	go l.watchLoop(ctx)

	// Add directory to watch (recursive watch for subdirs)
	if err := l.watchRecursive(l.dir); err != nil {
		// Directory might not exist yet, that's okay
		logging.Errorf("[advisors] Could not watch %s: %v", l.dir, err)
	}

	return nil
}

// watchRecursive adds a directory and all subdirectories to the watcher
func (l *Loader) watchRecursive(dir string) error {
	return filepath.Walk(dir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return nil // Skip errors
		}
		if info.IsDir() {
			if err := l.watcher.Add(path); err != nil {
				logging.Debugf("[advisors] Could not watch %s: %v", path, err)
			}
		}
		return nil
	})
}

// watchLoop handles file system events
func (l *Loader) watchLoop(ctx context.Context) {
	for {
		select {
		case <-ctx.Done():
			return
		case event, ok := <-l.watcher.Events:
			if !ok {
				return
			}
			l.handleEvent(event)
		case err, ok := <-l.watcher.Errors:
			if !ok {
				return
			}
			logging.Errorf("[advisors] Watch error: %v", err)
		}
	}
}

// handleEvent processes a file system event
func (l *Loader) handleEvent(event fsnotify.Event) {
	// Only care about ADVISOR.md files (case-insensitive check)
	if !strings.EqualFold(filepath.Base(event.Name), AdvisorFileName) {
		return
	}

	logging.Debugf("[advisors] File event: %s %s", event.Op, event.Name)

	switch {
	case event.Op&fsnotify.Write == fsnotify.Write,
		event.Op&fsnotify.Create == fsnotify.Create:
		// Reload the specific file
		l.mu.Lock()
		if err := l.loadFile(event.Name); err != nil {
			logging.Errorf("[advisors] Error reloading %s: %v", event.Name, err)
		}
		l.mu.Unlock()

	case event.Op&fsnotify.Remove == fsnotify.Remove,
		event.Op&fsnotify.Rename == fsnotify.Rename:
		// Find and remove advisor loaded from this file
		l.mu.Lock()
		for name, advisor := range l.advisors {
			if advisor.FilePath == event.Name {
				delete(l.advisors, name)
				logging.Infof("[advisors] Unloaded advisor: %s", name)
				break
			}
		}
		l.mu.Unlock()
	}

	// Notify callback
	if l.onChange != nil {
		l.onChange(l.List())
	}
}

// OnChange sets a callback for when advisors are loaded/unloaded
func (l *Loader) OnChange(fn func([]*Advisor)) {
	l.onChange = fn
}

// Stop stops watching for changes
func (l *Loader) Stop() {
	if l.cancelCtx != nil {
		l.cancelCtx()
	}
	if l.watcher != nil {
		l.watcher.Close()
	}
}

// Get returns an advisor by name
func (l *Loader) Get(name string) (*Advisor, bool) {
	l.mu.RLock()
	defer l.mu.RUnlock()
	advisor, ok := l.advisors[name]
	return advisor, ok
}

// List returns all loaded and enabled advisors sorted by priority (highest first)
func (l *Loader) List() []*Advisor {
	l.mu.RLock()
	defer l.mu.RUnlock()

	advisors := make([]*Advisor, 0, len(l.advisors))
	for _, advisor := range l.advisors {
		if advisor.Enabled {
			advisors = append(advisors, advisor)
		}
	}

	sort.Slice(advisors, func(i, j int) bool {
		return advisors[i].Priority > advisors[j].Priority
	})

	return advisors
}

// ListAll returns all loaded advisors (including disabled) sorted by priority
func (l *Loader) ListAll() []*Advisor {
	l.mu.RLock()
	defer l.mu.RUnlock()

	advisors := make([]*Advisor, 0, len(l.advisors))
	for _, advisor := range l.advisors {
		advisors = append(advisors, advisor)
	}

	sort.Slice(advisors, func(i, j int) bool {
		return advisors[i].Priority > advisors[j].Priority
	})

	return advisors
}

// Count returns the number of enabled advisors
func (l *Loader) Count() int {
	l.mu.RLock()
	defer l.mu.RUnlock()
	count := 0
	for _, advisor := range l.advisors {
		if advisor.Enabled {
			count++
		}
	}
	return count
}

// SetEnabled sets the enabled state of an advisor by name
func (l *Loader) SetEnabled(name string, enabled bool) bool {
	l.mu.Lock()
	defer l.mu.Unlock()
	if advisor, ok := l.advisors[name]; ok {
		advisor.Enabled = enabled
		return true
	}
	return false
}

// LoadFromDB loads advisors from database rows into the in-memory map.
// DB advisors override file-based advisors with the same name.
func (l *Loader) LoadFromDB(rows []db.Advisor) {
	l.mu.Lock()
	defer l.mu.Unlock()

	for _, row := range rows {
		l.advisors[row.Name] = &Advisor{
			Name:           row.Name,
			Role:           row.Role,
			Description:    row.Description,
			Priority:       int(row.Priority),
			Enabled:        row.Enabled == 1,
			MemoryAccess:   row.MemoryAccess == 1,
			Persona:        row.Persona,
			TimeoutSeconds: int(row.TimeoutSeconds),
		}
	}

	logging.Infof("[advisors] Loaded %d advisors from database", len(rows))
}

// Dir returns the directory being watched
func (l *Loader) Dir() string {
	return l.dir
}
