package provider

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"time"

	"github.com/fsnotify/fsnotify"
	"gopkg.in/yaml.v3"
)

// ModelPricing describes pricing per million tokens
type ModelPricing struct {
	Input       float64 `json:"input,omitempty" yaml:"input,omitempty"`             // $ per 1M input tokens
	Output      float64 `json:"output,omitempty" yaml:"output,omitempty"`           // $ per 1M output tokens
	CachedInput float64 `json:"cachedInput,omitempty" yaml:"cachedInput,omitempty"` // $ per 1M cached input tokens
}

// ModelInfo describes an AI model
type ModelInfo struct {
	ID            string        `json:"id" yaml:"id"`
	DisplayName   string        `json:"displayName" yaml:"displayName"`
	ContextWindow int           `json:"contextWindow" yaml:"contextWindow"`
	Pricing       *ModelPricing `json:"pricing,omitempty" yaml:"pricing,omitempty"`
	Capabilities  []string      `json:"capabilities,omitempty" yaml:"capabilities,omitempty"`
	Kind          []string      `json:"kind,omitempty" yaml:"kind,omitempty"`               // Semantic tags: fast, smart, code, cheap, etc.
	Preferred     bool          `json:"preferred,omitempty" yaml:"preferred,omitempty"`     // User's preferred model for this kind
	Active        *bool         `json:"active,omitempty" yaml:"active,omitempty"`           // nil = true (default active)
}

// IsActive returns whether the model is active (defaults to true)
func (m *ModelInfo) IsActive() bool {
	if m.Active == nil {
		return true
	}
	return *m.Active
}

// ProviderCredentials holds API credentials for a provider
type ProviderCredentials struct {
	APIKey  string `json:"apiKey,omitempty" yaml:"api_key,omitempty"`
	BaseURL string `json:"baseUrl,omitempty" yaml:"base_url,omitempty"` // For Ollama or custom endpoints
	Command string `json:"command,omitempty" yaml:"command,omitempty"` // For CLI providers
	Args    string `json:"args,omitempty" yaml:"args,omitempty"`       // CLI args
}

// TaskRouting defines which models to use for different task types
type TaskRouting struct {
	Vision    string              `yaml:"vision" json:"vision"`
	Audio     string              `yaml:"audio" json:"audio"`
	Reasoning string              `yaml:"reasoning" json:"reasoning"`
	Code      string              `yaml:"code" json:"code"`
	General   string              `yaml:"general" json:"general"`
	Fallbacks map[string][]string `yaml:"fallbacks,omitempty" json:"fallbacks,omitempty"`
}

// Defaults defines default model selection
type Defaults struct {
	Primary   string   `yaml:"primary" json:"primary"`
	Fallbacks []string `yaml:"fallbacks,omitempty" json:"fallbacks,omitempty"`
}

// ModelAlias maps a user-friendly alias to a model ID
type ModelAlias struct {
	Alias   string `yaml:"alias" json:"alias"`
	ModelId string `yaml:"modelId" json:"modelId"`
}

// ModelsConfig is the YAML structure for storing provider models
// The agent populates this file itself using its tools (web search, memory, etc.)
type ModelsConfig struct {
	Version     string                         `yaml:"version"`
	UpdatedAt   string                         `yaml:"updatedAt"`
	Credentials map[string]ProviderCredentials `yaml:"credentials,omitempty"`
	Defaults    *Defaults                      `yaml:"defaults,omitempty"`
	TaskRouting *TaskRouting                   `yaml:"task_routing,omitempty"`
	Aliases     []ModelAlias                   `yaml:"aliases,omitempty"`
	Providers   map[string][]ModelInfo         `yaml:"providers"`
}

// Singleton instance
var (
	modelsInstance *ModelsConfig
	modelsOnce     sync.Once
	modelsMu       sync.RWMutex
	modelsFilePath string

	// File watcher
	configWatcher    *fsnotify.Watcher
	reloadCallbacks  []func(*ModelsConfig)
	callbacksMu      sync.RWMutex
)

// InitModelsStore sets up the models YAML file path and loads the singleton
func InitModelsStore(dataDir string) {
	modelsFilePath = filepath.Join(dataDir, "models.yaml")
	// Initialize the singleton from YAML
	ReloadModels()
}

// GetModelsFilePath returns the current models file path
func GetModelsFilePath() string {
	if modelsFilePath == "" {
		// Default to ~/.nebo/models.yaml
		home, _ := os.UserHomeDir()
		modelsFilePath = filepath.Join(home, ".nebo", "models.yaml")
	}
	return modelsFilePath
}

// GetModelsConfig returns the singleton instance, loading from YAML on first call
func GetModelsConfig() *ModelsConfig {
	modelsOnce.Do(func() {
		modelsInstance = loadFromYAML()
	})
	modelsMu.RLock()
	defer modelsMu.RUnlock()
	return modelsInstance
}

// ReloadModels reloads the config from YAML (call when file changes)
func ReloadModels() {
	modelsMu.Lock()
	modelsInstance = loadFromYAML()
	modelsMu.Unlock()

	// Notify all registered callbacks
	callbacksMu.RLock()
	callbacks := make([]func(*ModelsConfig), len(reloadCallbacks))
	copy(callbacks, reloadCallbacks)
	callbacksMu.RUnlock()

	for _, cb := range callbacks {
		cb(modelsInstance)
	}
}

// OnConfigReload registers a callback to be called when the config is reloaded
func OnConfigReload(callback func(*ModelsConfig)) {
	callbacksMu.Lock()
	defer callbacksMu.Unlock()
	reloadCallbacks = append(reloadCallbacks, callback)
}

// StartConfigWatcher starts watching the ~/.nebo directory for config changes
func StartConfigWatcher(dataDir string) error {
	watcher, err := fsnotify.NewWatcher()
	if err != nil {
		return fmt.Errorf("failed to create watcher: %w", err)
	}
	configWatcher = watcher

	// Watch the data directory
	if err := watcher.Add(dataDir); err != nil {
		return fmt.Errorf("failed to watch directory %s: %w", dataDir, err)
	}

	// Also watch models.yaml specifically if it exists
	modelsPath := filepath.Join(dataDir, "models.yaml")
	if _, err := os.Stat(modelsPath); err == nil {
		if err := watcher.Add(modelsPath); err != nil {
			fmt.Printf("[config] Warning: could not watch models.yaml directly: %v\n", err)
		}
	}

	go func() {
		var debounceTimer *time.Timer
		for {
			select {
			case event, ok := <-watcher.Events:
				if !ok {
					return
				}
				// Only care about models.yaml changes
				if filepath.Base(event.Name) != "models.yaml" {
					continue
				}
				if event.Op&(fsnotify.Write|fsnotify.Create) != 0 {
					// Debounce: wait 100ms before reloading (editors may write multiple times)
					if debounceTimer != nil {
						debounceTimer.Stop()
					}
					debounceTimer = time.AfterFunc(100*time.Millisecond, func() {
						fmt.Printf("[config] models.yaml changed, reloading...\n")
						ReloadModels()
						fmt.Printf("[config] models.yaml reloaded successfully\n")
					})
				}
			case err, ok := <-watcher.Errors:
				if !ok {
					return
				}
				fmt.Printf("[config] Watcher error: %v\n", err)
			}
		}
	}()

	fmt.Printf("[config] Watching %s for changes\n", dataDir)
	return nil
}

// StopConfigWatcher stops the file watcher
func StopConfigWatcher() {
	if configWatcher != nil {
		configWatcher.Close()
		configWatcher = nil
	}
}

// loadFromYAML reads the YAML file
func loadFromYAML() *ModelsConfig {
	path := GetModelsFilePath()
	data, err := os.ReadFile(path)
	if err != nil {
		// Return empty config - agent will populate the file
		return &ModelsConfig{
			Version:   "1.0",
			UpdatedAt: time.Now().Format(time.RFC3339),
			Providers: make(map[string][]ModelInfo),
		}
	}

	var config ModelsConfig
	if err := yaml.Unmarshal(data, &config); err != nil {
		return &ModelsConfig{
			Version:   "1.0",
			UpdatedAt: time.Now().Format(time.RFC3339),
			Providers: make(map[string][]ModelInfo),
		}
	}

	if config.Providers == nil {
		config.Providers = make(map[string][]ModelInfo)
	}

	return &config
}

// LoadModels returns the singleton (for backwards compatibility)
func LoadModels() (*ModelsConfig, error) {
	return GetModelsConfig(), nil
}

// SaveModels saves the singleton to YAML
func SaveModels(config *ModelsConfig) error {
	config.UpdatedAt = time.Now().Format(time.RFC3339)

	path := GetModelsFilePath()
	if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
		return err
	}

	data, err := yaml.Marshal(config)
	if err != nil {
		return err
	}

	// Update singleton
	modelsInstance = config
	return os.WriteFile(path, data, 0644)
}

// GetProviderModels returns models for a provider from the singleton
func GetProviderModels(providerType string) []ModelInfo {
	config := GetModelsConfig()
	return config.Providers[providerType]
}

// SetProviderModels updates models for a provider in the singleton and saves to YAML
func SetProviderModels(providerType string, models []ModelInfo) error {
	modelsMu.Lock()
	defer modelsMu.Unlock()

	if modelsInstance == nil {
		modelsInstance = loadFromYAML()
	}
	modelsInstance.Providers[providerType] = models
	return SaveModels(modelsInstance)
}

// ClearModelsCache reloads the singleton from YAML
func ClearModelsCache() {
	ReloadModels()
}

// GetCredentials returns credentials for a provider type
func GetCredentials(providerType string) *ProviderCredentials {
	config := GetModelsConfig()
	if config.Credentials == nil {
		return nil
	}
	creds, ok := config.Credentials[providerType]
	if !ok {
		return nil
	}
	return &creds
}

// GetAllCredentials returns all configured credentials
func GetAllCredentials() map[string]ProviderCredentials {
	config := GetModelsConfig()
	if config.Credentials == nil {
		return make(map[string]ProviderCredentials)
	}
	return config.Credentials
}

// SetModelActive sets the active status of a specific model
func SetModelActive(providerType, modelID string, active bool) error {
	modelsMu.Lock()
	defer modelsMu.Unlock()

	if modelsInstance == nil {
		modelsInstance = loadFromYAML()
	}

	models, ok := modelsInstance.Providers[providerType]
	if !ok {
		return nil // Provider not found, nothing to do
	}

	for i := range models {
		if models[i].ID == modelID {
			models[i].Active = &active
			modelsInstance.Providers[providerType] = models
			return SaveModels(modelsInstance)
		}
	}

	return nil // Model not found, nothing to do
}

// ModelUpdate contains fields to update on a model
type ModelUpdate struct {
	Active    *bool
	Kind      []string
	Preferred *bool
}

// UpdateModel updates a model's settings (active, kind, preferred)
func UpdateModel(providerType, modelID string, update ModelUpdate) error {
	modelsMu.Lock()
	defer modelsMu.Unlock()

	if modelsInstance == nil {
		modelsInstance = loadFromYAML()
	}

	models, ok := modelsInstance.Providers[providerType]
	if !ok {
		return nil // Provider not found, nothing to do
	}

	for i := range models {
		if models[i].ID == modelID {
			if update.Active != nil {
				models[i].Active = update.Active
			}
			if update.Kind != nil {
				models[i].Kind = update.Kind
			}
			if update.Preferred != nil {
				models[i].Preferred = *update.Preferred
			}
			modelsInstance.Providers[providerType] = models
			return SaveModels(modelsInstance)
		}
	}

	return nil // Model not found, nothing to do
}

// ============================================
// CLI PROVIDER DETECTION
// ============================================

// CLIProviderInfo describes an available CLI provider
type CLIProviderInfo struct {
	ID          string   `json:"id"`          // e.g., "claude-code"
	DisplayName string   `json:"displayName"` // e.g., "Claude Code CLI"
	Command     string   `json:"command"`     // e.g., "claude"
	Installed   bool     `json:"installed"`   // true if command found in PATH
	Path        string   `json:"path"`        // Full path to command (if installed)
	InstallHint string   `json:"installHint"` // e.g., "brew install claude-code"
	Models      []string `json:"models"`      // Available model aliases
}

// KnownCLIProviders defines the CLI providers we support
var KnownCLIProviders = []CLIProviderInfo{
	{
		ID:          "claude-code",
		DisplayName: "Claude Code CLI",
		Command:     "claude",
		InstallHint: "brew install claude-code",
		Models:      []string{"opus", "sonnet", "haiku"},
	},
	{
		ID:          "codex-cli",
		DisplayName: "OpenAI Codex CLI",
		Command:     "codex",
		InstallHint: "npm i -g @openai/codex",
		Models:      []string{"gpt-5.2", "o3", "o4-mini"},
	},
	{
		ID:          "gemini-cli",
		DisplayName: "Gemini CLI",
		Command:     "gemini",
		InstallHint: "npm i -g @google/gemini-cli",
		Models:      []string{"gemini-3-flash", "gemini-3-pro"},
	},
}

// IsCLIProvider returns true if the provider ID is a CLI provider
func IsCLIProvider(providerID string) bool {
	for _, p := range KnownCLIProviders {
		if p.ID == providerID {
			return true
		}
	}
	return false
}

// CheckCLIInstalled checks if a CLI command is available in PATH
func CheckCLIInstalled(command string) (bool, string) {
	path, err := exec.LookPath(command)
	if err != nil {
		return false, ""
	}
	return true, path
}

// GetAvailableCLIProviders returns all CLI providers with installation status
func GetAvailableCLIProviders() []CLIProviderInfo {
	result := make([]CLIProviderInfo, len(KnownCLIProviders))
	for i, p := range KnownCLIProviders {
		result[i] = p
		result[i].Installed, result[i].Path = CheckCLIInstalled(p.Command)
	}
	return result
}

// GetInstalledCLIProviders returns only the installed CLI providers
func GetInstalledCLIProviders() []CLIProviderInfo {
	var result []CLIProviderInfo
	for _, p := range KnownCLIProviders {
		installed, path := CheckCLIInstalled(p.Command)
		if installed {
			p.Installed = true
			p.Path = path
			result = append(result, p)
		}
	}
	return result
}

// GetCLIProviderByID returns a CLI provider by ID (with installation status)
func GetCLIProviderByID(id string) *CLIProviderInfo {
	for _, p := range KnownCLIProviders {
		if p.ID == id {
			p.Installed, p.Path = CheckCLIInstalled(p.Command)
			return &p
		}
	}
	return nil
}

// GetDefaultModel returns the default model for a provider from models.yaml
// Returns empty string if not configured - callers should handle this appropriately
func GetDefaultModel(providerType string) string {
	config := GetModelsConfig()

	// Try to get from defaults.primary (if it matches this provider)
	if config.Defaults != nil && config.Defaults.Primary != "" {
		parts := splitModelID(config.Defaults.Primary)
		if len(parts) == 2 && parts[0] == providerType {
			return parts[1]
		}
	}

	// Try to get first active model for this provider from the providers list
	models := config.Providers[providerType]
	for _, m := range models {
		if m.IsActive() {
			return m.ID
		}
	}

	// No model found in config
	return ""
}

// splitModelID splits "provider/model" into parts
func splitModelID(modelID string) []string {
	for i := 0; i < len(modelID); i++ {
		if modelID[i] == '/' {
			return []string{modelID[:i], modelID[i+1:]}
		}
	}
	return []string{modelID}
}
