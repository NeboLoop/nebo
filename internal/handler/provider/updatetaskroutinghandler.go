package provider

import (
	"net/http"

	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/provider"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
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

		// Update routing — always write all fields so clearing works
		config.TaskRouting.Vision = req.Vision
		config.TaskRouting.Audio = req.Audio
		config.TaskRouting.Reasoning = req.Reasoning
		config.TaskRouting.Code = req.Code
		config.TaskRouting.General = req.General
		config.TaskRouting.Fallbacks = req.Fallbacks

		// Update lane routing
		if req.LaneRouting != nil {
			config.LaneRouting = &provider.LaneRouting{
				Heartbeat: req.LaneRouting["heartbeat"],
				Events:    req.LaneRouting["events"],
				Comm:      req.LaneRouting["comm"],
				Subagent:  req.LaneRouting["subagent"],
			}
		}

		// Update aliases — always write so removals persist
		config.Aliases = make([]provider.ModelAlias, len(req.Aliases))
		for i, a := range req.Aliases {
			config.Aliases[i] = provider.ModelAlias{
				Alias:   a.Alias,
				ModelId: a.ModelId,
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
