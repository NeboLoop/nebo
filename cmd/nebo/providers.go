package cli

import (
	"context"
	"fmt"
	"os"
	"strings"

	"nebo/agent/ai"
	agentcfg "nebo/agent/config"
	"nebo/internal/provider"
)

var _ = agentcfg.Config{} // silence unused import if needed

// createProviders creates AI providers from config and database
func createProviders(cfg *agentcfg.Config) []ai.Provider {
	var providers []ai.Provider

	// First priority: API providers from database (UI-configured keys) - these support true streaming
	dbProviders := loadProvidersFromDB(cfg.DBPath())
	providers = append(providers, dbProviders...)

	// Then add providers from config (env vars, models.yaml)
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
				providers = append(providers, ai.NewClaudeCodeProvider())
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

	// Fallback: Claude Code CLI if no API providers were configured
	// CLI providers don't support true streaming but work as a last resort
	if len(providers) == 0 && ai.CheckCLIAvailable("claude") {
		providers = append(providers, ai.NewClaudeCodeProvider())
		fmt.Println("[Providers] Using Claude Code CLI as fallback (no API keys configured)")
	}

	return providers
}

// loadProvidersFromDB loads API providers from database auth profiles
// These are configured via the UI in Settings > Providers
func loadProvidersFromDB(dbPath string) []ai.Provider {
	var providers []ai.Provider

	mgr, err := agentcfg.NewAuthProfileManager(dbPath)
	if err != nil {
		if verbose {
			fmt.Fprintf(os.Stderr, "Warning: Could not load auth profiles from DB: %v\n", err)
		}
		return providers
	}
	defer mgr.Close()

	ctx := context.Background()

	// Load anthropic profiles
	if profiles, err := mgr.ListActiveProfiles(ctx, "anthropic"); err == nil {
		for _, p := range profiles {
			if p.APIKey != "" {
				model := p.Model
				if model == "" {
					model = provider.GetDefaultModel("anthropic")
				}
				providers = append(providers, ai.NewAnthropicProvider(p.APIKey, model))
				if verbose {
					fmt.Printf("Loaded Anthropic provider from DB: %s (model: %s)\n", p.Name, model)
				}
			}
		}
	}

	// Load openai profiles
	if profiles, err := mgr.ListActiveProfiles(ctx, "openai"); err == nil {
		for _, p := range profiles {
			if p.APIKey != "" {
				model := p.Model
				if model == "" {
					model = provider.GetDefaultModel("openai")
				}
				providers = append(providers, ai.NewOpenAIProvider(p.APIKey, model))
				if verbose {
					fmt.Printf("Loaded OpenAI provider from DB: %s (model: %s)\n", p.Name, model)
				}
			}
		}
	}

	// Load google/gemini profiles
	if profiles, err := mgr.ListActiveProfiles(ctx, "google"); err == nil {
		for _, p := range profiles {
			if p.APIKey != "" {
				model := p.Model
				if model == "" {
					model = provider.GetDefaultModel("google")
				}
				providers = append(providers, ai.NewGeminiProvider(p.APIKey, model))
				if verbose {
					fmt.Printf("Loaded Gemini provider from DB: %s (model: %s)\n", p.Name, model)
				}
			}
		}
	}

	// Load ollama profiles
	if profiles, err := mgr.ListActiveProfiles(ctx, "ollama"); err == nil {
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
				providers = append(providers, ai.NewOllamaProvider(baseURL, model))
				if verbose {
					fmt.Printf("Loaded Ollama provider from DB: %s (model: %s)\n", p.Name, model)
				}
			}
		}
	}

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

