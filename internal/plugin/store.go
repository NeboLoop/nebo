package plugin

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"sync"

	"github.com/google/uuid"
	db "github.com/nebolabs/nebo/internal/db"
)

// ChangeHandler is called when plugin settings change.
// pluginName is the plugin whose settings changed, settings is the full current map.
type ChangeHandler func(pluginName string, settings map[string]string)

// Store provides DB-backed CRUD for plugin registry and settings
// with change notification for hot-reload.
type Store struct {
	queries  *db.Queries
	mu       sync.RWMutex
	handlers []ChangeHandler

	// configurables maps plugin name -> Configurable implementation
	// for dispatching OnSettingsChanged when values change
	configurables map[string]Configurable
}

// NewStore creates a new plugin settings store backed by the given database.
func NewStore(sqlDB *sql.DB) *Store {
	return &Store{
		queries:       db.New(sqlDB),
		configurables: make(map[string]Configurable),
	}
}

// OnChange registers a handler called when any plugin's settings change.
// Useful for WebSocket broadcast to the UI.
func (s *Store) OnChange(fn ChangeHandler) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.handlers = append(s.handlers, fn)
}

// RegisterConfigurable registers a Configurable plugin implementation.
// The manifest is persisted to the plugin_registry row so the UI can render it.
func (s *Store) RegisterConfigurable(ctx context.Context, pluginName string, c Configurable) error {
	s.mu.Lock()
	s.configurables[pluginName] = c
	s.mu.Unlock()

	// Persist manifest to plugin_registry
	manifest := c.Manifest()
	manifestJSON, err := json.Marshal(manifest)
	if err != nil {
		return fmt.Errorf("marshal manifest for %s: %w", pluginName, err)
	}

	row, err := s.queries.GetPluginByName(ctx, pluginName)
	if err != nil {
		return fmt.Errorf("get plugin %s: %w", pluginName, err)
	}

	err = s.queries.UpdatePlugin(ctx, db.UpdatePluginParams{
		DisplayName:      row.DisplayName,
		Description:      row.Description,
		Icon:             row.Icon,
		Version:          row.Version,
		IsEnabled:        row.IsEnabled,
		SettingsManifest: string(manifestJSON),
		Metadata:         row.Metadata,
		ID:               row.ID,
	})
	if err != nil {
		return fmt.Errorf("update manifest for %s: %w", pluginName, err)
	}

	return nil
}

// GetPlugin returns a plugin registry entry by name.
func (s *Store) GetPlugin(ctx context.Context, name string) (*db.PluginRegistry, error) {
	row, err := s.queries.GetPluginByName(ctx, name)
	if err != nil {
		return nil, err
	}
	return &row, nil
}

// GetPluginByID returns a plugin registry entry by ID.
func (s *Store) GetPluginByID(ctx context.Context, id string) (*db.PluginRegistry, error) {
	row, err := s.queries.GetPlugin(ctx, id)
	if err != nil {
		return nil, err
	}
	return &row, nil
}

// ListPlugins returns all plugins, optionally filtered by type.
func (s *Store) ListPlugins(ctx context.Context, pluginType string) ([]db.PluginRegistry, error) {
	if pluginType != "" {
		return s.queries.ListPluginsByType(ctx, pluginType)
	}
	return s.queries.ListPlugins(ctx)
}

// GetSettings returns all settings for a plugin as a flat map.
func (s *Store) GetSettings(ctx context.Context, pluginID string) (map[string]string, error) {
	rows, err := s.queries.ListPluginSettings(ctx, pluginID)
	if err != nil {
		return nil, err
	}

	settings := make(map[string]string, len(rows))
	for _, row := range rows {
		settings[row.SettingKey] = row.SettingValue
	}
	return settings, nil
}

// GetSettingsByName returns all settings for a plugin looked up by name.
func (s *Store) GetSettingsByName(ctx context.Context, pluginName string) (map[string]string, error) {
	p, err := s.queries.GetPluginByName(ctx, pluginName)
	if err != nil {
		return nil, fmt.Errorf("plugin %q not found: %w", pluginName, err)
	}
	return s.GetSettings(ctx, p.ID)
}

// UpdateSettings bulk-updates settings for a plugin and triggers hot-reload.
// Only the keys present in the map are upserted; existing keys not in the map are untouched.
func (s *Store) UpdateSettings(ctx context.Context, pluginID string, values map[string]string, secrets map[string]bool) error {
	for key, value := range values {
		isSecret := int64(0)
		if secrets != nil && secrets[key] {
			isSecret = 1
		}
		_, err := s.queries.UpsertPluginSetting(ctx, db.UpsertPluginSettingParams{
			ID:           uuid.New().String(),
			PluginID:     pluginID,
			SettingKey:   key,
			SettingValue: value,
			IsSecret:     isSecret,
		})
		if err != nil {
			return fmt.Errorf("upsert setting %s: %w", key, err)
		}
	}

	// Fetch the full current settings for notification
	allSettings, err := s.GetSettings(ctx, pluginID)
	if err != nil {
		return fmt.Errorf("fetch settings after update: %w", err)
	}

	// Resolve plugin name for notification
	p, err := s.queries.GetPlugin(ctx, pluginID)
	if err != nil {
		return fmt.Errorf("get plugin for notification: %w", err)
	}

	// Notify Configurable plugin (hot-reload)
	s.mu.RLock()
	c, hasConfigurable := s.configurables[p.Name]
	s.mu.RUnlock()

	if hasConfigurable {
		if err := c.OnSettingsChanged(allSettings); err != nil {
			fmt.Printf("[PluginStore] Warning: %s.OnSettingsChanged failed: %v\n", p.Name, err)
		}
	}

	// Notify external handlers (WebSocket broadcast, etc.)
	s.notifyChange(p.Name, allSettings)

	return nil
}

// TogglePlugin enables or disables a plugin.
func (s *Store) TogglePlugin(ctx context.Context, pluginID string, enabled bool) error {
	val := int64(0)
	if enabled {
		val = 1
	}
	return s.queries.TogglePlugin(ctx, db.TogglePluginParams{
		IsEnabled: val,
		ID:        pluginID,
	})
}

// UpdateStatus updates a plugin's connection status.
func (s *Store) UpdateStatus(ctx context.Context, pluginID, status string, lastError string) error {
	var errVal sql.NullString
	if lastError != "" {
		errVal = sql.NullString{String: lastError, Valid: true}
	}
	var connectedAt sql.NullInt64
	if status == "connected" {
		connectedAt = sql.NullInt64{Int64: 0, Valid: true} // DB will use unixepoch() default
	}
	return s.queries.UpdatePluginStatus(ctx, db.UpdatePluginStatusParams{
		ConnectionStatus: status,
		LastConnectedAt:  connectedAt,
		LastError:        errVal,
		ID:               pluginID,
	})
}

// DeleteSetting removes a single setting for a plugin.
func (s *Store) DeleteSetting(ctx context.Context, pluginID, key string) error {
	return s.queries.DeletePluginSetting(ctx, db.DeletePluginSettingParams{
		PluginID:   pluginID,
		SettingKey: key,
	})
}

// notifyChange calls all registered change handlers.
func (s *Store) notifyChange(pluginName string, settings map[string]string) {
	s.mu.RLock()
	handlers := make([]ChangeHandler, len(s.handlers))
	copy(handlers, s.handlers)
	s.mu.RUnlock()

	for _, fn := range handlers {
		fn(pluginName, settings)
	}
}
