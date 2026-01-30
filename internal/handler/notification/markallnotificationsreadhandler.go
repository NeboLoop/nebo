package notification

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/notification"
	"gobot/internal/svc"
)

// Mark all notifications as read
func MarkAllNotificationsReadHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		l := notification.NewMarkAllNotificationsReadLogic(r.Context(), svcCtx)
		resp, err := l.MarkAllNotificationsRead()
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
