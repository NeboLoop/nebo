package config

import (
	"context"
	"database/sql"
	"encoding/json"
	"time"

	"github.com/neboloop/nebo/internal/credential"
	"github.com/neboloop/nebo/internal/db"
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

// AuthProfileManager manages API key profiles using sqlc queries
type AuthProfileManager struct {
	queries *db.Queries
}

// NewAuthProfileManager creates a new auth profile manager using the shared connection.
// The schema must be created via migrations (internal/db/migrations/0010_auth_profiles.sql).
func NewAuthProfileManager(sqlDB *sql.DB) (*AuthProfileManager, error) {
	if sqlDB == nil {
		return nil, sql.ErrConnDone
	}

	return &AuthProfileManager{
		queries: db.New(sqlDB),
	}, nil
}

// Close is a no-op since we use a shared connection
func (m *AuthProfileManager) Close() error {
	return nil
}

// dbProfileToAuthProfile converts a sqlc AuthProfile to config.AuthProfile
func dbProfileToAuthProfile(p db.AuthProfile) *AuthProfile {
	// Decrypt API key (handles both enc:-prefixed and plaintext for migration window)
	apiKey := p.ApiKey
	if decrypted, err := credential.Decrypt(apiKey); err == nil {
		apiKey = decrypted
	}

	result := &AuthProfile{
		ID:       p.ID,
		Name:     p.Name,
		Provider: p.Provider,
		APIKey:   apiKey,
	}

	if p.Model.Valid {
		result.Model = p.Model.String
	}
	if p.BaseUrl.Valid {
		result.BaseURL = p.BaseUrl.String
	}
	if p.Priority.Valid {
		result.Priority = int(p.Priority.Int64)
	}
	if p.IsActive.Valid {
		result.IsActive = p.IsActive.Int64 == 1
	}
	if p.CooldownUntil.Valid {
		t := time.Unix(p.CooldownUntil.Int64, 0)
		result.CooldownUntil = &t
	}
	if p.LastUsedAt.Valid {
		t := time.Unix(p.LastUsedAt.Int64, 0)
		result.LastUsedAt = &t
	}
	if p.UsageCount.Valid {
		result.UsageCount = int(p.UsageCount.Int64)
	}
	if p.ErrorCount.Valid {
		result.ErrorCount = int(p.ErrorCount.Int64)
	}
	if p.Metadata.Valid && p.Metadata.String != "" {
		json.Unmarshal([]byte(p.Metadata.String), &result.Metadata)
	}

	return result
}

// GetBestProfile returns the best available profile for a provider using round-robin
// within the same priority level. Selection order:
// 1. Auth type (OAuth > Token > API Key)
// 2. Highest priority
// 3. Within same priority: least recently used (round-robin)
// 4. If no LastUsedAt: sort by error count ascending
func (m *AuthProfileManager) GetBestProfile(ctx context.Context, provider string) (*AuthProfile, error) {
	p, err := m.queries.GetBestAuthProfile(ctx, provider)
	if err == sql.ErrNoRows {
		return nil, nil // No profile available
	}
	if err != nil {
		return nil, err
	}

	return dbProfileToAuthProfile(p), nil
}

// ListActiveProfiles returns active profiles for a provider that are NOT on cooldown.
// Use this for request-level profile selection (round-robin, failover).
func (m *AuthProfileManager) ListActiveProfiles(ctx context.Context, provider string) ([]AuthProfile, error) {
	dbProfiles, err := m.queries.ListActiveAuthProfilesByProvider(ctx, provider)
	if err != nil {
		return nil, err
	}

	profiles := make([]AuthProfile, 0, len(dbProfiles))
	for _, p := range dbProfiles {
		profiles = append(profiles, *dbProfileToAuthProfile(p))
	}

	return profiles, nil
}

// ListAllActiveProfiles returns ALL active profiles for a provider, regardless of cooldown.
// Use this for provider loading — cooldown affects request routing, not provider existence.
func (m *AuthProfileManager) ListAllActiveProfiles(ctx context.Context, provider string) ([]AuthProfile, error) {
	dbProfiles, err := m.queries.ListAllActiveAuthProfilesByProvider(ctx, provider)
	if err != nil {
		return nil, err
	}

	profiles := make([]AuthProfile, 0, len(dbProfiles))
	for _, p := range dbProfiles {
		profiles = append(profiles, *dbProfileToAuthProfile(p))
	}

	return profiles, nil
}

// RecordUsage marks a profile as successfully used
func (m *AuthProfileManager) RecordUsage(ctx context.Context, profileID string) error {
	return m.queries.UpdateAuthProfileUsage(ctx, profileID)
}

// RecordError marks a profile as having an error
func (m *AuthProfileManager) RecordError(ctx context.Context, profileID string) error {
	return m.queries.UpdateAuthProfileError(ctx, profileID)
}

// SetCooldown puts a profile on cooldown until the specified time
func (m *AuthProfileManager) SetCooldown(ctx context.Context, profileID string, until time.Time) error {
	return m.queries.SetAuthProfileCooldown(ctx, db.SetAuthProfileCooldownParams{
		CooldownUntil: sql.NullInt64{Int64: until.Unix(), Valid: true},
		ID:            profileID,
	})
}

// ErrorReason categorizes the type of error for cooldown duration
type ErrorReason string

const (
	ErrorReasonBilling   ErrorReason = "billing"    // Payment/quota issues - long cooldown
	ErrorReasonRateLimit ErrorReason = "rate_limit" // Rate limiting - medium cooldown
	ErrorReasonAuth      ErrorReason = "auth"       // Authentication error - long cooldown
	ErrorReasonTimeout   ErrorReason = "timeout"    // Timeout - short cooldown
	ErrorReasonOther     ErrorReason = "other"      // Other errors - standard cooldown
)

// toErrorReason converts a string reason to ErrorReason type
func toErrorReason(reason string) ErrorReason {
	switch reason {
	case "billing":
		return ErrorReasonBilling
	case "rate_limit":
		return ErrorReasonRateLimit
	case "auth":
		return ErrorReasonAuth
	case "timeout":
		return ErrorReasonTimeout
	default:
		return ErrorReasonOther
	}
}

// RecordErrorWithCooldownString records an error with a string reason (implements ProfileTracker interface)
func (m *AuthProfileManager) RecordErrorWithCooldownString(ctx context.Context, profileID string, reason string) error {
	return m.RecordErrorWithCooldown(ctx, profileID, toErrorReason(reason))
}

// RecordErrorWithCooldown records an error and applies exponential backoff cooldown
// Uses exponential backoff: 60s * 5^(errorCount-1), max 1 hour (or 24h for billing)
func (m *AuthProfileManager) RecordErrorWithCooldown(ctx context.Context, profileID string, reason ErrorReason) error {
	// First, record the error to increment error_count
	if err := m.RecordError(ctx, profileID); err != nil {
		return err
	}

	// Get current error count using sqlc
	errorCount, err := m.queries.GetAuthProfileErrorCount(ctx, profileID)
	if err != nil {
		return err
	}

	// Convert sql.NullInt64 to int
	errCount := int(0)
	if errorCount.Valid {
		errCount = int(errorCount.Int64)
	}

	// Calculate cooldown duration based on error count and reason
	cooldownDuration := calculateCooldownDuration(errCount, reason)
	cooldownUntil := time.Now().Add(cooldownDuration)

	return m.SetCooldown(ctx, profileID, cooldownUntil)
}

// calculateCooldownDuration calculates exponential backoff cooldown
// Base formula: 60s * 5^(errorCount-1), capped by maxDuration
func calculateCooldownDuration(errorCount int, reason ErrorReason) time.Duration {
	if errorCount < 1 {
		errorCount = 1
	}

	// Base: 60 seconds (1 minute)
	baseSeconds := 60

	// Exponential: 60s, 300s (5min), 1500s (25min), 7500s (~2hr), etc.
	multiplier := 1
	for i := 1; i < errorCount; i++ {
		multiplier *= 5
		if multiplier > 3600 { // Cap multiplier to avoid overflow
			multiplier = 3600
			break
		}
	}

	cooldownSeconds := baseSeconds * multiplier

	// Apply max duration based on error reason
	var maxSeconds int
	switch reason {
	case ErrorReasonBilling:
		maxSeconds = 86400 // 24 hours - billing issues need manual intervention
	case ErrorReasonAuth:
		maxSeconds = 86400 // 24 hours - auth issues need manual intervention
	case ErrorReasonRateLimit:
		maxSeconds = 3600 // 1 hour - rate limits recover
	case ErrorReasonTimeout:
		maxSeconds = 300 // 5 minutes - transient timeouts
	default:
		maxSeconds = 3600 // 1 hour default
	}

	if cooldownSeconds > maxSeconds {
		cooldownSeconds = maxSeconds
	}

	return time.Duration(cooldownSeconds) * time.Second
}

// ResetErrorCountIfStale resets error count if no failures in the failure window (24h)
// This implements the failure window pattern
func (m *AuthProfileManager) ResetErrorCountIfStale(ctx context.Context, profileID string) error {
	failureWindowStart := time.Now().Unix() - 86400 // 24 hours ago
	return m.queries.ResetAuthProfileErrorCountIfStale(ctx, db.ResetAuthProfileErrorCountIfStaleParams{
		ID:        profileID,
		UpdatedAt: failureWindowStart,
	})
}

// CreateProfile creates a new auth profile
func (m *AuthProfileManager) CreateProfile(ctx context.Context, p *AuthProfile) error {
	var metadata sql.NullString
	if len(p.Metadata) > 0 {
		data, _ := json.Marshal(p.Metadata)
		metadata = sql.NullString{String: string(data), Valid: true}
	}

	isActive := int64(0)
	if p.IsActive {
		isActive = 1
	}

	_, err := m.queries.CreateAuthProfile(ctx, db.CreateAuthProfileParams{
		ID:       p.ID,
		Name:     p.Name,
		Provider: p.Provider,
		ApiKey:   p.APIKey,
		Model:    sql.NullString{String: p.Model, Valid: p.Model != ""},
		BaseUrl:  sql.NullString{String: p.BaseURL, Valid: p.BaseURL != ""},
		Priority: sql.NullInt64{Int64: int64(p.Priority), Valid: true},
		IsActive: sql.NullInt64{Int64: isActive, Valid: true},
		AuthType: sql.NullString{}, // Default to api_key
		Metadata: metadata,
	})
	return err
}

// DeleteProfile removes a profile
func (m *AuthProfileManager) DeleteProfile(ctx context.Context, profileID string) error {
	return m.queries.DeleteAuthProfile(ctx, profileID)
}

// ToProviderConfig converts an AuthProfile to a ProviderConfig for the agent
// Note: This loses the profile ID - prefer using AuthProfile directly when tracking is needed
func (p *AuthProfile) ToProviderConfig() ProviderConfig {
	return ProviderConfig{
		Name:    p.Name,
		Type:    "api", // Auth profiles are always API-based
		APIKey:  p.APIKey,
		Model:   p.Model,
		BaseURL: p.BaseURL,
	}
}
