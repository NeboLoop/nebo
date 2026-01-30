package auth

import (
	"fmt"
	"net/http"

	"nebo/internal/httputil"
	"nebo/internal/logging"
	"nebo/internal/svc"
	"nebo/internal/types"
)

func VerifyEmailHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.EmailVerificationRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		if svcCtx.Auth == nil {
			httputil.Error(w, fmt.Errorf("auth service not configured"))
			return
		}

		err := svcCtx.Auth.VerifyEmail(r.Context(), req.Token)
		if err != nil {
			logging.Errorf("Email verification failed: %v", err)
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.MessageResponse{
			Message: "Email verified successfully.",
		})
	}
}
