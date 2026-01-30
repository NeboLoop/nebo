package notification

import (
	"net/http"

	"gobot/internal/auth"
	"gobot/internal/db"
	"gobot/internal/httputil"
	"gobot/internal/logging"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Mark notification as read
func MarkNotificationReadHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.MarkNotificationReadRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		if !svcCtx.Config.IsNotificationsEnabled() {
			httputil.OkJSON(w, &types.MessageResponse{Message: "Notifications not enabled"})
			return
		}

		if !svcCtx.UseLocal() {
			httputil.OkJSON(w, &types.MessageResponse{Message: "Notification marked as read"})
			return
		}

		// Get user ID from context
		userID, err := auth.GetUserIDFromContext(ctx)
		if err != nil {
			logging.Errorf("Failed to get user ID: %v", err)
			httputil.Error(w, err)
			return
		}

		// Mark as read
		err = svcCtx.DB.Queries.MarkNotificationRead(ctx, db.MarkNotificationReadParams{
			ID:     req.Id,
			UserID: userID.String(),
		})
		if err != nil {
			logging.Errorf("Failed to mark notification as read: %v", err)
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.MessageResponse{Message: "Notification marked as read"})
	}
}
