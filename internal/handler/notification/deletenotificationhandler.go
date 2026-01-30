package notification

import (
	"net/http"

	"nebo/internal/auth"
	"nebo/internal/db"
	"nebo/internal/httputil"
	"nebo/internal/logging"
	"nebo/internal/svc"
	"nebo/internal/types"
)

// Delete notification
func DeleteNotificationHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.DeleteNotificationRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		if !svcCtx.Config.IsNotificationsEnabled() {
			httputil.OkJSON(w, &types.MessageResponse{Message: "Notifications not enabled"})
			return
		}

		if !svcCtx.UseLocal() {
			httputil.OkJSON(w, &types.MessageResponse{Message: "Notification deleted"})
			return
		}

		// Get user ID from context
		userID, err := auth.GetUserIDFromContext(ctx)
		if err != nil {
			logging.Errorf("Failed to get user ID: %v", err)
			httputil.Error(w, err)
			return
		}

		// Delete notification
		err = svcCtx.DB.Queries.DeleteNotification(ctx, db.DeleteNotificationParams{
			ID:     req.Id,
			UserID: userID.String(),
		})
		if err != nil {
			logging.Errorf("Failed to delete notification: %v", err)
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.MessageResponse{Message: "Notification deleted"})
	}
}
