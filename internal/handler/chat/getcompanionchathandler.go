package chat

import (
	"database/sql"
	"errors"
	"net/http"
	"time"

	"github.com/nebolabs/nebo/internal/db"
	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/logging"
	"github.com/nebolabs/nebo/internal/middleware"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"

	"github.com/google/uuid"
)

const companionUserIDFallback = "companion-default"
const defaultContextMessageLimit = 50 // Number of recent messages to load for context

// Get companion chat (auto-creates if needed)
func GetCompanionChatHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		// Get user ID from JWT context, fall back to default for standalone mode
		userID := middleware.GetUserID(ctx)
		if userID == "" {
			userID = companionUserIDFallback
		}

		// Get or create the companion chat
		chat, err := svcCtx.DB.GetOrCreateCompanionChat(ctx, db.GetOrCreateCompanionChatParams{
			ID:     uuid.New().String(),
			UserID: sql.NullString{String: userID, Valid: true},
		})
		if err != nil {
			logging.Errorf("Failed to get/create companion chat: %v", err)
			httputil.Error(w, err)
			return
		}

		// Get recent messages (limited for context window)
		messages, err := svcCtx.DB.GetRecentChatMessages(ctx, db.GetRecentChatMessagesParams{
			ChatID: chat.ID,
			Limit:  defaultContextMessageLimit,
		})
		if err != nil {
			if !errors.Is(err, sql.ErrNoRows) {
				logging.Errorf("Failed to get messages: %v", err)
				httputil.Error(w, err)
				return
			}
			messages = nil
		}

		// Get total message count for UI (to show "X more messages in history")
		totalCount, _ := svcCtx.DB.CountChatMessages(ctx, chat.ID)

		msgList := make([]types.ChatMessage, len(messages))
		for i, m := range messages {
			metadata := ""
			if m.Metadata.Valid {
				metadata = m.Metadata.String
			}
			msgList[i] = types.ChatMessage{
				Id:        m.ID,
				ChatId:    m.ChatID,
				Role:      m.Role,
				Content:   m.Content,
				Metadata:  metadata,
				CreatedAt: time.Unix(m.CreatedAt, 0).Format(time.RFC3339),
			}
		}

		httputil.OkJSON(w, &types.GetChatResponse{
			Chat: types.Chat{
				Id:        chat.ID,
				Title:     chat.Title,
				CreatedAt: time.Unix(chat.CreatedAt, 0).Format(time.RFC3339),
				UpdatedAt: time.Unix(chat.UpdatedAt, 0).Format(time.RFC3339),
			},
			Messages:      msgList,
			TotalMessages: int(totalCount),
		})
	}
}
