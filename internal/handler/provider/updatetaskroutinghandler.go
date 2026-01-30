package provider

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/provider"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Update task routing configuration
func UpdateTaskRoutingHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.UpdateTaskRoutingRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := provider.NewUpdateTaskRoutingLogic(r.Context(), svcCtx)
		resp, err := l.UpdateTaskRouting(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
