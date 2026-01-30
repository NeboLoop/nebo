package extensions

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/extensions"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Get single skill details
func GetSkillHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.GetSkillRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := extensions.NewGetSkillLogic(r.Context(), svcCtx)
		resp, err := l.GetSkill(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
