package local

import (
	"context"
	"database/sql"
	"sync"

	"github.com/neboloop/nebo/internal/db"
)

// AgentSettings holds the agent configuration
type AgentSettings struct {
	AutonomousMode           bool   `json:"autonomousMode"`
	AutoApproveRead          bool   `json:"autoApproveRead"`
	AutoApproveWrite         bool   `json:"autoApproveWrite"`
	AutoApproveBash          bool   `json:"autoApproveBash"`
	HeartbeatIntervalMinutes int    `json:"heartbeatIntervalMinutes"`
	CommEnabled              bool   `json:"commEnabled"`
	CommPlugin               string `json:"commPlugin,omitempty"`
	DeveloperMode            bool   `json:"developerMode"`
}

// SettingsChangeCallback is called when settings are updated
type SettingsChangeCallback func(AgentSettings)

// Package-level singleton
var (
	instance     *AgentSettingsStore
	instanceOnce sync.Once
)

// AgentSettingsStore is the singleton settings store.
// Call InitSettings() once at startup, then Settings() anywhere.
type AgentSettingsStore struct {
	queries   *db.Queries
	mu        sync.RWMutex
	cached    AgentSettings
	callbacks []SettingsChangeCallback
}

// InitSettings initializes the singleton from the database. Call once at startup.
func InitSettings(database *sql.DB) {
	instanceOnce.Do(func() {
		instance = &AgentSettingsStore{
			queries: db.New(database),
			cached: AgentSettings{
				AutoApproveRead:          true,
				HeartbeatIntervalMinutes: 30,
			},
		}
		instance.load()
	})
}

// GetAgentSettings returns the singleton settings store.
func GetAgentSettings() *AgentSettingsStore {
	return instance
}

// Get returns the current settings from the in-memory cache.
func (s *AgentSettingsStore) Get() AgentSettings {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.cached
}

// Update persists settings to the database, updates the cache,
// and fires all registered change callbacks.
func (s *AgentSettingsStore) Update(settings AgentSettings) error {
	s.mu.Lock()

	err := s.queries.UpdateSettings(context.Background(), db.UpdateSettingsParams{
		AutonomousMode:           boolToInt(settings.AutonomousMode),
		AutoApproveRead:          boolToInt(settings.AutoApproveRead),
		AutoApproveWrite:         boolToInt(settings.AutoApproveWrite),
		AutoApproveBash:          boolToInt(settings.AutoApproveBash),
		HeartbeatIntervalMinutes: int64(settings.HeartbeatIntervalMinutes),
		CommEnabled:              boolToInt(settings.CommEnabled),
		CommPlugin:               settings.CommPlugin,
		DeveloperMode:            boolToInt(settings.DeveloperMode),
	})
	if err != nil {
		s.mu.Unlock()
		return err
	}

	s.cached = settings
	cbs := make([]SettingsChangeCallback, len(s.callbacks))
	copy(cbs, s.callbacks)
	s.mu.Unlock()

	for _, cb := range cbs {
		cb(settings)
	}

	return nil
}

// OnChange registers a callback that fires whenever settings are updated.
func (s *AgentSettingsStore) OnChange(cb SettingsChangeCallback) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.callbacks = append(s.callbacks, cb)
}

func (s *AgentSettingsStore) load() {
	row, err := s.queries.GetSettings(context.Background())
	if err != nil {
		return
	}

	s.cached = AgentSettings{
		AutonomousMode:           row.AutonomousMode != 0,
		AutoApproveRead:          row.AutoApproveRead != 0,
		AutoApproveWrite:         row.AutoApproveWrite != 0,
		AutoApproveBash:          row.AutoApproveBash != 0,
		HeartbeatIntervalMinutes: int(row.HeartbeatIntervalMinutes),
		CommEnabled:              row.CommEnabled != 0,
		CommPlugin:               row.CommPlugin,
		DeveloperMode:            row.DeveloperMode != 0,
	}
}

func boolToInt(b bool) int64 {
	if b {
		return 1
	}
	return 0
}
