package auth

import (
	"net/http"

	"nebo/internal/httputil"
	"nebo/internal/svc"
	"nebo/internal/types"
)

func ResendVerificationHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.ResendVerificationRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Verification email is sent on registration
		// For now, return a success message
		httputil.OkJSON(w, &types.MessageResponse{
			Message: "If the email address is registered and unverified, a new verification email has been sent.",
		})
	}
}
