package user

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/user"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Delete current user account
func DeleteAccountHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.DeleteAccountRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := user.NewDeleteAccountLogic(r.Context(), svcCtx)
		resp, err := l.DeleteAccount(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
