package setup

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/setup"
	"gobot/internal/svc"
)

// Mark initial setup as complete
func CompleteSetupHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		l := setup.NewCompleteSetupLogic(r.Context(), svcCtx)
		resp, err := l.CompleteSetup()
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
