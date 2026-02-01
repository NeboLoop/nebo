package provider

import (
	"net/http"

	"nebo/internal/httputil"
	"nebo/internal/provider"
	"nebo/internal/svc"
	"nebo/internal/types"
)

// UpdateModelConfigHandler updates the model configuration (defaults, fallbacks)
func UpdateModelConfigHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.UpdateModelConfigRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		config := provider.GetModelsConfig()

		// Initialize defaults if nil
		if config.Defaults == nil {
			config.Defaults = &provider.Defaults{}
		}

		// Update primary model (e.g., "claude-code/opus", "anthropic/claude-sonnet")
		if req.Primary != "" {
			config.Defaults.Primary = req.Primary
		}

		// Update fallbacks if provided
		if req.Fallbacks != nil {
			config.Defaults.Fallbacks = req.Fallbacks
		}

		// Save to YAML
		if err := provider.SaveModels(config); err != nil {
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.UpdateModelConfigResponse{
			Success: true,
			Primary: config.Defaults.Primary,
		})
	}
}
