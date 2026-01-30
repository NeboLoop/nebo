package notification

import (
	"net/http"

	"nebo/internal/auth"
	"nebo/internal/httputil"
	"nebo/internal/logging"
	"nebo/internal/svc"
	"nebo/internal/types"
)

// Mark all notifications as read
func MarkAllNotificationsReadHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		// Check if notifications are enabled
		if !svcCtx.Config.IsNotificationsEnabled() {
			httputil.OkJSON(w, &types.MessageResponse{Message: "Notifications not enabled"})
			return
		}

		if !svcCtx.UseLocal() {
			httputil.OkJSON(w, &types.MessageResponse{Message: "All notifications marked as read"})
			return
		}

		// Get user ID from context
		userID, err := auth.GetUserIDFromContext(ctx)
		if err != nil {
			logging.Errorf("Failed to get user ID: %v", err)
			httputil.Error(w, err)
			return
		}

		// Mark all as read
		err = svcCtx.DB.Queries.MarkAllNotificationsRead(ctx, userID.String())
		if err != nil {
			logging.Errorf("Failed to mark all notifications as read: %v", err)
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.MessageResponse{Message: "All notifications marked as read"})
	}
}
