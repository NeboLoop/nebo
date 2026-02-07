package config

import (
	"os"
	"path/filepath"
	"strings"

	"github.com/nebolabs/nebo/internal/defaults"
	"github.com/nebolabs/nebo/internal/provider"

	"gopkg.in/yaml.v3"
)

// Config holds the agent configuration
type Config struct {
	// Provider configuration loaded from models.yaml credentials
	Providers []ProviderConfig `yaml:"-"` // Not in config.yaml, loaded from models.yaml

	// Session settings
	DataDir    string `yaml:"data_dir"`    // Platform data directory
	MaxContext int    `yaml:"max_context"` // Max messages before compaction

	// Execution settings
	MaxIterations int `yaml:"max_iterations"` // Safety limit (default: 100)
	MaxTurns      int `yaml:"max_turns"`      // Max agentic turns per CLI provider request (0 = unlimited)

	// Tool settings
	Policy PolicyConfig `yaml:"policy"`

	// Lane concurrency settings (0 = unlimited)
	Lanes LaneConfig `yaml:"lanes"`

	// Advisors settings (internal deliberation system)
	Advisors AdvisorsConfig `yaml:"advisors"`

	// Context pruning settings (two-stage: soft trim + hard clear)
	ContextPruning ContextPruningConfig `yaml:"context_pruning"`

	// Comm plugin settings (inter-agent communication)
	Comm CommConfig `yaml:"comm"`

	// Memory settings (sanitization, limits)
	Memory MemorySettingsConfig `yaml:"memory"`

	// SaaS connection settings
	ServerURL string `yaml:"server_url"` // SaaS server URL
	Token     string `yaml:"token"`      // Authentication token
}

// CommConfig holds configuration for the comm lane plugin system
type CommConfig struct {
	Enabled     bool              `yaml:"enabled"`      // Enable comm lane
	Plugin      string            `yaml:"plugin"`       // Active plugin: "loopback", "mqtt", "nats"
	AutoConnect bool              `yaml:"auto_connect"` // Connect on agent startup
	AgentID     string            `yaml:"agent_id"`     // Agent identity (empty = hostname)
	Config      map[string]string `yaml:"config"`       // Plugin-specific config passed to Connect()
}

// MemorySettingsConfig holds settings for the memory subsystem
type MemorySettingsConfig struct {
	// SanitizeContent enables injection-pattern filtering on stored memory content (default: true)
	SanitizeContent bool `yaml:"sanitize_content"`
	// Embeddings enables vector embedding generation for stored memories (default: false)
	// When enabled, memories are embedded for semantic search; this incurs API costs.
	Embeddings bool `yaml:"embeddings"`
}

// LaneConfig holds concurrency limits for each lane
// 0 = unlimited, any positive number = max concurrent tasks
type LaneConfig struct {
	Main      int `yaml:"main"`      // User conversations (default: 1, serialized)
	Events    int `yaml:"events"`    // Scheduled/triggered tasks (default: 2)
	Subagent  int `yaml:"subagent"`  // Sub-agent operations (default: 0, unlimited)
	Nested    int `yaml:"nested"`    // Nested tool calls (default: 3)
	Heartbeat int `yaml:"heartbeat"` // Proactive heartbeat ticks (default: 1)
	Comm      int `yaml:"comm"`      // Inter-agent communication (default: 5)
}

// AdvisorsConfig holds settings for the internal deliberation system
type AdvisorsConfig struct {
	// Enabled controls whether advisors are consulted before responding
	// When enabled, the agent spawns internal "voices" to deliberate on tasks
	Enabled bool `yaml:"enabled"`

	// MaxAdvisors caps the number of advisors that run concurrently (default: 5)
	MaxAdvisors int `yaml:"max_advisors"`

	// TimeoutSeconds is the max time to wait for all advisors (default: 30)
	TimeoutSeconds int `yaml:"timeout_seconds"`
}

// ContextPruningConfig holds settings for the two-stage context pruning system.
// Stage 1 (soft trim): when total context chars exceed SoftTrimRatio of the budget,
// trim unprotected tool results to head+tail. Stage 2 (hard clear): when total chars
// still exceed HardClearRatio, replace unprotected tool results with a placeholder.
type ContextPruningConfig struct {
	ContextTokens        int     `yaml:"context_tokens"`        // Token budget estimate (default: 200000)
	SoftTrimRatio        float64 `yaml:"soft_trim_ratio"`       // Start soft trim at this fraction of budget (default: 0.3)
	HardClearRatio       float64 `yaml:"hard_clear_ratio"`      // Start hard clear at this fraction of budget (default: 0.5)
	KeepLastAssistant    int     `yaml:"keep_last_assistant"`   // Protect last N assistant messages from pruning (default: 3)
	SoftTrimMaxChars     int     `yaml:"soft_trim_max_chars"`   // Only trim results over this length (default: 4000)
	SoftTrimHead         int     `yaml:"soft_trim_head"`        // Chars to keep from start of result (default: 1500)
	SoftTrimTail         int     `yaml:"soft_trim_tail"`        // Chars to keep from end of result (default: 1500)
	HardClearPlaceholder string  `yaml:"hard_clear_placeholder"` // Placeholder for cleared results (default: "[Old tool result cleared]")
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
		MaxTurns:      0,
		Policy: PolicyConfig{
			Level:   "allowlist",
			AskMode: "on-miss",
			Allowlist: []string{
				"ls", "pwd", "cat", "head", "tail", "grep", "find",
				"jq", "cut", "sort", "uniq", "wc", "echo", "date",
				"git status", "git log", "git diff", "git branch",
			},
		},
		ContextPruning: ContextPruningConfig{
			ContextTokens:        200000,
			SoftTrimRatio:        0.3,
			HardClearRatio:       0.5,
			KeepLastAssistant:    3,
			SoftTrimMaxChars:     4000,
			SoftTrimHead:         1500,
			SoftTrimTail:         1500,
			HardClearPlaceholder: "[Old tool result cleared]",
		},
		Memory: MemorySettingsConfig{
			SanitizeContent: true,  // Enabled by default for security
			Embeddings:      false, // Disabled by default (incurs API costs)
		},
		ServerURL: "http://localhost:27895", // Default local server
	}
}

// DefaultDataDir returns the platform-appropriate data directory.
func DefaultDataDir() string {
	dir, err := defaults.DataDir()
	if err != nil {
		return ".nebo"
	}
	return dir
}

// Load loads config from the Nebo data directory's config.yaml
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

	// Expand ~ in DataDir (config file may have a tilde path)
	if strings.HasPrefix(cfg.DataDir, "~/") {
		home, _ := os.UserHomeDir()
		cfg.DataDir = filepath.Join(home, cfg.DataDir[2:])
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

	// Expand ~ in DataDir (config file may have a tilde path)
	if strings.HasPrefix(cfg.DataDir, "~/") {
		home, _ := os.UserHomeDir()
		cfg.DataDir = filepath.Join(home, cfg.DataDir[2:])
	}

	cfg.ServerURL = os.ExpandEnv(cfg.ServerURL)
	cfg.Token = os.ExpandEnv(cfg.Token)

	// Load providers from models.yaml
	cfg.loadProvidersFromModels()

	return cfg, nil
}

// Save saves the config to the Nebo data directory's config.yaml
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
// Uses <data_dir>/data/nebo.db to match the server's database location
func (c *Config) DBPath() string {
	return filepath.Join(c.DataDir, "data", "nebo.db")
}

// EnsureDataDir creates the data directory if it doesn't exist
func (c *Config) EnsureDataDir() error {
	return os.MkdirAll(c.DataDir, 0700)
}

// AdvisorsDir returns the path to the advisors directory
func (c *Config) AdvisorsDir() string {
	return filepath.Join(c.DataDir, "advisors")
}

// AdvisorsEnabled returns true if advisors are enabled and configured
func (c *Config) AdvisorsEnabled() bool {
	return c.Advisors.Enabled
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
