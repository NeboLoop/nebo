package provider

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/provider"
	"gobot/internal/svc"
)

// List all auth profiles (API keys)
func ListAuthProfilesHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		l := provider.NewListAuthProfilesLogic(r.Context(), svcCtx)
		resp, err := l.ListAuthProfiles()
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
