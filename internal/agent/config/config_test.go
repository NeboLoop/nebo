package config

import (
	"os"
	"path/filepath"
	"testing"
)

func TestDefaultConfig(t *testing.T) {
	cfg := DefaultConfig()

	if cfg == nil {
		t.Fatal("DefaultConfig returned nil")
	}

	// Providers are now loaded from models.yaml, not hardcoded in DefaultConfig
	// DefaultConfig returns empty providers slice
	if cfg.Providers == nil {
		t.Error("expected Providers to be non-nil (even if empty)")
	}

	// Check defaults
	if cfg.MaxContext != 50 {
		t.Errorf("expected MaxContext 50, got %d", cfg.MaxContext)
	}

	if cfg.MaxIterations != 100 {
		t.Errorf("expected MaxIterations 100, got %d", cfg.MaxIterations)
	}

	// Check policy defaults
	if cfg.Policy.Level != "allowlist" {
		t.Errorf("expected policy level 'allowlist', got %s", cfg.Policy.Level)
	}

	if cfg.Policy.AskMode != "on-miss" {
		t.Errorf("expected ask mode 'on-miss', got %s", cfg.Policy.AskMode)
	}

	if len(cfg.Policy.Allowlist) == 0 {
		t.Error("expected non-empty allowlist")
	}
}

func TestDefaultDataDir(t *testing.T) {
	dir := DefaultDataDir()

	if dir == "" {
		t.Error("DefaultDataDir returned empty string")
	}

	// Should end with Nebo or nebo (platform-dependent casing)
	base := filepath.Base(dir)
	if base != "Nebo" && base != "nebo" {
		t.Errorf("expected data dir to end with Nebo or nebo, got %s", base)
	}
}

func TestDBPath(t *testing.T) {
	cfg := DefaultConfig()
	dbPath := cfg.DBPath()

	if dbPath == "" {
		t.Error("DBPath returned empty string")
	}

	// Should end with nebo.db
	if filepath.Base(dbPath) != "nebo.db" {
		t.Errorf("expected db path to end with nebo.db, got %s", dbPath)
	}
}

func TestEnsureDataDir(t *testing.T) {
	// Use temp directory
	tmpDir := t.TempDir()
	cfg := &Config{
		DataDir: filepath.Join(tmpDir, "testdata"),
	}

	err := cfg.EnsureDataDir()
	if err != nil {
		t.Fatalf("EnsureDataDir failed: %v", err)
	}

	// Check directory was created
	info, err := os.Stat(cfg.DataDir)
	if err != nil {
		t.Fatalf("data dir not created: %v", err)
	}

	if !info.IsDir() {
		t.Error("data dir is not a directory")
	}
}

func TestGetProvider(t *testing.T) {
	// Create config with test providers (providers are now loaded from models.yaml, not DefaultConfig)
	cfg := &Config{
		Providers: []ProviderConfig{
			{Name: "anthropic", Type: "api", APIKey: "test-key", Model: "claude-sonnet-4-5"},
			{Name: "openai", Type: "api", APIKey: "test-key", Model: "gpt-5.2"},
		},
	}

	// Test existing provider
	p := cfg.GetProvider("anthropic")
	if p == nil {
		t.Error("GetProvider returned nil for existing provider")
	}
	if p.Name != "anthropic" {
		t.Errorf("expected provider name 'anthropic', got %s", p.Name)
	}

	// Test non-existing provider
	p = cfg.GetProvider("nonexistent")
	if p != nil {
		t.Error("GetProvider should return nil for non-existing provider")
	}
}

func TestFirstValidProvider(t *testing.T) {
	// Test with no valid providers
	cfg := &Config{
		Providers: []ProviderConfig{
			{Name: "empty", Type: "api", APIKey: ""},
			{Name: "cli-no-cmd", Type: "cli", Command: ""},
		},
	}

	p := cfg.FirstValidProvider()
	if p != nil {
		t.Error("FirstValidProvider should return nil when no valid providers")
	}

	// Test with valid API provider
	cfg.Providers = append(cfg.Providers, ProviderConfig{
		Name:   "valid-api",
		Type:   "api",
		APIKey: "test-key",
	})

	p = cfg.FirstValidProvider()
	if p == nil {
		t.Fatal("FirstValidProvider returned nil with valid provider")
	}
	if p.Name != "valid-api" {
		t.Errorf("expected 'valid-api', got %s", p.Name)
	}

	// Test with valid CLI provider (should be first)
	cfg.Providers = []ProviderConfig{
		{Name: "valid-cli", Type: "cli", Command: "/usr/bin/test"},
		{Name: "valid-api", Type: "api", APIKey: "test-key"},
	}

	p = cfg.FirstValidProvider()
	if p == nil {
		t.Fatal("FirstValidProvider returned nil")
	}
	if p.Name != "valid-cli" {
		t.Errorf("expected CLI provider first, got %s", p.Name)
	}
}

func TestLoadAndSave(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := &Config{
		// Note: Providers are NOT saved to config.yaml (yaml:"-" tag)
		// They are loaded from models.yaml instead
		DataDir:       tmpDir,
		MaxContext:    100,
		MaxIterations: 50,
		Policy: PolicyConfig{
			Level:     "full",
			AskMode:   "always",
			Allowlist: []string{"ls", "pwd"},
		},
	}

	// Save config
	err := cfg.Save()
	if err != nil {
		t.Fatalf("Save failed: %v", err)
	}

	// Check file exists
	configPath := filepath.Join(tmpDir, "config.yaml")
	if _, err := os.Stat(configPath); err != nil {
		t.Fatalf("config file not created: %v", err)
	}

	// Load config from file
	loaded, err := LoadFrom(configPath)
	if err != nil {
		t.Fatalf("LoadFrom failed: %v", err)
	}

	// Verify loaded values (providers come from models.yaml, not config.yaml)
	if loaded.MaxContext != 100 {
		t.Errorf("expected MaxContext 100, got %d", loaded.MaxContext)
	}

	if loaded.Policy.Level != "full" {
		t.Errorf("expected policy level 'full', got %s", loaded.Policy.Level)
	}

	if len(loaded.Policy.Allowlist) != 2 {
		t.Errorf("expected 2 allowlist entries, got %d", len(loaded.Policy.Allowlist))
	}
}

func TestLoadNonExistent(t *testing.T) {
	// Load should return defaults when config doesn't exist
	// Note: providers are now loaded from models.yaml, not hardcoded in defaults

	// Load should succeed with defaults
	loaded, err := Load()
	if err != nil {
		t.Fatalf("Load failed for non-existent config: %v", err)
	}

	// Should have default config values (providers may or may not exist depending on models.yaml)
	if loaded.MaxContext != 50 {
		t.Errorf("expected MaxContext 50, got %d", loaded.MaxContext)
	}

	if loaded.MaxIterations != 100 {
		t.Errorf("expected MaxIterations 100, got %d", loaded.MaxIterations)
	}
}

func TestEnvironmentVariableExpansion(t *testing.T) {
	// Set test env vars
	os.Setenv("TEST_SERVER_URL", "http://test-server:8080")
	os.Setenv("TEST_TOKEN", "expanded-token")
	defer os.Unsetenv("TEST_SERVER_URL")
	defer os.Unsetenv("TEST_TOKEN")

	tmpDir := t.TempDir()
	// Note: providers are loaded from models.yaml (yaml:"-" tag), not config.yaml
	// Test env expansion on fields that ARE loaded from config.yaml
	configContent := `
server_url: ${TEST_SERVER_URL}
token: ${TEST_TOKEN}
max_context: 75
`
	configPath := filepath.Join(tmpDir, "config.yaml")
	err := os.WriteFile(configPath, []byte(configContent), 0600)
	if err != nil {
		t.Fatalf("failed to write test config: %v", err)
	}

	loaded, err := LoadFrom(configPath)
	if err != nil {
		t.Fatalf("LoadFrom failed: %v", err)
	}

	if loaded.ServerURL != "http://test-server:8080" {
		t.Errorf("expected expanded ServerURL 'http://test-server:8080', got %s", loaded.ServerURL)
	}

	if loaded.Token != "expanded-token" {
		t.Errorf("expected expanded Token 'expanded-token', got %s", loaded.Token)
	}

	if loaded.MaxContext != 75 {
		t.Errorf("expected MaxContext 75, got %d", loaded.MaxContext)
	}
}

func TestCalculateCooldownDuration(t *testing.T) {
	tests := []struct {
		name       string
		errorCount int
		reason     ErrorReason
		minSeconds int
		maxSeconds int
	}{
		// First error: 60s
		{"first error rate_limit", 1, ErrorReasonRateLimit, 60, 60},
		{"first error billing", 1, ErrorReasonBilling, 60, 60},
		{"first error other", 1, ErrorReasonOther, 60, 60},

		// Second error: 300s (5min)
		{"second error rate_limit", 2, ErrorReasonRateLimit, 300, 300},

		// Third error: 1500s (25min)
		{"third error rate_limit", 3, ErrorReasonRateLimit, 1500, 1500},

		// Fourth error: 7500s but capped at 3600s (1h) for rate_limit
		{"fourth error rate_limit", 4, ErrorReasonRateLimit, 3600, 3600},

		// Billing errors have 24h max
		{"fourth error billing", 4, ErrorReasonBilling, 7500, 7500},

		// Timeout errors have 5min max
		{"second error timeout", 2, ErrorReasonTimeout, 300, 300},
		{"third error timeout", 3, ErrorReasonTimeout, 300, 300}, // capped at 300s

		// High error count should be capped
		{"many errors rate_limit", 10, ErrorReasonRateLimit, 3600, 3600},
		{"many errors billing", 10, ErrorReasonBilling, 86400, 86400}, // 24h
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			duration := calculateCooldownDuration(tt.errorCount, tt.reason)
			seconds := int(duration.Seconds())

			if seconds < tt.minSeconds || seconds > tt.maxSeconds {
				t.Errorf("calculateCooldownDuration(%d, %s) = %ds, expected between %d-%ds",
					tt.errorCount, tt.reason, seconds, tt.minSeconds, tt.maxSeconds)
			}
		})
	}
}
