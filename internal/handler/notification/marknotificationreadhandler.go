package notification

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/notification"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Mark notification as read
func MarkNotificationReadHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.MarkNotificationReadRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := notification.NewMarkNotificationReadLogic(r.Context(), svcCtx)
		resp, err := l.MarkNotificationRead(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
