package setup

import (
	"net/http"

	"github.com/zeromicro/go-zero/rest/httpx"
	"gobot/internal/logic/setup"
	"gobot/internal/svc"
)

// Get AI personality configuration
func GetPersonalityHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		l := setup.NewGetPersonalityLogic(r.Context(), svcCtx)
		resp, err := l.GetPersonality()
		if err != nil {
			httpx.ErrorCtx(r.Context(), w, err)
		} else {
			httpx.OkJsonCtx(r.Context(), w, resp)
		}
	}
}
