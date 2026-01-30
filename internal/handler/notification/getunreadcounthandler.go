package notification

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/notification"
	"gobot/internal/svc"
)

// Get unread notification count
func GetUnreadCountHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		l := notification.NewGetUnreadCountLogic(r.Context(), svcCtx)
		resp, err := l.GetUnreadCount()
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
