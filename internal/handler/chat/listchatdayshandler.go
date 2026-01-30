package chat

import (
	"database/sql"
	"net/http"

	"nebo/internal/db"
	"nebo/internal/httputil"
	"nebo/internal/logging"
	"nebo/internal/svc"
	"nebo/internal/types"
)

// List days with messages for history browsing
func ListChatDaysHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.ListChatDaysRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Get companion chat first
		chat, err := svcCtx.DB.GetCompanionChatByUser(ctx, sql.NullString{String: companionUserID, Valid: true})
		if err != nil {
			if err == sql.ErrNoRows {
				// No companion chat yet, return empty
				httputil.OkJSON(w, &types.ListChatDaysResponse{Days: []types.DayInfo{}})
				return
			}
			logging.Errorf("Failed to get companion chat: %v", err)
			httputil.Error(w, err)
			return
		}

		// Set defaults
		pageSize := req.PageSize
		if pageSize <= 0 {
			pageSize = 30
		}
		page := req.Page
		if page <= 0 {
			page = 1
		}
		offset := (page - 1) * pageSize

		// Get days with message counts
		days, err := svcCtx.DB.GetDaysWithMessages(ctx, db.GetDaysWithMessagesParams{
			ChatID: chat.ID,
			Limit:  int64(pageSize),
			Offset: int64(offset),
		})
		if err != nil {
			logging.Errorf("Failed to get days with messages: %v", err)
			httputil.Error(w, err)
			return
		}

		dayList := make([]types.DayInfo, 0, len(days))
		for _, d := range days {
			if d.DayMarker.Valid {
				dayList = append(dayList, types.DayInfo{
					Day:          d.DayMarker.String,
					MessageCount: int(d.MessageCount),
				})
			}
		}

		httputil.OkJSON(w, &types.ListChatDaysResponse{
			Days: dayList,
		})
	}
}
