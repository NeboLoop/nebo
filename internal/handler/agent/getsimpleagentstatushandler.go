package agent

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/agent"
	"gobot/internal/svc"
)

// Get simple agent status (single agent model)
func GetSimpleAgentStatusHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		l := agent.NewGetSimpleAgentStatusLogic(r.Context(), svcCtx)
		resp, err := l.GetSimpleAgentStatus()
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
