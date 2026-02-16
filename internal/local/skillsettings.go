package local

import (
	"encoding/json"
	"os"
	"path/filepath"
	"sync"
)

// SkillSettings holds the enabled/disabled state of skills
type SkillSettings struct {
	DisabledSkills []string `json:"disabledSkills"`
}

// SkillSettingsStore manages skill settings persistence
type SkillSettingsStore struct {
	filePath string
	mu       sync.RWMutex
	settings SkillSettings
	onChange func(name string, enabled bool)
}

// NewSkillSettingsStore creates a new skill settings store
func NewSkillSettingsStore(dataDir string) *SkillSettingsStore {
	store := &SkillSettingsStore{
		filePath: filepath.Join(dataDir, "skill-settings.json"),
		settings: SkillSettings{
			DisabledSkills: []string{},
		},
	}
	store.load()
	return store
}

// OnChange registers a callback fired after a skill is toggled or its enabled state changes.
func (s *SkillSettingsStore) OnChange(fn func(name string, enabled bool)) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.onChange = fn
}

// Get returns the current settings
func (s *SkillSettingsStore) Get() SkillSettings {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.settings
}

// GetDisabledSkills returns the list of disabled skill names
func (s *SkillSettingsStore) GetDisabledSkills() []string {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.settings.DisabledSkills
}

// IsEnabled checks if a skill is enabled (not in disabled list)
func (s *SkillSettingsStore) IsEnabled(name string) bool {
	s.mu.RLock()
	defer s.mu.RUnlock()
	for _, disabled := range s.settings.DisabledSkills {
		if disabled == name {
			return false
		}
	}
	return true
}

// Toggle toggles the enabled state of a skill and returns the new state
func (s *SkillSettingsStore) Toggle(name string) (enabled bool, err error) {
	s.mu.Lock()

	// Check if currently disabled
	for i, disabled := range s.settings.DisabledSkills {
		if disabled == name {
			// Remove from disabled list (enable it)
			s.settings.DisabledSkills = append(
				s.settings.DisabledSkills[:i],
				s.settings.DisabledSkills[i+1:]...,
			)
			err = s.save()
			cb := s.onChange
			s.mu.Unlock()
			if err == nil && cb != nil {
				cb(name, true)
			}
			return true, err
		}
	}

	// Not in disabled list, add it (disable)
	s.settings.DisabledSkills = append(s.settings.DisabledSkills, name)
	err = s.save()
	cb := s.onChange
	s.mu.Unlock()
	if err == nil && cb != nil {
		cb(name, false)
	}
	return false, err
}

// SetEnabled sets the enabled state of a skill
func (s *SkillSettingsStore) SetEnabled(name string, enabled bool) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	// Find if it's in the disabled list
	idx := -1
	for i, disabled := range s.settings.DisabledSkills {
		if disabled == name {
			idx = i
			break
		}
	}

	if enabled && idx >= 0 {
		// Remove from disabled list
		s.settings.DisabledSkills = append(
			s.settings.DisabledSkills[:idx],
			s.settings.DisabledSkills[idx+1:]...,
		)
		return s.save()
	} else if !enabled && idx < 0 {
		// Add to disabled list
		s.settings.DisabledSkills = append(s.settings.DisabledSkills, name)
		return s.save()
	}

	// No change needed
	return nil
}

func (s *SkillSettingsStore) load() {
	data, err := os.ReadFile(s.filePath)
	if err != nil {
		// File doesn't exist yet, use defaults
		return
	}

	var settings SkillSettings
	if err := json.Unmarshal(data, &settings); err != nil {
		return
	}
	s.settings = settings
}

func (s *SkillSettingsStore) save() error {
	// Ensure directory exists
	dir := filepath.Dir(s.filePath)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return err
	}

	data, err := json.MarshalIndent(s.settings, "", "  ")
	if err != nil {
		return err
	}

	return os.WriteFile(s.filePath, data, 0644)
}
