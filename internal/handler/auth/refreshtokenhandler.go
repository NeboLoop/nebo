package auth

import (
	"fmt"
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logging"
	"gobot/internal/svc"
	"gobot/internal/types"
)

func RefreshTokenHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.RefreshTokenRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		if svcCtx.Auth == nil {
			httputil.Error(w, fmt.Errorf("auth service not configured"))
			return
		}

		authResp, err := svcCtx.Auth.RefreshToken(r.Context(), req.RefreshToken)
		if err != nil {
			logging.Errorf("Token refresh failed: %v", err)
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.RefreshTokenResponse{
			Token:        authResp.Token,
			RefreshToken: authResp.RefreshToken,
			ExpiresAt:    authResp.ExpiresAt.UnixMilli(),
		})
	}
}
