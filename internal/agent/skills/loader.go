package skills

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"

	"github.com/fsnotify/fsnotify"
	"github.com/nebolabs/nebo/internal/logging"
)

// SkillFileName is the expected filename for skill definitions
const SkillFileName = "SKILL.md"

// Loader manages loading and hot-reloading of skill definitions
type Loader struct {
	mu        sync.RWMutex
	skills    map[string]*Skill // name -> skill
	dir       string
	watcher   *fsnotify.Watcher
	onChange  func([]*Skill) // callback when skills change
	cancelCtx context.CancelFunc
}

// NewLoader creates a new skill loader for the given directory
func NewLoader(dir string) *Loader {
	return &Loader{
		skills: make(map[string]*Skill),
		dir:    dir,
	}
}

// LoadAll loads all skill files from the configured directory.
// Skills are expected to be in subdirectories with a SKILL.md file:
//
//	skills/
//	├── weather/
//	│   └── SKILL.md
//	├── code-review/
//	│   └── SKILL.md
//	└── github/
//	    └── SKILL.md
func (l *Loader) LoadAll() error {
	l.mu.Lock()
	defer l.mu.Unlock()

	// Clear existing skills
	l.skills = make(map[string]*Skill)

	// Ensure directory exists
	if _, err := os.Stat(l.dir); os.IsNotExist(err) {
		// Directory doesn't exist, that's okay - no skills loaded
		return nil
	}

	// Walk directory looking for SKILL.md files
	err := filepath.Walk(l.dir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		if info.IsDir() {
			return nil
		}

		// Only load SKILL.md files (case-insensitive check)
		if !strings.EqualFold(filepath.Base(path), SkillFileName) {
			return nil
		}

		return l.loadFile(path)
	})

	if err != nil {
		return fmt.Errorf("failed to load skills: %w", err)
	}

	logging.Infof("[skills] Loaded %d skills from %s", len(l.skills), l.dir)
	return nil
}

// loadFile loads a single SKILL.md file (must hold lock)
func (l *Loader) loadFile(path string) error {
	data, err := os.ReadFile(path)
	if err != nil {
		return fmt.Errorf("failed to read %s: %w", path, err)
	}

	skill, err := ParseSkillMD(data)
	if err != nil {
		return fmt.Errorf("failed to parse %s: %w", path, err)
	}

	// Set defaults
	if skill.Version == "" {
		skill.Version = "1.0.0"
	}
	skill.Enabled = true // Default to enabled
	skill.FilePath = path

	// Validate
	if err := skill.Validate(); err != nil {
		return fmt.Errorf("invalid skill %s: %w", path, err)
	}

	l.skills[skill.Name] = skill
	logging.Debugf("[skills] Loaded skill: %s", skill.Name)
	return nil
}

// Watch starts watching the skills directory for changes
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
		logging.Errorf("[skills] Could not watch %s: %v", l.dir, err)
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
				logging.Debugf("[skills] Could not watch %s: %v", path, err)
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
			logging.Errorf("[skills] Watch error: %v", err)
		}
	}
}

// handleEvent processes a file system event
func (l *Loader) handleEvent(event fsnotify.Event) {
	// Only care about SKILL.md files (case-insensitive check)
	if !strings.EqualFold(filepath.Base(event.Name), SkillFileName) {
		return
	}

	logging.Debugf("[skills] File event: %s %s", event.Op, event.Name)

	switch {
	case event.Op&fsnotify.Write == fsnotify.Write,
		event.Op&fsnotify.Create == fsnotify.Create:
		// Reload the specific file
		l.mu.Lock()
		if err := l.loadFile(event.Name); err != nil {
			logging.Errorf("[skills] Error reloading %s: %v", event.Name, err)
		}
		l.mu.Unlock()

	case event.Op&fsnotify.Remove == fsnotify.Remove,
		event.Op&fsnotify.Rename == fsnotify.Rename:
		// Find and remove skill loaded from this file
		l.mu.Lock()
		for name, skill := range l.skills {
			if skill.FilePath == event.Name {
				delete(l.skills, name)
				logging.Infof("[skills] Unloaded skill: %s", name)
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

// OnChange sets a callback for when skills are loaded/unloaded
func (l *Loader) OnChange(fn func([]*Skill)) {
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

// Get returns a skill by name
func (l *Loader) Get(name string) (*Skill, bool) {
	l.mu.RLock()
	defer l.mu.RUnlock()
	skill, ok := l.skills[name]
	return skill, ok
}

// List returns all loaded skills sorted by priority (highest first)
func (l *Loader) List() []*Skill {
	l.mu.RLock()
	defer l.mu.RUnlock()

	skills := make([]*Skill, 0, len(l.skills))
	for _, skill := range l.skills {
		skills = append(skills, skill)
	}

	sort.Slice(skills, func(i, j int) bool {
		return skills[i].Priority > skills[j].Priority
	})

	return skills
}

// Count returns the number of loaded skills
func (l *Loader) Count() int {
	l.mu.RLock()
	defer l.mu.RUnlock()
	return len(l.skills)
}

// Add adds a skill to the loader (used for merging from other loaders)
func (l *Loader) Add(skill *Skill) {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.skills[skill.Name] = skill
}

// SetEnabled sets the enabled state of a skill by name
func (l *Loader) SetEnabled(name string, enabled bool) bool {
	l.mu.Lock()
	defer l.mu.Unlock()
	if skill, ok := l.skills[name]; ok {
		skill.Enabled = enabled
		return true
	}
	return false
}

// SetDisabledSkills updates the enabled state based on a list of disabled skill names
func (l *Loader) SetDisabledSkills(disabled []string) {
	l.mu.Lock()
	defer l.mu.Unlock()

	disabledMap := make(map[string]bool)
	for _, name := range disabled {
		disabledMap[name] = true
	}

	for name, skill := range l.skills {
		skill.Enabled = !disabledMap[name]
	}
}
