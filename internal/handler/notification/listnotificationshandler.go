package notification

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/notification"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// List user notifications
func ListNotificationsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.ListNotificationsRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := notification.NewListNotificationsLogic(r.Context(), svcCtx)
		resp, err := l.ListNotifications(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
