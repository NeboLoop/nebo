package provider

import (
	"net/http"

	"github.com/neboloop/nebo/internal/agent/ai"
	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/provider"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
)

// List all available models from YAML cache
func ListModelsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		config := provider.GetModelsConfig()

		result := make(map[string][]types.ModelInfo)
		for providerType, models := range config.Providers {
			modelList := make([]types.ModelInfo, len(models))
			for i, m := range models {
				info := types.ModelInfo{
					Id:            m.ID,
					DisplayName:   m.DisplayName,
					ContextWindow: m.ContextWindow,
					Capabilities:  m.Capabilities,
					Kind:          m.Kind,
					Preferred:     m.Preferred,
					IsActive:      m.IsActive(),
				}
				if m.Pricing != nil {
					info.Pricing = &types.ModelPricing{
						Input:       m.Pricing.Input,
						Output:      m.Pricing.Output,
						CachedInput: m.Pricing.CachedInput,
					}
				}
				modelList[i] = info
			}
			result[providerType] = modelList
		}

		// Include task routing if configured
		var taskRouting *types.TaskRouting
		if config.TaskRouting != nil {
			taskRouting = &types.TaskRouting{
				Vision:    config.TaskRouting.Vision,
				Audio:     config.TaskRouting.Audio,
				Reasoning: config.TaskRouting.Reasoning,
				Code:      config.TaskRouting.Code,
				General:   config.TaskRouting.General,
				Fallbacks: config.TaskRouting.Fallbacks,
			}
		}

		// Include aliases if configured
		var aliases []types.ModelAlias
		for _, a := range config.Aliases {
			aliases = append(aliases, types.ModelAlias{
				Alias:   a.Alias,
				ModelId: a.ModelId,
			})
		}

		// Detect available CLI tools (legacy - just installed check)
		cliAvailability := &types.CLIAvailability{
			Claude: ai.CheckCLIAvailable("claude"),
			Codex:  ai.CheckCLIAvailable("codex"),
			Gemini: ai.CheckCLIAvailable("gemini"),
		}

		// Get detailed CLI status (installed + authenticated)
		cliStatusMap := ai.GetAllCLIStatuses()
		cliStatuses := &types.CLIStatusMap{
			Claude: types.CLIStatus{
				Installed:     cliStatusMap["claude"].Installed,
				Authenticated: cliStatusMap["claude"].Authenticated,
				Version:       cliStatusMap["claude"].Version,
			},
			Codex: types.CLIStatus{
				Installed:     cliStatusMap["codex"].Installed,
				Authenticated: cliStatusMap["codex"].Authenticated,
				Version:       cliStatusMap["codex"].Version,
			},
			Gemini: types.CLIStatus{
				Installed:     cliStatusMap["gemini"].Installed,
				Authenticated: cliStatusMap["gemini"].Authenticated,
				Version:       cliStatusMap["gemini"].Version,
			},
		}

		// Include CLI providers from models.yaml config
		var cliProviders []types.CLIProviderInfo
		for _, cp := range provider.GetAvailableCLIProviders() {
			cliProviders = append(cliProviders, types.CLIProviderInfo{
				ID:           cp.ID,
				DisplayName:  cp.DisplayName,
				Command:      cp.Command,
				InstallHint:  cp.InstallHint,
				Models:       cp.Models,
				DefaultModel: cp.DefaultModel,
			})
		}

		httputil.OkJSON(w, &types.ListModelsResponse{
			Models:        result,
			TaskRouting:   taskRouting,
			Aliases:       aliases,
			AvailableCLIs: cliAvailability,
			CLIStatuses:   cliStatuses,
			CLIProviders:  cliProviders,
		})
	}
}
