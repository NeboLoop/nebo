package setup

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/setup"
	"gobot/internal/svc"
)

// Check if setup is required (no admin exists)
func SetupStatusHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		l := setup.NewSetupStatusLogic(r.Context(), svcCtx)
		resp, err := l.SetupStatus()
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
