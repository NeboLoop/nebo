package setup

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/setup"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Update AI personality configuration
func UpdatePersonalityHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.UpdatePersonalityRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := setup.NewUpdatePersonalityLogic(r.Context(), svcCtx)
		resp, err := l.UpdatePersonality(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
