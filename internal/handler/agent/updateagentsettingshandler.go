package agent

import (
	"net/http"

	"github.com/nebolabs/nebo/internal/agenthub"
	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/local"
	"github.com/nebolabs/nebo/internal/logging"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
)

// Update agent settings
func UpdateAgentSettingsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.UpdateAgentSettingsRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Validate heartbeat interval (min 1 minute, max 1440 = 24 hours)
		interval := req.HeartbeatIntervalMinutes
		if interval < 1 {
			interval = 30 // default
		} else if interval > 1440 {
			interval = 1440
		}

		settings := local.AgentSettings{
			AutonomousMode:           req.AutonomousMode,
			AutoApproveRead:          req.AutoApproveRead,
			AutoApproveWrite:         req.AutoApproveWrite,
			AutoApproveBash:          req.AutoApproveBash,
			HeartbeatIntervalMinutes: interval,
			CommEnabled:              req.CommEnabled,
			CommPlugin:               req.CommPlugin,
			DeveloperMode:            req.DeveloperMode,
		}

		if err := local.GetAgentSettings().Update(settings); err != nil {
			httputil.Error(w, err)
			return
		}

		// Broadcast settings to all connected agents
		frame := &agenthub.Frame{
			Type:   "event",
			Method: "settings_updated",
			Payload: map[string]any{
				"autonomousMode":           settings.AutonomousMode,
				"autoApproveRead":          settings.AutoApproveRead,
				"autoApproveWrite":         settings.AutoApproveWrite,
				"autoApproveBash":          settings.AutoApproveBash,
				"heartbeatIntervalMinutes": settings.HeartbeatIntervalMinutes,
				"commEnabled":              settings.CommEnabled,
				"commPlugin":               settings.CommPlugin,
				"developerMode":            settings.DeveloperMode,
			},
		}

		// Broadcast to all connected agents
		svcCtx.AgentHub.Broadcast(frame)

		logging.Infof("Agent settings updated: autonomous=%v", settings.AutonomousMode)

		httputil.OkJSON(w, &types.GetAgentSettingsResponse{
			Settings: types.AgentSettings{
				AutonomousMode:           settings.AutonomousMode,
				AutoApproveRead:          settings.AutoApproveRead,
				AutoApproveWrite:         settings.AutoApproveWrite,
				AutoApproveBash:          settings.AutoApproveBash,
				HeartbeatIntervalMinutes: settings.HeartbeatIntervalMinutes,
				CommEnabled:              settings.CommEnabled,
				CommPlugin:               settings.CommPlugin,
				DeveloperMode:            settings.DeveloperMode,
			},
		})
	}
}
