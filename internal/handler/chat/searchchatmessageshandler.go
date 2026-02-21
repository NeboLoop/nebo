package chat

import (
	"database/sql"
	"net/http"
	"time"

	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/logging"
	"github.com/neboloop/nebo/internal/markdown"
	"github.com/neboloop/nebo/internal/middleware"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
)

// Search chat messages
func SearchChatMessagesHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.SearchChatMessagesRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Get user ID from JWT context
		userID := middleware.GetUserID(ctx)
		if userID == "" {
			userID = companionUserIDFallback
		}

		// Get companion chat first
		chat, err := svcCtx.DB.GetCompanionChatByUser(ctx, sql.NullString{String: userID, Valid: true})
		if err != nil {
			if err == sql.ErrNoRows {
				// No companion chat yet, return empty
				httputil.OkJSON(w, &types.SearchChatMessagesResponse{
					Messages: []types.ChatMessage{},
					Total:    0,
				})
				return
			}
			logging.Errorf("Failed to get companion chat: %v", err)
			httputil.Error(w, err)
			return
		}

		// Set defaults
		pageSize := req.PageSize
		if pageSize <= 0 {
			pageSize = 20
		}
		page := req.Page
		if page <= 0 {
			page = 1
		}
		offset := (page - 1) * pageSize

		// Search messages
		messages, err := svcCtx.DB.SearchChatMessages(ctx, db.SearchChatMessagesParams{
			ChatID:  chat.ID,
			Column2: sql.NullString{String: req.Query, Valid: true},
			Limit:   int64(pageSize),
			Offset:  int64(offset),
		})
		if err != nil {
			logging.Errorf("Failed to search messages: %v", err)
			httputil.Error(w, err)
			return
		}

		msgList := make([]types.ChatMessage, len(messages))
		for i, m := range messages {
			metadata := ""
			if m.Metadata.Valid {
				metadata = m.Metadata.String
			}
			msgList[i] = types.ChatMessage{
				Id:          m.ID,
				ChatId:      m.ChatID,
				Role:        m.Role,
				Content:     m.Content,
				ContentHtml: markdown.Render(m.Content),
				Metadata:    metadata,
				CreatedAt:   time.Unix(m.CreatedAt, 0).Format(time.RFC3339),
			}
		}

		httputil.OkJSON(w, &types.SearchChatMessagesResponse{
			Messages: msgList,
			Total:    len(msgList),
		})
	}
}
