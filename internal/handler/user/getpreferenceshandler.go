package user

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/user"
	"gobot/internal/svc"
)

// Get user preferences
func GetPreferencesHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		l := user.NewGetPreferencesLogic(r.Context(), svcCtx)
		resp, err := l.GetPreferences()
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
