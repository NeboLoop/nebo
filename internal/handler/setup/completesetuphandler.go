package setup

import (
	"net/http"

	"github.com/zeromicro/go-zero/rest/httpx"
	"gobot/internal/logic/setup"
	"gobot/internal/svc"
)

// Mark initial setup as complete
func CompleteSetupHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		l := setup.NewCompleteSetupLogic(r.Context(), svcCtx)
		resp, err := l.CompleteSetup()
		if err != nil {
			httpx.ErrorCtx(r.Context(), w, err)
		} else {
			httpx.OkJsonCtx(r.Context(), w, resp)
		}
	}
}
