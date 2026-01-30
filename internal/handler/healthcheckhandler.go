package handler

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic"
	"gobot/internal/svc"
)

// Health check endpoint
func HealthCheckHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		l := logic.NewHealthCheckLogic(r.Context(), svcCtx)
		resp, err := l.HealthCheck()
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
