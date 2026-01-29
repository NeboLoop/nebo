package agent

import (
	"net/http"

	"github.com/zeromicro/go-zero/rest/httpx"
	"gobot/internal/logic/agent"
	"gobot/internal/svc"
)

// List agent sessions
func ListAgentSessionsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		l := agent.NewListAgentSessionsLogic(r.Context(), svcCtx)
		resp, err := l.ListAgentSessions()
		if err != nil {
			httpx.ErrorCtx(r.Context(), w, err)
		} else {
			httpx.OkJsonCtx(r.Context(), w, resp)
		}
	}
}
