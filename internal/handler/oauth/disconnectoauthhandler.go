package oauth

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/oauth"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Disconnect OAuth provider
func DisconnectOAuthHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.DisconnectOAuthRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := oauth.NewDisconnectOAuthLogic(r.Context(), svcCtx)
		resp, err := l.DisconnectOAuth(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
