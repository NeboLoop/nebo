package extensions

import (
	"net/http"

	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
)

// Toggle skill enabled/disabled
func ToggleSkillHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.ToggleSkillRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Toggle the skill's enabled state in persistent storage
		enabled, err := svcCtx.SkillSettings.Toggle(req.Name)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.ToggleSkillResponse{
			Name:    req.Name,
			Enabled: enabled,
		})
	}
}
