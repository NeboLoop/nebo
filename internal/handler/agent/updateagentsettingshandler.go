package agent

import (
	"net/http"

	"gobot/internal/agenthub"
	"gobot/internal/httputil"
	"gobot/internal/local"
	"gobot/internal/logging"
	"gobot/internal/svc"
	"gobot/internal/types"
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

		// Update local settings store
		settings := local.AgentSettings{
			AutonomousMode:           req.AutonomousMode,
			AutoApproveRead:          req.AutoApproveRead,
			AutoApproveWrite:         req.AutoApproveWrite,
			AutoApproveBash:          req.AutoApproveBash,
			HeartbeatIntervalMinutes: interval,
		}

		if err := svcCtx.AgentSettings.Update(settings); err != nil {
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
			},
		}

		// Broadcast to all connected agents
		svcCtx.AgentHub.Broadcast(frame)

		logging.Infof("Agent settings updated and broadcast: autonomous=%v", settings.AutonomousMode)

		httputil.OkJSON(w, &types.GetAgentSettingsResponse{
			Settings: types.AgentSettings{
				AutonomousMode:           settings.AutonomousMode,
				AutoApproveRead:          settings.AutoApproveRead,
				AutoApproveWrite:         settings.AutoApproveWrite,
				AutoApproveBash:          settings.AutoApproveBash,
				HeartbeatIntervalMinutes: settings.HeartbeatIntervalMinutes,
			},
		})
	}
}
