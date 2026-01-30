package provider

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/provider"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Update task routing configuration
func UpdateTaskRoutingHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.UpdateTaskRoutingRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		config := provider.GetModelsConfig()

		// Initialize task routing if nil
		if config.TaskRouting == nil {
			config.TaskRouting = &provider.TaskRouting{}
		}

		// Update routing configuration
		if req.Vision != "" {
			config.TaskRouting.Vision = req.Vision
		}
		if req.Audio != "" {
			config.TaskRouting.Audio = req.Audio
		}
		if req.Reasoning != "" {
			config.TaskRouting.Reasoning = req.Reasoning
		}
		if req.Code != "" {
			config.TaskRouting.Code = req.Code
		}
		if req.General != "" {
			config.TaskRouting.General = req.General
		}
		if req.Fallbacks != nil {
			config.TaskRouting.Fallbacks = req.Fallbacks
		}

		// Update aliases if provided
		if req.Aliases != nil {
			config.Aliases = make([]provider.ModelAlias, len(req.Aliases))
			for i, a := range req.Aliases {
				config.Aliases[i] = provider.ModelAlias{
					Alias:   a.Alias,
					ModelId: a.ModelId,
				}
			}
		}

		// Save to YAML
		if err := provider.SaveModels(config); err != nil {
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.MessageResponse{
			Message: "Task routing updated successfully",
		})
	}
}
