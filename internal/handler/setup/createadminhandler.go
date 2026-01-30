package setup

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/setup"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Create the first admin user (only works when no admin exists)
func CreateAdminHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.CreateAdminRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := setup.NewCreateAdminLogic(r.Context(), svcCtx)
		resp, err := l.CreateAdmin(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
