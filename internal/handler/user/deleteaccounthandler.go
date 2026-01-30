package user

import (
	"errors"
	"fmt"
	"net/http"

	"nebo/internal/auth"
	"nebo/internal/httputil"
	"nebo/internal/logging"
	"nebo/internal/svc"
	"nebo/internal/types"
)

func DeleteAccountHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.DeleteAccountRequest
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

		_, err = svcCtx.Auth.Login(ctx, email, req.Password)
		if err != nil {
			logging.Errorf("Password verification failed for delete account: %v", err)
			httputil.Error(w, errors.New("invalid password"))
			return
		}

		user, err := svcCtx.Auth.GetUserByEmail(ctx, email)
		if err != nil {
			logging.Errorf("Failed to get user %s: %v", email, err)
			httputil.Error(w, err)
			return
		}

		err = svcCtx.Auth.DeleteUser(ctx, user.ID)
		if err != nil {
			logging.Errorf("Failed to delete user %s: %v", email, err)
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.MessageResponse{
			Message: "Account deleted successfully.",
		})
	}
}
