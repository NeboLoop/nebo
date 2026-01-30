package agent

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/agent"
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

		l := agent.NewUpdateAgentSettingsLogic(r.Context(), svcCtx)
		resp, err := l.UpdateAgentSettings(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
