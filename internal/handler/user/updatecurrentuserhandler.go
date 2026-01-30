package user

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/user"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Update current user profile
func UpdateCurrentUserHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.UpdateUserRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := user.NewUpdateCurrentUserLogic(r.Context(), svcCtx)
		resp, err := l.UpdateCurrentUser(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
