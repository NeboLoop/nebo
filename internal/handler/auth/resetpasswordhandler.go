package auth

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/auth"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Reset password with token
func ResetPasswordHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.ResetPasswordRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := auth.NewResetPasswordLogic(r.Context(), svcCtx)
		resp, err := l.ResetPassword(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
