package agent

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// List connected agents
func ListAgentsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		// Get agents from the hub
		hub := svcCtx.AgentHub
		if hub == nil {
			httputil.OkJSON(w, &types.ListAgentsResponse{
				Agents: []types.AgentInfo{},
				Total:  0,
			})
			return
		}

		agents := hub.GetAllAgents()
		agentInfos := make([]types.AgentInfo, 0, len(agents))

		for _, agent := range agents {
			agentInfos = append(agentInfos, types.AgentInfo{
				AgentId:   agent.ID,
				Connected: true,
				CreatedAt: agent.CreatedAt.Format("2006-01-02T15:04:05Z"),
			})
		}

		httputil.OkJSON(w, &types.ListAgentsResponse{
			Agents: agentInfos,
			Total:  len(agentInfos),
		})
	}
}
