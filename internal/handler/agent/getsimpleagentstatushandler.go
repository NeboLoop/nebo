package agent

import (
	"net/http"
	"time"

	"gobot/internal/httputil"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Get simple agent status (single agent model)
func GetSimpleAgentStatusHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		hub := svcCtx.AgentHub
		if hub == nil {
			httputil.OkJSON(w, &types.SimpleAgentStatusResponse{
				Connected: false,
			})
			return
		}

		// Get any connected agent (single agent model)
		agent := hub.GetAnyAgent()
		if agent == nil {
			httputil.OkJSON(w, &types.SimpleAgentStatusResponse{
				Connected: false,
			})
			return
		}

		uptime := int64(time.Since(agent.CreatedAt).Seconds())
		httputil.OkJSON(w, &types.SimpleAgentStatusResponse{
			Connected: true,
			AgentId:   agent.ID,
			Uptime:    uptime,
		})
	}
}
