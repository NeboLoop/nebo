package agent

import (
	"net/http"
	"time"

	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
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
