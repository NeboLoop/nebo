package auth

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/auth"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Refresh authentication token
func RefreshTokenHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.RefreshTokenRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := auth.NewRefreshTokenLogic(r.Context(), svcCtx)
		resp, err := l.RefreshToken(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
