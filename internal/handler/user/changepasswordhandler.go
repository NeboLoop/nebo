package user

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/user"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Change password for authenticated user
func ChangePasswordHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.ChangePasswordRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := user.NewChangePasswordLogic(r.Context(), svcCtx)
		resp, err := l.ChangePassword(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
