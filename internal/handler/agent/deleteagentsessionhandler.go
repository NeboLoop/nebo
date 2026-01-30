package agent

import (
	"net/http"

	"nebo/internal/httputil"
	"nebo/internal/svc"
	"nebo/internal/types"
)

// Delete agent session
func DeleteAgentSessionHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.DeleteAgentSessionRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		if err := svcCtx.DB.DeleteSession(ctx, req.Id); err != nil {
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.MessageResponse{
			Message: "Session deleted successfully",
		})
	}
}
