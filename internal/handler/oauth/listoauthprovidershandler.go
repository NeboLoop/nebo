package oauth

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/oauth"
	"gobot/internal/svc"
)

// List connected OAuth providers
func ListOAuthProvidersHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		l := oauth.NewListOAuthProvidersLogic(r.Context(), svcCtx)
		resp, err := l.ListOAuthProviders()
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
