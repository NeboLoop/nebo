package agent

import (
	"net/http"

	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
)

// Get agent status
func GetAgentStatusHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.AgentStatusRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		hub := svcCtx.AgentHub
		if hub == nil {
			httputil.OkJSON(w, &types.AgentStatusResponse{
				AgentId:   req.AgentId,
				Connected: false,
			})
			return
		}

		// TODO: Get org ID from JWT context and look up agent
		// For now, return not connected
		httputil.OkJSON(w, &types.AgentStatusResponse{
			AgentId:   req.AgentId,
			Connected: false,
			Uptime:    0,
		})
	}
}
