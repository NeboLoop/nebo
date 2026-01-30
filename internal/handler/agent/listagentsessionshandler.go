package agent

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/agent"
	"gobot/internal/svc"
)

// List agent sessions
func ListAgentSessionsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		l := agent.NewListAgentSessionsLogic(r.Context(), svcCtx)
		resp, err := l.ListAgentSessions()
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
