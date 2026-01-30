package user

import (
	"fmt"
	"net/http"

	"gobot/internal/auth"
	"gobot/internal/httputil"
	"gobot/internal/logging"
	"gobot/internal/svc"
	"gobot/internal/types"
)

func ChangePasswordHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.ChangePasswordRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		if svcCtx.Auth == nil {
			httputil.Error(w, fmt.Errorf("auth service not configured"))
			return
		}

		email, err := auth.GetEmailFromContext(ctx)
		if err != nil {
			logging.Errorf("Failed to get email from context: %v", err)
			httputil.Error(w, err)
			return
		}

		user, err := svcCtx.Auth.GetUserByEmail(ctx, email)
		if err != nil {
			logging.Errorf("Failed to get user: %v", err)
			httputil.Error(w, err)
			return
		}

		err = svcCtx.Auth.ChangePassword(ctx, user.ID, req.CurrentPassword, req.NewPassword)
		if err != nil {
			logging.Errorf("Failed to change password for %s: %v", email, err)
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.MessageResponse{
			Message: "Password changed successfully.",
		})
	}
}
