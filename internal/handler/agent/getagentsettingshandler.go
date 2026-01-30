package agent

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Get agent settings
func GetAgentSettingsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		settings := svcCtx.AgentSettings.Get()

		// Ensure heartbeat interval has a default
		interval := settings.HeartbeatIntervalMinutes
		if interval < 1 {
			interval = 30
		}

		httputil.OkJSON(w, &types.GetAgentSettingsResponse{
			Settings: types.AgentSettings{
				AutonomousMode:           settings.AutonomousMode,
				AutoApproveRead:          settings.AutoApproveRead,
				AutoApproveWrite:         settings.AutoApproveWrite,
				AutoApproveBash:          settings.AutoApproveBash,
				HeartbeatIntervalMinutes: interval,
			},
		})
	}
}
