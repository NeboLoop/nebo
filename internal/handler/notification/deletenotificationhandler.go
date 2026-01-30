package notification

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/notification"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Delete notification
func DeleteNotificationHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.DeleteNotificationRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := notification.NewDeleteNotificationLogic(r.Context(), svcCtx)
		resp, err := l.DeleteNotification(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
