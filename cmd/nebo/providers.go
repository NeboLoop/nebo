package cli

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"os"
	"strings"

	"github.com/neboloop/nebo/internal/agent/ai"
	agentcfg "github.com/neboloop/nebo/internal/agent/config"
	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/provider"
)

var _ = agentcfg.Config{} // silence unused import if needed

// Shared database connection for provider loading
var sharedDB *sql.DB

// SetSharedDB sets the shared database connection for provider loading
func SetSharedDB(db *sql.DB) {
	sharedDB = db
}

// Janus URL for NeboLoop provider (set from config via SetJanusURL)
var sharedJanusURL string

// SetJanusURL overrides the Janus gateway URL (e.g. for local dev)
func SetJanusURL(url string) {
	if url != "" {
		sharedJanusURL = url
	}
}

// createProviders creates AI providers from config and database
func createProviders(cfg *agentcfg.Config) []ai.Provider {
	var providers []ai.Provider

	// NOTE: We do NOT call provider.InitModelsStore here because:
	// 1. It's already initialized during server startup in servicecontext.go
	// 2. Calling it here would trigger ReloadModels() → OnConfigReload callbacks
	//    → r.ReloadProviders() → createProviders() → INFINITE RECURSION!
	// The models config is already loaded and available via GetModelsConfig()

	// Primary source: API providers from database (UI-configured keys)
	// This is the ONLY supported method for non-technical users
	// Use the shared DB connection
	dbProviders := loadProvidersFromDB(sharedDB)
	providers = append(providers, dbProviders...)

	if len(dbProviders) > 0 {
		fmt.Printf("[Providers] Loaded %d provider(s) from database\n", len(dbProviders))
	}

	// Secondary source: Providers from config file (for advanced users/developers)
	for _, pcfg := range cfg.Providers {
		if providerArg != "" && pcfg.Name != providerArg {
			continue
		}

		switch pcfg.Type {
		case "api":
			if pcfg.APIKey == "" {
				continue
			}
			switch {
			case strings.Contains(pcfg.Name, "anthropic") || pcfg.Name == "claude":
				providers = append(providers, ai.NewAnthropicProvider(pcfg.APIKey, pcfg.Model))
			case strings.Contains(pcfg.Name, "openai") || pcfg.Name == "gpt":
				providers = append(providers, ai.NewOpenAIProvider(pcfg.APIKey, pcfg.Model))
			case strings.Contains(pcfg.Name, "gemini") || strings.Contains(pcfg.Name, "google"):
				providers = append(providers, ai.NewGeminiProvider(pcfg.APIKey, pcfg.Model))
			}

		case "ollama":
			baseURL := pcfg.BaseURL
			if baseURL == "" {
				baseURL = "http://localhost:11434"
			}
			if ai.CheckOllamaAvailable(baseURL) {
				// Auto-pull the model if not present
				if pcfg.Model != "" {
					if err := ai.EnsureOllamaModel(baseURL, pcfg.Model); err != nil {
						fmt.Printf("[Providers] Warning: could not ensure Ollama model %s: %v\n", pcfg.Model, err)
					}
				}
				providers = append(providers, ai.NewOllamaProvider(baseURL, pcfg.Model))
			} else if verbose {
				fmt.Fprintf(os.Stderr, "Ollama provider %s: not available at %s\n", pcfg.Name, baseURL)
			}

		case "cli":
			if pcfg.Command == "" {
				continue
			}
			if !ai.CheckCLIAvailable(pcfg.Command) {
				if verbose {
					fmt.Fprintf(os.Stderr, "CLI provider %s: command '%s' not found in PATH\n", pcfg.Name, pcfg.Command)
				}
				continue
			}
			// Use factory functions for known CLIs to ensure correct flags
			switch pcfg.Command {
			case "claude":
				providers = append(providers, ai.NewClaudeCodeProvider(cfg.MaxTurns, 0))
			case "codex":
				providers = append(providers, ai.NewCodexCLIProvider())
			case "gemini":
				providers = append(providers, ai.NewGeminiCLIProvider())
			default:
				providers = append(providers, ai.NewCLIProvider(pcfg.Name, pcfg.Command, pcfg.Args))
			}

		default:
			if pcfg.APIKey != "" {
				if strings.Contains(pcfg.Name, "openai") || strings.Contains(pcfg.Name, "gpt") {
					providers = append(providers, ai.NewOpenAIProvider(pcfg.APIKey, pcfg.Model))
				} else if strings.Contains(pcfg.Name, "gemini") || strings.Contains(pcfg.Name, "google") {
					providers = append(providers, ai.NewGeminiProvider(pcfg.APIKey, pcfg.Model))
				} else {
					providers = append(providers, ai.NewAnthropicProvider(pcfg.APIKey, pcfg.Model))
				}
			}
		}
	}

	// Add active CLI providers
	for _, cli := range provider.GetAvailableCLIProviders() {
		if !cli.Active || !cli.Installed {
			continue
		}
		switch cli.Command {
		case "claude":
			providers = append(providers, ai.NewClaudeCodeProvider(cfg.MaxTurns, 0))
		case "codex":
			providers = append(providers, ai.NewCodexCLIProvider())
		case "gemini":
			providers = append(providers, ai.NewGeminiCLIProvider())
		default:
			providers = append(providers, ai.NewCLIProvider(cli.ID, cli.Command, nil))
		}
		fmt.Printf("[Providers] Added %s CLI provider\n", cli.DisplayName)
	}

	if len(providers) == 0 {
		fmt.Println("[Providers] No API providers configured!")
		fmt.Println("[Providers] Please configure an API key in the web UI: Settings > Providers")
		fmt.Println("[Providers] Visit http://localhost:27895/settings/providers to add your API key")
	}

	return providers
}

// loadProvidersFromDB loads API providers from database auth profiles
// These are configured via the UI in Settings > Providers
// Accepts a shared *sql.DB connection - does NOT close it
func loadProvidersFromDB(db *sql.DB) []ai.Provider {
	var providers []ai.Provider

	if db == nil {
		fmt.Printf("[Providers] Warning: No database connection for loading auth profiles\n")
		return providers
	}

	fmt.Printf("[Providers] Creating auth profile manager with shared DB\n")
	mgr, err := agentcfg.NewAuthProfileManager(db)
	if err != nil {
		fmt.Printf("[Providers] Warning: Could not load auth profiles from DB: %v\n", err)
		return providers
	}
	defer mgr.Close() // No-op since we use shared connection
	fmt.Printf("[Providers] Auth profile manager created successfully\n")

	ctx := context.Background()

	// Load anthropic profiles
	if profiles, err := mgr.ListAllActiveProfiles(ctx, "anthropic"); err == nil {
		fmt.Printf("[Providers] Found %d anthropic profiles\n", len(profiles))
		for _, p := range profiles {
			if p.APIKey != "" {
				model := p.Model
				if model == "" {
					model = provider.GetDefaultModel("anthropic")
				}
				// Wrap with ProfiledProvider for per-request profile tracking
				baseProvider := ai.NewAnthropicProvider(p.APIKey, model)
				providers = append(providers, ai.NewProfiledProvider(baseProvider, p.ID))
				fmt.Printf("[Providers] Loaded Anthropic provider: %s (model: %s, profileID: %s)\n", p.Name, model, p.ID)
			}
		}
	} else {
		fmt.Printf("[Providers] Error loading anthropic profiles: %v\n", err)
	}

	// Load openai profiles
	if profiles, err := mgr.ListAllActiveProfiles(ctx, "openai"); err == nil {
		fmt.Printf("[Providers] Found %d openai profiles\n", len(profiles))
		for _, p := range profiles {
			if p.APIKey != "" {
				model := p.Model
				if model == "" {
					model = provider.GetDefaultModel("openai")
				}
				// Wrap with ProfiledProvider for per-request profile tracking
				baseProvider := ai.NewOpenAIProvider(p.APIKey, model)
				providers = append(providers, ai.NewProfiledProvider(baseProvider, p.ID))
				fmt.Printf("[Providers] Loaded OpenAI provider: %s (model: %s, profileID: %s)\n", p.Name, model, p.ID)
			}
		}
	} else {
		fmt.Printf("[Providers] Error loading openai profiles: %v\n", err)
	}

	// Load google/gemini profiles
	if profiles, err := mgr.ListAllActiveProfiles(ctx, "google"); err == nil {
		fmt.Printf("[Providers] Found %d google profiles\n", len(profiles))
		for _, p := range profiles {
			if p.APIKey != "" {
				model := p.Model
				if model == "" {
					model = provider.GetDefaultModel("google")
				}
				// Wrap with ProfiledProvider for per-request profile tracking
				baseProvider := ai.NewGeminiProvider(p.APIKey, model)
				providers = append(providers, ai.NewProfiledProvider(baseProvider, p.ID))
				fmt.Printf("[Providers] Loaded Gemini provider: %s (model: %s, profileID: %s)\n", p.Name, model, p.ID)
			}
		}
	} else {
		fmt.Printf("[Providers] Error loading google profiles: %v\n", err)
	}

	// Load ollama profiles
	if profiles, err := mgr.ListAllActiveProfiles(ctx, "ollama"); err == nil {
		fmt.Printf("[Providers] Found %d ollama profiles\n", len(profiles))
		for _, p := range profiles {
			baseURL := p.BaseURL
			if baseURL == "" {
				baseURL = "http://localhost:11434"
			}
			if ai.CheckOllamaAvailable(baseURL) {
				model := p.Model
				if model == "" {
					model = provider.GetDefaultModel("ollama")
				}
				// Auto-pull the chat model if not present
				if err := ai.EnsureOllamaModel(baseURL, model); err != nil {
					fmt.Printf("[Providers] Warning: could not ensure Ollama model %s: %v\n", model, err)
				}
				// Wrap with ProfiledProvider for per-request profile tracking
				baseProvider := ai.NewOllamaProvider(baseURL, model)
				providers = append(providers, ai.NewProfiledProvider(baseProvider, p.ID))
				fmt.Printf("[Providers] Loaded Ollama provider: %s (model: %s, profileID: %s)\n", p.Name, model, p.ID)
			}
		}
	} else {
		fmt.Printf("[Providers] Error loading ollama profiles: %v\n", err)
	}

	// Load neboloop/janus provider — only when the user explicitly opted in.
	// A neboloop auth profile always exists for comms/app store, but it should
	// only create a Janus AI provider when metadata has janus_provider=true.
	if profiles, err := mgr.ListAllActiveProfiles(ctx, "neboloop"); err == nil {
		fmt.Printf("[Providers] Found %d neboloop profiles\n", len(profiles))

		// Read bot_id for X-Bot-ID header (required by Janus for per-bot billing)
		var botID string
		if db != nil {
			_ = db.QueryRowContext(ctx,
				`SELECT ps.setting_value FROM plugin_settings ps
				 JOIN plugin_registry pr ON pr.id = ps.plugin_id
				 WHERE pr.name = 'neboloop' AND ps.setting_key = 'bot_id'`,
			).Scan(&botID)
		}

		for _, p := range profiles {
			if p.APIKey == "" || sharedJanusURL == "" {
				continue
			}
			if p.Metadata["janus_provider"] != "true" {
				fmt.Printf("[Providers] Skipping Janus for neboloop profile %s (janus_provider not enabled)\n", p.Name)
				continue
			}
			model := provider.GetDefaultModel("janus")
			if model == "" {
				model = "janus"
			}
			baseProvider := ai.NewOpenAIProvider(p.APIKey, model, sharedJanusURL+"/v1")
			baseProvider.SetProviderID("janus")
			if botID != "" {
				baseProvider.SetBotID(botID)
			}
			providers = append(providers, ai.NewProfiledProvider(baseProvider, p.ID))
			fmt.Printf("[Providers] Loaded Janus provider: %s (model: %s, baseURL: %s, botID: %s, profileID: %s)\n", p.Name, model, sharedJanusURL, botID, p.ID)
		}
	} else {
		fmt.Printf("[Providers] Error loading neboloop profiles: %v\n", err)
	}

	fmt.Printf("[Providers] Total providers from DB: %d\n", len(providers))
	return providers
}

// CLIAvailability represents which CLI tools are installed on the system
type CLIAvailability struct {
	Claude bool `json:"claude"`
	Codex  bool `json:"codex"`
	Gemini bool `json:"gemini"`
}

// DetectAvailableCLIs checks which CLI tools are installed and available
// This is used to inform the UI what can be configured, not to auto-create providers
func DetectAvailableCLIs() *CLIAvailability {
	return &CLIAvailability{
		Claude: ai.CheckCLIAvailable("claude"),
		Codex:  ai.CheckCLIAvailable("codex"),
		Gemini: ai.CheckCLIAvailable("gemini"),
	}
}

// loadToolPermissions loads capability permissions from the database for the default user.
// Returns nil on error (nil means "register all tools" - safe fallback).
func loadToolPermissions(sqlDB *sql.DB) map[string]bool {
	if sqlDB == nil {
		return nil
	}

	queries := db.New(sqlDB)
	permJSON, err := queries.GetToolPermissions(context.Background(), "default-user")
	if err != nil {
		fmt.Printf("[Permissions] Could not load tool permissions: %v (using defaults)\n", err)
		return nil
	}

	var permissions map[string]bool
	if err := json.Unmarshal([]byte(permJSON), &permissions); err != nil {
		fmt.Printf("[Permissions] Could not parse tool permissions: %v (using defaults)\n", err)
		return nil
	}

	// Empty map means no permissions set yet — register all tools
	if len(permissions) == 0 {
		return nil
	}

	// Migrate old defaults: if the only enabled permission is "chat" and all others
	// are false, the user likely went through onboarding with the old restrictive
	// defaults. Upgrade to the new sensible defaults.
	onlyChat := true
	for key, val := range permissions {
		if key != "chat" && val {
			onlyChat = false
			break
		}
	}
	if onlyChat && permissions["chat"] {
		fmt.Println("[Permissions] Detected old defaults (only chat enabled) — upgrading to sensible defaults")
		permissions["file"] = true
		permissions["web"] = true
		permissions["desktop"] = true
		permissions["system"] = true
		// Persist the upgrade so it doesn't run again
		if data, err := json.Marshal(permissions); err == nil {
			_ = queries.UpdateToolPermissions(context.Background(), db.UpdateToolPermissionsParams{
				ToolPermissions: sql.NullString{String: string(data), Valid: true},
				UserID:          "default-user",
			})
		}
	}

	// Backfill any missing keys from the current defaults (handles future additions)
	defaults := map[string]bool{
		"chat": true, "file": true, "shell": false, "web": true,
		"contacts": false, "desktop": true, "media": false, "system": true,
	}
	for key, defVal := range defaults {
		if _, exists := permissions[key]; !exists {
			permissions[key] = defVal
		}
	}

	fmt.Printf("[Permissions] Loaded tool permissions: %v\n", permissions)
	return permissions
}

