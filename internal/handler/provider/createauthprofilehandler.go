package provider

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/provider"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Create a new auth profile
func CreateAuthProfileHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.CreateAuthProfileRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := provider.NewCreateAuthProfileLogic(r.Context(), svcCtx)
		resp, err := l.CreateAuthProfile(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
