package extensions

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/extensions"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Toggle skill enabled/disabled
func ToggleSkillHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.ToggleSkillRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := extensions.NewToggleSkillLogic(r.Context(), svcCtx)
		resp, err := l.ToggleSkill(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
