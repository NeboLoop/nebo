package notification

import (
	"net/http"
	"time"

	"github.com/nebolabs/nebo/internal/auth"
	"github.com/nebolabs/nebo/internal/db"
	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/logging"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
)

// List user notifications
func ListNotificationsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.ListNotificationsRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Check if notifications are enabled
		if !svcCtx.Config.IsNotificationsEnabled() {
			httputil.OkJSON(w, &types.ListNotificationsResponse{
				Notifications: []types.Notification{},
				UnreadCount:   0,
				TotalCount:    0,
			})
			return
		}

		if !svcCtx.UseLocal() {
			httputil.OkJSON(w, &types.ListNotificationsResponse{
				Notifications: []types.Notification{},
				UnreadCount:   0,
				TotalCount:    0,
			})
			return
		}

		// Get user ID from context
		userID, err := auth.GetUserIDFromContext(ctx)
		if err != nil {
			logging.Errorf("Failed to get user ID: %v", err)
			httputil.Error(w, err)
			return
		}

		// Set defaults
		pageSize := int64(20)
		if req.PageSize > 0 && req.PageSize <= 100 {
			pageSize = int64(req.PageSize)
		}
		page := 1
		if req.Page > 0 {
			page = req.Page
		}
		offset := int64((page - 1)) * pageSize

		var notifications []db.Notification
		if req.Unread {
			notifications, err = svcCtx.DB.Queries.ListUnreadNotifications(ctx, db.ListUnreadNotificationsParams{
				UserID:   userID.String(),
				PageSize: pageSize,
			})
		} else {
			notifications, err = svcCtx.DB.Queries.ListUserNotifications(ctx, db.ListUserNotificationsParams{
				UserID:     userID.String(),
				PageOffset: offset,
				PageSize:   pageSize,
			})
		}
		if err != nil {
			logging.Errorf("Failed to list notifications: %v", err)
			httputil.Error(w, err)
			return
		}

		// Get unread count
		unreadCount, err := svcCtx.DB.Queries.CountUnreadNotifications(ctx, userID.String())
		if err != nil {
			logging.Errorf("Failed to count unread notifications: %v", err)
			httputil.Error(w, err)
			return
		}

		// Convert to response type
		result := make([]types.Notification, len(notifications))
		for i, n := range notifications {
			result[i] = types.Notification{
				Id:        n.ID,
				Type:      n.Type,
				Title:     n.Title,
				Body:      n.Body.String,
				ActionUrl: n.ActionUrl.String,
				Icon:      n.Icon.String,
				CreatedAt: time.Unix(n.CreatedAt, 0).Format(time.RFC3339),
			}
			if n.ReadAt.Valid {
				result[i].ReadAt = time.Unix(n.ReadAt.Int64, 0).Format(time.RFC3339)
			}
		}

		httputil.OkJSON(w, &types.ListNotificationsResponse{
			Notifications: result,
			UnreadCount:   int(unreadCount),
			TotalCount:    len(result),
		})
	}
}
