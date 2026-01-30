package setup

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/setup"
	"gobot/internal/svc"
)

// Get AI personality configuration
func GetPersonalityHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		l := setup.NewGetPersonalityLogic(r.Context(), svcCtx)
		resp, err := l.GetPersonality()
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
