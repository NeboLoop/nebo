package oauth

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/oauth"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// OAuth callback - exchange code for tokens
func OAuthCallbackHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.OAuthLoginRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := oauth.NewOAuthCallbackLogic(r.Context(), svcCtx)
		resp, err := l.OAuthCallback(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
