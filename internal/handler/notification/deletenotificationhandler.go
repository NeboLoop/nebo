package notification

import (
	"net/http"

	"github.com/nebolabs/nebo/internal/auth"
	"github.com/nebolabs/nebo/internal/db"
	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/logging"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
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
