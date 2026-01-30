package notification

import (
	"net/http"

	"gobot/internal/auth"
	"gobot/internal/httputil"
	"gobot/internal/logging"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Get unread notification count
func GetUnreadCountHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		// Check if notifications are enabled
		if !svcCtx.Config.IsNotificationsEnabled() {
			httputil.OkJSON(w, &types.GetUnreadCountResponse{Count: 0})
			return
		}

		if !svcCtx.UseLocal() {
			httputil.OkJSON(w, &types.GetUnreadCountResponse{Count: 0})
			return
		}

		// Get user ID from context
		userID, err := auth.GetUserIDFromContext(ctx)
		if err != nil {
			logging.Errorf("Failed to get user ID: %v", err)
			httputil.Error(w, err)
			return
		}

		// Get unread count
		count, err := svcCtx.DB.Queries.CountUnreadNotifications(ctx, userID.String())
		if err != nil {
			logging.Errorf("Failed to count unread notifications: %v", err)
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.GetUnreadCountResponse{Count: int(count)})
	}
}
