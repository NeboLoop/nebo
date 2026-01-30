package user

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/user"
	"gobot/internal/svc"
)

// Get current user profile
func GetCurrentUserHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		l := user.NewGetCurrentUserLogic(r.Context(), svcCtx)
		resp, err := l.GetCurrentUser()
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
