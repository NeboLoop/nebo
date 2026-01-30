package auth

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/auth"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Resend email verification
func ResendVerificationHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.ResendVerificationRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := auth.NewResendVerificationLogic(r.Context(), svcCtx)
		resp, err := l.ResendVerification(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
