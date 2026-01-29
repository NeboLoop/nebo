package config

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"time"

	_ "modernc.org/sqlite"
)

// AuthProfile represents an API key configuration stored in the database
type AuthProfile struct {
	ID            string            `json:"id"`
	Name          string            `json:"name"`
	Provider      string            `json:"provider"` // anthropic, openai, google, ollama
	APIKey        string            `json:"api_key"`
	Model         string            `json:"model,omitempty"`
	BaseURL       string            `json:"base_url,omitempty"`
	Priority      int               `json:"priority"`
	IsActive      bool              `json:"is_active"`
	CooldownUntil *time.Time        `json:"cooldown_until,omitempty"`
	LastUsedAt    *time.Time        `json:"last_used_at,omitempty"`
	UsageCount    int               `json:"usage_count"`
	ErrorCount    int               `json:"error_count"`
	Metadata      map[string]string `json:"metadata,omitempty"`
}

// AuthProfileManager manages API key profiles from SQLite
type AuthProfileManager struct {
	db *sql.DB
}

// NewAuthProfileManager creates a new auth profile manager
func NewAuthProfileManager(dbPath string) (*AuthProfileManager, error) {
	db, err := sql.Open("sqlite", dbPath)
	if err != nil {
		return nil, fmt.Errorf("failed to open database: %w", err)
	}

	m := &AuthProfileManager{db: db}
	if err := m.ensureSchema(); err != nil {
		db.Close()
		return nil, fmt.Errorf("failed to ensure schema: %w", err)
	}

	return m, nil
}

// ensureSchema creates the auth_profiles table if it doesn't exist
func (m *AuthProfileManager) ensureSchema() error {
	schema := `
	CREATE TABLE IF NOT EXISTS auth_profiles (
		id TEXT PRIMARY KEY,
		name TEXT NOT NULL,
		provider TEXT NOT NULL,
		api_key TEXT NOT NULL,
		model TEXT,
		base_url TEXT,
		priority INTEGER DEFAULT 0,
		is_active INTEGER DEFAULT 1,
		cooldown_until INTEGER,
		last_used_at INTEGER,
		usage_count INTEGER DEFAULT 0,
		error_count INTEGER DEFAULT 0,
		metadata TEXT,
		created_at INTEGER NOT NULL,
		updated_at INTEGER NOT NULL
	);
	CREATE INDEX IF NOT EXISTS idx_auth_profiles_provider ON auth_profiles(provider, is_active);
	CREATE INDEX IF NOT EXISTS idx_auth_profiles_priority ON auth_profiles(provider, priority DESC, is_active);
	`
	_, err := m.db.Exec(schema)
	return err
}

// Close closes the database connection
func (m *AuthProfileManager) Close() error {
	return m.db.Close()
}

// GetBestProfile returns the best available profile for a provider using round-robin
// within the same priority level. Selection order:
// 1. Highest priority
// 2. Within same priority: least recently used (round-robin)
// 3. If no LastUsedAt: sort by error count ascending
func (m *AuthProfileManager) GetBestProfile(ctx context.Context, provider string) (*AuthProfile, error) {
	// Get all available profiles and sort in Go for proper round-robin
	query := `
		SELECT id, name, provider, api_key, model, base_url, priority, is_active,
		       cooldown_until, last_used_at, usage_count, error_count, metadata
		FROM auth_profiles
		WHERE provider = ? AND is_active = 1 AND (cooldown_until IS NULL OR cooldown_until < ?)
		ORDER BY priority DESC, COALESCE(last_used_at, 0) ASC, error_count ASC
		LIMIT 1
	`

	now := time.Now().Unix()
	row := m.db.QueryRowContext(ctx, query, provider, now)

	var p AuthProfile
	var cooldownUntil, lastUsedAt sql.NullInt64
	var model, baseURL, metadata sql.NullString
	var isActive int

	err := row.Scan(
		&p.ID, &p.Name, &p.Provider, &p.APIKey, &model, &baseURL,
		&p.Priority, &isActive, &cooldownUntil, &lastUsedAt,
		&p.UsageCount, &p.ErrorCount, &metadata,
	)
	if err == sql.ErrNoRows {
		return nil, nil // No profile available
	}
	if err != nil {
		return nil, err
	}

	p.IsActive = isActive == 1
	if model.Valid {
		p.Model = model.String
	}
	if baseURL.Valid {
		p.BaseURL = baseURL.String
	}
	if cooldownUntil.Valid {
		t := time.Unix(cooldownUntil.Int64, 0)
		p.CooldownUntil = &t
	}
	if lastUsedAt.Valid {
		t := time.Unix(lastUsedAt.Int64, 0)
		p.LastUsedAt = &t
	}
	if metadata.Valid {
		json.Unmarshal([]byte(metadata.String), &p.Metadata)
	}

	return &p, nil
}

// ListActiveProfiles returns all active profiles for a provider
func (m *AuthProfileManager) ListActiveProfiles(ctx context.Context, provider string) ([]AuthProfile, error) {
	query := `
		SELECT id, name, provider, api_key, model, base_url, priority, is_active,
		       cooldown_until, last_used_at, usage_count, error_count, metadata
		FROM auth_profiles
		WHERE provider = ? AND is_active = 1 AND (cooldown_until IS NULL OR cooldown_until < ?)
		ORDER BY priority DESC, error_count ASC
	`

	now := time.Now().Unix()
	rows, err := m.db.QueryContext(ctx, query, provider, now)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var profiles []AuthProfile
	for rows.Next() {
		var p AuthProfile
		var cooldownUntil, lastUsedAt sql.NullInt64
		var model, baseURL, metadata sql.NullString
		var isActive int

		err := rows.Scan(
			&p.ID, &p.Name, &p.Provider, &p.APIKey, &model, &baseURL,
			&p.Priority, &isActive, &cooldownUntil, &lastUsedAt,
			&p.UsageCount, &p.ErrorCount, &metadata,
		)
		if err != nil {
			return nil, err
		}

		p.IsActive = isActive == 1
		if model.Valid {
			p.Model = model.String
		}
		if baseURL.Valid {
			p.BaseURL = baseURL.String
		}
		if cooldownUntil.Valid {
			t := time.Unix(cooldownUntil.Int64, 0)
			p.CooldownUntil = &t
		}
		if lastUsedAt.Valid {
			t := time.Unix(lastUsedAt.Int64, 0)
			p.LastUsedAt = &t
		}
		if metadata.Valid {
			json.Unmarshal([]byte(metadata.String), &p.Metadata)
		}

		profiles = append(profiles, p)
	}

	return profiles, rows.Err()
}

// RecordUsage marks a profile as successfully used
func (m *AuthProfileManager) RecordUsage(ctx context.Context, profileID string) error {
	query := `
		UPDATE auth_profiles
		SET last_used_at = ?, usage_count = usage_count + 1, error_count = 0, updated_at = ?
		WHERE id = ?
	`
	now := time.Now().Unix()
	_, err := m.db.ExecContext(ctx, query, now, now, profileID)
	return err
}

// RecordError marks a profile as having an error
func (m *AuthProfileManager) RecordError(ctx context.Context, profileID string) error {
	query := `
		UPDATE auth_profiles
		SET error_count = error_count + 1, updated_at = ?
		WHERE id = ?
	`
	now := time.Now().Unix()
	_, err := m.db.ExecContext(ctx, query, now, profileID)
	return err
}

// SetCooldown puts a profile on cooldown until the specified time
func (m *AuthProfileManager) SetCooldown(ctx context.Context, profileID string, until time.Time) error {
	query := `
		UPDATE auth_profiles
		SET cooldown_until = ?, updated_at = ?
		WHERE id = ?
	`
	now := time.Now().Unix()
	_, err := m.db.ExecContext(ctx, query, until.Unix(), now, profileID)
	return err
}

// CreateProfile creates a new auth profile
func (m *AuthProfileManager) CreateProfile(ctx context.Context, p *AuthProfile) error {
	query := `
		INSERT INTO auth_profiles (id, name, provider, api_key, model, base_url, priority, is_active, metadata, created_at, updated_at)
		VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
	`

	now := time.Now().Unix()
	var metadata sql.NullString
	if len(p.Metadata) > 0 {
		data, _ := json.Marshal(p.Metadata)
		metadata = sql.NullString{String: string(data), Valid: true}
	}

	isActive := 0
	if p.IsActive {
		isActive = 1
	}

	_, err := m.db.ExecContext(ctx, query,
		p.ID, p.Name, p.Provider, p.APIKey, p.Model, p.BaseURL,
		p.Priority, isActive, metadata, now, now,
	)
	return err
}

// DeleteProfile removes a profile
func (m *AuthProfileManager) DeleteProfile(ctx context.Context, profileID string) error {
	_, err := m.db.ExecContext(ctx, "DELETE FROM auth_profiles WHERE id = ?", profileID)
	return err
}

// ToProviderConfig converts an AuthProfile to a ProviderConfig for the agent
func (p *AuthProfile) ToProviderConfig() ProviderConfig {
	return ProviderConfig{
		Name:    p.Name,
		Type:    "api", // Auth profiles are always API-based
		APIKey:  p.APIKey,
		Model:   p.Model,
		BaseURL: p.BaseURL,
	}
}

// LoadProvidersFromDB loads all active auth profiles as provider configs
func LoadProvidersFromDB(dbPath string) ([]ProviderConfig, error) {
	m, err := NewAuthProfileManager(dbPath)
	if err != nil {
		return nil, err
	}
	defer m.Close()

	ctx := context.Background()
	providers := []string{"anthropic", "openai", "google", "ollama"}
	var configs []ProviderConfig

	for _, provider := range providers {
		profiles, err := m.ListActiveProfiles(ctx, provider)
		if err != nil {
			continue
		}
		for _, p := range profiles {
			configs = append(configs, p.ToProviderConfig())
		}
	}

	return configs, nil
}
