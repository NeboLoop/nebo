package agent

import (
	"context"
	"net/http"
	"time"

	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/svc"
)

// GetLanesHandler returns lane statistics from the agent
func GetLanesHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		hub := svcCtx.AgentHub
		if hub == nil || hub.GetAnyAgent() == nil {
			httputil.ErrorWithCode(w, http.StatusServiceUnavailable, "Agent not connected")
			return
		}

		ctx, cancel := context.WithTimeout(r.Context(), 5*time.Second)
		defer cancel()

		frame, err := hub.SendRequestSync(ctx, "get_lanes", nil)
		if err != nil {
			httputil.ErrorWithCode(w, http.StatusGatewayTimeout, "Agent did not respond: "+err.Error())
			return
		}

		httputil.OkJSON(w, frame.Payload)
	}
}
