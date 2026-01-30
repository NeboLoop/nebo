package agent

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/agent"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Get session messages
func GetAgentSessionMessagesHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.GetAgentSessionRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := agent.NewGetAgentSessionMessagesLogic(r.Context(), svcCtx)
		resp, err := l.GetAgentSessionMessages(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
