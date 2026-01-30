package provider

import (
	"net/http"

	"gobot/agent/ai"
	"gobot/internal/httputil"
	"gobot/internal/provider"
	"gobot/internal/svc"
	"gobot/internal/types"
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

		// Detect available CLI tools
		cliAvailability := &types.CLIAvailability{
			Claude: ai.CheckCLIAvailable("claude"),
			Codex:  ai.CheckCLIAvailable("codex"),
			Gemini: ai.CheckCLIAvailable("gemini"),
		}

		httputil.OkJSON(w, &types.ListModelsResponse{
			Models:        result,
			TaskRouting:   taskRouting,
			Aliases:       aliases,
			AvailableCLIs: cliAvailability,
		})
	}
}
