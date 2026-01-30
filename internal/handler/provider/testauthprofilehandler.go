package provider

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/provider"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Test auth profile (verify API key works)
func TestAuthProfileHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.TestAuthProfileRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := provider.NewTestAuthProfileLogic(r.Context(), svcCtx)
		resp, err := l.TestAuthProfile(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
