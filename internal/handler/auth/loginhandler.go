package auth

import (
	"fmt"
	"net/http"

	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/logging"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
)

func LoginHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.LoginRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		if svcCtx.Auth == nil {
			httputil.Error(w, fmt.Errorf("auth service not configured"))
			return
		}

		authResp, err := svcCtx.Auth.Login(r.Context(), req.Email, req.Password)
		if err != nil {
			logging.Errorf("Login failed for %s: %v", req.Email, err)
			httputil.Error(w, err)
			return
		}

		logging.Infof("User logged in: %s", req.Email)

		httputil.OkJSON(w, &types.LoginResponse{
			Token:        authResp.Token,
			RefreshToken: authResp.RefreshToken,
			ExpiresAt:    authResp.ExpiresAt.UnixMilli(),
		})
	}
}
