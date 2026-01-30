package config

import (
	"os"
	"path/filepath"
	"strings"

	"gobot/internal/provider"

	"gopkg.in/yaml.v3"
)

// Config holds the agent configuration
type Config struct {
	// Provider configuration loaded from models.yaml credentials
	Providers []ProviderConfig `yaml:"-"` // Not in config.yaml, loaded from models.yaml

	// Session settings
	DataDir    string `yaml:"data_dir"`    // ~/.gobot
	MaxContext int    `yaml:"max_context"` // Max messages before compaction

	// Execution settings
	MaxIterations int `yaml:"max_iterations"` // Safety limit (default: 100)

	// Tool settings
	Policy PolicyConfig `yaml:"policy"`

	// SaaS connection settings
	ServerURL string `yaml:"server_url"` // SaaS server URL
	Token     string `yaml:"token"`      // Authentication token
}

// ProviderConfig holds configuration for a single provider
type ProviderConfig struct {
	Name    string   `yaml:"name"`               // Identifier for this provider
	Type    string   `yaml:"type"`               // "api", "cli", or "ollama"
	APIKey  string   `yaml:"api_key,omitempty"`  // For API providers
	Model   string   `yaml:"model,omitempty"`    // Model to use
	Command string   `yaml:"command,omitempty"`  // For CLI providers (binary path)
	Args    []string `yaml:"args,omitempty"`     // Default CLI arguments
	BaseURL string   `yaml:"base_url,omitempty"` // For Ollama (default: http://localhost:11434)
}

// PolicyConfig holds approval policy settings
type PolicyConfig struct {
	Level     string   `yaml:"level"`     // "deny", "allowlist", "full"
	AskMode   string   `yaml:"ask_mode"`  // "off", "on-miss", "always"
	Allowlist []string `yaml:"allowlist"` // Approved command patterns
}

// DefaultConfig returns a config with sensible defaults
func DefaultConfig() *Config {
	return &Config{
		Providers:     []ProviderConfig{}, // Loaded from models.yaml
		DataDir:       DefaultDataDir(),
		MaxContext:    50,
		MaxIterations: 100,
		Policy: PolicyConfig{
			Level:   "allowlist",
			AskMode: "on-miss",
			Allowlist: []string{
				"ls", "pwd", "cat", "head", "tail", "grep", "find",
				"jq", "cut", "sort", "uniq", "wc", "echo", "date",
				"git status", "git log", "git diff", "git branch",
			},
		},
		ServerURL: "http://localhost:27895", // Default local dev server
	}
}

// DefaultDataDir returns the default data directory (~/.gobot)
func DefaultDataDir() string {
	home, err := os.UserHomeDir()
	if err != nil {
		return ".gobot"
	}
	return filepath.Join(home, ".gobot")
}

// Load loads config from ~/.gobot/config.yaml
func Load() (*Config, error) {
	cfg := DefaultConfig()

	configPath := filepath.Join(cfg.DataDir, "config.yaml")
	data, err := os.ReadFile(configPath)
	if err != nil {
		if os.IsNotExist(err) {
			// Config doesn't exist, use defaults
			return cfg, nil
		}
		return nil, err
	}

	if err := yaml.Unmarshal(data, cfg); err != nil {
		return nil, err
	}

	cfg.ServerURL = os.ExpandEnv(cfg.ServerURL)
	cfg.Token = os.ExpandEnv(cfg.Token)

	// Load providers from models.yaml
	cfg.loadProvidersFromModels()

	return cfg, nil
}

// LoadFrom loads config from a specific path
func LoadFrom(path string) (*Config, error) {
	cfg := DefaultConfig()

	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}

	if err := yaml.Unmarshal(data, cfg); err != nil {
		return nil, err
	}

	cfg.ServerURL = os.ExpandEnv(cfg.ServerURL)
	cfg.Token = os.ExpandEnv(cfg.Token)

	// Load providers from models.yaml
	cfg.loadProvidersFromModels()

	return cfg, nil
}

// Save saves the config to ~/.gobot/config.yaml
func (c *Config) Save() error {
	// Ensure data dir exists
	if err := os.MkdirAll(c.DataDir, 0700); err != nil {
		return err
	}

	data, err := yaml.Marshal(c)
	if err != nil {
		return err
	}

	configPath := filepath.Join(c.DataDir, "config.yaml")
	return os.WriteFile(configPath, data, 0600)
}

// DBPath returns the path to the SQLite database
// Uses ~/.gobot/data/gobot.db to match the server's database location
func (c *Config) DBPath() string {
	return filepath.Join(c.DataDir, "data", "gobot.db")
}

// EnsureDataDir creates the data directory if it doesn't exist
func (c *Config) EnsureDataDir() error {
	return os.MkdirAll(c.DataDir, 0700)
}

// GetProvider returns the provider config by name, or nil if not found
func (c *Config) GetProvider(name string) *ProviderConfig {
	for i := range c.Providers {
		if c.Providers[i].Name == name {
			return &c.Providers[i]
		}
	}
	return nil
}

// FirstValidProvider returns the first provider that appears configured
func (c *Config) FirstValidProvider() *ProviderConfig {
	for i := range c.Providers {
		p := &c.Providers[i]
		if p.Type == "cli" && p.Command != "" {
			return p
		}
		if p.Type == "api" && p.APIKey != "" {
			return p
		}
	}
	return nil
}

// loadProvidersFromModels loads provider credentials from models.yaml
func (c *Config) loadProvidersFromModels() {
	// Initialize the models store with the data directory
	provider.InitModelsStore(c.DataDir)

	// Get all credentials from models.yaml
	creds := provider.GetAllCredentials()
	if len(creds) == 0 {
		return
	}

	// Convert credentials to ProviderConfig entries
	for name, cred := range creds {
		providerType := "api"
		if cred.Command != "" {
			providerType = "cli"
		} else if name == "ollama" {
			providerType = "ollama"
		}

		// Get the first active model for this provider
		models := provider.GetProviderModels(name)
		var model string
		for _, m := range models {
			if m.IsActive() {
				model = m.ID
				break
			}
		}

		// Parse args string into slice
		var args []string
		if cred.Args != "" {
			args = strings.Fields(cred.Args)
		}

		c.Providers = append(c.Providers, ProviderConfig{
			Name:    name,
			Type:    providerType,
			APIKey:  os.ExpandEnv(cred.APIKey), // Expand env vars
			Model:   model,
			Command: cred.Command,
			Args:    args,
			BaseURL: os.ExpandEnv(cred.BaseURL), // Expand env vars
		})
	}
}
