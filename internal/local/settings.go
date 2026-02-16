package local

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"

	"github.com/neboloop/nebo/internal/defaults"
)

// Settings holds local configuration that can't be in the embedded yaml
type Settings struct {
	AccessSecret       string `json:"accessSecret"`
	AccessExpire       int64  `json:"accessExpire"`
	RefreshTokenExpire int64  `json:"refreshTokenExpire"`
}

// DefaultSettings returns sensible defaults
func DefaultSettings() Settings {
	return Settings{
		AccessExpire:       2592000, // 30 days
		RefreshTokenExpire: 2592000, // 30 days
	}
}

// settingsPath returns the path to the local settings file
func settingsPath() (string, error) {
	dataDir, err := defaults.DataDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(dataDir, "settings.json"), nil
}

// LoadSettings loads local settings, creating defaults if needed
func LoadSettings() (*Settings, error) {
	path, err := settingsPath()
	if err != nil {
		return nil, err
	}

	// Ensure directory exists
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0700); err != nil {
		return nil, fmt.Errorf("failed to create settings directory: %w", err)
	}

	// Try to load existing settings
	data, err := os.ReadFile(path)
	if err == nil {
		var settings Settings
		if err := json.Unmarshal(data, &settings); err == nil {
			// Ensure secret exists (upgrade from older settings)
			if settings.AccessSecret == "" {
				settings.AccessSecret = generateSecret()
				if err := SaveSettings(&settings); err != nil {
					return nil, err
				}
			}
			return &settings, nil
		}
	}

	// Create new settings with generated secret
	settings := DefaultSettings()
	settings.AccessSecret = generateSecret()

	if err := SaveSettings(&settings); err != nil {
		return nil, err
	}

	return &settings, nil
}

// SaveSettings persists settings to disk
func SaveSettings(settings *Settings) error {
	path, err := settingsPath()
	if err != nil {
		return err
	}

	data, err := json.MarshalIndent(settings, "", "  ")
	if err != nil {
		return err
	}

	return os.WriteFile(path, data, 0600)
}

// generateSecret creates a cryptographically secure random secret
func generateSecret() string {
	bytes := make([]byte, 32)
	if _, err := rand.Read(bytes); err != nil {
		// Fallback to less secure but still random
		return fmt.Sprintf("nebo-%d", os.Getpid())
	}
	return hex.EncodeToString(bytes)
}
