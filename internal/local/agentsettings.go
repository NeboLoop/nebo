package local

import (
	"encoding/json"
	"os"
	"path/filepath"
	"sync"
)

// AgentSettings holds the agent configuration
type AgentSettings struct {
	AutonomousMode   bool `json:"autonomousMode"`
	AutoApproveRead  bool `json:"autoApproveRead"`
	AutoApproveWrite bool `json:"autoApproveWrite"`
	AutoApproveBash  bool `json:"autoApproveBash"`
}

// AgentSettingsStore manages agent settings persistence
type AgentSettingsStore struct {
	filePath string
	mu       sync.RWMutex
	settings AgentSettings
}

// NewAgentSettingsStore creates a new settings store
func NewAgentSettingsStore(dataDir string) *AgentSettingsStore {
	store := &AgentSettingsStore{
		filePath: filepath.Join(dataDir, "agent-settings.json"),
		settings: AgentSettings{
			AutonomousMode:   false,
			AutoApproveRead:  true, // Safe default
			AutoApproveWrite: false,
			AutoApproveBash:  false,
		},
	}
	store.load()
	return store
}

// Get returns the current settings
func (s *AgentSettingsStore) Get() AgentSettings {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.settings
}

// Update saves new settings
func (s *AgentSettingsStore) Update(settings AgentSettings) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	s.settings = settings
	return s.save()
}

func (s *AgentSettingsStore) load() {
	data, err := os.ReadFile(s.filePath)
	if err != nil {
		// File doesn't exist yet, use defaults
		return
	}

	var settings AgentSettings
	if err := json.Unmarshal(data, &settings); err != nil {
		return
	}
	s.settings = settings
}

func (s *AgentSettingsStore) save() error {
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
