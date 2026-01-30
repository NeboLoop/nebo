package agent

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/agent"
	"gobot/internal/svc"
)

// Get agent settings
func GetAgentSettingsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		l := agent.NewGetAgentSettingsLogic(r.Context(), svcCtx)
		resp, err := l.GetAgentSettings()
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
