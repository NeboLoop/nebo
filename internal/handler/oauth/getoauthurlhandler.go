package oauth

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/oauth"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Get OAuth authorization URL
func GetOAuthUrlHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.GetOAuthUrlRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := oauth.NewGetOAuthUrlLogic(r.Context(), svcCtx)
		resp, err := l.GetOAuthUrl(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
