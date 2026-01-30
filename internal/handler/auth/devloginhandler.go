package auth

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/auth"
	"gobot/internal/svc"
)

// Dev auto-login (local development only)
func DevLoginHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		l := auth.NewDevLoginLogic(r.Context(), svcCtx)
		resp, err := l.DevLogin()
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
